/* 由 vendor/pdfjs-src/ 生成可内联的 src/pdfjs.part —— 唯一入口:node scripts/make-pdfjs-part.js
 *
 * pdfjs-dist ≥4 只发布 ESM(.mjs),没有 UMD。这里做**机械化改写**成 classic 脚本:
 *   1. 结尾的 `export{a as B, C}` → `window.pdfjsLib = {B: a, C: C}`(worker 同理 → globalThis.pdfjsWorker)
 *   2. `import.meta.url` → 预置变量 __pdfjsModuleUrl(classic script 里 import.meta 是语法错误)
 *   3. 动态 `await import(x)` → 预置函数(classic script 里动态 import 合法,但内联单文件里无用;
 *      保留一个能真正兜底的实现,见 __pdfjsFakeWorkerImport)
 *   4. 整体包进 IIFE + "use strict"(等价于 ESM 的模块作用域与严格模式)
 *
 * 产物结构(3 个块,顺序即依赖顺序):
 *   1. <script>                                       display 库 → window.pdfjsLib
 *   2. <script type="text/plain" id="pdfjs-worker-src"> worker 源码(classic,只存文本不执行)
 *   3. <script>                                       PDFKit 封装层(ES5)
 *
 * 幂等:同样输入生成完全相同的字节(产物不含时间戳)。
 */
const fs = require('fs');
const path = require('path');

const ROOT = path.resolve(__dirname, '..');
const VENDOR = path.join(ROOT, 'vendor', 'pdfjs-src');
const SRC = path.join(VENDOR, 'legacy', 'build');
const DISPLAY = path.join(SRC, 'pdf.min.mjs');
const WORKER = path.join(SRC, 'pdf.worker.min.mjs');
const OUT = path.join(ROOT, 'src', 'pdfjs.part');
const WORKER_SRC_ID = 'pdfjs-worker-src';

function read(f) {
  return fs.readFileSync(f, 'utf8').replace(/\r\n/g, '\n').replace(/\r/g, '\n');
}

/* ---------------------------------------------------------------------------
 * 内联脚本转义:HTML 解析器不认 JS 语法,"</script" 会直接终止脚本块。
 * JS 里 "/" 是可转义字符:字符串 "\/" === "/"、正则 /<\/script/ 等价、注释里是纯文本。
 * "<!--" 在 classic script 里是行注释起始,同样转义。
 * ------------------------------------------------------------------------- */
function escapeForInline(code, label) {
  const a = (code.match(/<\/script/gi) || []).length;
  const b = (code.match(/<!--/g) || []).length;
  const out = code.replace(/<\/script/gi, '<\\/script').replace(/<!--/g, '<\\!--');
  console.log('  转义 ' + label + ': </script x' + a + ', <!-- x' + b);
  return out;
}
function unescapeInline(code) {
  return code.replace(/<\\\/script/gi, '</script').replace(/<\\!--/g, '<!--');
}

/* ---------------------------------------------------------------------------
 * ESM → classic 机械改写
 * ------------------------------------------------------------------------- */
function parseExportMap(stmt) {
  const inner = stmt.slice('export{'.length, stmt.lastIndexOf('}'));
  return inner.split(',').map(s => s.trim()).filter(Boolean).map(e => {
    const m = e.split(/\s+as\s+/);
    const local = m[0].trim();
    const exported = (m[1] || m[0]).trim();
    return { local, exported };
  });
}

