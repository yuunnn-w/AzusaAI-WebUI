/* 由 vendor/render-src/ 生成可内联的 src/render.part —— 唯一入口:node scripts/make-render-part.js [--verify]
 *
 * 产物结构(4 个可执行 <script> 块,顺序即依赖顺序):
 *   1. jszip 3.10.2 (MIT 分支)          —— jszip.min.js(UMD,classic script)
 *   2. docx-preview 0.4.0 (Apache-2.0)  —— docx-preview.min.js;其 UMD 在**加载时**读 globalThis.JSZip
 *                                          ⇒ 必须在 jszip 之后(顺序即依赖顺序)
 *   3. modern-screenshot 4.7.0 (MIT)    —— dist/index.js(IIFE,全局 modernScreenshot)
 *   4. window.RenderKit 元信息(版本串从 LIBS[] 拼 —— 单点,防两处漂)
 *
 * 幂等:同样输入生成完全相同的字节(产物不含时间戳)。
 * 任一条断言失败 → 打印原因 + exit 1,且**不写盘**。
 *
 * 断言清单(12 条;--verify 与正常模式共用同一生成路径):
 *   1  存在/字节/SHA-256(3 库;逐件对 vendor/render-src/ 现盘)
 *   2  shebang "#!" 计数(三库各 0)
 *   3  sourceMappingURL 剥离 + 断言归零(docx-preview 1 → 0;jszip 0;ms 0)
 *   4  许可面(三库分流):① docx-preview 含 "Released under Apache License 2.0"(横幅,必须保留)
 *      ② jszip 含 "Dual licenced under the MIT license or GPLv3"(横幅,必须保留)
 *      ③ modern-screenshot 的 dist 无许可头 = 上游事实(grep -i licen = 0)⇒ 不断言横幅,
 *         改断言 LICENSE.modern-screenshot 存在且 sha256 锁定(与 MANIFEST 同源)+ 块头注释自述
 *   5  转义与 HTML 安全:源内 "</script"/"<script"/"<!--" 各 0;escapeForInline 往返逐字节可逆
 *   6  主机白名单:三库 https?://<host> 并集 = 8 个,出现表外主机 ⇒ exit 1
 *      (**不做字符串中和**:schemas.microsoft.com、www.w3.org 与 docx 是命名空间与关系类型 URI,参与解析,替换会坏解析;
 *       其余只在注释/许可链接里 —— 方案 §2.1-② 断言 6)
 *   7  块结构与编译:openCount === closeCount === 4;3 库 + RenderKit 行 new Function() 全过
 *   8  内容标识(marks;防"文件被换但哈希表没改"的双保险)
 *   9  幂等:与现盘 src/render.part 逐字节比对:一致 ⇒ 与现有产物一致(未写盘)
 *   10 输出统计:每库 vendor B → 产物 B(剥 map 后)/ 中和 0 / 产物总字节 + 行数 + md5
 *   11 RenderKit 版本串与 LIBS[] 表同源(生成期断言版本逐个命中)
 *   12 --verify:按当前输入重新生成的期望产物 ↔ 现盘 src/render.part 逐字节一致 ⇒ PASS;否则 exit 1,不写盘
 */
'use strict';
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');

const ROOT = path.resolve(__dirname, '..');
const VENDOR = path.join(ROOT, 'vendor', 'render-src');
const SRC = path.join(ROOT, 'src');
const OUT = path.join(SRC, 'render.part');
const VERIFY = process.argv.slice(2).some((a) => a === '--verify');
const unknownArgs = process.argv.slice(2).filter((a) => a !== '--verify');
if (unknownArgs.length) {
  console.error('未知参数: ' + unknownArgs.join(' ') + '(可用:--verify)');
  process.exit(1);
}

