/* 由 vendor/office-src/ + src/office-worker.src.js + src/officekit.src.js
 *   + src/officewrite.src.js
 * 生成可内联的 src/office.part —— 唯一入口:node scripts/make-office-part.js
 *
 * 产物结构(7 个块,顺序即依赖顺序):
 *   1-5. <script type="text/plain" id="office-lib-*|office-worker-glue">
 *        mammoth / SheetJS / docstream / fflate + 解析核心胶水(只存文本不执行)
 *   6.   <script>  OfficeKit 包装层(ES5,可执行)
 *   7.   <script>  OfficeWrite 写入层(ES5,可执行;office-tool 批次 A 起)
 *
 * 幂等:同样输入生成完全相同的字节(产物不含时间戳)。
 * 任一条断言失败 → 打印原因 + exit 1,且**不写盘**。
 *
 * 断言清单(十三条,与生成时的自检一一对应):
 *   1 存在与哈希  2 shebang  3 sourceMappingURL  4 jsdelivr 横幅  5 转义与 HTML 安全
 *   6 URL 中和与主机白名单  7 块结构与编译  8 内容标识  9 幂等  10 输出统计
 *   11 常量同值(TEXT_MAX ↔ ATTACH_TEXT_MAX)
 *   12 常量同值(OfficeWrite 的 5 个 LIMITS ↔ 解析层同值;含 maxImagesPerCall 算式断言)
 *   13 模板件必含 XML 声明(var TPL_* 模板块内每个部件都以标准声明开头;批次 A 审查 P3-2 (b))
 */
'use strict';
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');

const ROOT = path.resolve(__dirname, '..');
const VENDOR = path.join(ROOT, 'vendor', 'office-src');
const SRC = path.join(ROOT, 'src');
const OUT = path.join(SRC, 'office.part');
const GLUE = path.join(SRC, 'office-worker.src.js');
const KIT = path.join(SRC, 'officekit.src.js');
const WRITE = path.join(SRC, 'officewrite.src.js');

const LIBS = [
  {
    id: 'office-lib-mammoth',
    file: 'mammoth.browser.min.js',
    bytes: 636898,
    sha: 'f9465c0e4b91c6ab5fe03db285aa3c5bd33a1d20da781b8dffc5676bc9689a95',
    header: '/* 内嵌库: mammoth 1.12.3 (BSD-2-Clause) —— mammoth.browser.min.js,由 make-office-part.js 内联,勿手改 */',
    marks: [['extractRawText', 1], ['convertToHtml', 1]],
    shebangs: 0
  },
  {
    id: 'office-lib-xlsx',
    file: 'xlsx-0.20.3.full.min.js',
    bytes: 951904,
    sha: 'cc015130aa8521e7f088f88898eba949ccdcbfb38df0bd129b44b7273c3a6f41',
    header: '/* 内嵌库: SheetJS CE 0.20.3 (Apache-2.0) —— xlsx.full.min.js(官方 CDN 版),由 make-office-part.js 内联,勿手改 */',
    marks: [['version="0.20.3"', 1, true]],   /* 三参数 = 精确计数 */
    shebangs: 0
  },
  {
    id: 'office-lib-docstream',
    file: 'docstream-0.1.3.browser.js',
    bytes: 1271443,
    sha: '6e54af623064c4c8afd1193cc797fc91d39c909f7ddcfc7f0aba8bb45b0aa9f5',
    header: '/* 内嵌库: @jose.espana/docstream 0.1.3 (MIT) —— officeparser.browser.js,由 make-office-part.js 内联,勿手改 */',
    marks: [['officeParser', 1], ['parseOffice', 1], ['extractAttachments', 1]],
    shebangs: 2   /* 1 处真 shebang(剥) + 1 处 file-type 魔数字符串 "#!AMR"(保留) */
  },
  {
    id: 'office-lib-fflate',
    file: 'fflate-0.8.3.umd.min.js',
    bytes: 33311,
    sha: 'c72116be617457f640b764feb29a1fc1e12da4c487b4108eba46f77b751c8a63',
    header: '/* 内嵌库: fflate 0.8.3 (MIT) —— index.min.js(umd),由 make-office-part.js 内联,勿手改 */',
    marks: [['unzipSync', 1]],
    shebangs: 0
  }
];
const GLUE_ID = 'office-worker-glue';

