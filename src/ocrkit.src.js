/* ============================================================
   OCRKit —— 单文件内联 Tesseract.js 封装(ES5:只用 var / function)

   设计要点(细节见 docs/TESS-NOTES.md):
   · 所有资源(tesseract.js 主线程库 / worker 源码 / core glue /
     wasm 二进制 / eng+chi_sim 训练数据)都内嵌在本片段里,零网络。
   · worker 用 blob URL 启动,资源通过 postMessage 投递进去;
     因为 file:// 下 blob worker 里 importScripts(blob:) 与 fetch(blob:)
     都会被浏览器拒绝,worker 内的 fetch 被引导层改写成「只认内嵌数据」。
   · 首次 recognize 才解码 / 解压 / 起 worker;之后复用,同一时刻只跑一个任务。
   ============================================================ */
(function (W, D) {
  "use strict";

  var VERSION = "__OCR_VERSION__";
  var EMBED_LANGS = __OCR_LANGS__;
  var DEFAULT_LANGS = "__OCR_DEFAULT_LANGS__";
  var DEFAULT_TIMEOUT = 120000;
  var ACK_TIMEOUT = 20000;

  var ID = {
    boot: "ocr-embed-boot",
    core: "ocr-embed-core",
    adapter: "ocr-embed-adapter",
    worker: "ocr-embed-worker",
    wasm: "ocr-embed-wasm",
    lang: function (l) { return "ocr-embed-lang-" + l; }
  };

  var S = { res: null, readyP: null, langsUsed: null, chain: null, logger: null, lastError: null };

  /* ---------------- 基础工具 ---------------- */

  function byId(id) { return D.getElementById(id); }
  function textOf(id) { var e = byId(id); return e ? e.textContent : ""; }
  function now() {
    return (W.performance && W.performance.now) ? W.performance.now() : Date.now();
  }
  function trim(s) { return String(s == null ? "" : s).replace(/^[\s\uFEFF\xA0]+|[\s\uFEFF\xA0]+$/g, ""); }
  function errMsg(e) {
    if (e == null) return "未知错误";
    if (typeof e === "string") return e;
    if (e.message) return String(e.message);
    try { return JSON.stringify(e); } catch (x) { return String(e); }
  }

  function b64ToBytes(b64) {
    var s = String(b64).replace(/[\s\u3000]+/g, "");
    var bin = W.atob(s);
    var n = bin.length;
    var out = new Uint8Array(n);
    for (var i = 0; i < n; i++) out[i] = bin.charCodeAt(i) & 255;
    return out;
  }

  /*__OCR_INFLATE__*/

  function gunzip(u8) {
    if (typeof W.DecompressionStream === "function" && typeof W.Response === "function") {
      try {
        var ds = new W.DecompressionStream("gzip");
        var st = new W.Blob([u8]).stream().pipeThrough(ds);
        S.decode = "DecompressionStream";
        return new W.Response(st).arrayBuffer().then(function (ab) { return new Uint8Array(ab); });
      } catch (e) { /* 落到内联 inflate */ }
    }
    S.decode = "inflate(内联 tiny-inflate)";
    return new Promise(function (resolve, reject) {
      try { resolve(inflateGzip(u8)); } catch (e) { reject(e); }
    });
  }

  function inflateGzip(u8) {
    if (u8[0] !== 0x1f || u8[1] !== 0x8b) throw new Error("内嵌数据不是 gzip 流");
    var flg = u8[3];
    var p = 10;
    if (flg & 4) { var xlen = u8[p] | (u8[p + 1] << 8); p += 2 + xlen; }
    if (flg & 8) { while (p < u8.length && u8[p] !== 0) p++; p++; }
    if (flg & 16) { while (p < u8.length && u8[p] !== 0) p++; p++; }
    if (flg & 2) p += 2;
    var n = u8.length;
    /* gzip trailer 最后 4 字节 = 原始长度 */
    var size = (u8[n - 4] | (u8[n - 3] << 8) | (u8[n - 2] << 16) | (u8[n - 1] << 24)) >>> 0;
    var out = new Uint8Array(size);
    inflateRaw(u8.subarray(p, n - 8), out);
    return out;
  }

  /* ---------------- 环境自检 ---------------- */

  function detect() {
    var miss = [];
    if (typeof W.Promise !== "function") miss.push("Promise");
    if (typeof W.Blob !== "function") miss.push("Blob");
    if (typeof W.FileReader !== "function") miss.push("FileReader");
    if (!W.URL || typeof W.URL.createObjectURL !== "function") miss.push("URL.createObjectURL");
    if (typeof W.Worker !== "function") miss.push("Worker");
    if (typeof W.WebAssembly !== "object") miss.push("WebAssembly");
    if (typeof W.atob !== "function") miss.push("atob");
    if (typeof W.Uint8Array !== "function") miss.push("Uint8Array");
    if (typeof W.Tesseract !== "object" || typeof W.Tesseract.createWorker !== "function") miss.push("内嵌 tesseract.js");
    if (typeof W.DecompressionStream !== "function" && typeof inflateRaw !== "function") miss.push("解压能力(gzip)");
    return miss;
  }

  /* ---------------- 资源解码 ---------------- */

  function loadResources() {
    if (S.res) return Promise.resolve(S.res);

    var srcParts = {};
    var names = ["boot", "core", "adapter", "worker"];
    var i;
    for (i = 0; i < names.length; i++) {
      srcParts[names[i]] = textOf(ID[names[i]]);
      if (!srcParts[names[i]]) {
        return Promise.reject(new Error("OCRKit: 缺少内嵌资源 " + ID[names[i]]));
      }
    }
    var wasmTxt = textOf(ID.wasm);
    if (!wasmTxt) return Promise.reject(new Error("OCRKit: 缺少内嵌资源 " + ID.wasm));

    var jobs = [gunzip(b64ToBytes(wasmTxt))];
    var order = [];
    for (i = 0; i < EMBED_LANGS.length; i++) {
      (function (lang) {
        var t = textOf(ID.lang(lang));
        if (!t) { jobs.push(Promise.reject(new Error("OCRKit: 缺少内嵌语言数据 " + lang))); return; }
        order.push(lang);
        jobs.push(gunzip(b64ToBytes(t)));
      })(EMBED_LANGS[i]);
    }

    return Promise.all(jobs).then(function (out) {
      var res = {
        wasm: toArrayBuffer(out[0]),
        langs: {},
        /* 拼装 blob worker 脚本:引导层 → core glue → core 适配层 → 官方 worker */
        src: srcParts.boot + "\n" + srcParts.core + "\n" + srcParts.adapter + "\n" + srcParts.worker
      };
      for (i = 0; i < order.length; i++) res.langs[order[i]] = toArrayBuffer(out[i + 1]);
      S.res = res;
      return res;
    });
  }

  function toArrayBuffer(u8) {
    if (u8.byteOffset === 0 && u8.byteLength === u8.buffer.byteLength) return u8.buffer;
    return u8.buffer.slice(u8.byteOffset, u8.byteOffset + u8.byteLength);
  }

  /* ---------------- 起 worker ---------------- */

  function workerLog(m) {
    if (S.logger) {
      try { S.logger(m); } catch (e) { /* 忽略回调异常 */ }
    }
  }
  function workerError(e) { S.lastError = errMsg(e); }

  function buildWorker(langs) {
    return loadResources().then(function (res) {
      var url = W.URL.createObjectURL(new W.Blob([res.src], { type: "application/javascript" }));
      var captured = null;
      var OrigWorker = W.Worker;

      /* createWorker 内部用 `new Worker(workerPath)` 起 worker,
         这里短暂劫持全局 Worker 拿到实例(同步,调用后立刻还原),
         以便在它发第一个 load 包之前把内嵌资源投递进去。 */
      function Hook(u, o) { captured = new OrigWorker(u, o); return captured; }
      Hook.prototype = OrigWorker.prototype;
      W.Worker = Hook;

      var pending;
      try {
        pending = W.Tesseract.createWorker(langs, 1, {
          workerPath: url,
          workerBlobURL: false,
          corePath: "ocrkit-core.js",
          langPath: "ocrkit-embedded",
          gzip: false,
          cacheMethod: "none",
          logger: workerLog,
          errorHandler: workerError
        });
      } catch (e) {
        W.Worker = OrigWorker;
        W.URL.revokeObjectURL(url);
        return Promise.reject(e);
      }
      W.Worker = OrigWorker;

      if (!captured) {
        W.URL.revokeObjectURL(url);
        return Promise.reject(new Error("OCRKit: 无法接管 worker 实例"));
      }

      var w = captured;
      var ack = new Promise(function (resolve, reject) {
        var done = false;
        var t = setTimeout(function () {
          if (done) return;
          done = true;
          reject(new Error("OCRKit: worker 资源投递超时"));
        }, ACK_TIMEOUT);
        w.addEventListener("message", function (ev) {
          var d = ev.data;
          if (d && d.__ocrEmbedAck === 1 && !done) {
            done = true;
            clearTimeout(t);
            resolve(true);
          }
        });
      });

      w.postMessage({ __ocrEmbed: 1, wasm: res.wasm, langs: res.langs });

      return ack.then(function () { return pending; }, function (e) {
        try { w.terminate(); } catch (e2) { /* ignore */ }
        throw e;
      }).then(function (tess) {
        W.URL.revokeObjectURL(url);
        return tess;
      });
    });
  }

  function ensureWorker(langs) {
    if (S.readyP) {
      if (S.langsUsed === langs) return S.readyP;
      return S.readyP.then(function (tess) {
        return tess.reinitialize(langs).then(function () {
          S.langsUsed = langs;
          return tess;
        });
      });
    }
    S.readyP = buildWorker(langs).then(function (tess) {
      S.langsUsed = langs;
      return tess;
    }, function (e) {
      S.readyP = null;
      S.langsUsed = null;
      throw e;
    });
    return S.readyP;
  }

  function dropWorker() {
    var p = S.readyP;
    S.readyP = null;
    S.langsUsed = null;
    if (p) {
      p.then(function (tess) {
        try { tess.terminate(); } catch (e) { /* ignore */ }
      }, function () { /* ignore */ });
    }
  }

  /* ---------------- 输入归一化 ---------------- */

  function normalizeInput(input) {
    if (input == null) return { error: "OCRKit: 输入为空" };
    if (typeof input === "string") {
      if (/^data:image\//i.test(input) || /^blob:/i.test(input)) return { input: input };
      return { error: "OCRKit: 字符串输入只接受 data:image/ 或 blob: URL(离线保证)" };
    }
    if (typeof W.Blob === "function" && input instanceof W.Blob) return { input: input };
    if (typeof W.ArrayBuffer === "function"
      && (input instanceof W.ArrayBuffer || (W.ArrayBuffer.isView && W.ArrayBuffer.isView(input)))) {
      return { input: input };
    }
    if (typeof W.HTMLCanvasElement === "function" && input instanceof W.HTMLCanvasElement) return { input: input };
    if (typeof W.OffscreenCanvas !== "undefined" && input instanceof W.OffscreenCanvas) return { input: input };
    if (typeof W.HTMLImageElement === "function" && input instanceof W.HTMLImageElement) return { input: input };
    return { error: "OCRKit: 不支持的输入类型(支持 Blob/File/ArrayBuffer/Uint8Array/dataURL/Canvas)" };
  }

  function jobsOpts(opts) {
    var o = {}, k;
    for (k in opts) {
      if (!Object.prototype.hasOwnProperty.call(opts, k)) continue;
      if (k === "langs" || k === "onProgress" || k === "timeout" || k === "words") continue;
      o[k] = opts[k];
    }
    return o;
  }

  function mapProgress(m) {
    var p = typeof m.progress === "number" ? m.progress : 0;
    switch (m.status) {
      case "loading tesseract core": return 0.05 + p * 0.10;
      case "loading language traineddata": return 0.15 + p * 0.15;
      case "initializing api": return 0.30 + p * 0.10;
      case "recognizing text": return 0.40 + p * 0.60;
      default: return null;
    }
  }

  function flattenWords(blocks) {
    var out = [];
    if (!blocks || !blocks.length) return out;
    for (var i = 0; i < blocks.length; i++) {
      var ps = (blocks[i] && blocks[i].paragraphs) || [];
      for (var j = 0; j < ps.length; j++) {
        var ls = (ps[j] && ps[j].lines) || [];
        for (var k = 0; k < ls.length; k++) {
          var ws = (ls[k] && ls[k].words) || [];
          for (var n = 0; n < ws.length; n++) {
            out.push({
              text: ws[n].text,
              confidence: ws[n].confidence,
              bbox: ws[n].bbox
            });
          }
        }
      }
    }
    return out;
  }

  /* ---------------- 排队 / 超时 ---------------- */

  function enqueue(fn) {
    var run = S.chain ? S.chain.then(fn, fn) : fn();
    S.chain = run.then(function () { return null; }, function () { return null; });
    return run;
  }

  function withTimeout(p, ms) {
    return new Promise(function (resolve, reject) {
      var done = false;
      var t = setTimeout(function () {
        if (done) return;
        done = true;
        dropWorker();
        reject(new Error("识别超时(" + ms + "ms),worker 已重建"));
      }, ms);
      p.then(function (v) {
        if (done) return;
        done = true; clearTimeout(t); resolve(v);
      }, function (e) {
        if (done) return;
        done = true; clearTimeout(t); reject(e);
      });
    });
  }

  /* ---------------- 主入口 ---------------- */

  function recognize(input, opts) {
    opts = opts || {};
    var t0 = now();

    if (!api.available) {
      return Promise.resolve({ ok: false, error: "OCRKit 不可用: " + api.why, ms: 0 });
    }

    var norm = normalizeInput(input);
    if (norm.error) return Promise.resolve({ ok: false, error: norm.error, ms: now() - t0 });

    var langs = opts.langs ? String(opts.langs) : DEFAULT_LANGS;
    var timeout = Number(opts.timeout) > 0 ? Number(opts.timeout) : DEFAULT_TIMEOUT;
    var onProgress = typeof opts.onProgress === "function" ? opts.onProgress : null;
    var wantWords = !!opts.words;
    var tessOpts = jobsOpts(opts);
    var output = wantWords ? { text: true, blocks: true } : { text: true };
    var image = norm.input;

    return enqueue(function () {
      var stage = "init";
      S.logger = function (m) {
        if (!onProgress) return;
        if (m && m.status === "recognizing text") stage = "rec";
        var p = mapProgress(m);
        if (p !== null) { try { onProgress(p); } catch (e) { /* ignore */ } }
      };
      var started = now();

      return ensureWorker(langs).then(function (tess) {
        return withTimeout(tess.recognize(image, tessOpts, output), timeout);
      }).then(function (res) {
        var d = (res && res.data) || {};
        var out = {
          ok: true,
          text: trim(d.text),
          confidence: typeof d.confidence === "number" ? d.confidence : null,
          ms: Math.round(now() - t0)
        };
        if (wantWords) out.words = flattenWords(d.blocks);
        if (onProgress) { try { onProgress(1); } catch (e) { /* ignore */ } }
        return out;
      }, function (err) {
        return { ok: false, error: errMsg(err), ms: Math.round(now() - t0) };
      });
    });
  }

  function terminate() {
    dropWorker();
    S.chain = null;
    return Promise.resolve(true);
  }

  /* ---------------- 导出 ---------------- */

  var api = {
    available: false,
    why: "",
    version: VERSION,
    langs: EMBED_LANGS.slice(),
    mode: "worker",
    recognize: recognize,
    terminate: terminate
  };
  try {
    /* 诊断用:实际走了解压哪条路径 */
    Object.defineProperty(api, "decode", {
      enumerable: true,
      get: function () { return S.decode || ""; }
    });
  } catch (e) { api.decode = ""; }

  var missing = detect();
  if (missing.length) {
    api.available = false;
    api.why = "环境缺少: " + missing.join("、");
  } else {
    api.available = true;
  }

  W.OCRKit = api;
})(window, document);