const LIBS = [
  {
    id: 'render-lib-jszip',
    file: 'jszip-3.10.2.min.js',
    version: '3.10.2',
    bytes: 97781,
    sha: '7f839b2d4688b845c105ebf5d2f9803075f91ea0fe72bdaac176c3a04dd3d2c1',
    maps: 0,
    banner: 'Dual licenced under the MIT license or GPLv3',
    header: '/* ===== 内联库: jszip 3.10.2 (MIT 分支;含打包依赖 pako/lie/readable-stream/setimmediate,MIT)。'
      + '许可双分支 MIT OR GPL-3.0-or-later,本包采 MIT —— 顶部横幅随包分发 ===== */',
    marks: [['JSZip', 2], ['loadAsync', 3]],
    shebangs: 0
  },
  {
    id: 'render-lib-docx-preview',
    file: 'docx-preview-0.4.0.min.js',
    version: '0.4.0',
    bytes: 75297,
    sha: '051ef503f2677d53159a388b7384e950eda41ea4e47a103e5e36f124d7faea40',
    maps: 1,
    banner: 'Released under Apache License 2.0',
    header: '/* ===== 内联库: docx-preview 0.4.0 (Apache-2.0) =====\n'
      + '   ⚠ 必须在 jszip 之后:其 UMD 在**加载时**读取 globalThis.JSZip（min.js 的 UMD 尾 t((e=…globalThis…).docx={},e.JSZip)） */',
    marks: [['renderAsync', 1, true], ['parseAsync', 1, true], ['useBase64URL', 2]],
    shebangs: 0
  },
  {
    id: 'render-lib-modern-screenshot',
    file: 'modern-screenshot-4.7.0.iife.js',
    version: '4.7.0',
    bytes: 29290,
    sha: 'bb36665889124a0b6e15f16045265737449c3bdcf2712cdb08af3cfa01563e2b',
    maps: 0,
    banner: null,
    header: '/* ===== 内联库: modern-screenshot 4.7.0 (MIT；本库 dist 无许可头 = 上游事实,'
      + '许可文本见 vendor/render-src/LICENSE.modern-screenshot) ===== */',
    marks: [['domToCanvas', 1, true], ['domToForeignObjectSvg', 1, true], ['modernScreenshot', 1]],
    shebangs: 0
  }
];

/* modern-screenshot 的许可声明走 LICENSE 文件(其 dist 内无许可文本) —— 文件 sha 锁定(与 MANIFEST 同源) */
const MS_LICENSE = { file: 'LICENSE.modern-screenshot', bytes: 1077, sha: '1daae2db27daba18a7e21bb56e12e19b8de6c1197658921f09bbe550f7106579' };

/* 三库 https?://<host> 并集(实测;小写归一后).出现白名单外主机 ⇒ 失败,新增主机必须人工判定后更新本表 */
const HOST_WHITELIST = [
  'docx', 'github.com', 'raw.github.com', 'schemas.microsoft.com', 'schemas.openxmlformats.org',
  'stuartk.com', 'stuk.github.io', 'www.w3.org'
];

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

