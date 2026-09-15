/* ============================================================
   生成 src/tesseract.part(以及测试页 vendor/tess-src/libtest-ocr.html)
   用法: node scripts/make-tesseract-part.js
   幂等: 输入不变则输出字节级一致。
   ============================================================ */
'use strict';

const fs = require('fs');
const path = require('path');
const zlib = require('zlib');

const ROOT = path.resolve(__dirname, '..');
const SRC = path.join(ROOT, 'vendor', 'tess-src');
const NM = path.join(SRC, 'node_modules');

const TJ = path.join(NM, 'tesseract.js');
const CORE = path.join(NM, 'tesseract.js-core');
const TESS_VER = JSON.parse(fs.readFileSync(path.join(TJ, 'package.json'), 'utf8')).version;
const CORE_VER = JSON.parse(fs.readFileSync(path.join(CORE, 'package.json'), 'utf8')).version;

/* core 选择:simd-lstm(体积与纯 lstm 几乎相同,速度快;tessdata_fast 只有 LSTM 模型) */
const CORE_FILE = 'tesseract-core-simd-lstm';
const LANGS = ['eng', 'chi_sim'];
const DEFAULT_LANGS = 'chi_sim+eng';

const BAD_SCRIPT = '<' + '/script';
const BAD_COMMENT_OPEN = '<' + '!--';
const BAD_SCRIPT_OPEN = '<' + 'script';

const stats = [];
function note(name, bytes, extra) { stats.push({ name, bytes, extra: extra || '' }); }

