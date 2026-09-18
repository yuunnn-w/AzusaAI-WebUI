/* ============================================================
   OfficeKit —— 单文件内联办公文件解析封装(ES5:只用 var / function)

   设计要点(细节见 docs/OFFICE-NOTES.md):
   · 四个库(mammoth / SheetJS / docstream / fflate)与解析核心(胶水)都以
     text/plain 载荷标签(id=office-lib-*)内嵌在本片段之前,零网络。
   · 默认在 classic blob worker 里解析(file:// 下可用):库在 worker 内才
     会碰 window/global 垫片与网络桩,主线程全局不受污染(docstream 内含
     pdf.js 5.4.530 的一份拷贝,只在 worker 里加载)。
   · 环境不支持 Worker 时自动降级「主线程注入」:同一份「库体 + 胶水」经
     script 标签注入(无 shim、无网络桩),此时单文件上限 20MB,靠构建期
     URL 中和与网络审计保证零请求。
   · 同一时刻只跑一个文件(串行队列);超时/取消 = terminate 当前 worker,
     队列中的后续文件自动用新 worker 继续。
   · 结果契约与 PDFKit/OCRKit 一致:永不 reject,失败返回 {ok:false,error,errorKind,ms}。
   ============================================================ */
(function (W, D) {
  "use strict";

  var VERSION = "mammoth 1.12.3 / SheetJS 0.20.3 / docstream 0.1.3 / fflate 0.8.3";
  var FORMATS = ["docx", "xlsx", "xls", "pptx", "doc", "ppt"];
  var DEFAULT_TIMEOUT = 60000;

  var ID = {
    mammoth: "office-lib-mammoth",
    xlsx: "office-lib-xlsx",
    docstream: "office-lib-docstream",
    fflate: "office-lib-fflate",
    glue: "office-worker-glue"
  };
  var PAYLOADS = [ID.mammoth, ID.xlsx, ID.docstream, ID.fflate, ID.glue];

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

  /* 文件体积上限与解析超时的**唯一来源**就在这里:
     这两条数值此前在 worker 侧另有一份副本且从未被读取,已删。
     上限判定(parse 前)与两条解析路径的超时计时都由本文件执行;
     worker 侧只保留它自己真正要用的 ZIP 结构 / 图像上限(见 office-worker.src.js 的 LIMITS) */
  var MAIN_THREAD_CAP = 20 * 1024 * 1024;
  var MAX_BYTES = 50 * 1024 * 1024;

  var S = {
    ctx: null,        /* 当前 worker 上下文(懒建) */
    injected: false,  /* 主线程核心是否已注入 */
    chain: null,      /* 串行队列 */
    seq: 0,
    texts: null,      /* 载荷文本缓存 */
    loadErr: ""
  };

  /* ---------------- 基础工具 ---------------- */
  function byId(id) { return D.getElementById(id); }
  function textOf(id) { var e = byId(id); return e ? e.textContent : ""; }
  function now() {
    return (W.performance && W.performance.now) ? W.performance.now() : Date.now();
  }
  function msgOf(e) {
    if (e == null) { return "未知错误"; }
    if (typeof e === "string") { return e; }
    return e.message ? String(e.message) : String(e);
  }
  /* 构建期 escapeForInline 的逆操作(make-office-part.js 断言逐字节可逆);
     这里用 "<" + "/script" 拼出结果,避免源内出现会终止 script 块的字面序列 */
  function unescapeInline(code) {
    return String(code).replace(/<\\\/script/gi, "<" + "/script").replace(/<\\!--/g, "<" + "!--");
  }
  function payloadText(id) {
    if (!S.texts) { S.texts = {}; }
    if (typeof S.texts[id] === "string") { return S.texts[id]; }
    var t = "";
    try { t = unescapeInline(textOf(id)); } catch (e) { t = ""; }
    S.texts[id] = t;
    return t;
  }
  function toArrayBuffer(buf) {
    if (!buf) { return null; }
    if (typeof Uint8Array !== "undefined" && buf instanceof Uint8Array) {
      var u = new Uint8Array(buf.byteLength);
      u.set(buf);
      return u.buffer;
    }
    if (typeof ArrayBuffer !== "undefined" && buf instanceof ArrayBuffer) { return buf.slice(0); }
    if (buf.buffer) {
      var v = new Uint8Array(buf.byteLength);
      v.set(new Uint8Array(buf.buffer, buf.byteOffset || 0, buf.byteLength));
      return v.buffer;
    }
    return null;
  }
  function bytesOf(buf) {
    if (!buf) { return 0; }
    if (typeof buf.byteLength === "number") { return buf.byteLength; }
    return buf.length || 0;
  }
  /* S12(P2-a):按次调整单文档正文上限。缺省 / 非法(非正数)= 0 ⇒ 一路透传 0,
     胶水退回它自己的 TEXT_MAX(131072)⇒ 不传时与批前逐字节一致。 */
  function textMaxOf(opts) {
    var n = parseInt(opts && opts.textMax, 10);
    return (isFinite(n) && n > 0) ? n : 0;
  }
  /* ArrayBuffer → base64(分包,避免超长参数的栈/字符串峰值) */
  function abToBase64(buf) {
    var u8 = (buf && typeof Uint8Array !== "undefined" && buf instanceof Uint8Array) ? buf : new Uint8Array(buf);
    var CHUNK = 0x8000;
    var out = "";
    for (var i = 0; i < u8.length; i += CHUNK) {
      var end = Math.min(i + CHUNK, u8.length);
      var s = "";
      for (var j = i; j < end; j++) { s += String.fromCharCode(u8[j]); }
      out += s;
    }
    return W.btoa(out);
  }

  /* ---------------- 组装 ---------------- */
  /* setImmediate 垫片:必须在库体之前执行。docstream 自带的是 `setTimeout(cb,0)` 版本,
     实测其异步管线会被 4ms 最小定时器间隔放大(pptx 92KB:1.3~3.5s → 40~66ms),
     改为 MessageChannel 宏任务后输出逐字节一致但快约 20 倍(读数见 docs/OFFICE-NOTES.md)。
     库体自带的 shim 只在 typeof setImmediate === "undefined" 时安装,故这里先占位即可。 */
  var SETIMMEDIATE_SHIM = "(function(){var g=self;"
    + "if(typeof g.MessageChannel!=='function'){if(typeof g.setImmediate!=='function'){g.setImmediate=function(cb){return g.setTimeout(cb,0);};}return;}"
    + "var mc=null,q=[];"
    + "g.setImmediate=function(cb){if(!mc){mc=new g.MessageChannel();"
    + "mc.port1.onmessage=function(){var f=q.shift();if(f){f();}};if(mc.port1.start){mc.port1.start();}}"
    + "q.push(cb);mc.port2.postMessage(0);return 0;};})();\n";
  function libSource() {
    return [payloadText(ID.mammoth), payloadText(ID.xlsx),
      payloadText(ID.docstream), payloadText(ID.fflate)].join("\n;\n");
  }
  function workerSource() {
    /* 顺序硬性:先垫片,再四库,最后胶水 */
    return "self.window = self;\nself.global = self;\nself.__OFFICE_WORKER__ = 1;\n"
      + SETIMMEDIATE_SHIM + libSource() + "\n;\n" + payloadText(ID.glue) + "\n";
  }
  function canWorker() {
    return typeof W.Worker === "function" && !!(W.Blob && W.URL && W.URL.createObjectURL);
  }
  function killCtx(ctx, kind, message) {
    if (!ctx || ctx.dead) { return; }
    ctx.dead = true;
    try { ctx.worker.terminate(); } catch (e) { /* ignore */ }
    try { if (ctx.url) { W.URL.revokeObjectURL(ctx.url); } } catch (e) { /* ignore */ }
    var keys = [];
    var k;
    for (k in ctx.pending) { if (Object.prototype.hasOwnProperty.call(ctx.pending, k)) { keys.push(k); } }
    for (var i = 0; i < keys.length; i++) {
      var p = ctx.pending[keys[i]];
      delete ctx.pending[keys[i]];
      if (p && p.done) { p.done(false, kind || "worker", message || MSG.worker, null); }
    }
    if (S.ctx === ctx) { S.ctx = null; }
  }
  function makeCtx() {
    var src = workerSource();
    var url = W.URL.createObjectURL(new Blob([src], { type: "text/javascript" }));
    var worker = new W.Worker(url);
    var ctx = { worker: worker, url: url, pending: {}, dead: false };
    worker.onmessage = function (ev) { onWorkerMessage(ctx, ev && ev.data); };
    worker.onerror = function (e) {
      var detail = (e && (e.message || e.filename)) ? msgOf(e) : "";
      killCtx(ctx, "worker", MSG.worker + (detail ? "(" + detail + ")" : ""));
      if (e && e.preventDefault) { e.preventDefault(); }
    };
    S.ctx = ctx;
    return ctx;
  }
  function onWorkerMessage(ctx, m) {
    if (!m || !ctx || ctx.dead) { return; }
    var p = ctx.pending[m.id];
    if (!p || !p.done) { return; }
    if (m.type === "progress") {
      if (typeof p.onProgress === "function") {
        try { p.onProgress(m.phase); } catch (e) { /* 回调异常不影响解析 */ }
      }
      return;
    }
    if (m.type !== "done") { return; }
    delete ctx.pending[m.id];
    if (m.ok) { p.done(true, null, null, m.result); }
    else { p.done(false, m.errorKind || "corrupt", m.error || MSG.corrupt, null); }
  }

  /* ---------------- 结果归一 ---------------- */
  function failResult(errorKind, error, t0) {
    return { ok: false, error: error, errorKind: errorKind || "corrupt", ms: Math.round(now() - t0) };
  }
  function normalizeResult(r, t0) {
    if (!r) { return failResult("corrupt", MSG.corrupt, t0); }
    var src = r.images || [];
    var images = [];
    for (var i = 0; i < src.length; i++) {
      var im = src[i] || {};
      var b64 = "";
      try { b64 = im.data ? abToBase64(im.data) : ""; } catch (e) { b64 = ""; }
      images.push({
        mime: im.mime || "",
        data: b64,
        bytes: im.bytes || 0,
        label: im.label || ("图 " + (i + 1)),
        /* 页号原样透传;**0 要保住**(不是"缺省"):pptx 认不出归属的图由胶水显式写 0,
           应用层据此保留整篇序号 + 如实注记(P2-a / S11b)。缺字段时才退回整篇序号。 */
        page: (typeof im.page === "number" && im.page >= 0) ? im.page : (i + 1)
      });
    }
    return {
      ok: true,
      subtype: r.subtype || "",
      text: (r.text == null) ? "" : String(r.text),
      textChars: r.textChars || 0,
      clipped: !!r.clipped,
      images: images,
      imgCount: images.length,
      skippedImgs: r.skippedImgs || 0,
      pages: r.pages || 0,
      unit: r.unit || "",
      ms: Math.round(now() - t0)
    };
  }

  /* ---------------- worker 路径 ---------------- */
  function runWorker(buf, ext, opts, timeout, t0) {
    var ctx;
    try {
      ctx = S.ctx || makeCtx();
    } catch (e) {
      return Promise.resolve(failResult("worker", "无法启动解析进程:" + msgOf(e), t0));
    }
    return new Promise(function (resolve) {
      var settled = false;
      var id = ++S.seq;
      var timer = setTimeout(function () {
        if (settled) { return; }
        settled = true;
        delete ctx.pending[id];
        /* 超时:terminate 当前 worker(在跑的解析随之结束),队列后续自动重建 */
        killCtx(ctx, "timeout", MSG.timeout);
        resolve(failResult("timeout", MSG.timeout, t0));
      }, timeout);
      ctx.pending[id] = {
        onProgress: opts.onProgress,
        done: function (ok, kind, error, result) {
          if (settled) { return; }
          settled = true;
          clearTimeout(timer);
          if (ok) { resolve(normalizeResult(result, t0)); }
          else { resolve(failResult(kind, error, t0)); }
        }
      };
      var copy = toArrayBuffer(buf);
      if (!copy) {
        delete ctx.pending[id];
        clearTimeout(timer);
        if (!settled) { settled = true; resolve(failResult("corrupt", "解析失败:没有拿到文件内容", t0)); }
        return;
      }
      try {
        ctx.worker.postMessage({ type: "parse", id: id, ext: ext, buf: copy, textMax: opts.textMax }, [copy]);
      } catch (e) {
        delete ctx.pending[id];
        clearTimeout(timer);
        if (!settled) { settled = true; resolve(failResult("worker", MSG.worker + "(" + msgOf(e) + ")", t0)); }
      }
    });
  }

  /* ---------------- 主线程降级路径 ---------------- */
  function injectCore() {
    if (S.injected && W.OfficeParseCore) { return true; }
    var src = SETIMMEDIATE_SHIM + libSource() + "\n;\n" + payloadText(ID.glue) + "\n";
    var s = D.createElement("script");
    s.textContent = src;
    (D.head || D.documentElement || D.body).appendChild(s);
    if (s.parentNode) { s.parentNode.removeChild(s); }
    S.injected = !!W.OfficeParseCore;
    return S.injected;
  }
  function runMainThread(buf, ext, opts, timeout, t0) {
    return new Promise(function (resolve) {
      var settled = false;
      /* 主线程模式无法中途停止同步解析,超时只是对调用方的兜底(worker 模式另见 killCtx) */
      var timer = setTimeout(function () {
        if (settled) { return; }
        settled = true;
        resolve(failResult("timeout", MSG.timeout, t0));
      }, timeout);
      setTimeout(function () {
        if (settled) { return; }
        var ok = false;
        try { ok = injectCore(); } catch (e) { ok = false; }
        if (!ok) {
          settled = true; clearTimeout(timer);
          resolve(failResult("worker", "解析核心未能装入主线程", t0));
          return;
        }
        var core = W.OfficeParseCore;
        var copy = toArrayBuffer(buf);
        if (!copy) {
          settled = true; clearTimeout(timer);
          resolve(failResult("corrupt", "解析失败:没有拿到文件内容", t0));
          return;
        }
        core.parse(copy, ext, { onProgress: opts.onProgress, textMax: opts.textMax }).then(function (r) {
          if (settled) { return; }
          settled = true; clearTimeout(timer);
          resolve(normalizeResult(r, t0));
        }, function (e) {
          if (settled) { return; }
          settled = true; clearTimeout(timer);
          var kind = (e && e.officeKind) ? e.officeKind : "corrupt";
          resolve(failResult(kind, msgOf(e), t0));
        });
      }, 0);
    });
  }

  /* ---------------- 串行队列 ---------------- */
  function enqueue(task) {
    var run = S.chain ? S.chain.then(task, task) : task();
    S.chain = run.then(function () { }, function () { });
    return run;
  }

  /* ---------------- 对外:parse / cancelCurrent / terminate ----------------
     parse(buf, ext, opts):opts.timeout / opts.onProgress / opts.textMax(P2-a:单文档正文上限,
       0 / 缺省 = 不指定 ⇒ 胶水用 TEXT_MAX 131072;正数则抬高,只在**本次解析**生效)。 */
  function parse(buf, ext, opts) {
    opts = opts || {};
    var t0 = now();
    var e = String(ext == null ? "" : ext).toLowerCase().replace(/^\./, "");
    if (!api.available) {
      return Promise.resolve(failResult("unsupported", "办公解析不可用:" + (api.why || "未知原因"), t0));
    }
    if (FORMATS.indexOf(e) < 0) {
      return Promise.resolve(failResult("unsupported", MSG.unsupported + "(." + e + ")", t0));
    }
    var size = bytesOf(buf);
    var cap = api.mode === "main-thread" ? MAIN_THREAD_CAP : MAX_BYTES;
    if (size > cap) {
      return Promise.resolve(failResult("too-big",
        "文件过大(" + Math.round(size / 1048576) + "MB),当前" + (api.mode === "main-thread" ? "降级(主线程)模式" : "")
        + "上限 " + Math.round(cap / 1048576) + "MB", t0));
    }
    var timeout = Number(opts.timeout) > 0 ? Number(opts.timeout) : DEFAULT_TIMEOUT;
    var hooks = {
      onProgress: typeof opts.onProgress === "function" ? opts.onProgress : null,
      textMax: textMaxOf(opts)          /* S12:0 = 不指定(胶水用 TEXT_MAX) */
    };
    return enqueue(function () {
      if (api.mode === "main-thread") { return runMainThread(buf, e, hooks, timeout, t0); }
      return runWorker(buf, e, hooks, timeout, t0);
    });
  }
  function cancelCurrent() {
    killCtx(S.ctx, "cancelled", MSG.cancelled);
  }
  function terminate() {
    killCtx(S.ctx, "cancelled", MSG.cancelled);
    return Promise.resolve(true);
  }

  /* ---------------- 自检 ---------------- */
  function missingCaps() {
    var miss = [];
    if (typeof Promise !== "function") { miss.push("Promise"); }
    if (typeof W.Blob !== "function") { miss.push("Blob"); }
    if (typeof Uint8Array !== "function") { miss.push("Uint8Array"); }
    if (typeof ArrayBuffer !== "function") { miss.push("ArrayBuffer"); }
    if (typeof W.TextDecoder !== "function") { miss.push("TextDecoder"); }
    if (typeof W.atob !== "function") { miss.push("atob"); }
    if (typeof W.btoa !== "function") { miss.push("btoa"); }
    return miss;
  }
  function missingPayloads() {
    var miss = [];
    for (var i = 0; i < PAYLOADS.length; i++) {
      if (!payloadText(PAYLOADS[i])) { miss.push(PAYLOADS[i]); }
    }
    return miss;
  }
  function compileProbe(t0) {
    var bad = [];
    var canFn = true;
    try { /* eslint-disable-next-line no-new-func */
      new Function("return 1;");
    } catch (e) { canFn = false; }
    if (!canFn) { return bad; }
    for (var i = 0; i < PAYLOADS.length; i++) {
      try { new Function(payloadText(PAYLOADS[i])); } catch (e2) { bad.push(PAYLOADS[i]); }
    }
    api.diag.compileMs = Math.round(now() - t0);
    return bad;
  }

  /* ---------------- 导出 ---------------- */
  var api = {
    available: false,
    why: "",
    version: VERSION,
    formats: FORMATS.slice(),
    mode: "none",
    limits: { mainThreadSizeCap: MAIN_THREAD_CAP, maxBytes: MAX_BYTES, timeoutMs: DEFAULT_TIMEOUT },
    parse: parse,
    cancelCurrent: cancelCurrent,
    terminate: terminate,
    diag: { compileMs: 0, mode: "" }
  };

  var tSelf = now();
  var caps = missingCaps();
  var pays = missingPayloads();
  if (caps.length) {
    api.available = false;
    api.why = "环境缺少: " + caps.join("、");
  } else if (pays.length) {
    api.available = false;
    api.why = "缺少内嵌载荷: " + pays.join("、") + "(请跑 node scripts/make-office-part.js 重新生成 src/office.part)";
  } else {
    var bad = compileProbe(tSelf);
    if (bad.length) {
      api.available = false;
      api.why = "内嵌载荷无法编译: " + bad.join("、") + "(src/office.part 可能已损坏,请重新生成)";
    } else {
      api.available = true;
      api.mode = canWorker() ? "worker" : "main-thread";
    }
  }
  api.diag.mode = api.mode;
  if (!api.available && !api.mode) { api.mode = "none"; }
  W.OfficeKit = api;
})(window, document);