/* ------------------------------------------------------------------ */
/* 检测 1:存在 + 字节数 + SHA-256                                      */
const raw = {};
LIBS.forEach((lib) => {
  const p = path.join(VENDOR, lib.file);
  if (!fs.existsSync(p)) {
    bad('缺少 vendor 文件:vendor/render-src/' + lib.file
      + '(取件命令见 specs/b31-s7-renderer-plan.md 附录 E;三库哈希见本脚本 LIBS 表 / vendor/render-src/MANIFEST.json)');
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
/* 检测 4③:modern-screenshot 的 LICENSE 文件(其 dist 无许可头) */
{
  const p = path.join(VENDOR, MS_LICENSE.file);
  if (!fs.existsSync(p)) {
    bad('缺少 vendor 文件:vendor/render-src/' + MS_LICENSE.file
      + '(modern-screenshot 的 dist 无许可头,许可随该文件分发 —— 缺它 = 许可声明链断)');
  } else {
    const buf = fs.readFileSync(p);
    if (buf.length !== MS_LICENSE.bytes) {
      bad(MS_LICENSE.file + ' 字节数不符:实际 ' + buf.length + ',期望 ' + MS_LICENSE.bytes);
    }
    const sha = sha256Buf(buf);
    if (sha !== MS_LICENSE.sha) {
      bad(MS_LICENSE.file + ' SHA-256 不符:实际 ' + sha + ',期望 ' + MS_LICENSE.sha);
    }
  }
}
function earlyReport() {
  console.log('==== make-render-part:检测报告 ====');
  notes.forEach((n) => console.log('  · ' + n));
  console.log('==== 失败 ' + problems.length + ' 项 ====');
  problems.forEach((p) => console.log('  ✗ ' + p));
  console.log('  断言:1 存在与哈希 —— 未通过,已中止(未写盘)');
}
if (problems.length) { earlyReport(); process.exit(1); }

/* ------------------------------------------------------------------ */
/* 逐库预处理:shebang / sourceMappingURL / 许可横幅                     */
const shrunk = {};
LIBS.forEach((lib) => {
  let s = raw[lib.file].toString('utf8').replace(/\r\n/g, '\n').replace(/\r/g, '\n');
  const before = s;

  /* 检测 2:shebang */
  const shebangs = countOf(s, '#!');
  if (shebangs !== lib.shebangs) {
    bad(lib.file + ' 的 "#!" 计数不符:实际 ' + shebangs + ',期望 ' + lib.shebangs);
  }
  if (/^#!/.test(s)) {
    bad(lib.file + ' 以 "#!" 开头(本表期望 0,剥离逻辑未启用 —— 先人工判定)');
  }

  /* 检测 3:sourceMappingURL */
  const maps = countOf(s, 'sourceMappingURL');
  if (maps !== lib.maps) {
    bad(lib.file + ' 的 sourceMappingURL 计数不符:实际 ' + maps + ',期望 ' + lib.maps);
  }
  s = s.replace(/\n?\/\/# sourceMappingURL=[^\n]*\n?/g, '\n');
  if (countOf(s, 'sourceMappingURL') !== 0) { bad(lib.file + ' 剥离 sourceMappingURL 后仍有残留'); }

  /* 检测 4①②:许可横幅(ms 的 banner = null,走 LICENSE 文件断言) */
  if (lib.banner !== null) {
    if (countOf(s, lib.banner) < 1) {
      bad(lib.file + ' 缺许可横幅 "' + lib.banner + '"(许可声明随包分发,横幅必须保留)');
    }
  } else if (/(licen)/i.test(s)) {
    note(lib.file + ' 出现 "licen"(大小写不敏感)x' + (s.match(/licen/gi) || []).length
      + ' —— 本表记为"dist 无许可头",若上游改了需人工复核(不改写断言)');
  }

  shrunk[lib.id] = { text: s, raw: before, maps: maps };
});

/* ------------------------------------------------------------------ */
/* 检测 5:转义与 HTML 安全(源内)                                        */
const srcs = LIBS.map((l) => ({ label: l.file, text: shrunk[l.id].text }));
srcs.forEach((s) => {
  const closeTags = countOf(s.text, '</script');
  const openTags = countOf(s.text, '<script');
  const comments = countOf(s.text, '<!--');
  if (closeTags !== 0) { bad(s.label + ' 源内出现 "</script"(会破坏内联),需人工处理'); }
  if (openTags !== 0) { bad(s.label + ' 源内出现 "<script"(会破坏内联),需人工处理'); }
  if (comments !== 0) { bad(s.label + ' 源内出现 "<!--"(会破坏内联),需人工处理'); }
});

/* ------------------------------------------------------------------ */
/* 检测 6:主机白名单(并集;不中和)                                      */
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
const wlSorted = HOST_WHITELIST.slice().sort();
if (hostList.join(',') !== wlSorted.join(',')) {
  bad('主机并集与本表不一致(并集 ' + hostList.length + ' 个: ' + hostList.join(', ')
    + ';本表 ' + wlSorted.length + ' 个: ' + wlSorted.join(', ') + ')—— 库换版 / 表漂皆需人工对齐');
} else {
  note('主机白名单:并集恰 ' + hostList.length + ' 个,全部在内(' + hostList.join(', ') + ')');
}

/* ------------------------------------------------------------------ */
/* 检测 8:内容标识(版本锁定的职责在检测 1 的 SHA-256)                     */
LIBS.forEach((lib) => {
  lib.marks.forEach((mk) => {
    const n = countOf(shrunk[lib.id].text, mk[0]);
    const ok = mk[2] ? (n === mk[1]) : (n >= mk[1]);
    if (!ok) {
      bad(lib.file + ' 内容标识 "' + mk[0] + '" 计数异常:实际 ' + n + ',期望 ' + (mk[2] ? '=' + mk[1] : '≥' + mk[1]));
    }
  });
});

/* ------------------------------------------------------------------ */
/* 检测 11:RenderKit 版本串与 LIBS[] 同源(生成期)                        */
function versionOf(id) {
  const l = LIBS.filter((x) => x.id === id)[0];
  if (!l) { bad('LIBS 表缺 ' + id + '(版本串无法拼接 —— 单点前提被破坏)'); return ''; }
  return l.version;
}
const vDocx = versionOf('render-lib-docx-preview');
const vJszip = versionOf('render-lib-jszip');
const vMs = versionOf('render-lib-modern-screenshot');
const renderkitCode = 'window.RenderKit = { available: true, why: "", libs: { docxPreview: "'
  + vDocx + '", jszip: "' + vJszip + '", modernScreenshot: "' + vMs + '" } };\n';

/* ------------------------------------------------------------------ */
/* 拼装                                                                 */
const banner = '<!-- ===== 内嵌渲染库（docx-preview ' + vDocx + ' / jszip ' + vJszip + ' / modern-screenshot '
  + vMs + '）+ RenderKit 元信息 ——\n     由 scripts/make-render-part.js 生成，请勿手改 ===== -->\n';
const blocks = LIBS.map((l) => '<script>\n' + l.header + '\n' + escapeForInline(shrunk[l.id].text) + '\n<' + '/script>\n');
blocks.push('<script>\n/* ===== RenderKit 元信息（生成期写死；可用性唯一来源） ===== */\n'
  + escapeForInline(renderkitCode) + '<' + '/script>\n');
const out = banner + blocks.join('');

/* 检测 7:块结构与编译 */
const openCount = countOf(out, '<script>');
const closeCount = countOf(out, '<' + '/script>');
if (openCount !== closeCount) { bad('<script> 与 </script> 数量不等: ' + openCount + ' vs ' + closeCount); }
if (openCount !== LIBS.length + 1) { bad('script 块数量异常: ' + openCount + ',期望 ' + (LIBS.length + 1)); }
function compileProbe(text, label) {
  try {
    /* eslint-disable-next-line no-new-func */
    new Function(text);
  } catch (e) {
    bad(label + ' new Function 编译失败:' + (e && e.message));
  }
}
LIBS.forEach((l) => { compileProbe(shrunk[l.id].text, l.file); });
compileProbe(renderkitCode, 'RenderKit 元信息行');

/* 转义往返逐字节可逆 */
LIBS.forEach((l) => {
  const back = unescapeInline(escapeForInline(shrunk[l.id].text));
  if (back !== shrunk[l.id].text) { bad(l.file + ' 转义后无法逐字节还原'); }
});
{
  const back = unescapeInline(escapeForInline(renderkitCode));
  if (back !== renderkitCode) { bad('RenderKit 元信息行 转义后无法逐字节还原'); }
}

/* 检测 11(后半):版本串逐个命中产物 */
[[vDocx, 'docxPreview'], [vJszip, 'jszip'], [vMs, 'modernScreenshot']].forEach((pair) => {
  if (countOf(renderkitCode, pair[0]) < 1) {
    bad('RenderKit 版本串缺 ' + pair[1] + ' = "' + pair[0] + '"(版本串必须从 LIBS[] 拼)');
  }
});

if (problems.length) {
  console.log('==== make-render-part:检测报告 ====');
  console.log('==== 失败 ' + problems.length + ' 项 ====');
  problems.forEach((p) => console.log('  ✗ ' + p));
  console.log('  断言:未全部通过 —— 已中止(未写盘)');
  process.exit(1);
}

/* ------------------------------------------------------------------ */
/* 检测 12 / 9:--verify(只读) / 幂等写盘                                 */
const old = fs.existsSync(OUT) ? fs.readFileSync(OUT, 'utf8') : null;
let verifyVerdict = null;
if (VERIFY) {
  if (old === null) {
    verifyVerdict = { pass: false, why: 'src/render.part 不存在(先跑 node scripts/make-render-part.js 生成)' };
  } else if (old === out) {
    verifyVerdict = { pass: true, why: '与现盘逐字节一致' };
  } else {
    verifyVerdict = { pass: false, why: '与现盘不一致(现盘 ' + Buffer.byteLength(old) + ' B / md5 '
      + md5(old) + ';期望 ' + Buffer.byteLength(out) + ' B / md5 ' + md5(out) + ')' };
  }
} else if (old === out) {
  idempotent = '与现有产物一致(未写盘)';
} else {
  fs.writeFileSync(OUT, out);
}

/* ------------------------------------------------------------------ */
/* 检测 10:输出统计                                                     */
console.log('==== make-render-part:检测报告 ====');
LIBS.forEach((l) => {
  const s = shrunk[l.id];
  console.log('  ' + l.id.padEnd(28) + ' vendor=' + Buffer.byteLength(raw[l.file])
    + ' B  产物=' + Buffer.byteLength(s.text) + ' B  map剥=' + s.maps
    + '  中和=' + 0 + '(本段设计不做字符串中和;理由见脚本头断言 6)');
});
console.log('  主机白名单检查:' + hostList.length + ' 个主机全部在内(' + hostList.join(', ') + ')');
console.log('  许可面:docx-preview "Released under Apache License 2.0" 横幅保留;jszip "Dual licenced under the MIT license or GPLv3" 横幅保留;'
  + ' modern-screenshot dist 无许可头 ⇒ LICENSE.modern-screenshot(' + MS_LICENSE.bytes + ' B / ' + MS_LICENSE.sha.slice(0, 8) + '…)sha 锁定 + 块头自述');
console.log('  RenderKit:' + renderkitCode.trim());
console.log('  产物:render.part ' + Buffer.byteLength(out) + ' B (' + out.split('\n').length + ' 行)  md5=' + md5(out));
if (VERIFY) {
  console.log('  --verify:' + (verifyVerdict.pass ? 'PASS(' + verifyVerdict.why + ')' : 'FAIL(' + verifyVerdict.why + ')')
    + '  [只读,未写盘]');
} else {
  console.log('  幂等:' + idempotent);
}
console.log('  断言:全部通过(1 存在与哈希 / 2 shebang / 3 sourceMappingURL / 4 许可面(三库分流)'
  + ' / 5 转义与 HTML 安全 / 6 主机白名单(并集恰 ' + HOST_WHITELIST.length + ' 个) / 7 块结构与编译'
  + ' / 8 内容标识 / 9 幂等 / 10 输出统计 / 11 版本串同源 / 12 --verify(本次' + (VERIFY ? '启用' : '未启用') + '))');
if (VERIFY && !verifyVerdict.pass) { process.exit(1); }