function rewriteEsm(src, opts) {
  const label = opts.label;
  const notes = [];

  /* --- 1. 定位唯一的 export{...} 语句(必须是最后一条语句) --- */
  const exStart = src.lastIndexOf('export{');
  if (exStart < 0) { throw new Error(label + ': 找不到 export{ 语句'); }
  const before = src.slice(0, exStart);
  const exportStmt = src.slice(exStart);
  const tail = exportStmt.slice(exportStmt.indexOf('}') + 1);
  if (tail.trim() !== '' && tail.trim() !== ';') { throw new Error(label + ': export 语句后还有内容:' + JSON.stringify(tail.slice(0, 80))); }
  if (/[A-Za-z0-9_$]/.test(before.slice(-1)) && before.slice(-1) !== '}') {
    throw new Error(label + ': export 前一个字符可疑:' + JSON.stringify(before.slice(-20)));
  }
  if ((src.match(/\bexport\b/g) || []).length !== 1) { throw new Error(label + ': export 出现次数不为 1'); }
  const map = parseExportMap(exportStmt);
  map.forEach(e => {
    if (!/^[A-Za-z_$][A-Za-z0-9_$]*$/.test(e.exported) || !/^[A-Za-z_$][A-Za-z0-9_$]*$/.test(e.local)) {
      throw new Error(label + ': 导出名不是普通标识符:' + JSON.stringify(e));
    }
  });

  /* --- 2. 表达式级改写(记录替换点上下文,供事后校验;expect 为数字时做数量断言,'all' 表示全部替换) --- */
  let body = before;
  const seen = {};
  const windows = [];
  function sub(re, to, name, expect) {
    const rx = new RegExp(re.source, re.flags.indexOf('g') < 0 ? re.flags + 'g' : re.flags);
    let m, n = 0, out = '', last = 0;
    rx.lastIndex = 0;
    while ((m = rx.exec(body)) !== null) {
      n++;
      out += body.slice(last, m.index);
      const rep = typeof to === 'function' ? to(m[0]) : to;
      if (m[0].length > 160) { throw new Error(label + ': ' + name + ' 匹配过长(' + m[0].length + ' 字符),疑似正则越界'); }
      windows.push({
        name: name,
        before: body.slice(Math.max(0, m.index - 48), m.index),
        after: body.slice(m.index + m[0].length, m.index + m[0].length + 48),
        delta: rep.length - m[0].length
      });
      out += rep;
      last = m.index + m[0].length;
      if (m[0].length === 0) { rx.lastIndex++; }
    }
    out += body.slice(last);
    if (expect !== 'all' && n !== expect) { throw new Error(label + ': ' + name + ' 预期 ' + expect + ' 处,实际 ' + n + ' 处'); }
    seen[name] = n;
    body = out;
    return n;
  }
  sub(/import\.meta\.url/g, '__pdfjsModuleUrl', 'import.meta.url', opts.importMeta || 'all');
  sub(/await\s+import\(\s*(?:\/\*[\s\S]{0,40}?\*\/\s*)*([A-Za-z_$][\w$.]*|`[^`]*`)\s*\)/g,
    m => {
      const arg = m.slice(m.indexOf('(') + 1, m.lastIndexOf(')')).replace(/\/\*[\s\S]*?\*\//g, '').trim();
      return 'await ' + (arg === 'this.workerSrc' ? '__pdfjsFakeWorkerImport' : '__pdfjsWasmImport') + '(' + arg + ')';
    }, 'await import(...)', opts.dynImport || 'all');
  const rewrittenBodyLen = before.length + windows.reduce((a, w) => a + w.delta, 0);
  if (body.length !== rewrittenBodyLen) { throw new Error(label + ': 长度账不平(有非预期改动)'); }

  /* --- 3. 组装 --- */
  const assign = Object.keys(map).map(e => e.exported + ': ' + e.local).join(', ');
  const exported = '{\n' + map.map(e => '  ' + e.exported + ': ' + e.local).join(',\n') + '\n}';
  const script = '(function () {\n"use strict";\n/* pdfjs-dist ' + opts.version + ' legacy —— ESM 机械化改写为 classic 脚本,勿手改 */\n'
    + opts.prelude + '\n'
    + body
    + '\n' + opts.assignTarget + ' = ' + exported + ';\n})();\n';

  /* --- 4. 断言:改写后没有静态 import/export、没有动态 import 调用,且能被 V8 编译 --- */
  const problems = [];
  if (/\bexport\b/.test(body)) { problems.push('改写后 body 仍含 export'); }
  if (/import\.meta/.test(body)) { problems.push('改写后 body 仍含 import.meta'); }
  /* _createCDNWrapper 里那句 `await import("${t}");` 是模板字符串(死代码:只有跨源 workerSrc 才会用到),
     其余位置不得再有任何 import(...) 调用 */
  const DEAD = '`await import("${t}");`';
  const deadCount = body.split(DEAD).length - 1;
  const probe = body.split(DEAD).join('/*DEAD*/');
  const residualImport = (probe.match(/import\s*\(/g) || []).length;
  if (residualImport) { problems.push('改写后仍含 ' + residualImport + ' 处 import(...) 调用'); }
  let compileErr = null;
  try { new Function(script); } catch (e) { compileErr = e.message; }
  if (compileErr) { problems.push('new Function 编译失败: ' + compileErr); }
  if (problems.length) { throw new Error(label + ': ' + problems.join('; ')); }

  notes.push('export 条目 ' + map.length + ' | ' + Object.keys(seen).map(k => k + '×' + seen[k]).join(', ')
    + ' | 残留 import(...) 调用 ' + residualImport + ',死代码模板串 ' + deadCount);
  return { script, body, before, map, windows, residualImport, deadCount, notes };
}

/* ---------------------------------------------------------------------------
 * 两个库的预置代码(prelude)
 * ------------------------------------------------------------------------- */
const DISPLAY_PRELUDE = `var __pdfjsModuleUrl = (typeof document !== "undefined" && document.baseURI) || "";
/* pdf.js 在"没有真 worker 也没有主线程 handler"时会走到这里;正常流程(workerPort / 主线程
 * fake worker)根本不会调用它。这里做最后一层兜底:把内联的 worker 源码在页面主线程跑一次,
 * 让 pdf.js 拿到 WorkerMessageHandler,而不是抛未捕获错误。 */
function __pdfjsFakeWorkerImport(spec) {
  var handler = null;
  try { handler = globalThis.pdfjsWorker || null; } catch (e) {}
  if (handler && handler.WorkerMessageHandler) { return Promise.resolve(handler); }
  try {
    if (typeof document !== "undefined") {
      var el = document.getElementById("${WORKER_SRC_ID}");
      if (el && typeof el.textContent === "string" && el.textContent) {
        var s = document.createElement("script");
        s.textContent = el.textContent;
        (document.head || document.documentElement).appendChild(s);
        if (s.parentNode) { s.parentNode.removeChild(s); }
      }
    }
  } catch (e) {}
  try { handler = globalThis.pdfjsWorker || null; } catch (e) {}
  return handler && handler.WorkerMessageHandler
    ? Promise.resolve(handler)
    : Promise.reject(new Error("pdf.js fake worker 不可用:无法加载 " + String(spec)));
}`;

const WORKER_PRELUDE = `var __pdfjsModuleUrl = (typeof location !== "undefined" && location.href) || "";
/* 原本是动态 import(wasmUrl 下的 JBIG2/OpenJPEG 模块)。单文件内联环境不加载外部文件,
 * pdf.js 侧用 useWasm:false 关掉这条路径;这里保留一个干净的 reject 作兜底(调用点在 try/catch 内)。 */
function __pdfjsWasmImport(spec) {
  return Promise.reject(new Error("内联单文件环境不加载外部模块:" + String(spec)));
}`;

/* ---------------------------------------------------------------------------
 * 流异步迭代垫片(Chromium<124 / 旧 Safari 缺 ReadableStream.prototype[Symbol.asyncIterator])
 * —— 单一来源,同时挂到 display 与 worker 两个 prelude(prelude 在各自 IIFE 的库体之前执行)。
 * 背景:pdf.js 6 的 display `getTextContent()` 内部是 `for await (const t of this.streamTextContent())`,
 * 在缺该能力的宿主上直接抛 `TypeError: ... is not async iterable`(用户真机 2026-09-15 报错);
 * 适配层的取文本路径已改为手动 reader 泵,这里再补一层通用垫片兜住其余异步迭代调用点。
 * 覆盖边界:只覆盖 pdf.js display realm + pdf.js worker realm;src/office.part 内 docstream 自带的
 * 那份 pdf.js 不在覆盖内(其唯一同类调用点属不可达的签名/注释路径)—— 见 docs/PDFJS-NOTES.md §9。
 * ------------------------------------------------------------------------- */
const STREAM_ITER_SHIM = `/* 流异步迭代垫片(宿主缺 ReadableStream.prototype[Symbol.asyncIterator] 时补一个符合规范的实现;有则不动)。
   覆盖 pdf.js display realm + pdf.js worker realm 两个 realm;未实现 throw():for await 的正常路径只用
   next()/return(),本应用不产生需要 throw 的场景(有意简化)。 */
(function (shimGlobal) {
  try {
    var RS = (typeof ReadableStream !== "undefined") ? ReadableStream : null;
    var sym = (typeof Symbol !== "undefined") ? Symbol.asyncIterator : null;
    /* 打补丁【前】的原生能力快照 —— 必须写全局:本 prelude 与 PDFKit 是两个独立 IIFE,模块级 var 跨不过去;
       worker realm 的那份标志主线程读不到(诊断面板只报主线程,文案不声称 worker 已实测)。
       语义 = 【首次快照】:已初始化则不覆写 —— 主线程降级(runWorkerOnMainThread 把 worker 载荷当脚本元素
       注入主文档)会让本段在主 realm 再跑一次,那时 ReadableStream 原型已被上一份垫片改过,照常写会把
       native 从 false 覆写为 true,令 diag().streamIter 假报「原生」。worker realm 是全新 global ⇒ 照常初始化。 */
    if (typeof shimGlobal.__pdfStreamIterNative === "undefined") {
      try { shimGlobal.__pdfStreamIterNative = !!(RS && sym && typeof RS.prototype[sym] === "function"); }
      catch (e0) { shimGlobal.__pdfStreamIterNative = false; }
    }
    if (!RS || !sym || typeof RS.prototype[sym] === "function") { return; }
    RS.prototype[sym] = function () {
      var reader = this.getReader();
      var it = {};
      function unlock() {
        /* 与原生对齐:原生 [Symbol.asyncIterator] 经 ReadableStreamReaderGenericRelease 释放读锁 */
        try { if (typeof reader.releaseLock === "function") { reader.releaseLock(); } } catch (e2) {}
      }
      it.next = function () {
        return reader.read().then(function (r) {
          return r.done ? { done: true, value: undefined } : { done: false, value: r.value };
        });
      };
      it["return"] = function () {
        var doneValue = { done: true, value: undefined };
        try {
          return reader.cancel().then(function () { unlock(); return doneValue; },
            function () { unlock(); return doneValue; });
        } catch (e) {
          unlock();
          return Promise.resolve(doneValue);
        }
      };
      /* 原生 ReadableStreamAsyncIteratorPrototype 的 [Symbol.asyncIterator] 返回迭代器自身;补上以提高互换性 */
      it[sym] = function () { return it; };
      return it;
    };
  } catch (e) { /* 只读原型等极端情况:忽略 —— 取文本路径另有手动 reader 泵兜底 */ }
})(typeof globalThis !== "undefined" ? globalThis : this);
`;

/* ===========================================================================
 * PDFKit 封装层(ES5:var + function,无箭头函数 / 可选链 / 模板字符串)
 * ========================================================================= */
const PDFKIT_JS = `/* ===== PDFKit —— pdf.js 统一封装(文本抽取 + 页面渲染为图片 + 按页取图)=====
 * 依赖同一 part 内先出现的两个块:window.pdfjsLib 与 #${WORKER_SRC_ID}
 * 契约:
 *   window.PDFKit.available            bool
 *   window.PDFKit.why                  string(available=false 时说明)
 *   window.PDFKit.version              pdf.js 版本号
 *   window.PDFKit.extractText(buf, opts)  -> Promise<{ok,text,pages,chars,truncated,error?}>
 *   window.PDFKit.renderPages(buf, opts)  -> Promise<{ok,images:[{dataUrl,width,height,page}],pages,truncated,error?}>
 *   window.PDFKit.pageImages(buf, opts)   -> Promise<{ok,pageCount,scanned,pages,truncated,ms,mode,error?}>
 *        pages:[{page,hasImages,imgOps,maskOps,repeats,mode:"rect"|"full",rects:[{x,y,w,h}]}]
 *        opts 额外 { region:"rect" } —— 默认 rect;给定 "full" 时全部页按整页兜底
 *   window.PDFKit.renderCrops(buf, opts)  -> Promise<{ok,images:[{page,dataUrl,width,height}],ms,mode,error?}>
 *        opts 额外 { groups:[{page,rect:{x,y,w,h}}] } —— 每页一次渲染,画布 = 该页 rect 的并集包围盒
 * opts: { maxPages:50, dpi:150, format:"image/jpeg", quality:0.85,
 *         onProgress:(done,total)=>void, maxChars:400000, timeout:60000 }
 * 额外(非契约):PDFKit.mode() 返回 worker 加载方式,PDFKit.setupWorker() 提前初始化 worker,
 *   PDFKit.diag() 返回自检读数(版本/worker 模式/取文本路径/流异步迭代/取图),
 *   PDFKit.notes 为「仅提示、不拦」的环境备注(不参与 available 判据)。
 * ===================================================================== */
(function (g) {
  'use strict';

  var SRC_ID = '${WORKER_SRC_ID}';
  var DEF = {
    maxPages: 50, dpi: 150, format: 'image/jpeg', quality: 0.85,
    maxChars: 400000, timeout: 60000
  };
  var lib = g.pdfjsLib || null;
  var mode = '';            /* '' 未初始化 | 'worker' | 'main-thread' | 'none' */
  var textFallback = false; /* true = 上一次取文本回落到了 getTextContent(pdf.js 版本没有 streamTextContent 时),见 pageTextViaReader */
  var setupDone = false;
  var sharedWorker = null;  /* 真 worker(整页复用) */
  var sharedPdfWorker = null; /* 对应 PDFWorker 实例,显式传给 getDocument —— task.destroy() 只销毁该文档的 transport,共享 worker 存活 */

  /* ---------- 兼容垫片(仅当缺失时定义;pdf.js legacy 自带 core-js,这里是兜底) ---------- */
  if (typeof Promise.withResolvers !== 'function') {
    Promise.withResolvers = function () {
      var d = {};
      d.promise = new Promise(function (res, rej) { d.resolve = res; d.reject = rej; });
      return d;
    };
  }
  if (typeof g.structuredClone !== 'function') {
    g.structuredClone = function (v) { return v === undefined ? v : JSON.parse(JSON.stringify(v)); };
  }
  if (typeof Array.prototype.at !== 'function') {
    Array.prototype.at = function (i) { i = Math.trunc(i) || 0; return i < 0 ? this[this.length + i] : this[i]; };
  }

  /* ---------- 环境能力 ---------- */
  function hasCanvas() {
    try {
      var c = document.createElement('canvas');
      return !!(c && c.getContext && c.getContext('2d'));
    } catch (e) { return false; }
  }

  /* missing() 只列**拦截项**(进 available 判据);「仅提示、不拦」的环境备注走 envNotes()/api.notes,
     两条通道不混:提示项一旦混进这里,会让原本可用的 PDF 功能被整个关掉(用户真机 122 就是反例)。 */
  function missing() {
    var m = [];
    if (!lib || typeof lib.getDocument !== 'function') { m.push('pdfjsLib'); }
    if (typeof Promise !== 'function') { m.push('Promise'); }
    if (typeof Uint8Array !== 'function' || typeof ArrayBuffer !== 'function') { m.push('TypedArray'); }
    if (!hasCanvas()) { m.push('Canvas2D'); }
    return m;
  }

  /* 宿主是否具备 ReadableStream 异步迭代(打补丁后再读即为「可用」,故另用 __pdfStreamIterNative 记原生态) */
  function hasStreamIter() {
    try {
      var RS = (typeof g.ReadableStream !== 'undefined') ? g.ReadableStream : null;
      var sym = (typeof Symbol !== 'undefined') ? Symbol.asyncIterator : null;
      return !!(RS && sym && typeof RS.prototype[sym] === 'function');
    } catch (e) { return false; }
  }

  /* 仅提示、不拦:不参与 available,只作为诊断面板/上报的备注 */
  function envNotes() {
    var n = [];
    var native = false;
    try { native = !!g.__pdfStreamIterNative; } catch (e) { native = false; }
    if (!native) {
      n.push(hasStreamIter()
        ? 'ReadableStream 异步迭代:宿主缺失(Chrome<124 一类),已由垫片兜底,取文本走手动 reader 泵'
        : 'ReadableStream 异步迭代:宿主缺失且垫片未生效 —— 取文本仍走手动 reader 泵,渲染路径可能受限');
    }
    return n;
  }

  /* ---------- worker 加载:真 worker 优先(经典脚本 blob,http/file 都可用),失败降级主线程 ---------- */
  function workerSrcText() {
    var el = document.getElementById(SRC_ID);
    return el && typeof el.textContent === 'string' ? el.textContent : '';
  }

  function runWorkerOnMainThread(src) {
    if (g.pdfjsWorker && g.pdfjsWorker.WorkerMessageHandler) { return true; }
    try {
      var s = document.createElement('script');
      s.textContent = src;
      (document.head || document.documentElement || document.body).appendChild(s);
      if (s.parentNode) { s.parentNode.removeChild(s); }
    } catch (e) { return false; }
    return !!(g.pdfjsWorker && g.pdfjsWorker.WorkerMessageHandler);
  }

  function useMainThread(src) {
    mode = runWorkerOnMainThread(src) ? 'main-thread' : 'none';
    return mode;
  }

  function setupWorker() {
    if (setupDone) { return mode; }
    setupDone = true;
    var src = workerSrcText();
    if (!src) { mode = 'none'; return mode; }
    var url = null;
    if (g.Worker && g.Blob && g.URL && g.URL.createObjectURL) {
      try { url = g.URL.createObjectURL(new Blob([src], { type: 'text/javascript' })); } catch (e) { url = null; }
    }
    if (url) {
      var w = null;
      try { w = new g.Worker(url); } catch (e) { w = null; }
      if (w) {
        try {
          /* v4+ 内部固定 new Worker(workerSrc,{type:'module'}),module worker 在 file:// 下不工作;
             自己建经典 worker 并通过 workerPort 交给 pdf.js,http/file 行为一致。 */
          sharedPdfWorker = new lib.PDFWorker({ port: w });
          g.pdfjsLib.GlobalWorkerOptions.workerPort = w;
          lib.GlobalWorkerOptions.workerSrc = url;
          sharedWorker = w;
          mode = 'worker';
          return mode;
        } catch (e) {
          try { w.terminate(); } catch (e2) {}
          sharedPdfWorker = null;
        }
      }
      try { g.URL.revokeObjectURL(url); } catch (e) {}
    }
    return useMainThread(src);
  }

  /* ---------- 输入归一化(总是复制,pdf.js 会把 buffer transfer 给 worker) ---------- */
  function base64ToBytes(b64) {
    var s = b64.indexOf(',') >= 0 ? b64.slice(b64.indexOf(',') + 1) : b64;
    var bin = g.atob(s);
    var out = new Uint8Array(bin.length);
    for (var i = 0; i < bin.length; i++) { out[i] = bin.charCodeAt(i) & 255; }
    return out;
  }

  function toBytes(input) {
    try {
      if (typeof input === 'string') { return base64ToBytes(input); }
      if (input instanceof ArrayBuffer) { return new Uint8Array(input.slice(0)); }
      if (input && typeof input.byteLength === 'number' && input.buffer instanceof ArrayBuffer) {
        return new Uint8Array(input.buffer.slice(input.byteOffset, input.byteOffset + input.byteLength));
      }
    } catch (e) {}
    return null;
  }

  /* 开文档:返回 { task, promise }。task = PDFDocumentLoadingTask —— 释放**必须**走它:
     PDFDocumentProxy 没有 destroy(),旧写法 doc.destroy() 是静默 no-op(文档与传输层永不释放)。
     显式传 params.worker 时 getDocument 不设内部 _worker ⇒ task.destroy() 只销毁该文档的
     transport(worker 侧同 docId 的文档随之销毁),整页复用的共享 worker 存活。 */
  function openDoc(bytes) {
    setupWorker();
    var params = {
      data: bytes,
      isEvalSupported: false,
      useSystemFonts: true,
      useWasm: false,          /* 不用外部 wasm(单文件零请求),JBIG2/JPEG2000 走内置 JS 解码 */
      disableAutoFetch: true,
      disableStream: true
    };
    if (mode === 'worker' && sharedPdfWorker) { params.worker = sharedPdfWorker; }
    var task = lib.getDocument(params);
    return { task: task, promise: task.promise };
  }

  /* 释放一次 openDoc 的 loadingTask(成功 / 失败 / 超时三条路径共用,可重复调用:destroy() 幂等)。
     destroy() 本身是异步的且可能在 transport 销毁失败时拒绝 —— 释放尽力而为,不让它冒泡成任务失败。 */
  function releaseJob(job) {
    if (!job || !job.task) { return; }
    try {
      var p = job.task.destroy();
      if (p && typeof p.then === 'function') { p.then(function () {}, function () {}); }
    } catch (e) {}
  }

  /* ---------- 工具 ---------- */
  function num(v, d) { return typeof v === 'number' && isFinite(v) && v > 0 ? v : d; }
  function now() { return (g.performance && g.performance.now) ? g.performance.now() : Date.now(); }
  function errText(e) {
    if (!e) { return 'unknown error'; }
    if (typeof e === 'string') { return e; }
    return e.message ? (e.name ? e.name + ': ' + e.message : e.message) : String(e);
  }
  function fail(msg, extra) {
    var r = { ok: false, error: msg, pages: 0, truncated: false };
    if (extra) { for (var k in extra) { if (extra.hasOwnProperty(k)) { r[k] = extra[k]; } } }
    return r;
  }
  function withTimeout(p, ms, onLate) {
    return new Promise(function (resolve, reject) {
      var settled = false;
      var timer = setTimeout(function () {
        if (settled) { return; }
        settled = true;
        if (onLate) { try { onLate(); } catch (e) {} }
        reject(new Error('处理超时(>' + ms + 'ms)'));
      }, ms);
      p.then(function (v) {
        if (settled) { return; }
        settled = true; clearTimeout(timer); resolve(v);
      }, function (e) {
        if (settled) { return; }
        settled = true; clearTimeout(timer); reject(e);
      });
    });
  }

  /* ---------- 文本:手动 reader 泵取 + 按行/空格重排 ----------
   * pdf.js 6 的 display getTextContent() 内部是 "for await (const t of this.streamTextContent())":
   * 宿主缺 ReadableStream.prototype[Symbol.asyncIterator](Chrome<124 / 旧 Safari)时直接抛
   * "TypeError: e is not async iterable"(用户真机 2026-09-15 报错)。这里改为逐个 read() 拉流,
   * 不依赖异步迭代;垫片(prelude)只作第二层保险。 */
  function drainStream(stream, onChunk) {
    return new Promise(function (resolve, reject) {
      var reader, done = false;
      try { reader = stream.getReader(); } catch (e) { reject(e); return; }
      function unlock() {
        try { if (typeof reader.releaseLock === 'function') { reader.releaseLock(); } } catch (e) {}
      }
      function finish(err) {
        if (done) { return; }
        done = true;
        if (err) {
          /* 失败/中止:先 cancel(尽力而为,不 await、不抛)再 releaseLock —— 与规范版
             [Symbol.asyncIterator] 在 next() 拒绝时释放锁的行为对齐,避免流在失败后保持 locked
             (对本应用无功能影响,纯卫生)。超时链路:withTimeout → loadingTask.destroy() → 待读 promise 拒绝 → 这里 */
          try {
            if (typeof reader.cancel === 'function') {
              var p = reader.cancel();
              if (p && typeof p.then === 'function') { p.then(function () {}, function () {}); }
            }
          } catch (e) {}
          unlock();
          reject(err);
          return;
        }
        unlock();
        resolve();
      }
      function pump() {
        reader.read().then(function (r) {
          if (r.done) { finish(null); return; }
          try { onChunk(r.value); } catch (e) { /* 单个 chunk 坏不致命 */ }
          pump();
        }, finish);
      }
      pump();
    });
  }

  /* 与 pdf.js getTextContent() 语义逐条等价:{items, styles, lang} 合并顺序一致。
   * lang 对齐原文 "i.lang ??= t.lang":首个非 nullish 值生效;全为 nullish 时终值是最后一个 chunk 的
   *   lang(实际取值只能是 null/undefined,消费者只看真值)⇒ 与原文等价。
   * styles 逐键拷贝 ≡ Object.assign(键为字体 id 字符串);items 顺序 push ≡ push(...chunk.items)。
   * 旁路说明:原文首句 "if (this._transport._htmlForXfa) …" 在新路径下不再命中 —— 本应用 openDoc
   *   不启用 XFA ⇒ 影响可忽略(见 docs/PDFJS-NOTES.md §9)。 */
  function pageTextViaReader(page, opts) {
    var acc = { items: [], styles: Object.create(null), lang: null };
    /* 主分支与兜底分支同一传参口径:opts 缺省 ⇒ false,与 pdf.js 默认值/既有调用点一致(P2-3) */
    var imc = !!(opts && opts.includeMarkedContent);
    /* diag().textStrategy 的读数 = **本次**实际走的路径:逐次赋值而非一次性置真 ——
       否则任一次兜底会把读数永久钉成 getTextContent(粘性状态残留) */
    textFallback = (typeof page.streamTextContent !== 'function');
    if (!textFallback) {
      var st = page.streamTextContent({
        includeMarkedContent: imc,
        disableNormalization: false
      });
      if (st && typeof st.getReader === 'function') {
        return drainStream(st, function (chunk) {
          if (!chunk) { return; }
          if (acc.lang == null && chunk.lang != null) { acc.lang = chunk.lang; }
          if (chunk.styles) {
            for (var k in chunk.styles) {
              if (Object.prototype.hasOwnProperty.call(chunk.styles, k)) { acc.styles[k] = chunk.styles[k]; }
            }
          }
          if (chunk.items) {
            for (var i = 0; i < chunk.items.length; i++) { acc.items.push(chunk.items[i]); }
          }
        }).then(function () { return acc; });
      }
    }
    /* 兜底:老/异版本 pdf.js 没有 streamTextContent 时才走 getTextContent(它自身在无 asyncIterator
       的宿主里会挂 ⇒ 这条分支只对「有该 API 的新版本」生效),textFallback 已在函数开头按本次能力标好。
       includeMarkedContent 与主分支同口径(同变量 imc),不再硬编码 false(P2-3) */
    if (typeof page.getTextContent === 'function') {
      textFallback = true;    /* streamTextContent 在位但没给出可读流(理论不可达)⇒ 读数按实际路径标 */
      return page.getTextContent({ includeMarkedContent: imc });
    }
    return Promise.reject(new Error('该 pdf.js 版本既无 streamTextContent 也无 getTextContent'));
  }

  /* ---------- 自检读数(设置 → 环境 的「诊断」用) ----------
   * 读数本身只读;唯一副作用 = mode 会经 setupWorker() 首次建立共享 worker / 注入主线程 worker 载荷
   *   (setupDone 守卫 ⇒ 一次性、幂等,重复调用不再改状态)。 */
  function diag() {
    var native = false;
    try { native = !!g.__pdfStreamIterNative; } catch (e) { native = false; }
    var has = hasStreamIter();
    return {
      version: lib && lib.version ? lib.version : '',
      mode: setupWorker(),
      textStrategy: textFallback ? 'getTextContent' : 'reader',
      /* native = 垫片打补丁【前】的能力(全局标志,跨 IIFE);shimmed = 现在可用但不原生;
         worker = 固定字符串 —— worker realm 的实测标志主线程读不到,文案不得声称 worker 已实测 */
      streamIter: { native: native, shimmed: !native && has, worker: 'static-inferred' },
      regionRender: (typeof renderCrops === 'function')   /* 精确取区属批次 B;未实现时恒 false */
    };
  }

  /* ---------- 文本:把 getTextContent() 的 item 数组按行/空格重排 ---------- */
  function pageText(content) {
    var items = (content && content.items) || [];
    var out = '';
    var prev = null;
    for (var i = 0; i < items.length; i++) {
      var it = items[i];
      if (!it || typeof it.str !== 'string') { continue; }
      if (it.str === '') {
        if (it.hasEOL) { out += '\\n'; prev = null; }
        continue;
      }
      var tr = it.transform || [1, 0, 0, 1, 0, 0];
      var x = tr[4], y = tr[5];
      var h = Math.abs(it.height || tr[3] || 0) || 1;
      if (prev) {
        if (Math.abs(prev.y - y) > h * 0.6) {
          out += '\\n';
        } else if (x - (prev.x + prev.w) > h * 0.25) {
          out += ' ';
        }
      }
      out += it.str;
      prev = { x: x, y: y, w: typeof it.width === 'number' ? it.width : 0 };
      if (it.hasEOL) { out += '\\n'; prev = null; }
    }
    return out.replace(/[ \\t]+\\n/g, '\\n').replace(/\\n{3,}/g, '\\n\\n').replace(/^[\\s\\n]+|[\\s\\n]+$/g, '');
  }

  /* ---------- API:extractText ---------- */
  function extractText(input, opts) {
    var o = opts || {};
    var maxPages = Math.floor(num(o.maxPages, DEF.maxPages));
    var maxChars = Math.floor(num(o.maxChars, DEF.maxChars));
    var timeout = num(o.timeout, DEF.timeout);
    var onProgress = typeof o.onProgress === 'function' ? o.onProgress : null;
    var t0 = now();
    var miss = missing();
    if (miss.length) { return Promise.resolve(fail('环境不支持: 缺少 ' + miss.join(', '), { text: '', chars: 0 })); }
    var bytes = toBytes(input);
    if (!bytes || !bytes.length) { return Promise.resolve(fail('PDF 数据为空或类型不支持(需要 ArrayBuffer/Uint8Array/base64)', { text: '', chars: 0 })); }

    var aborted = false;
    var job = openDoc(bytes);
    var work = job.promise.then(function (d) {
      if (aborted) { return fail('已超时,放弃该任务', { text: '', chars: 0 }); }
      var total = Math.min(d.numPages, maxPages);
      var text = '';
      var done = 0;
      var truncated = d.numPages > total;
      function step() {
        if (done >= total) { return Promise.resolve(); }
        var n = done + 1;
        return d.getPage(n).then(function (page) {
          return pageTextViaReader(page, {}).then(function (content) {
            done = n;
            var body = pageText(content);
            var block = '----- 第 ' + n + ' 页 -----' + (body ? '\\n' + body : '');
            var sep = text ? '\\n\\n' : '';
            if (text.length + sep.length + block.length > maxChars) {
              text += sep + block.slice(0, Math.max(0, maxChars - text.length - sep.length));
              truncated = true;
              return;
            }
            text += sep + block;
            if (typeof page.cleanup === 'function') { page.cleanup(); }
            if (onProgress) { try { onProgress(n, total); } catch (e) {} }
            return step();
          });
        });
      }
      return step().then(function () {
        return {
          ok: true, text: text, pages: done, pageCount: d.numPages,
          chars: text.length, truncated: truncated, ms: Math.round(now() - t0), mode: mode
        };
      });
    });

    return withTimeout(work, timeout, function () { aborted = true; releaseJob(job); })
      .then(function (res) {
        releaseJob(job);
        return res;
      }, function (e) {
        aborted = true;
        releaseJob(job);
        return fail(errText(e), { text: '', chars: 0, ms: Math.round(now() - t0) });
      });
  }

  /* ---------- API:renderPages ---------- */
  function renderPages(input, opts) {
    var o = opts || {};
    var maxPages = Math.floor(num(o.maxPages, DEF.maxPages));
    var dpi = num(o.dpi, DEF.dpi);
    var format = o.format === 'image/png' ? 'image/png' : 'image/jpeg';
    var quality = typeof o.quality === 'number' && isFinite(o.quality) ? o.quality : DEF.quality;
    var timeout = num(o.timeout, DEF.timeout);
    var onProgress = typeof o.onProgress === 'function' ? o.onProgress : null;
    var t0 = now();
    var miss = missing();
    if (miss.length) { return Promise.resolve(fail('环境不支持: 缺少 ' + miss.join(', '), { images: [] })); }
    var bytes = toBytes(input);
    if (!bytes || !bytes.length) { return Promise.resolve(fail('PDF 数据为空或类型不支持(需要 ArrayBuffer/Uint8Array/base64)', { images: [] })); }

    var canvas = null;
    var aborted = false;
    var scale = dpi / 72;
    var job = openDoc(bytes);
    var work = job.promise.then(function (d) {
      if (aborted) { return fail('已超时,放弃该任务', { images: [] }); }
      var total = Math.min(d.numPages, maxPages);
      var images = [];
      var done = 0;
      var truncated = d.numPages > total;
      function step() {
        if (done >= total) { return Promise.resolve(); }
        var n = done + 1;
        return d.getPage(n).then(function (page) {
          var vp = page.getViewport({ scale: scale });
          canvas = document.createElement('canvas');
          canvas.width = Math.floor(vp.width);
          canvas.height = Math.floor(vp.height);
          var ctx = canvas.getContext('2d');
          ctx.fillStyle = '#ffffff';
          ctx.fillRect(0, 0, canvas.width, canvas.height);
          return page.render({ canvasContext: ctx, viewport: vp, background: '#ffffff' }).promise.then(function () {
            images.push({
              dataUrl: canvas.toDataURL(format, quality),
              width: canvas.width, height: canvas.height, page: n
            });
            canvas.width = 0; canvas.height = 0; canvas = null;
            done = n;
            if (typeof page.cleanup === 'function') { page.cleanup(); }
            if (onProgress) { try { onProgress(n, total); } catch (e) {} }
            return step();
          });
        });
      }
      return step().then(function () {
        return {
          ok: true, images: images, pages: done, pageCount: d.numPages,
          truncated: truncated, ms: Math.round(now() - t0), mode: mode
        };
      });
    });

    return withTimeout(work, timeout, function () { aborted = true; releaseJob(job); })
      .then(function (res) {
        releaseJob(job);
        return res;
      }, function (e) {
        aborted = true;
        releaseJob(job);
        if (canvas) { canvas.width = 0; canvas.height = 0; }
        return fail(errText(e), { images: [], ms: Math.round(now() - t0) });
      });
  }

  /* ---------- 取图:算子表扫描(Route B/C)+ 子区裁剪渲染 ----------
   * 与 extractText/renderPages 并列的独立能力:各自 openDoc 一次并在结束时 releaseJob()(销毁该文档的
   * loadingTask/transport);每次读到的页
   * 处理完各自 page.cleanup()(getOperatorList 会把图像数据拉进缓存,不清会在多页/大图 PDF 上抬内存峰值)。
   * 几何口径(P3 探针在真实样件上逐位对拍 + pdf.js 6.3.289 display/canvas.js 实读):
   *   1. 初值 CTM m = page.getViewport({scale: dpi/72}).transform(设备空间,含 y 翻转);
   *   2. save/restore 压弹栈;transform → Util.transform(m, args);paintFormXObjectBegin/End 压弹栈
   *      (args[0] = 矩阵);beginGroup/endGroup 按 save/restore 处理并以后乘 args[0].matrix;
   *   3. 图像在 CTM 下占【单位方】[0,1]² —— args 里的 w/h 是位图内在像素而不是放置尺寸 ⇒
   *      rect = 单位方四角变换后的包围盒(设备像素);group 变体逐项用 map[i].transform(args[1] 是图集 imgData);
   *   4. 保守降级(该页 mode:"full"):paintImageXObjectRepeat 命中、group/矩阵结构异常、栈不平衡、
   *      该页算不出 rect(只剩掩码);region 非 "rect" 时全部按整页兜底。
   * 例外口径:Util.applyTransform(p, m) 在本版本是【原地改写 p 并返回 undefined】⇒ 一律传副本、读回 p。 */

  /* 6 元仿射矩阵校验:不是 6 个有限数 ⇒ null(视为结构异常,调用方保守降级) */
  function matSix(v) {
    if (!v || typeof v.length !== 'number' || v.length !== 6) { return null; }
    for (var i = 0; i < 6; i++) { if (typeof v[i] !== 'number' || !isFinite(v[i])) { return null; } }
    return v;
  }

  /* m1 之后叠加 m2(与 ctx.transform 的后乘口径一致);Util.transform 不可用时用同式本地兜底 */
  function mulMat(m1, m2) {
    try {
      if (lib && lib.Util && typeof lib.Util.transform === 'function') { return lib.Util.transform(m1, m2); }
    } catch (e) {}
    return [
      m1[0] * m2[0] + m1[2] * m2[1],
      m1[1] * m2[0] + m1[3] * m2[1],
      m1[0] * m2[2] + m1[2] * m2[3],
      m1[1] * m2[2] + m1[3] * m2[3],
      m1[0] * m2[4] + m1[2] * m2[5] + m1[4],
      m1[1] * m2[4] + m1[3] * m2[5] + m1[5]
    ];
  }

  /* 点变换:必须先传副本 —— 本版本 Util.applyTransform(p, m) 原地改写 p 且返回 undefined */
  function mapPt(mm, x, y) {
    var p = [x, y];
    try {
      if (lib && lib.Util && typeof lib.Util.applyTransform === 'function') {
        lib.Util.applyTransform(p, mm);
        if (isFinite(p[0]) && isFinite(p[1])) { return p; }
      }
    } catch (e) {}
    return [mm[0] * x + mm[2] * y + mm[4], mm[1] * x + mm[3] * y + mm[5]];
  }

  /* 单位方 [0,1]² 经矩阵 mm 变换后的轴对齐包围盒(设备像素) */
  function unitBox(mm) {
    var a = mapPt(mm, 0, 0), b = mapPt(mm, 1, 0), c = mapPt(mm, 0, 1), d = mapPt(mm, 1, 1);
    var x0 = Math.min(a[0], b[0], c[0], d[0]), x1 = Math.max(a[0], b[0], c[0], d[0]);
    var y0 = Math.min(a[1], b[1], c[1], d[1]), y1 = Math.max(a[1], b[1], c[1], d[1]);
    return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
  }

  function rectOk(r) {
    return !!r && isFinite(r.x) && isFinite(r.y) && isFinite(r.w) && isFinite(r.h) && r.w > 0 && r.h > 0;
  }

  /* 扫一页的算子表:统计图像 op 并算出各图的设备像素矩形;结构异常记进 uncertain(该页转整页兜底) */
  function scanImages(ops, vp) {
    var O = (lib && lib.OPS) || {};
    var res = { imgOps: 0, maskOps: 0, repeats: 0, groups: 0, rects: [], uncertain: '', stackLeft: 0 };
    var vt = vp ? vp.transform : null;
    var m = (vt && vt.length === 6) ? [vt[0], vt[1], vt[2], vt[3], vt[4], vt[5]] : null;
    if (!matSix(m)) { res.uncertain = 'viewport'; return res; }
    var fn = ops ? ops.fnArray : null, ar = ops ? ops.argsArray : null;
    if (!fn || !ar || typeof fn.length !== 'number') { res.uncertain = 'operatorList'; return res; }
    var stack = [];
    for (var i = 0; i < fn.length; i++) {
      var f = fn[i], a = ar[i] || [];
      if (f === O.save) { stack.push(m.slice()); }
      else if (f === O.restore) {
        if (stack.length) { m = stack.pop(); } else { res.uncertain = res.uncertain || 'restore-underflow'; }
      } else if (f === O.transform) {
        var tm = matSix(a);
        if (tm) { m = mulMat(m, tm); } else { res.uncertain = res.uncertain || 'transform-args'; }
      } else if (f === O.paintFormXObjectBegin) {
        stack.push(m.slice());
        if (a[0] != null) {
          var fm = matSix(a[0]);
          if (fm) { m = mulMat(m, fm); } else { res.uncertain = res.uncertain || 'form-matrix'; }
        }
      } else if (f === O.paintFormXObjectEnd) {
        if (stack.length) { m = stack.pop(); } else { res.uncertain = res.uncertain || 'form-underflow'; }
      } else if (f === O.beginGroup) {
        res.groups++;
        stack.push(m.slice());
        var g0 = a[0];
        if (g0 != null && typeof g0 === 'object') {
          /* canvas.js: beginGroup(ctx, {bbox, matrix, ...}) —— 只有 matrix 参与 CTM 后乘 */
          if (g0.matrix != null) {
            var gm = matSix(g0.matrix);
            if (gm) { m = mulMat(m, gm); } else { res.uncertain = res.uncertain || 'group-matrix'; }
          }
        } else {
          var gm2 = matSix(g0) || matSix(a[1]);
          if (gm2) { m = mulMat(m, gm2); }
        }
      } else if (f === O.endGroup) {
        if (stack.length) { m = stack.pop(); } else { res.uncertain = res.uncertain || 'group-underflow'; }
      } else if (f === O.paintImageXObject || f === O.paintInlineImageXObject) {
        res.imgOps++;
        var r1 = unitBox(m);
        if (rectOk(r1)) { res.rects.push(r1); } else { res.uncertain = res.uncertain || 'rect'; }
      } else if (f === O.paintInlineImageXObjectGroup) {
        res.imgOps++;
        var map = a[0], cnt = (map && typeof map.length === 'number') ? map.length : 0;
        if (!cnt) { res.uncertain = res.uncertain || 'inline-group'; }
        for (var j = 0; j < cnt; j++) {
          var em = matSix(map[j] ? map[j].transform : null);
          if (!em) { res.uncertain = res.uncertain || 'inline-group-entry'; continue; }
          var r2 = unitBox(mulMat(m, em));
          if (rectOk(r2)) { res.rects.push(r2); } else { res.uncertain = res.uncertain || 'rect'; }
        }
      } else if (f === O.paintImageXObjectRepeat) {
        res.repeats++;   /* args 形状未证 ⇒ 不计 rect,该页命中即整页兜底 */
      } else if (f === O.paintImageMaskXObject || f === O.paintImageMaskXObjectGroup ||
                 f === O.paintImageMaskXObjectRepeat || f === O.paintSolidColorImageMask) {
        res.maskOps++;   /* 掩码/纯色无 OCR 价值:不计 rect,只计入 hasImages */
      }
    }
    res.stackLeft = stack.length;
    return res;
  }

  /* ---------- API:pageImages(按页含图判定 + 精确取区)----------
   * -> {ok,pageCount,scanned:[页号],pages:[{page,hasImages,imgOps,maskOps,repeats,mode,rects}],truncated,ms,mode}
   * 失败恒为 {ok:false,error,pages:[],scanned:[],ms}(永不抛错)。pages 按页号升序含**每个已扫描页**;
   * hasImages=false 的页 mode 恒为 "full" 且 rects 为空。判据唯一 = mode(rect 才用 rects)。 */
  function pageImages(input, opts) {
    var o = opts || {};
    var maxPages = Math.floor(num(o.maxPages, 8));
    var dpi = num(o.dpi, DEF.dpi);
    var wantRects = o.region !== 'full';
    var timeout = num(o.timeout, DEF.timeout);
    var onProgress = typeof o.onProgress === 'function' ? o.onProgress : null;
    var t0 = now();
    var miss = missing();
    if (miss.length) { return Promise.resolve(fail('环境不支持: 缺少 ' + miss.join(', '), { pages: [], scanned: [] })); }
    var bytes = toBytes(input);
    if (!bytes || !bytes.length) { return Promise.resolve(fail('PDF 数据为空或类型不支持(需要 ArrayBuffer/Uint8Array/base64)', { pages: [], scanned: [] })); }

    var aborted = false;
    var job = openDoc(bytes);
    var work = job.promise.then(function (d) {
      if (aborted) { return fail('已超时,放弃该任务', { pages: [], scanned: [] }); }
      var total = Math.min(d.numPages, maxPages);
      var pages = [], scanned = [], done = 0;
      var truncated = d.numPages > total;
      function step() {
        if (done >= total) { return Promise.resolve(); }
        var n = done + 1;
        return d.getPage(n).then(function (page) {
          var vp = page.getViewport({ scale: dpi / 72 });
          return page.getOperatorList().then(function (ops) {
            var sc = scanImages(ops, vp);
            var hasImages = (sc.imgOps + sc.maskOps + sc.repeats) > 0;
            var pgMode = 'full';
            if (hasImages && wantRects && sc.repeats === 0 && !sc.uncertain && sc.rects.length > 0) { pgMode = 'rect'; }
            pages.push({
              page: n, hasImages: hasImages, imgOps: sc.imgOps, maskOps: sc.maskOps,
              repeats: sc.repeats, mode: pgMode, rects: sc.rects
            });
            scanned.push(n);
            if (typeof page.cleanup === 'function') { page.cleanup(); }
            done = n;
            if (onProgress) { try { onProgress(n, total); } catch (e) {} }
            return step();
          });
        });
      }
      return step().then(function () {
        return {
          ok: true, pageCount: d.numPages, scanned: scanned, pages: pages,
          truncated: truncated, ms: Math.round(now() - t0), mode: mode
        };
      });
    });

    return withTimeout(work, timeout, function () { aborted = true; releaseJob(job); })
      .then(function (res) {
        releaseJob(job);
        return res;
      }, function (e) {
        aborted = true;
        releaseJob(job);
        return fail(errText(e), { pages: [], scanned: [], ms: Math.round(now() - t0) });
      });
  }

  /* ---------- API:renderCrops(子区裁剪渲染)----------
   * -> {ok,images:[{page,dataUrl,width,height}],ms,mode} | {ok:false,error,images:[],ms}
   * 每页只渲染一次:画布 = 该页全部 rect 的并集包围盒,用 render 的 transform 平移到位,再逐 rect 裁出;
   * 裁剪区面积 >= 整页面积(或矩形不合法)⇒ 该页退整页渲染(上限护栏)。任何异常 → ok:false 单点降级。 */
  function renderCrops(input, opts) {
    var o = opts || {};
    var dpi = num(o.dpi, DEF.dpi);
    var format = o.format === 'image/png' ? 'image/png' : 'image/jpeg';
    var quality = typeof o.quality === 'number' && isFinite(o.quality) ? o.quality : DEF.quality;
    var timeout = num(o.timeout, DEF.timeout);
    var t0 = now();
    var miss = missing();
    if (miss.length) { return Promise.resolve(fail('环境不支持: 缺少 ' + miss.join(', '), { images: [] })); }
    var list = (o.groups && typeof o.groups.length === 'number') ? o.groups : [];
    if (!list.length) { return Promise.resolve(fail('未提供裁剪区域(groups 为空)', { images: [] })); }
    var groups = [];
    for (var gi = 0; gi < list.length; gi++) {
      var en = list[gi] || {}, gr = en.rect || {};
      var pageNo = Math.floor(num(en.page, 0));
      var rect = { x: Number(gr.x), y: Number(gr.y), w: Number(gr.w), h: Number(gr.h) };
      if (!pageNo || !rectOk(rect)) {
        return Promise.resolve(fail('裁剪区域不合法(groups 第 ' + (gi + 1) + ' 项的 page/rect 无效)', { images: [] }));
      }
      groups.push({ at: gi, page: pageNo, rect: rect });
    }
    var bytes = toBytes(input);
    if (!bytes || !bytes.length) { return Promise.resolve(fail('PDF 数据为空或类型不支持(需要 ArrayBuffer/Uint8Array/base64)', { images: [] })); }

    var aborted = false, liveCanvas = null;
    var job = openDoc(bytes);
    var work = job.promise.then(function (d) {
      if (aborted) { return fail('已超时,放弃该任务', { images: [] }); }
      var images = new Array(groups.length);
      /* 按页归并(同页多个 rect 只渲染一次),页序 = 首次出现序 */
      var pageJobs = [], slotOf = {};
      for (var k = 0; k < groups.length; k++) {
        var key = String(groups[k].page);
        if (slotOf[key] === undefined) { slotOf[key] = pageJobs.length; pageJobs.push({ page: groups[k].page, list: [] }); }
        pageJobs[slotOf[key]].list.push(groups[k]);
      }
      function step(idx) {
        if (idx >= pageJobs.length) { return Promise.resolve(); }
        var PJ = pageJobs[idx];
        return d.getPage(PJ.page).then(function (page) {
          var vp = page.getViewport({ scale: dpi / 72 });
          var bx = Infinity, by = Infinity, ex = -Infinity, ey = -Infinity, q;
          for (q = 0; q < PJ.list.length; q++) {
            var rc = PJ.list[q].rect;
            bx = Math.min(bx, rc.x); by = Math.min(by, rc.y);
            ex = Math.max(ex, rc.x + rc.w); ey = Math.max(ey, rc.y + rc.h);
          }
          bx = Math.floor(bx); by = Math.floor(by);
          var bw = Math.ceil(ex) - bx, bh = Math.ceil(ey) - by;
          var pw = Math.max(1, Math.floor(vp.width)), ph = Math.max(1, Math.floor(vp.height));
          if (!(bw > 0) || !(bh > 0) || (bw * bh) >= (pw * ph)) { bx = 0; by = 0; bw = pw; bh = ph; }
          var canvas = document.createElement('canvas');
          canvas.width = bw; canvas.height = bh;
          liveCanvas = canvas;
          var ctx = canvas.getContext('2d');
          ctx.fillStyle = '#ffffff';
          ctx.fillRect(0, 0, bw, bh);
          return page.render({
            canvasContext: ctx, viewport: vp, background: '#ffffff', transform: [1, 0, 0, 1, -bx, -by]
          }).promise.then(function () {
            for (var m = 0; m < PJ.list.length; m++) {
              var ent = PJ.list[m], r = ent.rect;
              var sx = Math.floor(r.x) - bx, sy = Math.floor(r.y) - by;
              var sw = Math.ceil(r.w), sh = Math.ceil(r.h);
              if (sx < 0) { sw += sx; sx = 0; }
              if (sy < 0) { sh += sy; sy = 0; }
              if (sx + sw > bw) { sw = bw - sx; }
              if (sy + sh > bh) { sh = bh - sy; }
              if (!(sw > 0) || !(sh > 0)) {
                sw = Math.max(1, Math.min(bw, Math.ceil(r.w)));
                sh = Math.max(1, Math.min(bh, Math.ceil(r.h)));
                sx = Math.max(0, Math.min(bw - sw, sx));
                sy = Math.max(0, Math.min(bh - sh, sy));
              }
              var c2 = document.createElement('canvas');
              c2.width = sw; c2.height = sh;
              var x2 = c2.getContext('2d');
              x2.fillStyle = '#ffffff';
              x2.fillRect(0, 0, sw, sh);
              x2.drawImage(canvas, sx, sy, sw, sh, 0, 0, sw, sh);
              images[ent.at] = { page: PJ.page, dataUrl: c2.toDataURL(format, quality), width: sw, height: sh };
              c2.width = 0; c2.height = 0;
            }
            canvas.width = 0; canvas.height = 0;
            liveCanvas = null;
            if (typeof page.cleanup === 'function') { page.cleanup(); }
            return step(idx + 1);
          });
        });
      }
      return step(0).then(function () {
        return { ok: true, images: images, ms: Math.round(now() - t0), mode: mode };
      });
    });

    return withTimeout(work, timeout, function () { aborted = true; releaseJob(job); })
      .then(function (res) {
        releaseJob(job);
        return res;
      }, function (e) {
        aborted = true;
        releaseJob(job);
        if (liveCanvas) { liveCanvas.width = 0; liveCanvas.height = 0; }
        return fail(errText(e), { images: [], ms: Math.round(now() - t0) });
      });
  }

  /* ---------- 导出 ---------- */
  var miss = missing();
  var api = {
    available: miss.length === 0,
    why: miss.length ? ('缺少 ' + miss.join(', ')) : '',
    version: lib && lib.version ? lib.version : '',
    notes: envNotes(),        /* 仅提示、不拦(不参与上面的 available 判据) */
    extractText: extractText,
    renderPages: renderPages,
    pageImages: pageImages,
    renderCrops: renderCrops,
    diag: diag,
    mode: function () { return setupWorker(); },
    setupWorker: setupWorker
  };
  g.PDFKit = api;
})(typeof window !== 'undefined' ? window : this);
`;

/* ===========================================================================
 * 生成
 * ========================================================================= */
const displaySrc = read(DISPLAY);
const workerSrc = read(WORKER);
const version = (displaySrc.match(/apiVersion:"([^"]+)"/) || displaySrc.match(/version[:=]"([0-9][^"]*)"/) || [])[1]
  || JSON.parse(read(path.join(VENDOR, 'package.json.orig'))).version;

const display = rewriteEsm(displaySrc, {
  label: 'pdf.min.mjs', version,
  prelude: DISPLAY_PRELUDE + '\n' + STREAM_ITER_SHIM,
  assignTarget: '(typeof window !== "undefined" ? window : globalThis).pdfjsLib'
});
const worker = rewriteEsm(workerSrc, {
  label: 'pdf.worker.min.mjs', version,
  prelude: WORKER_PRELUDE + '\n' + STREAM_ITER_SHIM,
  assignTarget: 'globalThis.pdfjsWorker'
});

/* ---------- 导出物硬性检查 ---------- */
const mustExport = ['getDocument', 'version', 'GlobalWorkerOptions', 'PDFWorker', 'AbortException', 'Util', 'OPS'];
const exportNames = display.map.map(e => e.exported);
mustExport.forEach(k => { if (exportNames.indexOf(k) < 0) { throw new Error('display 缺少导出 ' + k); } });
if (!worker.map.some(e => e.exported === 'WorkerMessageHandler')) { throw new Error('worker 缺少 WorkerMessageHandler'); }
if (!/globalThis\.pdfjsWorker\s*=/.test(worker.script)) { throw new Error('worker 产物未设置 globalThis.pdfjsWorker'); }

const displayInline = escapeForInline(display.script, 'display');
const workerInline = escapeForInline(worker.script, 'worker');
const kitInline = escapeForInline(PDFKIT_JS, 'PDFKit');

const part =
  '<!-- ===== 内联 pdf.js ' + version + ' (legacy, ESM→classic 改写, 已修复 CVE-2024-4367) + PDFKit —— 由 scripts/make-pdfjs-part.js 生成,请勿手改 ===== -->\n' +
  '<script>\n' + displayInline + '</script>\n' +
  '<script type="text/plain" id="' + WORKER_SRC_ID + '">\n' + workerInline + '</script>\n' +
  '<script>\n' + kitInline + '</script>\n';

/* ---------- 自检 ---------- */
const problems = [];
if (unescapeInline(displayInline) !== display.script) { problems.push('display 转义后无法还原'); }
if (unescapeInline(workerInline) !== worker.script) { problems.push('worker 转义后无法还原'); }
if (unescapeInline(kitInline) !== PDFKIT_JS) { problems.push('PDFKit 转义后无法还原'); }
/* 改写前后一致性:除 export 尾句外,只有 windows 记录的 N 个替换点被改动 */
[['display', display], ['worker', worker]].forEach(function (t) {
  const name = t[0], r = t[1];
  if (r.body.length !== r.before.length + r.windows.reduce((a, w) => a + w.delta, 0)) {
    problems.push(name + ': 长度账不平');
  }
  r.windows.forEach(function (w, i) {
    if (w.before && r.body.indexOf(w.before) < 0) { problems.push(name + ': 替换点 ' + i + '(' + w.name + ') 前文丢失'); }
    if (w.after && r.body.indexOf(w.after) < 0) { problems.push(name + ': 替换点 ' + i + '(' + w.name + ') 后文丢失'); }
  });
  if (r.script.indexOf(r.body) < 0) { problems.push(name + ': 产物里的 body 与改写结果不一致'); }
  if (/\bexport\b/.test(r.body) || /import\.meta/.test(r.body)) { problems.push(name + ': body 残留 export/import.meta'); }
});
/* 逐个 <script> 块检查:块内不得再有裸 </script / <!-- */
const blocks = part.match(/<script[^>]*>\n?[\s\S]*?<\/script>/g) || [];
blocks.forEach(function (blk, i) {
  const body = blk.replace(/^<script[^>]*>/, '').replace(/<\/script>$/, '');
  if (/<\/script/i.test(body)) { problems.push('块 ' + i + ' 内仍有裸 </script'); }
  if (/<!--/.test(body)) { problems.push('块 ' + i + ' 内仍有裸 <!--'); }
});
const openTags = (part.match(/<script/g) || []).length;
const closeTags = (part.match(/<\/script>/g) || []).length;
if (openTags !== closeTags) { problems.push('script 开闭标签数不一致 ' + openTags + '/' + closeTags); }

if (problems.length) {
  console.error('自检失败:\n  - ' + problems.join('\n  - '));
  process.exit(1);
}

fs.writeFileSync(OUT, part, 'utf8');
const stat = fs.statSync(OUT);
console.log('pdf.js ' + version + ' legacy(ESM → classic)');
console.log('  display : ' + displaySrc.length + ' chars → ' + display.script.length + ' chars(导出 ' + display.map.length + ' 项,残留动态 import ' + display.residualImport + ')');
console.log('  worker  : ' + workerSrc.length + ' chars → ' + worker.script.length + ' chars(导出 ' + worker.map.length + ' 项,残留动态 import ' + worker.residualImport + ')');
console.log('  PDFKit  : ' + PDFKIT_JS.length + ' chars');
console.log('  script 块: ' + openTags + '(1 display / 1 text-plain worker / 1 PDFKit)');
console.log('  -> ' + path.relative(process.cwd(), OUT).replace(/\\/g, '/') + '  ' + stat.size + ' bytes');