/* 四库 + 胶水全量实测并集(小写归一后);出现白名单外主机即失败,
   新增主机必须人工判定后更新本表 */
const HOST_WHITELIST = [
  'docs.oasis-open.org', 'example.com', 'feross.org', 'foo.bar', 'github.com', 'goo.gl',
  'localhost', 'macvmlschemauri', 'opencollective.com', 'openoffice.org', 'purl.oclc.org',
  'purl.org', 'schemas.microsoft.com', 'schemas.openxmlformats.org', 'schemas.zwobble.org',
  'sheetjs.com', 'sheetjs.openxmlformats.org', 'stuk.github.io', 'www.w3.org'
];
const NEUTRAL = ['https://unpkg.com/', 'https://cdn.jsdelivr.net/'];
const NEUTRAL_TO = 'about:blocked#';

const problems = [];
const notes = [];
let idempotent = '首写';
function bad(msg) { problems.push(msg); }
function note(msg) { notes.push(msg); }
function countOf(s, needle) { return s.split(needle).length - 1; }
function md5(s) { return crypto.createHash('md5').update(s, 'utf8').digest('hex'); }
function sha256Buf(b) { return crypto.createHash('sha256').update(b).digest('hex'); }
function read(f) { return fs.readFileSync(f, 'utf8').replace(/\r\n/g, '\n').replace(/\r/g, '\n'); }
/* 内联脚本转义:HTML 解析器不认 JS 语法,"</script"/"<!--" 会破坏 script 块 */
function escapeForInline(code) {
  return code.replace(/<\/script/gi, '<\\/script').replace(/<!--/g, '<\\!--');
}
function unescapeInline(code) {
  return code.replace(/<\\\/script/gi, '</script').replace(/<\\!--/g, '<!--');
}
function textBlock(id, text) {
  return '<script type="text/plain" id="' + id + '">\n' + escapeForInline(text) + '\n<' + '/script>\n';
}

/* ------------------------------------------------------------------ */
/* 检测 1:存在 + 字节数 + SHA-256                                      */
const raw = {};
LIBS.forEach((lib) => {
  const p = path.join(VENDOR, lib.file);
  if (!fs.existsSync(p)) {
    bad('缺少 vendor 文件:vendor/office-src/' + lib.file
      + '(按 docs/OFFICE-NOTES.md 的来源重新获取,四库哈希见本脚本 LIBS 表)');
    return;
  }
  const buf = fs.readFileSync(p);
  raw[lib.file] = buf;
  if (buf.length !== lib.bytes) {
    bad(lib.file + ' 字节数不符:实际 ' + buf.length + ',期望 ' + lib.bytes);
  }
  const sha = sha256Buf(buf);
  if (sha !== lib.sha) {
    bad(lib.file + ' SHA-256 不符:实际 ' + sha + ',期望 ' + lib.sha);
  }
});
if (!fs.existsSync(GLUE)) { bad('缺少 src/office-worker.src.js'); }
if (!fs.existsSync(KIT)) { bad('缺少 src/officekit.src.js'); }
if (!fs.existsSync(WRITE)) { bad('缺少 src/officewrite.src.js'); }
/* 检测 1 失败就在这里收工:后面的统计量(shrunk / glueText / out …)还没算,
   不能走 report() —— 那会以 TDZ 崩溃收场,把准备好的 ✗ 诊断全丢掉 */
function earlyReport() {
  console.log('==== make-office-part:检测报告 ====');
  notes.forEach((n) => console.log('  · ' + n));
  console.log('==== 失败 ' + problems.length + ' 项 ====');
  problems.forEach((p) => console.log('  ✗ ' + p));
  console.log('  断言:1 存在与哈希 —— 未通过,已中止(未写盘)');
}
if (problems.length) { earlyReport(); process.exit(1); }

