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
   · 文本上限 TEXT_MAX 与 src/appD.part 的 ATTACH_TEXT_MAX 必须一致(131072);
     调用方可选地用 opts.textMax 按次抬高(P2-a / S12),缺省仍是 TEXT_MAX ⇒ 输出与批前逐字节一致。
   · pptx 输出按幻灯插入页标记行 `----- 第 N 页 -----`(与 PDF 同一字面量;P2-a / S11),
     并把图片归属到真实幻灯号(page;P2-a / S11b)——两者都带**自检**,不成立就整篇回退原输出。
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
  var TEXT_MAX = 131072;                     /* == appD 的 ATTACH_TEXT_MAX;也是未传 opts.textMax 时的默认值 */
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
  /* S12:单文档正文上限按次可调。缺省 / 非法值(0 / 负数 / NaN)一律退回 TEXT_MAX ⇒ 不传时与批前逐字节一致。
     上限本身不做夹取 —— 它由调用方(OfficeKit.parse 的 opts.textMax)决定,取太大只是内存代价。 */
  function textMaxOf(opts) {
    var n = numOf(opts && opts.textMax);
    return n || TEXT_MAX;
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
  /* textMax = 本次生效的单文档正文上限(S12);未传 = TEXT_MAX ⇒ 截断行为与批前一致。
     超限时**只置 clipped 标志 + 保留原文长度 textChars**,文案(如实注记)由应用层出
     —— 胶水不改口径、不静默丢信息。 */
  function finish(subtype, rawText, sink, pages, unit, t0, textMax) {
    var raw = (rawText == null) ? "" : String(rawText);
    var limit = numOf(textMax) || TEXT_MAX;
    var textChars = raw.length;
    var clipped = false;
    var text = raw;
    if (textChars > limit) { text = raw.slice(0, limit); clipped = true; }
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
  function parseDocx(u8, opts, t0, textMax) {
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
        return finish("docx", text, sink, 0, "", t0, textMax);
      });
    });
  }

  /* ---------------- xlsx / xls:SheetJS 转 CSV(全工作表) ---------------- */
  function parseSheet(u8, ext, opts, t0, textMax) {
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
      return finish(ext, text, sink, names.length, "表", t0, textMax);
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
    var item = sink.add(mime, u8, label, page);
    /* S11b:调用方显式给了页号(含 0 = pptx 认不出归属)时按**调用方的语义**落定 ——
       sink 的 `numOf(page) || idx` 兜底会把 0 改写成整篇序号,这里覆盖回来;
       没给页号(docx/xlsx,page === undefined)时保持 sink 的兜底(整篇序号),行为与批前一致。 */
    if (item && page === 0) { item.page = 0; }
  }

  /* ---------------- pptx:幻灯标记 + 图片→幻灯号归属(P2-a:S11 / S11b) ----------------
     · 标记字面量 = `----- 第 N 页 -----`,与 PDF 生成端(scripts/make-pdfjs-part.js)和
       切页判据(src/appD.part 的 PDF_PAGE_SEP_RE)逐字一致 —— 三处必须同形,改一处要改三处。
     · **页号只认 vendor 写在 metadata 上的原始序号**:docstream 的 content 数组**只 push 有子节点的幻灯**
       (pptx 分支的 `!z` 守卫 —— 空幻灯不进数组) ⇒ 数组下标 ≠ 幻灯号;
       `metadata.slideNumber` 取自 `slideN.xml` 的文件名序号,不受该过滤影响(§8-R6)。 */
  var PPTX_SEP_RE = /^----- 第 (\d+) 页 -----$/;
  function pptxSepLine(no) { return "----- 第 " + no + " 页 -----"; }
  /* 幻灯号:只认 docstream 给的 metadata.slideNumber(slideN.xml 的文件名序号);取不到返回 **0 = 认不出**,
     **不**退回"数组下标 + 1" —— content 已把空幻灯滤掉,下标编号会让空幻灯之后的页号整体前移,
     正是 §8-R6 要避免的事。0 的两个消费点都按"认不出"处理:标记侧**整篇回退**(宁可没有编号,
     也不冒标错号的风险,与自检同一取舍)、归属侧落 page = 0(应用层如实注记"无法定位到幻灯")。
     该回退当前不可达:docstream 0.1.3 的 part 先过 `/ppt\/(notesSlides|slides)\/(notesSlide|slide)\d+.xml/`,
     再取 `/lide(\d+)\.xml/` ⇒ 必然匹配、slideNumber 恒 ≥ 1 —— 它是"厂商行为一变就踩"的防线。 */
  function pptxSlideNo(node) {
    return numOf(metaGet(node && node.metadata, "slideNumber")) || 0;
  }
  /* 注记页判定(*防御性*,pptx 路径当前不可达):vendor 的 pptx 分支把 note 节点挡在 content 之外
     (`B = {type: z ? "note" : "slide"}` + `if(…, !z) { … B.children.length > 0 && v.push(B) }`,
     z = 路径含 notesSlide ⇒ note 解析完即丢)⇒ `ast.content` 里根本没有 note 节点,本判定恒假。
     留它是为了将来 vendor 把 note 压进 content 时,注记页不会跟着拿一条 `----- 第 N 页 -----`
     (note 的 metadata.slideNumber 与所属幻灯同号 ⇒ 会多切出一段)。 */
  function pptxIsNote(node) {
    return !!node && (node.type === "note" || !!metaGet(node.metadata, "noteId"));
  }
  function stripPptxMarks(s) {
    var lines = String(s == null ? "" : s).split("\n");
    var keep = [];
    for (var i = 0; i < lines.length; i++) {
      if (!PPTX_SEP_RE.test(lines[i])) { keep.push(lines[i]); }
    }
    return keep.join("\n");
  }
  /* 逐张幻灯文本:借 docstream 自己的 toText() 现算 —— 把 content 数组临时缩成单元素后,
     单元素 join 不引入分隔符 ⇒ 拿到的是该幻灯文本的**逐字节**原样(不复制 vendor 的内部规则)。
     取完立即还原;还原后 toText() 与取值前不同 ⇒ 返回 null(上层整篇回退,不冒险)。 */
  function pptxSlideTexts(ast, plain) {
    var arr = ast && ast.content;
    if (!arr || typeof arr.slice !== "function" || typeof arr.length !== "number") { return null; }
    if (typeof ast.toText !== "function") { return null; }
    var saved = arr.slice();
    var per = [], ok = true;
    try {
      for (var i = 0; i < saved.length; i++) {
        arr.length = 0;
        arr.push(saved[i]);
        var t = ast.toText();
        per.push(String(t == null ? "" : t));
      }
    } catch (e) { ok = false; }
    arr.length = 0;
    for (var j = 0; j < saved.length; j++) { arr.push(saved[j]); }
    if (!ok || per.length !== saved.length) { return null; }
    var back = "";
    try { back = String(ast.toText() == null ? "" : ast.toText()); } catch (e2) { return null; }
    if (back !== plain) { return null; }
    return { nodes: saved, per: per };
  }
  /* 加标记 + 自检:去掉标记行后必须与 toText() **逐字节相同**,否则返回 null(整篇回退,R6:不冒错标风险)。
     空幻灯:不出标记、不占行(它的文本本来就被 vendor 的 filter 丢掉);注记页不出标记,但正文照旧参与
     (pptx 路径当前不会有 note 节点,见 pptxIsNote)。幻灯号认不出(pptxSlideNo = 0)⇒ 也**整篇回退**:
     编号错比没有编号更糟,取舍与自检一致。
     注意:幻灯正文里若**本来就有**一行长得像标记,自检必然失败 ⇒ 回退(这正是自检要拦的情形)。 */
  function pptxMarkedText(ast, plain) {
    var r = pptxSlideTexts(ast, plain);
    if (!r) { return null; }
    var parts = [];
    for (var i = 0; i < r.per.length; i++) {
      if (r.per[i] === "") { continue; }
      var head = "";
      if (!pptxIsNote(r.nodes[i])) {
        var no = pptxSlideNo(r.nodes[i]);
        if (!no) { return null; }
        head = pptxSepLine(no) + "\n";
      }
      parts.push(head + r.per[i]);
    }
    var marked = parts.join("\n");
    if (stripPptxMarks(marked) !== plain) { return null; }
    return marked;
  }
  /* 附件名 → 幻灯号(所有节点);同名附件取**首次**出现的那张幻灯(文档顺序)
     —— docstream 的归属判据就是 attachments[].name === node.metadata.attachmentName 精确相等
     (见其 pptx 分支的后处理;§8-R13:不做模糊匹配)。
     幻灯号认不出 ⇒ 落 0 = "无法定位到幻灯"(应用层按整篇顺序编号并如实注记,不猜)。 */
  function pptxSlideOfAttachments(ast) {
    var map = {}, slides = (ast && ast.content) || [];
    function walk(node, no) {
      if (!node) { return; }
      var md = node.metadata;
      var name = md ? md.attachmentName : null;
      if (typeof name === "string" && name && !map[name]) { map[name] = no; }
      var kids = node.children;
      if (kids) { for (var i = 0; i < kids.length; i++) { walk(kids[i], no); } }
    }
    for (var i = 0; i < slides.length; i++) { walk(slides[i], pptxSlideNo(slides[i])); }
    return map;
  }

  /* ---------------- pptx / doc / ppt:docstream(文本 + 图像) ---------------- */
  function parseByDocstream(u8, ext, opts, t0, textMax) {
    if (!G.officeParser || typeof G.officeParser.parseOffice !== "function") {
      return Promise.reject(officeError("worker", "解析库未就绪(docstream)"));
    }
    var sink = new ImageSink();
    progress(opts, "parsing");
    return G.officeParser.parseOffice(toAB(u8), { extractAttachments: true }).then(function (ast) {
      var text = "";
      try { text = (ast && ast.toText) ? String(ast.toText()) : ""; } catch (e) { text = ""; }
      if (ext === "pptx") {
        var marked = null;
        try { marked = pptxMarkedText(ast, text); } catch (e2) { marked = null; }
        if (marked !== null) { text = marked; }
      }
      var atts = (ast && ast.attachments) || [];
      var slideOf = (ext === "pptx") ? pptxSlideOfAttachments(ast) : null;
      for (var i = 0; i < atts.length; i++) {
        /* pptx:认得出幻灯号就写进去,认不出写 0(应用层据此保留整篇序号 + 如实注记);
           docx/xlsx 恒不传页号 ⇒ 与批前逐字节一致 */
        var page = null;
        if (slideOf) {
          var nm = (atts[i] && atts[i].name) ? String(atts[i].name) : "";
          page = (nm && slideOf[nm]) ? slideOf[nm] : 0;
        }
        addOfficeAttachment(sink, atts[i], null, page);
      }
      var pages = 0, unit = "";
      if (ext === "pptx") {
        var sc = numOf(metaGet(ast && ast.metadata, "slideCount"));
        if (sc) { pages = sc; unit = "页"; }
      }
      if (LEGACY_EXT[ext] && !sink.images.length) { /* 老格式图像不支持 —— 由应用层换算降级文案 */ }
      progress(opts, "done");
      return finish(ext, text, sink, pages, unit, t0, textMax);
    });
  }

  /* ---------------- 入口 ---------------- */
  function parse(buf, ext, opts) {
    opts = opts || {};
    var t0 = now();
    var textMax = textMaxOf(opts);
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
        if (e === "docx") { run = parseDocx(u8, opts, t0, textMax); }
        else if (e === "xlsx" || e === "xls") { run = parseSheet(u8, e, opts, t0, textMax); }
        else { run = parseByDocstream(u8, e, opts, t0, textMax); }
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
        onProgress: function (phase) { postToMain({ type: "progress", id: id, phase: phase }); },
        textMax: m.textMax
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
