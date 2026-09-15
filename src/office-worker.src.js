/* ============================================================
   办公文件解析核心(胶水)—— 由 scripts/make-office-part.js 内联为
   text/plain 载荷(id=office-worker-glue),worker 与主线程共用。

   设计要点(细节见 docs/OFFICE-NOTES.md):
   · worker 内:组装顺序为 shim(self.window/self.global + __OFFICE_WORKER__)→
     四库(mammoth / SheetJS / docstream / fflate)→ 本文件;本文件检测到
     __OFFICE_WORKER__ 时先装网络桩(fetch/XHR/WebSocket/EventSource/importScripts),
     再挂 self.onmessage。
   · 主线程降级:同一份「四库 + 本文件」经 script 标签注入执行(无 shim、无标志位),
     此时不装网络桩(会破坏应用自身网络层),靠构建期 URL 中和 + 网络审计兜底。
   · 本文件只依赖 ES5 + Promise(项目既有基线);不含可选链 / 空值合并等新语法。
   · 文本上限 TEXT_MAX 与 src/appD.part 的 ATTACH_TEXT_MAX 必须一致(131072)。
   ============================================================ */
(function () {
  "use strict";

  var G = (typeof self !== "undefined") ? self : (typeof window !== "undefined" ? window : this);
  var IS_WORKER = !!(G && G.__OFFICE_WORKER__);

  /* ---------------- 网络桩(仅 worker) ---------------- */
  function blockNet(name) {
    return function () { throw new Error("office worker: 网络已禁用(" + name + ")"); };
  }
  function installNetStubs() {
    var names = ["fetch", "XMLHttpRequest", "WebSocket", "EventSource", "importScripts"];
    for (var i = 0; i < names.length; i++) {
      try { G[names[i]] = blockNet(names[i]); } catch (e) { /* 只读属性:忽略 */ }
    }
  }
  if (IS_WORKER) { installNetStubs(); }

  /* ---------------- 常量 ---------------- */
  var TEXT_MAX = 131072;                     /* == appD 的 ATTACH_TEXT_MAX */
  var FORMATS = ["docx", "xlsx", "xls", "pptx", "doc", "ppt"];
  var LIMITS = {
    /* 本文件只管"结构安全"这一层:ZIP 预扫描 + 图像字节账(zip-bomb / 图像超限的判定点)。
       文件体积上限(20MB 主线程降级 / 50MB worker)与解析超时(60s)是**主线程职责**,
       唯一来源在 src/officekit.src.js(解析前判上限 + 两条路径计时),
       worker 里没有读取点 —— 此前各复制一份在此,属死字段,已删 */
    /* ZIP 结构预扫描(docx/xlsx/pptx;fflate 零解压枚举) */
    maxEntries: 2000,
    maxUncompressedTotal: 150 * 1024 * 1024,
    maxSingleEntry: 64 * 1024 * 1024,
    maxRatio: 120,
    /* 图像 */
    maxImages: 20,
    maxImageBytes: 8 * 1024 * 1024,
    maxImagesTotal: 32 * 1024 * 1024
  };
  /* 浏览器可渲染的图片 MIME 白名单;EMF/WMF/TIFF 等一律跳过并计数 */
  var IMG_MIME = {
    "image/png": 1, "image/jpeg": 1, "image/gif": 1,
    "image/webp": 1, "image/bmp": 1, "image/avif": 1
  };
  var ZIP_EXT = { docx: 1, xlsx: 1, pptx: 1 };
  var LEGACY_EXT = { doc: 1, ppt: 1, xls: 1 };

  var MSG = {
    "too-big": "文件过大,已超过可解析上限",
    "zip-bomb": "这份文件的压缩结构异常(疑似压缩炸弹),出于安全考虑已停止解析",
    "encrypted": "这份文件可能已加密或受密码保护,无法解析",
    "corrupt": "文件已损坏或格式不完整,无法解析",
    "unsupported": "不支持的文件类型",
    "timeout": "解析超时,已中止",
    "cancelled": "解析已取消",
    "worker": "解析进程异常,请重试"
  };

  /* ---------------- 基础工具 ---------------- */
  function now() {
    return (G.performance && G.performance.now) ? G.performance.now() : Date.now();
  }
  function officeError(kind, msg) {
    var e = new Error(msg || MSG[kind] || MSG.corrupt);
    e.officeKind = kind;
    return e;
  }
  function kindOf(e) { return (e && e.officeKind) ? e.officeKind : "corrupt"; }
  function msgOf(e) {
    if (e == null) { return MSG.corrupt; }
    if (typeof e === "string") { return e; }
    return e.message ? String(e.message) : String(e);
  }
  function progress(opts, phase) {
    if (opts && typeof opts.onProgress === "function") {
      try { opts.onProgress(phase); } catch (e) { /* 回调异常不影响解析 */ }
    }
  }

  function toU8(buf) {
    if (!buf) { throw officeError("corrupt", "解析失败:没有拿到文件内容"); }
    if (typeof Uint8Array !== "undefined" && buf instanceof Uint8Array) { return buf; }
    if (typeof ArrayBuffer !== "undefined" && buf instanceof ArrayBuffer) { return new Uint8Array(buf); }
    if (buf.buffer) { return new Uint8Array(buf.buffer, buf.byteOffset || 0, buf.byteLength); }
    throw officeError("corrupt", "解析失败:文件内容类型不支持");
  }
  /* Uint8Array → 独立 ArrayBuffer(workers transferable 用;视图带偏移时截取副本) */
  function toAB(x) {
    if (!x) { return null; }
    if (typeof ArrayBuffer !== "undefined" && x instanceof ArrayBuffer) { return x; }
    var u8 = (typeof Uint8Array !== "undefined" && x instanceof Uint8Array) ? x : new Uint8Array(x.buffer || x);
    if (u8.byteOffset === 0 && u8.byteLength === u8.buffer.byteLength) { return u8.buffer; }
    return u8.buffer.slice(u8.byteOffset, u8.byteOffset + u8.byteLength);
  }
  function startsWithBytes(u8, bytes) {
    if (u8.length < bytes.length) { return false; }
    for (var i = 0; i < bytes.length; i++) { if (u8[i] !== bytes[i]) { return false; } }
    return true;
  }
  function isCfb(u8) {
    return startsWithBytes(u8, [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
  }
  function isZip(u8) {
    return u8.length > 3 && u8[0] === 0x50 && u8[1] === 0x4B
      && (u8[2] === 0x03 || u8[2] === 0x05 || u8[2] === 0x07 || u8[2] === 0x01);
  }
  function b64ToU8(b64) {
    var s = String(b64).replace(/[\s\u3000]+/g, "");
    var bin = G.atob ? G.atob(s) : "";
    var n = bin.length, out = new Uint8Array(n);
    for (var i = 0; i < n; i++) { out[i] = bin.charCodeAt(i) & 255; }
    return out;
  }
  function metaGet(md, key) {
    if (!md) { return undefined; }
    if (typeof md.get === "function") {
      try { return md.get(key); } catch (e) { return undefined; }
    }
    return md[key];
  }
  function numOf(v) {
    var n = parseInt(v, 10);
    return (isFinite(n) && n > 0) ? n : 0;
  }
  function errorMessage(e) {
    return String((e && e.message) || e || "");
  }
  /* 把库抛出的原始异常归类成可读中文 + errorKind */
  function classify(e) {
    if (e && e.officeKind) { return e; }
    var m = errorMessage(e);
    if (/password|encrypt|受保护|已加密/i.test(m)) { return officeError("encrypted", MSG.encrypted); }
    if (/zip|central directory|invalid signature|end of central|CRC|inflate/i.test(m)) {
      return officeError("corrupt", MSG.corrupt);
    }
    return officeError("corrupt", MSG.corrupt + "(解析库报错:" + m.slice(0, 100) + ")");
  }

  /* ---------------- ZIP 结构预扫描(零解压) ---------------- */
  function zipScan(u8) {
    var count = 0, totalOut = 0, ratioPeak = 0, ratioName = "", bomb = "";
    try {
      if (!G.fflate || typeof G.fflate.unzipSync !== "function") {
        throw officeError("worker", "解析库未就绪(fflate)");
      }
      G.fflate.unzipSync(u8, {
        filter: function (f) {
          count++;
          var os = f.originalSize || 0;
          var cs = f.size || 0;
          totalOut += os;
          var ratio = cs > 0 ? os / cs : (os > 0 ? 1e9 : 1);
          if (ratio > ratioPeak) { ratioPeak = ratio; ratioName = f.name || ""; }
          if (!bomb) {
            if (count > LIMITS.maxEntries) { bomb = "条目数超过 " + LIMITS.maxEntries; }
            else if (os > LIMITS.maxSingleEntry) { bomb = "单个条目超过 " + Math.round(LIMITS.maxSingleEntry / 1048576) + "MB"; }
            else if (totalOut > LIMITS.maxUncompressedTotal) { bomb = "解压总量超过 " + Math.round(LIMITS.maxUncompressedTotal / 1048576) + "MB"; }
            else if (ratio > LIMITS.maxRatio) { bomb = "压缩比 " + Math.round(ratio) + ":1 超过 " + LIMITS.maxRatio + ":1"; }
          }
          return false;   /* 零解压:只枚举中央目录 */
        }
      });
    } catch (e) {
      if (e && e.officeKind) { throw e; }
      throw officeError("corrupt", MSG.corrupt);
    }
    if (bomb) {
      throw officeError("zip-bomb", MSG["zip-bomb"] + "(" + bomb + (ratioName ? ",首见条目 " + ratioName : "") + ")");
    }
    return { entries: count, totalOut: totalOut, maxRatio: Math.round(ratioPeak * 10) / 10 };
  }

  function precheck(u8, ext) {
    if (ZIP_EXT[ext]) {
      /* 加密的 OOXML 会被另存成 CFB(OLE2)容器,靠魔数先认出来 */
      if (isZip(u8)) { zipScan(u8); return; }
      if (isCfb(u8)) { throw officeError("encrypted", MSG.encrypted); }
      throw officeError("corrupt", MSG.corrupt);
    }
    if (LEGACY_EXT[ext]) { /* 老格式是 CFB 复合文档;结构细化交给解析库 */ }
  }

  /* ---------------- 图像收集 ---------------- */
  function ImageSink() { this.images = []; this.bytes = 0; this.skipped = 0; }
  ImageSink.prototype.add = function (mime, data, label, page) {
    var ab = toAB(data);
    var n = ab ? ab.byteLength : 0;
    if (!IMG_MIME[mime] || !n || this.images.length >= LIMITS.maxImages
      || n > LIMITS.maxImageBytes || (this.bytes + n) > LIMITS.maxImagesTotal) {
      this.skipped++;
      return null;
    }
    this.bytes += n;
    var idx = this.images.length + 1;
    var item = {
      mime: mime,
      data: ab,
      bytes: n,
      label: label || ("图 " + idx),
      page: numOf(page) || idx
    };
    this.images.push(item);
    return item;
  };

  /* ---------------- 结果归一 ---------------- */
  function finish(subtype, rawText, sink, pages, unit, t0) {
    var raw = (rawText == null) ? "" : String(rawText);
    var textChars = raw.length;
    var clipped = false;
    var text = raw;
    if (textChars > TEXT_MAX) { text = raw.slice(0, TEXT_MAX); clipped = true; }
    return {
      subtype: subtype,
      text: text,
      textChars: textChars,
      clipped: clipped,
      images: sink.images,
      imgCount: sink.images.length,
      skippedImgs: sink.skipped,
      pages: numOf(pages) || 0,
      unit: unit || "",
      ms: Math.round(now() - t0)
    };
  }

  /* ---------------- docx:mammoth 双 pass(文本 + 正文引用的图像) ---------------- */
  function parseDocx(u8, opts, t0) {
    if (!G.mammoth || typeof G.mammoth.extractRawText !== "function") {
      return Promise.reject(officeError("worker", "解析库未就绪(mammoth)"));
    }
    var ab = toAB(u8);
    var sink = new ImageSink();
    progress(opts, "extracting");
    return G.mammoth.extractRawText({ arrayBuffer: ab }).then(function (res) {
      var text = (res && res.value) || "";
      progress(opts, "parsing");
      return G.mammoth.convertToHtml({ arrayBuffer: ab }, {
        convertImage: G.mammoth.images.imgElement(function (img) {
          var mime = img.contentType || "";
          if (!IMG_MIME[mime]) { sink.skipped++; return { src: "" }; }
          return img.readAsArrayBuffer().then(function (data) {
            sink.add(mime, data, null, null);
            return { src: "" };
          }, function () { sink.skipped++; return { src: "" }; });
        })
      }).then(function () {
        progress(opts, "done");
        return finish("docx", text, sink, 0, "", t0);
      });
    });
  }

  /* ---------------- xlsx / xls:SheetJS 转 CSV(全工作表) ---------------- */
  function parseSheet(u8, ext, opts, t0) {
    if (!G.XLSX || typeof G.XLSX.read !== "function") {
      return Promise.reject(officeError("worker", "解析库未就绪(SheetJS)"));
    }
    progress(opts, "parsing");
    var wb = G.XLSX.read(u8, { type: "array" });
    var names = (wb && wb.SheetNames) || [];
    var parts = [];
    for (var i = 0; i < names.length; i++) {
      var csv = "";
      try { csv = G.XLSX.utils.sheet_to_csv(wb.Sheets[names[i]]); } catch (e) { csv = ""; }
      parts.push("# " + names[i] + "\n\n" + csv);
    }
    var text = parts.join("\n\n");
    if (!names.length || !text.replace(/\s/g, "")) { text = "(空工作簿)"; }
    progress(opts, "extracting");
    return collectOfficeImages(u8, ext).then(function (sink) {
      progress(opts, "done");
      return finish(ext, text, sink, names.length, "表", t0);
    });
  }

  /* 走 docstream 的附件通道收图(xlsx/xls);失败不当成解析失败 —— 图像只是辅助信息 */
  function collectOfficeImages(u8, ext) {
    var sink = new ImageSink();
    if (!G.officeParser || typeof G.officeParser.parseOffice !== "function") {
      return Promise.resolve(sink);
    }
    return G.officeParser.parseOffice(toAB(u8), { extractAttachments: true }).then(function (ast) {
      var atts = (ast && ast.attachments) || [];
      for (var i = 0; i < atts.length; i++) { addOfficeAttachment(sink, atts[i]); }
      return sink;
    }, function () { return sink; });
  }

  function attachmentBytes(data) {
    if (!data) { return null; }
    if (typeof data === "string") { return b64ToU8(data); }
    if (typeof Uint8Array !== "undefined" && data instanceof Uint8Array) { return data; }
    if (typeof ArrayBuffer !== "undefined" && data instanceof ArrayBuffer) { return new Uint8Array(data); }
    if (data.buffer) { return new Uint8Array(data.buffer, data.byteOffset || 0, data.byteLength); }
    return null;
  }
  function addOfficeAttachment(sink, a, label, page) {
    var mime = (a && (a.mimeType || a.mime)) || "";
    if (!a || a.type !== "image" || !IMG_MIME[mime]) { sink.skipped++; return; }
    var u8 = attachmentBytes(a.data);
    if (!u8 || !u8.length) { sink.skipped++; return; }
    sink.add(mime, u8, label, page);
  }

  /* ---------------- pptx / doc / ppt:docstream(文本 + 图像) ---------------- */
  function parseByDocstream(u8, ext, opts, t0) {
    if (!G.officeParser || typeof G.officeParser.parseOffice !== "function") {
      return Promise.reject(officeError("worker", "解析库未就绪(docstream)"));
    }
    var sink = new ImageSink();
    progress(opts, "parsing");
    return G.officeParser.parseOffice(toAB(u8), { extractAttachments: true }).then(function (ast) {
      var text = "";
      try { text = (ast && ast.toText) ? ast.toText() : ""; } catch (e) { text = ""; }
      var atts = (ast && ast.attachments) || [];
      for (var i = 0; i < atts.length; i++) { addOfficeAttachment(sink, atts[i]); }
      var pages = 0, unit = "";
      if (ext === "pptx") {
        var sc = numOf(metaGet(ast && ast.metadata, "slideCount"));
        if (sc) { pages = sc; unit = "页"; }
      }
      if (LEGACY_EXT[ext] && !sink.images.length) { /* 老格式图像不支持 —— 由应用层换算降级文案 */ }
      progress(opts, "done");
      return finish(ext, text, sink, pages, unit, t0);
    });
  }

  /* ---------------- 入口 ---------------- */
  function parse(buf, ext, opts) {
    opts = opts || {};
    var t0 = now();
    var e = String(ext == null ? "" : ext).toLowerCase().replace(/^\./, "");
    if (FORMATS.indexOf(e) < 0) {
      return Promise.reject(officeError("unsupported", MSG.unsupported + "(." + e + ")"));
    }
    return new Promise(function (resolve, reject) {
      var u8;
      try {
        u8 = toU8(buf);
        precheck(u8, e);
      } catch (err) { reject(err); return; }
      progress(opts, "started");
      var run;
      try {
        if (e === "docx") { run = parseDocx(u8, opts, t0); }
        else if (e === "xlsx" || e === "xls") { run = parseSheet(u8, e, opts, t0); }
        else { run = parseByDocstream(u8, e, opts, t0); }
      } catch (err2) { reject(classify(err2)); return; }
      run.then(function (r) { resolve(r); }, function (err3) {
        reject((err3 && err3.officeKind) ? err3 : classify(err3));
      });
    });
  }

  var core = {
    formats: FORMATS.slice(),
    limits: LIMITS,
    textMax: TEXT_MAX,
    imageMimes: IMG_MIME,
    parse: parse
  };
  G.OfficeParseCore = core;

  /* ---------------- worker 消息入口 ---------------- */
  function postToMain(msg, transfer) {
    try { G.postMessage(msg, transfer || []); } catch (e) { G.postMessage(msg); }
  }
  if (IS_WORKER && typeof G.postMessage === "function") {
    G.onmessage = function (ev) {
      var m = (ev && ev.data) || {};
      if (m.type !== "parse") { return; }
      var id = m.id;
      core.parse(m.buf, m.ext, {
        onProgress: function (phase) { postToMain({ type: "progress", id: id, phase: phase }); }
      }).then(function (result) {
        var transfer = [];
        for (var i = 0; i < result.images.length; i++) {
          if (result.images[i].data) { transfer.push(result.images[i].data); }
        }
        postToMain({ type: "done", id: id, ok: true, result: result }, transfer);
      }, function (e) {
        postToMain({ type: "done", id: id, ok: false, error: msgOf(e), errorKind: kindOf(e) });
      });
    };
  }
})();