/* ------------------------------------------------------------------ */
/* 逐库预处理:shebang / sourceMappingURL / jsdelivr 横幅 / URL 中和      */
const shrunk = {};
LIBS.forEach((lib) => {
  let s = raw[lib.file].toString('utf8').replace(/\r\n/g, '\n').replace(/\r/g, '\n');
  const before = s;

  /* 检测 2:shebang */
  const shebangs = countOf(s, '#!');
  if (shebangs !== lib.shebangs) {
    bad(lib.file + ' 的 "#!" 计数不符:实际 ' + shebangs + ',期望 ' + lib.shebangs);
  }
  let strippedShebang = 0;
  if (/^#!/.test(s)) {
    s = s.replace(/^#![^\n]*\n/, '');
    strippedShebang = 1;
    if (/^#!/.test(s)) { bad(lib.file + ' 剥离 shebang 后仍以 "#!" 开头'); }
    if (countOf(s, '#!') !== lib.shebangs - 1) {
      bad(lib.file + ' 剥离 shebang 后 "#!" 计数应为 ' + (lib.shebangs - 1) + ',实际 ' + countOf(s, '#!'));
    }
  } else if (lib.shebangs > 0) {
    bad(lib.file + ' 期望有 shebang 但文件不以 "#!" 开头');
  }

  /* 检测 3:sourceMappingURL */
  const maps = countOf(s, 'sourceMappingURL');
  s = s.replace(/\n?\/\/# sourceMappingURL=[^\n]*\n?/g, '\n');
  if (countOf(s, 'sourceMappingURL') !== 0) { bad(lib.file + ' 剥离 sourceMappingURL 后仍有残留'); }

  /* 检测 4:jsdelivr 横幅(fflate) */
  let bannerBytes = 0;
  const banner = /^\/\*\*[\s\S]*?\*\/\n/.exec(s);
  if (banner && banner[0].indexOf('https://www.jsdelivr.com') > -1) {
    bannerBytes = Buffer.byteLength(banner[0]);
    s = s.slice(banner[0].length);
  }
  if (countOf(s, 'www.jsdelivr.com') !== 0) { bad(lib.file + ' 仍有 www.jsdelivr.com 残留'); }

  /* 检测 6(前半):URL 中和 */
  let neutralized = 0;
  NEUTRAL.forEach((prefix) => {
    neutralized += countOf(s, prefix);
    s = s.split(prefix).join(NEUTRAL_TO);
  });
  NEUTRAL.forEach((prefix) => {
    if (countOf(s, prefix) !== 0) { bad(lib.file + ' URL 中和后仍有 ' + prefix); }
  });

  const header = lib.header + '\n';
  shrunk[lib.id] = { text: header + s, raw: before, strippedShebang: strippedShebang, maps: maps, bannerBytes: bannerBytes, neutralized: neutralized };
});

let glueSrc = read(GLUE);
let kitSrc = read(KIT);
let writeSrc = read(WRITE);
/* 胶水、包装层与写入层同样做 URL 中和(自检/注释里不应留可发起请求的地址)。
   中和是**逐文件显式**的一步 —— 加进下面的 srcs 数组不会自动带上它 */
let glueNeutral = 0;
NEUTRAL.forEach((prefix) => { glueNeutral += countOf(glueSrc, prefix); glueSrc = glueSrc.split(prefix).join(NEUTRAL_TO); });
let kitNeutral = 0;
NEUTRAL.forEach((prefix) => { kitNeutral += countOf(kitSrc, prefix); kitSrc = kitSrc.split(prefix).join(NEUTRAL_TO); });
let writeNeutral = 0;
NEUTRAL.forEach((prefix) => { writeNeutral += countOf(writeSrc, prefix); writeSrc = writeSrc.split(prefix).join(NEUTRAL_TO); });
const glueText = '/* 办公文件解析核心(worker / 主线程共用;网络桩 + ZIP 预扫描 + 解析路径) —— src/office-worker.src.js,勿手改 */\n' + glueSrc;

/* ------------------------------------------------------------------ */
/* 检测 11:常量同值断言(TEXT_MAX ↔ ATTACH_TEXT_MAX)                    */
/* 胶水的 TEXT_MAX(办公文档正文截断)与 src/appD.part 的 ATTACH_TEXT_MAX(纯文本附件读取上限)
   现行值相同、但分处两个文件,没有任何机制锁在一起 —— 改一处就静默不同步(同一条消息里
   办公文档与文本附件会用两种上限)。最小做法 = 构建期取值比对:取不到(被改名/删除)或不等
   都直接 bad()(走既有的"失败不写盘"路径),避免断言悄悄失效 */
const textMaxGlue = /var\s+TEXT_MAX\s*=\s*(\d+)/.exec(glueSrc);
const textMaxAppD = /var\s+ATTACH_TEXT_MAX\s*=\s*(\d+)/.exec(read(path.join(SRC, 'appD.part')));
if (!textMaxGlue || !textMaxAppD) {
  bad('TEXT_MAX ↔ ATTACH_TEXT_MAX 断言无法取值(office-worker.src.js:'
    + (textMaxGlue ? textMaxGlue[1] : '缺') + ' / src/appD.part:' + (textMaxAppD ? textMaxAppD[1] : '缺')
    + ')—— 常量被改名或删除了吗?两处必须同步');
} else if (textMaxGlue[1] !== textMaxAppD[1]) {
  bad('TEXT_MAX(office-worker.src.js=' + textMaxGlue[1] + ') ≠ ATTACH_TEXT_MAX(src/appD.part='
    + textMaxAppD[1] + '):两处必须同值,否则同一条消息里办公文档与文本附件用两种截断上限');
} else {
  note('常量同值:TEXT_MAX == ATTACH_TEXT_MAX == ' + textMaxGlue[1] + '(office-worker.src.js ↔ src/appD.part)');
}

/* ------------------------------------------------------------------ */
/* 检测 12:常量同值断言(OfficeWrite 的 ZIP / 图像阈值 ↔ 解析层 LIMITS)   */
/* 方案 §2.7「阈值单一来源」:OfficeWrite(src/officewrite.src.js)与解析层
   (src/office-worker.src.js 的 LIMITS)分处两个文件、没有任何机制锁在一起 ——
   改一处就静默不同步(同一份文件会被两套判据判)。最小做法 = 构建期取值比对:
   取不到(被改名/删除)或不等都直接 bad()(走既有的"失败不写盘"路径),避免断言悄悄失效。
   两侧**键名**不同(maxTotalUncompressed ↔ maxUncompressedTotal),故按下表逐对取值;
   数字侧允许 150 * 1024 * 1024 这类算式(取值时展开) */
const LIMIT_PAIRS = [
  ['maxEntries', 'maxEntries'],
  ['maxTotalUncompressed', 'maxUncompressedTotal'],
  ['maxSingleEntry', 'maxSingleEntry'],
  ['maxRatio', 'maxRatio'],
  ['maxImageBytes', 'maxImageBytes']
];
function constNum(text, key) {
  const m = new RegExp('\\b' + key + '\\s*:\\s*([0-9][0-9\\s*]*)').exec(text);
  if (!m) { return null; }
  const expr = m[1].replace(/\s+/g, '');
  if (!/^[0-9]+(\*[0-9]+)*$/.test(expr)) { return null; }
  return expr.split('*').reduce((a, b) => a * Number(b), 1);
}
const workerSrc = read(path.join(SRC, 'office-worker.src.js'));
LIMIT_PAIRS.forEach((pair) => {
  const w = constNum(writeSrc, pair[0]);
  const p = constNum(workerSrc, pair[1]);
  if (w == null || p == null) {
    bad('常量同值断言无法取值(' + pair[0] + ' / 解析层 ' + pair[1] + '):'
      + 'officewrite.src.js=' + (w == null ? '缺' : w) + ' / office-worker.src.js='
      + (p == null ? '缺' : p) + ' —— 常量被改名或删除了吗?两处必须同步');
  } else if (w !== p) {
    bad('常量不同值:' + pair[0] + '(officewrite.src.js=' + w + ') ≠ ' + pair[1]
      + '(office-worker.src.js=' + p + '):ZIP 预扫描阈值必须单一来源,否则解析与写入用两套判据');
  } else {
    note('常量同值:' + pair[0] + ' == 解析层 ' + pair[1] + ' == ' + w);
  }
});
/* 算式断言:maxImagesPerCall x maxImageBytes == 解析层 maxImagesTotal(三值同源,改一处即红) */
const imgPerCall = constNum(writeSrc, 'maxImagesPerCall');
const imgBytes = constNum(writeSrc, 'maxImageBytes');
const imgTotal = constNum(workerSrc, 'maxImagesTotal');
if (imgPerCall == null || imgBytes == null || imgTotal == null) {
  bad('常量算式断言无法取值(maxImagesPerCall / maxImageBytes / 解析层 maxImagesTotal)'
    + ':officewrite.src.js=' + (imgPerCall == null ? '缺' : imgPerCall) + ' x '
    + (imgBytes == null ? '缺' : imgBytes) + ' / office-worker.src.js='
    + (imgTotal == null ? '缺' : imgTotal) + ' —— 常量被改名或删除了吗?');
} else if (imgPerCall * imgBytes !== imgTotal) {
  bad('常量算式不成立:maxImagesPerCall(' + imgPerCall + ') x maxImageBytes(' + imgBytes + ') = '
    + (imgPerCall * imgBytes) + ' ≠ 解析层 maxImagesTotal(' + imgTotal + '):三值同源,改一处即红');
} else {
  note('常量算式:maxImagesPerCall x maxImageBytes == 解析层 maxImagesTotal == ' + imgTotal
    + '(' + imgPerCall + ' x ' + imgBytes + ')');
}

/* ------------------------------------------------------------------ */
/* 检测 13:模板件必含 XML 声明(方案 §3 S5;批次 A 审查 P3-2 裁定 (b))     */
/* 背景:xmlSerialize 对"无声明"的部件会合成标准声明(裁定 (a),运行期兜底),但模板串
   自己漏写声明会让新产出的部件与 docx / pptx 惯例不一致(Word 常容忍、PowerPoint 对
   声明更敏感)。所以两半都做:模板必含 + 运行期合成。
   判据:`var TPL_* = { ... };` 模板块内**每个部件**(一层键,值全部是字符串)都必须以
   TPL_DECL 字面量开头 ⇒ 键数 == 声明数 == 标准声明串出现次数,三者不等即 bad()。
   **模板块必须保持扁平**(一层键):嵌套子对象会让键数虚高而"大声失败",不是静默放行,
   但那时请把子对象挪成独立的 `var TPL_*` 块或改判据,别把这条断言绕过去。 */
const TPL_DECL = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>';
const TPL_RE = /var\s+(TPL_[A-Z0-9_]+)\s*=\s*\{([\s\S]*?)\n[ \t]*\};\n/g;
const TPL_KEY_RE = /\n[ \t]+[A-Za-z][A-Za-z0-9]*: /g;
const tplBlocks = [];
const tplReadings = [];
let tplMatch = null;
while ((tplMatch = TPL_RE.exec(writeSrc)) !== null) {
  tplBlocks.push({ name: tplMatch[1], body: tplMatch[2] });
}
if (!tplBlocks.length) {
  bad('没有找到任何 "var TPL_* = { ... }" 模板块:检测 13(模板必含 XML 声明)取不到数'
    + ' —— 模板块被改名 / 删除了吗?(批次 B 起应有 TPL_DOCX)');
}
tplBlocks.forEach((b) => {
  const keys = (b.body.match(TPL_KEY_RE) || []).length;
  const decls = countOf(b.body, '<?xml ');
  const stds = countOf(b.body, TPL_DECL);
  tplReadings.push({ name: b.name, keys: keys, decls: decls, stds: stds });
  if (keys === 0) {
    bad(b.name + ' 里没解析出任何部件键(键写成"缩进 + 名字 + 冒号 + 空格"):分隔格式变了吗?');
  } else if (decls !== keys || stds !== keys) {
    bad(b.name + ' 的每个部件都必须以标准 XML 声明开头 —— 部件 ' + keys + ' / 声明 '
      + decls + ' / 标准声明串 ' + stds + ':三者不等即有人漏写(漏写的部件会静默产出无声明部件)');
  } else {
    note('模板声明:' + b.name + ' 的 ' + keys + ' 个部件全部以标准 XML 声明开头');
  }
});

/* ------------------------------------------------------------------ */
/* 检测 5:转义与 HTML 安全(源内)                                        */
const srcs = LIBS.map((l) => ({ label: l.file, text: shrunk[l.id].text }));
srcs.push({ label: 'src/office-worker.src.js', text: glueText });
srcs.push({ label: 'src/officekit.src.js', text: kitSrc });
srcs.push({ label: 'src/officewrite.src.js', text: writeSrc });
srcs.forEach((s) => {
  const closeTags = countOf(s.text, '</script');
  const openTags = countOf(s.text, '<script');
  if (closeTags !== 0) { bad(s.label + ' 源内出现 " </script"(会破坏内联),需人工处理'); }
  if (openTags !== 0) { bad(s.label + ' 源内出现 "<script"(会破坏内联),需人工处理'); }
});
const commentHits = srcs.reduce((n, s) => n + countOf(s.text, '<!--'), 0);
srcs.forEach((s) => {
  const n = countOf(s.text, '<!--');
  if (n) { note('源内 "<!--" x' + n + ' @ ' + s.label); }
});

/* 检测 6(后半):主机白名单 */
const hosts = {};
srcs.forEach((s) => {
  const re = /https?:\/\/([a-z0-9._:-]+)/gi;
  let m;
  while ((m = re.exec(s.text)) !== null) {
    const h = m[1].toLowerCase().replace(/:\d+$/, '');
    hosts[h] = (hosts[h] || 0) + 1;
  }
});
const hostList = Object.keys(hosts).sort();
hostList.forEach((h) => {
  if (HOST_WHITELIST.indexOf(h) < 0) {
    bad('出现白名单外主机: ' + h + '(若确属库内字符串需人工判定后更新 HOST_WHITELIST)');
  }
});

/* ------------------------------------------------------------------ */
/* 检测 8:内容标识(版本锁定的职责在检测 1 的 SHA-256)                     */
LIBS.forEach((lib) => {
  lib.marks.forEach((mk) => {
    const n = countOf(shrunk[lib.id].text, mk[0]);
    /* 内容标识用存在性断言(≥min);精确计数仅用于 SheetJS 的版本串(理论上唯一) */
    const ok = mk[2] ? (n === mk[1]) : (n >= mk[1]);
    if (!ok) {
      bad(lib.file + ' 内容标识 "' + mk[0] + '" 计数异常:实际 ' + n + ',期望 ' + (mk[2] ? '=' + mk[1] : '≥' + mk[1]));
    }
  });
});

/* ------------------------------------------------------------------ */
/* 拼装                                                                 */
const banner = '<!-- ===== 内联办公文件解析库(mammoth 1.12.3 / SheetJS CE 0.20.3 / '
  + '@jose.espana/docstream 0.1.3 / fflate 0.8.3)+ OfficeKit 包装层 + OfficeWrite 写入层 —— '
  + '由 scripts/make-office-part.js 生成,请勿手改 ===== -->\n';
const blocks = LIBS.map((l) => textBlock(l.id, shrunk[l.id].text));
blocks.push(textBlock(GLUE_ID, glueText));
blocks.push('<script>\n/* ===== OfficeKit 包装层(ES5) ===== */\n' + escapeForInline(kitSrc) + '\n<' + '/script>\n');
blocks.push('<script>\n/* ===== OfficeWrite 写入层(ES5) ===== */\n' + escapeForInline(writeSrc) + '\n<' + '/script>\n');
const out = banner + blocks.join('');

/* 检测 7:块结构与编译 */
const openCount = countOf(out, '<script>') + countOf(out, '<script type="text/plain"');
const closeCount = countOf(out, '<' + '/script>');
if (openCount !== closeCount) { bad('<script> 与 </script> 数量不等: ' + openCount + ' vs ' + closeCount); }
if (openCount !== LIBS.length + 3) { bad('script 块数量异常: ' + openCount + ',期望 ' + (LIBS.length + 3)); }

function compileProbe(text, label) {
  try {
    /* eslint-disable-next-line no-new-func */
    new Function(text);
  } catch (e) {
    bad(label + ' new Function 编译失败:' + (e && e.message));
  }
}
LIBS.forEach((l) => { compileProbe(shrunk[l.id].text, l.file); });
compileProbe(glueText, 'office-worker-glue');
compileProbe(kitSrc, 'OfficeKit 包装层');
compileProbe(writeSrc, 'OfficeWrite 写入层');

/* 文本块往返一致(逐字节可逆) */
const roundTrip = LIBS.map((l) => ({ id: l.id, text: shrunk[l.id].text }));
roundTrip.push({ id: GLUE_ID, text: glueText });
roundTrip.push({ id: 'officewrite.src.js', text: writeSrc });
roundTrip.forEach((l) => {
  const back = unescapeInline(escapeForInline(l.text));
  if (back !== l.text) { bad(l.id + ' 转义后无法逐字节还原'); }
});

if (problems.length) { report(); process.exit(1); }

/* 检测 9:幂等 */
const old = fs.existsSync(OUT) ? fs.readFileSync(OUT, 'utf8') : null;
if (old === out) { idempotent = '与现有产物一致(未写盘)'; }
else { fs.writeFileSync(OUT, out); }

/* ------------------------------------------------------------------ */
/* 检测 10:输出统计                                                     */
function report() {
  console.log('==== make-office-part:检测报告 ====');
  LIBS.forEach((l) => {
    const s = shrunk[l.id];
    console.log('  ' + l.id.padEnd(22) + ' vendor=' + Buffer.byteLength(raw[l.file])
      + ' B  产物=' + Buffer.byteLength(s.text) + ' B  shebang剥=' + s.strippedShebang
      + '  map剥=' + s.maps + '  横幅剥=' + s.bannerBytes + ' B  中和URL=' + s.neutralized
      + '  <!-- x' + countOf(s.text, '<!--'));
  });
  console.log('  ' + GLUE_ID.padEnd(22) + ' 产物=' + Buffer.byteLength(glueText) + ' B  中和URL=' + glueNeutral);
  console.log('  ' + 'officekit(包装层)'.padEnd(20) + ' 产物=' + Buffer.byteLength(kitSrc) + ' B  中和URL=' + kitNeutral);
  console.log('  ' + 'officewrite(写入层)'.padEnd(20) + ' 产物=' + Buffer.byteLength(writeSrc) + ' B  中和URL=' + writeNeutral);
  console.log('  主机白名单检查:' + hostList.length + ' 个主机全部在内(' + hostList.join(', ') + ')');
  if (textMaxGlue && textMaxAppD && textMaxGlue[1] === textMaxAppD[1]) {
    console.log('  常量同值:TEXT_MAX == ATTACH_TEXT_MAX == ' + textMaxGlue[1] + '(office-worker.src.js ↔ src/appD.part)');
  }
  /* 检测 12 的读数(取值成功且相等才打印;不等/取不到的情形由下面的 ✗ 列表点出) */
  LIMIT_PAIRS.forEach((pair) => {
    const w = constNum(writeSrc, pair[0]);
    const p = constNum(workerSrc, pair[1]);
    if (w != null && p != null && w === p) {
      console.log('  常量同值:' + pair[0] + ' == 解析层 ' + pair[1] + ' == ' + w + '(officewrite.src.js ↔ src/office-worker.src.js)');
    }
  });
  if (imgPerCall != null && imgBytes != null && imgTotal != null && imgPerCall * imgBytes === imgTotal) {
    console.log('  常量算式:maxImagesPerCall x maxImageBytes == 解析层 maxImagesTotal == ' + imgTotal
      + '(' + imgPerCall + ' x ' + imgBytes + ')');
  }
  console.log('  源内 <!-- 合计 x' + commentHits + '(转义后产物内未转义计数应为 0)');
  /* 检测 13 的读数("断言跑过必须可见") */
  tplReadings.forEach((r) => {
    console.log('  模板声明:' + r.name + ' 部件 ' + r.keys + ' / 标准声明串 x' + r.stds
      + (r.keys === r.stds && r.keys === r.decls ? ' ⇒ 每件都带声明' : ' ⇒ ✗ 有部件漏写声明'));
  });
  console.log('  产物:office.part ' + Buffer.byteLength(out) + ' B (' + out.split('\n').length + ' 行)  md5=' + md5(out));
  /* 失败路径不宣称幂等:idempotent 此时还停在"首写"(写盘那一步根本没执行)——
     如实写"失败未写盘",免得报告自相矛盾 */
  console.log(problems.length
    ? '  幂等:失败未写盘(断言未通过,现有产物未被触碰)'
    : '  幂等:' + idempotent);
  if (problems.length) {
    console.log('==== 失败 ' + problems.length + ' 项 ====');
    problems.forEach((p) => console.log('  ✗ ' + p));
  } else {
    console.log('  断言:全部通过(1 存在与哈希 / 2 shebang / 3 sourceMappingURL / 4 横幅 / 5 转义与 HTML 安全'
      + ' / 6 URL 中和与主机白名单 / 7 块结构与编译 / 8 内容标识 / 9 幂等 / 10 统计 / 11 常量同值'
      + ' / 12 常量同值(OfficeWrite ↔ 解析层 LIMITS) / 13 模板含 XML 声明)');
  }
}
report();
if (problems.length) { process.exit(1); }
