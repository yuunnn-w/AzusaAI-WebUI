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

/* ===========================================================================
 * PDFKit 封装层(ES5:var + function,无箭头函数 / 可选链 / 模板字符串)
 * ========================================================================= */
const PDFKIT_JS = `/* ===== PDFKit —— pdf.js 统一封装(文本抽取 + 页面渲染为图片)=====
 * 依赖同一 part 内先出现的两个块:window.pdfjsLib 与 #${WORKER_SRC_ID}
 * 契约:
 *   window.PDFKit.available            bool
 *   window.PDFKit.why                  string(available=false 时说明)
 *   window.PDFKit.version              pdf.js 版本号
 *   window.PDFKit.extractText(buf, opts)  -> Promise<{ok,text,pages,chars,truncated,error?}>
 *   window.PDFKit.renderPages(buf, opts)  -> Promise<{ok,images:[{dataUrl,width,height,page}],pages,truncated,error?}>
 * opts: { maxPages:50, dpi:150, format:"image/jpeg", quality:0.85,
 *         onProgress:(done,total)=>void, maxChars:400000, timeout:60000 }
 * 额外(非契约):PDFKit.mode() 返回 worker 加载方式,PDFKit.setupWorker() 提前初始化 worker。
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
  var setupDone = false;
  var sharedWorker = null;  /* 真 worker(整页复用) */
  var sharedPdfWorker = null; /* 对应 PDFWorker 实例,显式传给 getDocument,避免被 doc.destroy() 连带销毁 */

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

  function missing() {
    var m = [];
    if (!lib || typeof lib.getDocument !== 'function') { m.push('pdfjsLib'); }
    if (typeof Promise !== 'function') { m.push('Promise'); }
    if (typeof Uint8Array !== 'function' || typeof ArrayBuffer !== 'function') { m.push('TypedArray'); }
    if (!hasCanvas()) { m.push('Canvas2D'); }
    return m;
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
    return lib.getDocument(params).promise;
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

    var doc = null;
    var aborted = false;
    var job = openDoc(bytes).then(function (d) {
      doc = d;
      if (aborted) { try { d.destroy(); } catch (e) {} return fail('已超时,放弃该任务', { text: '', chars: 0 }); }
      var total = Math.min(d.numPages, maxPages);
      var text = '';
      var done = 0;
      var truncated = d.numPages > total;
      function step() {
        if (done >= total) { return Promise.resolve(); }
        var n = done + 1;
        return d.getPage(n).then(function (page) {
          return page.getTextContent({ includeMarkedContent: false }).then(function (content) {
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

    return withTimeout(job, timeout, function () { aborted = true; if (doc) { try { doc.destroy(); } catch (e) {} } })
      .then(function (res) {
        if (doc) { try { doc.destroy(); } catch (e) {} }
        return res;
      }, function (e) {
        aborted = true;
        if (doc) { try { doc.destroy(); } catch (e) {} }
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

    var doc = null;
    var canvas = null;
    var aborted = false;
    var scale = dpi / 72;
    var job = openDoc(bytes).then(function (d) {
      doc = d;
      if (aborted) { try { d.destroy(); } catch (e) {} return fail('已超时,放弃该任务', { images: [] }); }
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

    return withTimeout(job, timeout, function () { aborted = true; if (doc) { try { doc.destroy(); } catch (e) {} } })
      .then(function (res) {
        if (doc) { try { doc.destroy(); } catch (e) {} }
        return res;
      }, function (e) {
        aborted = true;
        if (doc) { try { doc.destroy(); } catch (e) {} }
        if (canvas) { canvas.width = 0; canvas.height = 0; }
        return fail(errText(e), { images: [], ms: Math.round(now() - t0) });
      });
  }

  /* ---------- 导出 ---------- */
  var miss = missing();
  var api = {
    available: miss.length === 0,
    why: miss.length ? ('缺少 ' + miss.join(', ')) : '',
    version: lib && lib.version ? lib.version : '',
    extractText: extractText,
    renderPages: renderPages,
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
  prelude: DISPLAY_PRELUDE, assignTarget: '(typeof window !== "undefined" ? window : globalThis).pdfjsLib'
});
const worker = rewriteEsm(workerSrc, {
  label: 'pdf.worker.min.mjs', version,
  prelude: WORKER_PRELUDE, assignTarget: 'globalThis.pdfjsWorker'
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