function readText(file, label) {
  let s = fs.readFileSync(file, 'utf8');
  /* 去掉 sourceMappingURL —— 拼进单文件后会变成一次无意义的请求 */
  const before = s.length;
  s = s.replace(/^[ \t]*\/\/[#@][ \t]*sourceMappingURL=.*$/gm, '');
  if (s.length !== before) note(label + ':sourceMappingURL', before - s.length, 'stripped');
  return s;
}

/* 内联脚本里不允许出现能终止 <script> 的序列 */
function guard(text, label) {
  const n = text.split(BAD_SCRIPT).length - 1;
  if (n) {
    /* JS 源码里 `</script` 只可能出现在字符串/正则/注释中,写成 <\/script 语义等价 */
    text = text.split(BAD_SCRIPT).join('<\\/script');
    note(label + ':escaped-close-script', n, 'occurrence');
  }
  if (text.indexOf(BAD_COMMENT_OPEN) >= 0 || text.indexOf(BAD_SCRIPT_OPEN) >= 0) {
    throw new Error(label + ' 里出现 "<!--" 或 "<script",会破坏 HTML 解析,需要人工处理');
  }
  if (n) note(label + ':escaped-' + BAD_SCRIPT, n, 'occurrence');
  return text;
}

function b64(buf, width) {
  const s = buf.toString('base64');
  const w = width || 120;
  if (s.length <= w) return s;
  const out = [];
  for (let i = 0; i < s.length; i += w) out.push(s.slice(i, i + w));
  return out.join('\n');
}

function inflateSource() {
  let s = fs.readFileSync(path.join(NM, 'tiny-inflate', 'index.js'), 'utf8');
  if (s.split('module.exports').length - 1 !== 1) throw new Error('tiny-inflate 结构变化,请检查');
  s = s.replace('module.exports = tinf_uncompress;', 'return tinf_uncompress;');
  const lic = fs.readFileSync(path.join(NM, 'tiny-inflate', 'LICENSE'), 'utf8').trim()
    .split('\n').map((l) => '      ' + l).join('\n');
  return '    /* --- tiny-inflate 1.0.3 (MIT) —— 仅在浏览器没有 DecompressionStream 时使用 ---\n'
    + lic + '\n    */\n'
    + '    var inflateRaw = (function () {\n'
    + s.split('\n').map((l) => (l ? '    ' + l : l)).join('\n')
    + '\n    })();\n';
}

/* ---------------- 读取各部分源码 ---------------- */

const mainJs = guard(readText(path.join(TJ, 'dist', 'tesseract.min.js'), 'tesseract.min.js'), 'tesseract.min.js');
const workerJs = guard(readText(path.join(TJ, 'dist', 'worker.min.js'), 'worker.min.js'), 'worker.min.js');
const coreJs = guard(readText(path.join(CORE, CORE_FILE + '.js'), 'core'), 'core');
const bootJs = guard(fs.readFileSync(path.join(SRC, 'ocr-worker-boot.js'), 'utf8'), 'boot');
const adapterJs = guard(fs.readFileSync(path.join(SRC, 'ocr-core-adapter.js'), 'utf8'), 'adapter');

let wrapperJs = guard(fs.readFileSync(path.join(ROOT, 'src', 'ocrkit.src.js'), 'utf8'), 'wrapper');
wrapperJs = wrapperJs
  .replace('__OCR_VERSION__', TESS_VER)
  .replace('__OCR_LANGS__', JSON.stringify(LANGS))
  .replace('__OCR_DEFAULT_LANGS__', DEFAULT_LANGS)
  .replace('  /*__OCR_INFLATE__*/', inflateSource());

const wasmRaw = fs.readFileSync(path.join(CORE, CORE_FILE + '.wasm'));
const langRaw = {};
LANGS.forEach((l) => { langRaw[l] = fs.readFileSync(path.join(SRC, 'langs', l + '.traineddata')); });

const gz = (buf) => zlib.gzipSync(buf, { level: 9 });

function section(id, text) {
  return '<script type="text/plain" id="' + id + '">\n' + text + '\n' + '<' + '/script>\n';
}

/* ---------------- 拼装 .part ---------------- */

const parts = [];
parts.push('<!-- ============================================================\n'
  + '     Tesseract.js v' + TESS_VER + ' (tesseract.js-core v' + CORE_VER + ') + OCRKit —— 内联 OCR\n'
  + '     由 scripts/make-tesseract-part.js 生成,请勿手改;重跑脚本即可重建。\n'
  + '     · 主线程库 tesseract.min.js / worker / core(' + CORE_FILE + '.wasm)/ eng+chi_sim 训练数据 全部内嵌\n'
  + '     · 零外部网络请求;http:// 与 file:// 双击打开均可运行\n'
  + '     · API: window.OCRKit.recognize(input, opts)\n'
  + '     ============================================================ -->\n');

parts.push('<script>\n/* ===== 官方主线程库 tesseract.js v' + TESS_VER + ' (dist/tesseract.min.js) ===== */\n'
  + mainJs + '\n' + '<' + '/script>\n');
note('tesseract.min.js', Buffer.byteLength(mainJs));

parts.push(section('ocr-embed-boot', bootJs));
note('boot', Buffer.byteLength(bootJs));

parts.push(section('ocr-embed-core', coreJs));
note('core-glue ' + CORE_FILE + '.js', Buffer.byteLength(coreJs));

parts.push(section('ocr-embed-adapter', adapterJs));
note('adapter', Buffer.byteLength(adapterJs));

parts.push(section('ocr-embed-worker', workerJs));
note('worker.min.js', Buffer.byteLength(workerJs));

const wasmGz = gz(wasmRaw);
const wasmB64 = b64(wasmGz);
parts.push(section('ocr-embed-wasm', wasmB64));
note('wasm raw ' + CORE_FILE + '.wasm', wasmRaw.length);
note('wasm gzip', wasmGz.length);
note('wasm base64', wasmB64.length);

const langB64 = {};
LANGS.forEach((l) => {
  const g = gz(langRaw[l]);
  langB64[l] = b64(g);
  parts.push(section('ocr-embed-lang-' + l, langB64[l]));
  note('lang ' + l + ' raw', langRaw[l].length);
  note('lang ' + l + ' gzip', g.length);
  note('lang ' + l + ' base64', langB64[l].length);
});

parts.push('<script>\n/* ===== OCRKit 包装层(ES5) ===== */\n' + wrapperJs + '\n' + '<' + '/script>\n');
note('wrapper(含 tiny-inflate)', Buffer.byteLength(wrapperJs));

const out = parts.join('\n');
const partPath = path.join(ROOT, 'src', 'tesseract.part');
fs.writeFileSync(partPath, out);

/* ---------------- 输出后自检 ---------------- */

const check = fs.readFileSync(partPath, 'utf8');
const blocks = check.split(new RegExp('<' + 'script', 'g')).length - 1;
const closes = check.split(BAD_SCRIPT).length - 1;
if (blocks !== closes) throw new Error('自检失败: <script> ' + blocks + ' 个, 结束标记 ' + closes + ' 个');
const inner = check.replace(new RegExp(BAD_SCRIPT + '>', 'g'), '\u0000');
if (inner.indexOf(BAD_SCRIPT) >= 0) throw new Error('自检失败: 脚本块内部仍有 ' + BAD_SCRIPT);
LANGS.forEach((l) => {
  if (check.indexOf('id="ocr-embed-lang-' + l + '"') < 0) throw new Error('自检失败: 缺语言 ' + l);
});

/* ---------------- 测试页 ---------------- */

const tmpl = path.join(SRC, 'libtest-ocr.tmpl.html');
if (fs.existsSync(tmpl)) {
  const html = fs.readFileSync(tmpl, 'utf8').replace('<!--__TESSERACT_PART__-->', () => check);
  fs.writeFileSync(path.join(SRC, 'libtest-ocr.html'), html);
  note('libtest-ocr.html', Buffer.byteLength(html));
}

/* ---------------- 报告 ---------------- */

console.log('tesseract.js v' + TESS_VER + ' + tesseract.js-core v' + CORE_VER);
let total = 0;
stats.filter((s) => s.extra !== 'stripped').forEach((s) => { total += s.bytes; });
stats.forEach((s) => {
  console.log('  ' + s.name.padEnd(32) + String(s.bytes).padStart(10)
    + (s.bytes > 1024 ? '  (' + (s.bytes / 1048576).toFixed(2) + ' MB)' : '') + '  ' + s.extra);
});
console.log('  ' + '-'.repeat(60));
console.log('  ' + 'tesseract.part'.padEnd(32) + String(Buffer.byteLength(out)).padStart(10)
  + '  (' + (Buffer.byteLength(out) / 1048576).toFixed(2) + ' MB)');
console.log('  script 块: ' + blocks + ' 个,全部自检通过');
