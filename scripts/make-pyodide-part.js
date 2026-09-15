/* 由 vendor/pyodide-src/ 生成可内联的 src/pyodide.part —— 唯一入口:node scripts/make-pyodide-part.js
 *
 * 产物结构:**一个** `<script type="text/plain" id="pyodide-assets">` 块,
 * 块内用行帧切分为多个"段"(section),**不含任何 JS 逻辑**——载荷由 worker 侧的加载器读取:
 *
 *   ;;;PYODIDE-PART 1 <段数>
 *   ;;;PYODIDE-META <单行 JSON: 版本/段清单/字节账>
 *   ;;;PYODIDE-SECTION <id> <text|b64gz> <字符数>
 *   <该段正文,恰好 N 个字符>
 *   ... 重复 ...
 *   ;;;PYODIDE-END
 *
 * 段 id 与文件名一一对应(加载器按 `indexURL + file_name` 直接取段):
 *   pyodide.asm.mjs      text   改写后的 asm.mjs(1 处 export default → 全局赋值;3 处 import.meta.url → 字符串字面量)
 *   pyodide.js           text   官方 classic 构建产物,原样内嵌(不改写)
 *   pyodide-lock.min.json text  由官方 pyodide-lock.json 剪裁,只含所选集(含依赖闭包)+ 自造条目
 *   pyodide.asm.wasm     b64gz  base64(gzip(原始字节))
 *   python_stdlib.zip    b64gz
 *   <每个 wheel 文件名>   b64gz
 *
 * 文本段转义:HTML 解析器不认 JS 语法,`</script` 会直接终止脚本块、`<!--` 会进入 escaped 状态。
 * 与 make-pdfjs-part.js 同法:`</script` → `<\/script`、`<!--` → `<\!--`(加载器读回后需还原)。
 * 段正文里**不得**出现帧标记前缀 `;;;PYODIDE`(构建期断言,当前实测 0 命中)。
 *
 * 输入(全部在 vendor/pyodide-src/,由取件流程落盘;vendor/ 不进版本库):
 *   pyodide-lock.json   官方全量锁文件(356 包)
 *   packages.json       选包清单:{fromOfficialLock:[规范名…], selfAuthored:[…], depExclusions:[…]}
 *   pyodide.asm.mjs / pyodide.js / pyodide.asm.wasm / python_stdlib.zip
 *   wheels/*.whl        官方闭包(jsDelivr)+ 自造条目(PyPI)的全部 wheel
 *   MANIFEST.json       取件清单(存在时逐条核对 sha256;缺失只告警)
 *   ../packages.json 缺失 → 直接退出(不猜包清单)
 *
 * 构建期断言(任一不过即 exit 1,不落盘半成品):
 *   [1] 输入齐全 + 官方闭包每包在锁文件里存在
 *   [2] 每个选中的包在 wheels/ 都有对应文件(锁条目 file_name ↔ 实存文件双向核对)
 *   [3] sha256:官方包 ↔ lock.sha256;自造条目 ↔ packages.json.sha256;MANIFEST 存在则再核一遍
 *   [4] depends 闭包完整:每个包的 depends 都落在(官方闭包 ∪ 自造条目)内;缺口 → 失败
 *       (唯一例外是在 packages.json.depExclusions 里**显式登记**的依赖,当前 = pdfplumber → pypdfium2)
 *   [5] 锁文件剪裁:micropip 不得出现(结构性关闭 pip 通道)
 *   [6] asm.mjs 改写:① 长度守恒(改后 = 改前 + Σ替换差)② 每个替换点前后 48 字符仍在
 *       ③ `new Function(改写结果)` 可编译 ④ 改写后 0 残留 export / import.meta
 *       ⑤ `isClassicWorker` 特征串存在且唯一(钉死版本 + 构建时断言)
 *       ⑥ 第 3 处 import.meta.url 替换点前 200 字符内有 `if(Module["locateFile"])` 守卫
 *   [7] 结构:段正文无裸 `</script` / `<!--`、无 `;;;PYODIDE` 帧标记、全 ASCII(字符数 == 字节数)
 *       整份产物只有 1 个 `<script>` 且为 `type="text/plain"`,开合标签数一致
 *   [8] 体积账:逐段 + 汇总打印(核心 / wasm / stdlib / wheel 组),并与体积预算核对 ±5%
 *   [9] 幂等:产物不含时间戳/随机量(连跑两次字节相同,由外部 md5 核验)
 */
const fs = require('fs');
const path = require('path');
const zlib = require('zlib');
const crypto = require('crypto');

const ROOT = path.resolve(__dirname, '..');
const VENDOR = path.join(ROOT, 'vendor', 'pyodide-src');
const WHEELS = path.join(VENDOR, 'wheels');
const PYODIDE_VERSION = '314.0.6';
const FRAME = ';;;PYODIDE';
const GLOBAL_FACTORY = '__pyodideCreateModule';

/* ===========================================================================
 * 0. CLI(--profile / --out / --dry-run / --all)
 *    档位名单与落盘路径的权威 = scripts/pyodide-profiles.json(单一来源)
 * ========================================================================= */
const PROFILES_PATH = path.join(ROOT, 'scripts', 'pyodide-profiles.json');
const argv = process.argv.slice(2);
function cliVal(name) {
  const hit = argv.filter(a => a.indexOf('--' + name + '=') === 0);
  if (hit.length > 1) { console.error('参数 --' + name + ' 重复'); process.exit(1); }
  return hit.length ? hit[0].slice(name.length + 3) : null;
}
const cliUnknown = argv.filter(a => a !== '--dry-run' && a !== '--all'
  && a.indexOf('--profile=') !== 0 && a.indexOf('--out=') !== 0);
if (cliUnknown.length) {
  console.error('未知参数: ' + cliUnknown.join(' '));
  console.error('可用参数:--profile=<id> --out=<path> --dry-run --all');
  process.exit(1);
}
/* 空值是「给了但无效」,与「没给」不是一回事:`--profile=$VAR` 里 VAR 为空时不许静默出完整版
   (未知/无效档位直接 exit 1,不猜测、不回退 full) */
const rawProfile = cliVal('profile');
const rawOut = cliVal('out');
if (rawProfile === '') {
  console.error('参数 --profile 的值为空(--profile=)。用法:--profile=<id>(有效 id 见 '
    + 'scripts/pyodide-profiles.json;缺省 = full)');
  process.exit(1);
}
if (rawOut === '') {
  console.error('参数 --out 的值为空(--out=)。用法:--out=<path>(缺省 = 档位文件里的 partFile)');
  process.exit(1);
}
const cliProfile = rawProfile || 'full';
const cliOut = rawOut;
const cliDry = argv.indexOf('--dry-run') >= 0;
const cliAll = argv.indexOf('--all') >= 0;
if (cliAll && cliOut) {
  console.error('--all 不支持 --out(三档要写三条路径),请逐档生成');
  process.exit(1);
}

/* 档位文件解析:严格 JSON(无注释);结构不合法即 exit 1,不猜、不回退 */
function loadProfiles() {
  if (!fs.existsSync(PROFILES_PATH)) {
    console.error('缺档位文件 ' + path.relative(ROOT, PROFILES_PATH).replace(/\\/g, '/') + '(三档名单与落盘路径的权威)');
    process.exit(1);
  }
  let doc;
  try { doc = JSON.parse(fs.readFileSync(PROFILES_PATH, 'utf8')); }
  catch (e) { console.error('档位文件不是合法 JSON: ' + e.message); process.exit(1); }
  const bad = [];
  if (doc.schema !== 1) { bad.push('schema 必须为 1(实际 ' + JSON.stringify(doc.schema) + ')'); }
  if (doc.pyodideVersion !== PYODIDE_VERSION) {
    bad.push('pyodideVersion=' + JSON.stringify(doc.pyodideVersion) + ' 与本脚本常量 ' + PYODIDE_VERSION + ' 不一致');
  }
  if (!Array.isArray(doc.profiles) || !doc.profiles.length) { bad.push('profiles 必须是非空数组'); }
  if (!doc.cullGroups || typeof doc.cullGroups !== 'object') { bad.push('缺 cullGroups 注册表'); }
  if (!Array.isArray(doc.unavailable)) { bad.push('缺 unavailable 名单'); }
  (doc.profiles || []).forEach(p => {
    ['id', 'label', 'partFile', 'outFile'].forEach(k => { if (typeof p[k] !== 'string' || !p[k]) { bad.push('档位项缺 ' + k); } });
    if (typeof p.profileSection !== 'boolean') { bad.push('档位 ' + p.id + ' 缺 profileSection(bool)'); }
    if (!Array.isArray(p.topLevel) || !p.topLevel.length) { bad.push('档位 ' + p.id + ' 的 topLevel 必须是非空数组'); }
    if (!p.expect || typeof p.expect !== 'object') { bad.push('档位 ' + p.id + ' 缺 expect'); }
  });
  if (bad.length) { console.error('档位文件不合法:\n  - ' + bad.join('\n  - ')); process.exit(1); }
  return doc;
}

/* --all:依次 spawn 三个单档子进程(每个档位一次完整生成;不共享进程状态) */
if (cliAll) {
  const spawnSync = require('child_process').spawnSync;
  const doc = loadProfiles();
  console.log('--all:依次生成 ' + doc.profiles.map(p => p.id).join(' → ') + '(每档一个子进程)');
  let rc = 0;
  for (let i = 0; i < doc.profiles.length; i++) {
    const id = doc.profiles[i].id;
    console.log('');
    console.log('=== [' + (i + 1) + '/' + doc.profiles.length + '] --profile=' + id + ' ===');
    const args = [__filename, '--profile=' + id].concat(cliDry ? ['--dry-run'] : []);
    const r = spawnSync(process.execPath, args, { stdio: 'inherit' });
    if (r.error) { console.error('子进程启动失败: ' + r.error.message); rc = 1; break; }
    if (r.status !== 0) { console.error('档位 ' + id + ' 生成失败(exit ' + r.status + '),--all 中止(后续档位不跑)'); rc = r.status || 1; break; }
  }
  console.log('');
  console.log(rc ? '--all:未全部完成(exit ' + rc + ')' : '--all:三档全部生成完成');
  process.exit(rc);
}

const PROFILES = loadProfiles();
const PROFILE_IDS = PROFILES.profiles.map(p => p.id);
const PROFILE = PROFILES.profiles.filter(p => p.id === cliProfile)[0];
if (!PROFILE) {
  console.error('未知档位 --profile=' + cliProfile + '(有效 id:' + PROFILE_IDS.join(' / ') + ',来自 '
    + path.relative(ROOT, PROFILES_PATH).replace(/\\/g, '/') + ')');
  process.exit(1);
}

/* --out 按 ROOT(代码目录)解析(也接受绝对路径);默认 = 档位文件里的 partFile */
const OUT = cliOut ? path.resolve(ROOT, cliOut) : path.join(ROOT, PROFILE.partFile);
function dispOut() { return path.relative(ROOT, OUT).replace(/\\/g, '/') || OUT; }

const problems = [];
function fail(msg) { problems.push(msg); }
function readText(f) { return fs.readFileSync(f, 'utf8'); }
function sha256File(f) { return crypto.createHash('sha256').update(fs.readFileSync(f)).digest('hex'); }
function b64(buf) { return buf.toString('base64'); }
function fmt(n) { return n.toLocaleString('en-US'); }
function mb(n) { return (n / 1000000).toFixed(2) + ' MB'; }

/* ===========================================================================
 * 1. 输入
 * ========================================================================= */
console.log('pyodide ' + PYODIDE_VERSION + ' —— 由 vendor/pyodide-src/ 生成 ' + dispOut()
  + ' [profile ' + cliProfile + (cliDry ? ' · dry-run' : '') + ']');
console.log('');

const REQUIRE = ['pyodide-lock.json', 'packages.json', 'pyodide.asm.mjs', 'pyodide.js', 'pyodide.asm.wasm', 'python_stdlib.zip'];
REQUIRE.forEach(f => { if (!fs.existsSync(path.join(VENDOR, f))) { fail('缺输入文件 vendor/pyodide-src/' + f); } });
if (!fs.existsSync(WHEELS)) { fail('缺 wheel 目录 vendor/pyodide-src/wheels/'); }
if (problems.length) { console.error('自检失败:\n  - ' + problems.join('\n  - ')); process.exit(1); }

const lock = JSON.parse(readText(path.join(VENDOR, 'pyodide-lock.json')));
const pkgs = JSON.parse(readText(path.join(VENDOR, 'packages.json')));
const asmSrc = readText(path.join(VENDOR, 'pyodide.asm.mjs'));
const apiSrc = readText(path.join(VENDOR, 'pyodide.js'));
const wasmBuf = fs.readFileSync(path.join(VENDOR, 'pyodide.asm.wasm'));
const stdlibBuf = fs.readFileSync(path.join(VENDOR, 'python_stdlib.zip'));
const manifest = fs.existsSync(path.join(VENDOR, 'MANIFEST.json'))
  ? JSON.parse(readText(path.join(VENDOR, 'MANIFEST.json'))) : null;
if (!manifest) { console.log('  ! 未找到 MANIFEST.json(取件清单),跳过该来源的 sha256 交叉核对'); }

/* ===========================================================================
 * 1.5 档位筛选:目录 / 闭包展开 / 断言 C1–C5、C9(构建期强断言)
 *   目录 = packages.json 的 fromOfficialLock(115) + selfAuthored(36) = 151 条
 *   依赖表 = 官方锁条目的 depends(原样,不重写) + 自造条目的 depends
 * ========================================================================= */
function canon(s) { return String(s).replace(/[-_.]+/g, '-').toLowerCase(); }

const catalogDep = {};    /* canon 名 → 依赖(canon 化) */
const catalogFile = {};   /* canon 名 → wheel 文件名 */
const catalogOrdered = [];
pkgs.fromOfficialLock.forEach(name => {
  const k = canon(name);
  const e = lock.packages[name];
  if (!e) { fail('packages.json 列出的官方包在 pyodide-lock.json 里不存在: ' + name); return; }
  catalogDep[k] = (e.depends || []).map(canon);
  catalogFile[k] = e.file_name;
  catalogOrdered.push(k);
});
pkgs.selfAuthored.forEach(s => {
  const k = canon(s.canonical);
  if (catalogDep[k]) { fail('自造条目与官方条目重名: ' + s.canonical); return; }
  catalogDep[k] = (s.depends || []).map(canon);
  catalogFile[k] = s.file_name;
  catalogOrdered.push(k);
});
const catalog = new Set(catalogOrdered);
const depExempt = new Set((pkgs.depExclusions || []).map(e => canon(e.package) + '>' + canon(e.dep)));

/* C1:profile.topLevel 每个名字必须命中目录(大小写敏感、canonical 名) */
const unknownTops = PROFILE.topLevel.filter(n => !catalog.has(canon(n)));
if (unknownTops.length) {
  fail('C1 未知包名(不在目录里): ' + unknownTops.join(', ')
    + ' —— 目录 ' + catalog.size + ' 条见 vendor/pyodide-src/packages.json;'
    + '若上游换件/升级,请重审 scripts/pyodide-profiles.json 的 topLevel');
}
const topSet = new Set(PROFILE.topLevel.map(canon));
if (topSet.size !== PROFILE.topLevel.length) { fail('C1 profile.topLevel 有重复名'); }

/* 闭包展开(固定点迭代;depExclusions 仅在相关包被选中时"激活") */
function expand(tops) {
  const sel = new Set();
  const queue = tops.map(canon);
  const crossCatalog = new Set();
  while (queue.length) {
    const k = queue.shift();
    if (sel.has(k)) { continue; }
    if (!catalog.has(k)) { crossCatalog.add(k); continue; }
    sel.add(k);
    (catalogDep[k] || []).forEach(d => {
      if (depExempt.has(k + '>' + d)) { return; }
      if (!catalog.has(d)) { crossCatalog.add(d); return; }
      queue.push(d);
    });
  }
  return { sel: sel, crossCatalog: crossCatalog };
}
const expanded = expand(PROFILE.topLevel);
const selected = expanded.sel;

/* C2:闭包 ⊆ 目录;缺口里"非 depExclusions 登记的"就是真缺口 */
if (expanded.crossCatalog.size) {
  const real = [];
  expanded.crossCatalog.forEach(d => {
    const fromExempt = PROFILE.topLevel.some(t => depExempt.has(canon(t) + '>' + d));
    if (!fromExempt) { real.push(d); }
  });
  if (real.length) { fail('C2 闭包含目录外的包(防引入未经取件/审计的包): ' + real.join(', ')); }
}

/* C4:单调性 minimal ⊆ normal ⊆ full;full 的闭包必须 == 目录 */
const profileById = {};
PROFILES.profiles.forEach(p => { profileById[p.id] = p; });
if (profileById.full) {
  const fullSet = expand(profileById.full.topLevel).sel;
  const nSet = profileById.normal ? expand(profileById.normal.topLevel).sel : null;
  const mSet = profileById.minimal ? expand(profileById.minimal.topLevel).sel : null;
  if (nSet) {
    const notInFull = [...nSet].filter(x => !fullSet.has(x));
    if (notInFull.length) { fail('C4 单调性:normal 有包不在 full 里: ' + notInFull.join(', ')); }
  }
  if (mSet) {
    const mNotNormal = nSet ? [...mSet].filter(x => !nSet.has(x)) : [];
    if (mNotNormal.length) { fail('C4 单调性:minimal 有包不在 normal 里: ' + mNotNormal.join(', ')); }
    const mNotFull = [...mSet].filter(x => !fullSet.has(x));
    if (mNotFull.length) { fail('C4 单调性:minimal 有包不在 full 里: ' + mNotFull.join(', ')); }
  }
  if (fullSet.size !== catalog.size) {
    const miss = [...catalog].filter(x => !fullSet.has(x));
    fail('C4 full 的闭包(' + fullSet.size + ')≠ 目录(' + catalog.size + '),缺: ' + miss.join(', ')
      + ' —— 名单漂移会导致"完整版不完整"');
  }
}

/* C5:反 pip(任何档) + unavailable 名单与目录/选中集无交集 */
if (selected.has('micropip')) { fail('C5 剪裁锁文件里仍有 micropip(违反结构性关闭 pip)'); }
PROFILES.unavailable.forEach(n => {
  const k = canon(n);
  if (catalog.has(k)) { fail('C5 unavailable 名单里的 ' + n + ' 出现在目录里(名单与取件状态不一致)'); }
  if (selected.has(k)) { fail('C5 unavailable 名单里的 ' + n + ' 被选进本档'); }
});

/* C9:culled 各分组的并集 == (full 顶层 − 本档选中) 的顶层部分
      culledDeps(仅闭包项)自动推导,组固定 dep,带 via(触发它的被裁顶层名) */
const fullTopsCanon = profileById.full ? profileById.full.topLevel.map(canon) : [];
const culledTops = fullTopsCanon.filter(k => !selected.has(k));
const culledDepNames = [...catalog].filter(k => !selected.has(k) && fullTopsCanon.indexOf(k) < 0).sort();
const culledDoc = [];   /* {name, group, imports} —— imports 在 3.2 之后补齐 */
if (PROFILE.profileSection) {
  const groups = PROFILE.culled || {};
  const seen = {};
  Object.keys(groups).forEach(g => {
    if (!PROFILES.cullGroups[g]) { fail('C9 culled 用了未注册的组 ' + g + '(注册表 = cullGroups)'); }
    (groups[g] || []).forEach(n => {
      const k = canon(n);
      if (seen[k]) { fail('C9 被裁顶层 ' + n + ' 出现在多个分组(' + seen[k] + ' / ' + g + ')'); }
      seen[k] = g;
      culledDoc.push({ name: k, group: g, imports: [] });
    });
  });
  const listed = Object.keys(seen);
  const missing = culledTops.filter(k => !seen[k]);
  const extra = listed.filter(k => culledTops.indexOf(k) < 0);
  if (missing.length) { fail('C9 分组未覆盖被裁顶层: ' + missing.join(', ')); }
  if (extra.length) { fail('C9 分组列了不该裁(或被裁闭包项)的顶层名: ' + extra.join(', ')); }
  culledDoc.sort((a, b) => a.name < b.name ? -1 : 1);
} else if (PROFILE.culled) {
  fail('C9 档位 ' + PROFILE.id + ' 不应带 culled 分组(仅轻档需要)');
}
console.log('档位 ' + PROFILE.id + '(' + PROFILE.label + '):顶层 ' + PROFILE.topLevel.length
  + ' → 闭包 ' + selected.size + ' 条;目录 ' + catalog.size + ' 条;未使用 ' + (catalog.size - selected.size)
  + ' 条' + (PROFILE.profileSection ? ';被裁顶层 ' + culledTops.length + ' / 被裁闭包 ' + culledDepNames.length : ''));
console.log('');

/* ===========================================================================
 * 2. asm.mjs 机械化改写(classic 脚本化)
 * ========================================================================= */
const RE_URL = '"file:///"';   /* 3 处 import.meta.url 的替身:合法绝对 URL,不发起任何请求 */
const windows = [];

function sub(src, re, to, name, expect) {
  const rx = new RegExp(re.source, re.flags.indexOf('g') < 0 ? re.flags + 'g' : re.flags);
  let m, n = 0, out = '', last = 0;
  while ((m = rx.exec(src)) !== null) {
    n++;
    out += src.slice(last, m.index);
    const rep = typeof to === 'function' ? to(m) : to;
    windows.push({
      name: name,
      idx: m.index,
      before: src.slice(Math.max(0, m.index - 48), m.index),
      after: src.slice(m.index + m[0].length, m.index + m[0].length + 48),
      delta: rep.length - m[0].length
    });
    out += rep;
    last = m.index + m[0].length;
  }
  out += src.slice(last);
  if (n !== expect) { fail('asm.mjs 改写:' + name + ' 预期 ' + expect + ' 处,实际 ' + n + ' 处'); }
  return { out: out, n: n };
}

const s1 = sub(asmSrc, /import\.meta\.url/g, RE_URL, 'import.meta.url', 3);
const s2 = sub(s1.out, /\bexport\s+default\s+([A-Za-z_$][\w$]*)\s*;?/g,
  m => 'globalThis.' + GLOBAL_FACTORY + ' = ' + m[1] + ';',
  'export default', 1);
let asmOut = s2.out;

/* 断言 ①:长度守恒(改后 = 改前 + Σ替换差) */
const expectLen = asmSrc.length + windows.reduce((a, w) => a + w.delta, 0);
if (asmOut.length !== expectLen) { fail('asm.mjs 长度账不平:改后 ' + asmOut.length + ' ≠ 改前 ' + asmSrc.length + ' + 替换差 ' + windows.reduce((a, w) => a + w.delta, 0)); }
/* 断言 ④:残留检查(用 ESM 语法形态判定,不用裸 \bexport\b —— asm.mjs 的报错模板串里有 "bad export type") */
if (/import\.meta/.test(asmOut)) { fail('asm.mjs 改写后仍含 import.meta'); }
if (/\bexport\s+default\b|\bexport\s*\{|\bexport\s+(?:const|let|var|function|class|async)\b/.test(asmOut)) { fail('asm.mjs 改写后仍含 ESM export 语法'); }
if (asmOut.indexOf('globalThis.' + GLOBAL_FACTORY + ' = ') < 0) { fail('asm.mjs 改写后缺少全局工厂赋值'); }
/* 断言 ⑤:Pyodide 的 classic worker 硬检查仍在(升级版本时这条会先炸,提示重跑探针) */
const classicHits = (asmOut.match(/"isClassicWorker"/g) || []).length;
const classicThrow = (asmOut.match(/Classic web workers are not supported/g) || []).length;
if (classicHits !== 1) { fail('asm.mjs 的 "isClassicWorker" 特征串出现 ' + classicHits + ' 次(预期 1)'); }
if (classicThrow !== 1) { fail('asm.mjs 的 "Classic web workers are not supported" 出现 ' + classicThrow + ' 次(预期 1)'); }
const importScriptsHits = (asmOut.match(/globalThis\.importScripts/g) || []).length;
if (importScriptsHits !== 2) { fail('asm.mjs 的 globalThis.importScripts 出现 ' + importScriptsHits + ' 次(预期 2:isClassicWorker 调用 + 主线程判定)'); }
/* 断言 ⑥:第 3 处 import.meta.url(wasm URL 拼装)必须落在 locateFile 守卫之后 —— 否则新 URL 会抛错 */
const urlSites = windows.filter(w => w.name === 'import.meta.url').map(w => w.idx);
const wasmSite = Math.max.apply(null, urlSites);
const guard = asmSrc.slice(Math.max(0, wasmSite - 200), wasmSite);
if (guard.indexOf('Module["locateFile"]') < 0) { fail('第 3 处 import.meta.url 前 200 字符内没有 Module["locateFile"] 守卫(替换后该分支可能抛错)'); }
/* 断言 ③:改写结果可被 V8 编译 */
try {
  new Function(asmOut);
} catch (e) {
  fail('asm.mjs 改写结果 new Function 编译失败:' + e.message);
}
/* 断言 ②:每个替换点前后 48 字符仍在改写结果里 */
windows.forEach((w, i) => {
  if (w.before && asmOut.indexOf(w.before) < 0) { fail('替换点 ' + i + '(' + w.name + ')前文丢失'); }
  if (w.after && asmOut.indexOf(w.after) < 0) { fail('替换点 ' + i + '(' + w.name + ')后文丢失'); }
});
/* pyodide.js(官方 classic 构建产物)不改写,但也必须能编译 */
if (/import\.meta/.test(apiSrc) || /\bexport\s+default\b|\bexport\s*\{/.test(apiSrc)) { fail('pyodide.js 含 import.meta/ESM export,不再是 classic 构建产物'); }
try { new Function(apiSrc); } catch (e) { fail('pyodide.js new Function 编译失败:' + e.message); }

console.log('asm.mjs 改写: ' + fmt(asmSrc.length) + ' → ' + fmt(asmOut.length) + ' 字符(替换 ' + windows.length + ' 处,Σ差 ' +
  windows.reduce((a, w) => a + w.delta, 0) + ')');
console.log('  替换点: ' + windows.map(w => w.name + '@' + w.idx).join(', '));
console.log('  classic worker 特征串: "isClassicWorker" ×' + classicHits + ',"Classic web workers are not supported" ×' + classicThrow);
console.log('');

/* ===========================================================================
 * 3. 剪裁锁文件(含自造条目)
 * ========================================================================= */
/* 3.1 官方条目:只保留 packages.json 列出、且落在本档闭包里的规范名
       (full 档 = 全选,与入库产物逐字节一致) */
const min = { info: lock.info, packages: {} };
const officialMissing = [];
const importsOf = {};    /* canon 名 → 顶层导入名(官方取自锁条目 imports,自造取自 wheel) */
pkgs.fromOfficialLock.slice().sort().forEach(name => {
  const e = lock.packages[name];
  if (!e) { officialMissing.push(name); return; }
  importsOf[canon(name)] = (e.imports || []).slice().sort();
  if (!selected.has(canon(name))) { return; }
  min.packages[name] = e;
});
if (officialMissing.length) { fail('packages.json 列出的官方包在 pyodide-lock.json 里不存在: ' + officialMissing.join(', ')); }

/* 3.2 自造条目:纯 Python wheel(不在官方锁文件内) —— 格式照官方 package 条目 */
function wheelTopLevels(buf) {
  let eocd = -1;
  const floor = Math.max(0, buf.length - 22 - 65536);
  for (let i = buf.length - 22; i >= floor; i--) { if (buf.readUInt32LE(i) === 0x06054b50) { eocd = i; break; } }
  if (eocd < 0) { throw new Error('不是有效 ZIP(找不到 EOCD)'); }
  const count = buf.readUInt16LE(eocd + 10);
  const cdOff = buf.readUInt32LE(eocd + 16);
  if (count === 0xffff || cdOff === 0xffffffff) { throw new Error('ZIP64 暂不支持'); }
  const names = [];
  let p = cdOff;
  for (let n = 0; n < count; n++) {
    if (buf.readUInt32LE(p) !== 0x02014b50) { throw new Error('中央目录签名错误 @' + p); }
    const nameLen = buf.readUInt16LE(p + 28);
    const extLen = buf.readUInt16LE(p + 30);
    const cmtLen = buf.readUInt16LE(p + 32);
    names.push(buf.toString('utf8', p + 46, p + 46 + nameLen));
    p += 46 + nameLen + extLen + cmtLen;
  }
  return names;
}
/* 由 wheel 内文件清单推导入名(等价于 dist-info 的 top_level.txt),用于 loadPackagesFromImports */
function importsFromWheel(names) {
  const top = [];
  names.forEach(n => {
    if (/^[^/]+\/__init__\.py$/.test(n)) { top.push(n.split('/')[0]); }
    else if (/^[^/]+\.py$/.test(n) && n !== 'setup.py') { top.push(n.slice(0, -3)); }
  });
  return [...new Set(top)].sort();
}

const authored = [];
const authoredUnselectedMissing = [];   /* 未选中且文件缺失:轻档不因此失败(如实记录、仅提示) */
pkgs.selfAuthored.slice().sort((a, b) => a.canonical < b.canonical ? -1 : 1).forEach(s => {
  const k = canon(s.canonical);
  const inProfile = selected.has(k);
  const wf = path.join(WHEELS, s.file_name);
  if (!fs.existsSync(wf)) {
    if (inProfile) { fail('自造条目缺 wheel: ' + s.file_name + '(' + s.name + ')'); }
    else { authoredUnselectedMissing.push(s.canonical); }
    return;
  }
  const buf = fs.readFileSync(wf);
  const got = crypto.createHash('sha256').update(buf).digest('hex');
  if (got !== s.sha256) {
    if (inProfile) { fail('自造条目 sha256 不符: ' + s.file_name + ' wheels/' + got + ' vs packages.json/' + s.sha256); }
    else { console.log('  ! 未选中自造条目 sha256 不符(不影响本档,仅提示): ' + s.file_name); }
    return;
  }
  let tops = [], meta = false;
  try {
    const names = wheelTopLevels(buf);
    meta = names.some(n => /^[^/]+\.dist-info\/METADATA$/.test(n));
    tops = importsFromWheel(names);
  } catch (e) {
    if (inProfile) { fail('wheel 结构解析失败 ' + s.file_name + '(' + e.message + ')'); }
    return;
  }
  if (inProfile) {
    if (!meta) { fail('wheel 内缺 *.dist-info/METADATA: ' + s.file_name); }
    if (!tops.length) { fail('wheel 内找不到顶层可导入名(imports 为空): ' + s.file_name); }
  }
  importsOf[k] = tops;
  if (!inProfile) { return; }
  authored.push({
    name: s.name,
    canonical: s.canonical,
    version: s.version,
    file_name: s.file_name,
    package_type: 'package',
    install_dir: 'site',
    sha256: s.sha256,
    imports: tops,
    depends: s.depends
  });
});
authored.forEach(a => { min.packages[a.canonical] = { name: a.name, version: a.version, file_name: a.file_name, package_type: 'package', install_dir: 'site', sha256: a.sha256, imports: a.imports, depends: a.depends }; });

/* 3.3 剪裁断言 */
if (min.packages.micropip) { fail('剪裁锁文件里仍有 micropip(违反结构性关闭 pip)'); }
const selectedCount = Object.keys(min.packages).length;
if (PROFILE.id === 'full') {
  /* full 保留原等式断言(向后兼容守卫) */
  if (selectedCount !== pkgs.fromOfficialLock.length + pkgs.selfAuthored.length) {
    fail('剪裁锁条目数不符:' + selectedCount + ' ≠ 官方 ' + pkgs.fromOfficialLock.length + ' + 自造 ' + pkgs.selfAuthored.length);
  }
} else if (selectedCount !== selected.size) {
  fail('剪裁锁条目数不符:' + selectedCount + ' ≠ 本档闭包 ' + selected.size);
}
/* 断言 ④:depends 闭包完整(缺依赖 → 构建失败;唯一放行 = packages.json.depExclusions 里显式登记的) */
const declared = new Set(Object.keys(min.packages));
const exempt = new Set((pkgs.depExclusions || []).map(e => e.package + ' -> ' + e.dep));
const depGaps = [];
Object.keys(min.packages).forEach(k => {
  const e = min.packages[k];
  (e.depends || []).forEach(d => {
    const dc = String(d).replace(/[-_.]+/g, '-').toLowerCase();
    if (!declared.has(dc)) {
      if (exempt.has(e.name + ' -> ' + dc) || exempt.has(k + ' -> ' + dc)) { return; }
      depGaps.push(e.name + ' -> ' + d);
    }
  });
});
if (depGaps.length) { fail('depends 闭包不完整(缺依赖): ' + depGaps.join(', ')); }

/* 断言 [2]:本档选中的每个条目在 wheels/ 都有文件;wheels/ 里不得有目录外的文件;
   profile=full 时保留原等式断言(文件数 == 锁条目数) */
const wheelFiles = new Set(fs.readdirSync(WHEELS));
Object.keys(min.packages).forEach(k => {
  const e = min.packages[k];
  if (!wheelFiles.has(e.file_name)) { fail('缺 wheel 文件: ' + e.file_name + '(包 ' + k + ')'); }
});
const catalogFiles = new Set(catalogOrdered.map(k => catalogFile[k]));
const foreignFiles = [...wheelFiles].filter(f => !catalogFiles.has(f));
if (foreignFiles.length) { fail('wheels/ 里出现目录外的文件(不属于 packages.json 的 151 条): ' + foreignFiles.join(', ')); }
if (PROFILE.id === 'full' && wheelFiles.size !== Object.keys(min.packages).length) {
  fail('wheels/ 文件数(' + wheelFiles.size + ')≠ 锁条目数(' + Object.keys(min.packages).length + ')');
}
let shaChecked = 0;
Object.keys(min.packages).forEach(k => {
  const e = min.packages[k];
  const f = path.join(WHEELS, e.file_name);
  if (!fs.existsSync(f)) { return; }
  const got = sha256File(f);
  if (got !== e.sha256) { fail('sha256 不符: ' + e.file_name + ' 实存 ' + got + ' vs 清单 ' + e.sha256); }
  shaChecked++;
});
/* 断言 [3]:MANIFEST.json 存在时逐条交叉核对(只核本档选中文件;未选中文件不参与) */
let manifestChecked = 0, manifestSkipped = 0;
const selectedFiles = new Set(Object.keys(min.packages).map(k => min.packages[k].file_name));
if (manifest) {
  (manifest.wheels || []).forEach(w => {
    if (!selectedFiles.has(w.file)) { manifestSkipped++; return; }
    const f = path.join(WHEELS, w.file);
    if (!fs.existsSync(f)) { fail('MANIFEST 记录的文件不存在: ' + w.file); return; }
    if (fs.statSync(f).size !== w.bytes) { fail('MANIFEST 字节数不符: ' + w.file); }
    if (sha256File(f) !== w.sha256) { fail('MANIFEST sha256 不符: ' + w.file); }
    manifestChecked++;
  });
  (manifest.entries || []).forEach(w => {
    const f = path.join(VENDOR, w.file);
    if (!fs.existsSync(f)) { fail('MANIFEST 记录的核心集文件不存在: ' + w.file); return; }
    if (sha256File(f) !== w.sha256) { fail('MANIFEST sha256 不符: ' + w.file); }
  });
}

const lockJson = JSON.stringify(min);
const authuredImports = authored.filter(a => a.imports.length).length;
console.log('剪裁锁文件: ' + (PROFILE.id === 'full' ? ('官方 ' + pkgs.fromOfficialLock.length + ' + 自造 ' + pkgs.selfAuthored.length + ' = ')
  : ('本档闭包 = ')) + Object.keys(min.packages).length +
  ' 条;JSON ' + fmt(lockJson.length) + ' 字符(原 ' + fmt(JSON.stringify(lock).length) + ')');
console.log('  wheels/ 目录 ' + wheelFiles.size + ' 个文件:本档使用 ' + Object.keys(min.packages).length
  + ' / 未使用 ' + (wheelFiles.size - Object.keys(min.packages).length)
  + ';sha256 核对 ' + shaChecked + ' 件' + (manifest ? (' + MANIFEST ' + manifestChecked + ' 件(未选中跳过 ' + manifestSkipped + ')') : ''));
console.log('  自造条目 imports 实测非空 ' + authuredImports + '/' + authored.length
  + (authoredUnselectedMissing.length ? (';未选中且缺文件 ' + authoredUnselectedMissing.length + ' 条(不影响本档)') : ''));
console.log('  显式剔除的依赖(不写进 depends): ' + JSON.stringify((pkgs.depExclusions || []).map(e => e.package + ' -> ' + e.dep)));
console.log('');

/* ===========================================================================
 * 4. 组装分段
 * ========================================================================= */
const sections = [];   /* { id, kind, body } */
const account = [];

function escapeInline(code) {
  const a = (code.match(/<\/script/gi) || []).length;
  const b = (code.match(/<!--/g) || []).length;
  return { out: code.replace(/<\/script/gi, '<\\/script').replace(/<!--/g, '<\\!--'), nScript: a, nComment: b };
}
function addText(id, text, note) {
  const e = escapeInline(text);
  if (e.out.indexOf(FRAME) >= 0) { fail('段 ' + id + ' 正文含帧标记 ' + FRAME); }
  if (e.out.length !== text.length) { fail('段 ' + id + ' 转义改变了长度(文本段需逐字符对齐)'); }
  sections.push({ id: id, kind: 'text', body: e.out });
  account.push({ id: id, group: 'core', raw: Buffer.byteLength(text), gz: 0, out: e.out.length, note: note + '(转义 </script ×' + e.nScript + ' / <!-- ×' + e.nComment + ')' });
}
function addB64Gz(id, buf, group, note) {
  const gz = zlib.gzipSync(buf, { level: 6 });
  const body = b64(gz);
  if (body.indexOf(FRAME) >= 0 || body.indexOf('</script') >= 0 || body.indexOf('<!--') >= 0) { fail('段 ' + id + ' 的 base64 正文含非法子串'); }
  sections.push({ id: id, kind: 'b64gz', body: body });
  account.push({ id: id, group: group, raw: buf.length, gz: gz.length, out: body.length, note: note });
}

addText('pyodide.asm.mjs', asmOut, '改写后的 asm.mjs');
addText('pyodide.js', apiSrc, '官方 classic 构建产物');
addText('pyodide-lock.min.json', lockJson, '剪裁锁文件');
/* 轻档的档位元数据段(必须 ASCII;full 不加段 —— 保逐字节零回归)。
   段内容 = 运行期 pySectionDecode('pyodide-profile.json') 读到的唯一档位来源 */
let profileDoc = null;
if (PROFILE.profileSection) {
  const viaOf = {};
  culledDepNames.forEach(d => { viaOf[d] = []; });
  fullTopsCanon.forEach(t => {
    if (selected.has(t)) { return; }
    const own = expand([t]).sel;
    own.forEach(x => { if (viaOf[x] && viaOf[x].indexOf(t) < 0) { viaOf[x].push(t); } });
  });
  const topDoc = culledDoc.map(c => ({ name: c.name, group: c.group, imports: importsOf[c.name] || [] }));
  const depDoc = culledDepNames.map(d => ({ name: d, via: (viaOf[d] || []).slice().sort(), group: 'dep', imports: importsOf[d] || [] }));
  profileDoc = {
    profile: PROFILE.id,
    label: PROFILE.id,
    packages: selected.size,
    topLevel: PROFILE.topLevel.length,
    culledTops: topDoc,
    culledDeps: depDoc,
    unavailable: PROFILES.unavailable.slice()
  };
  addText('pyodide-profile.json', JSON.stringify(profileDoc, null, 1), '档位元数据(轻档专有)');
}
const profileSection = PROFILE.profileSection ? sections[sections.length - 1] : null;
addB64Gz('pyodide.asm.wasm', wasmBuf, 'core', 'wasm');
addB64Gz('python_stdlib.zip', stdlibBuf, 'core', 'Python 标准库');
const authoredSet = new Set(pkgs.selfAuthored.map(s => s.canonical));
const wheelSections = [];
Object.keys(min.packages).sort().forEach(k => {
  const e = min.packages[k];
  addB64Gz(e.file_name, fs.readFileSync(path.join(WHEELS, e.file_name)), authoredSet.has(k) ? 'author' : 'official', k + '@' + e.version);
  wheelSections.push(e.file_name);
});

const meta = {
  pyodide: PYODIDE_VERSION,
  python: lock.info.python,
  abi: lock.info.abi_version,
  factory: GLOBAL_FACTORY,
  esc: 'backslash',            /* 文本段还原: <\\/script → </script;<\\!-- → <!-- */
  sections: sections.map(s => ({ id: s.id, kind: s.kind, len: s.body.length })),
  packages: Object.keys(min.packages).length
};
const metaLine = JSON.stringify(meta);
if (metaLine.indexOf('\n') >= 0 || metaLine.indexOf(FRAME) >= 0) { fail('META 行不合法'); }

let part = '<!-- ===== 内联 pyodide ' + PYODIDE_VERSION + '(核心集 + 预置包闭包,由 scripts/make-pyodide-part.js 生成,请勿手改)===== -->\n'
  + '<script type="text/plain" id="pyodide-assets">\n'
  + FRAME + '-PART 1 ' + sections.length + '\n'
  + FRAME + '-META ' + metaLine + '\n';
sections.forEach(s => {
  part += FRAME + '-SECTION ' + s.id + ' ' + s.kind + ' ' + s.body.length + '\n' + s.body + '\n';
});
part += FRAME + '-END\n</script>\n';

/* ===========================================================================
 * 5. 产物结构断言
 * ========================================================================= */
if (!/^<script type="text\/plain" id="pyodide-assets">\n/.test(part.slice(part.indexOf('\n') + 1))) { fail('载荷块不是 type="text/plain" id="pyodide-assets"'); }
const scriptOpen = (part.match(/<script/g) || []).length;
const scriptClose = (part.match(/<\/script>/g) || []).length;
if (scriptOpen !== 1 || scriptClose !== 1) { fail('script 标签数异常:开 ' + scriptOpen + ' / 闭 ' + scriptClose + '(预期 1/1)'); }
if (part.indexOf('</script') !== part.lastIndexOf('</script')) { fail('产物里出现多处 </script'); }
const OPEN_AT = part.indexOf('<script type="text/plain" id="pyodide-assets">') + '<script type="text/plain" id="pyodide-assets">'.length + 1;
const blockBody = part.slice(OPEN_AT, part.lastIndexOf('</script>'));
if (/<\/script/i.test(blockBody)) { fail('载荷块内仍有裸 </script'); }
if (/<!--/.test(blockBody)) { fail('载荷块内仍有裸 <!--'); }
if (/[^\x00-\x7F]/.test(blockBody)) { fail('载荷块正文含非 ASCII 字符(字符数 ≠ 字节数,加载器按字符数切片会错)'); }
/* 帧往返还原:按**字符数**（不是按行）逐段切回,必须与原 body 完全一致 —— 文本段自带换行,故用游标解析 */
(function () {
  const head0 = part.slice(0, part.indexOf('\n'));
  if (head0.indexOf('<!--') !== 0) { fail('产物首行不是注释头'); }
  const p1 = part.indexOf('\n') + 1;
  const l1 = part.slice(p1, part.indexOf('\n', p1));
  if (l1 !== '<script type="text/plain" id="pyodide-assets">') { fail('第 2 行不是载荷开标签'); }
  let p = part.indexOf('\n', p1) + 1;
  const l2 = part.slice(p, part.indexOf('\n', p));
  if (l2 !== FRAME + '-PART 1 ' + sections.length) { fail('第 3 行不是 PART 头: ' + l2); }
  p = part.indexOf('\n', p) + 1;
  const l3 = part.slice(p, part.indexOf('\n', p));
  if (l3.indexOf(FRAME + '-META ') !== 0) { fail('第 4 行不是 META 行'); }
  if (l3.slice(FRAME.length + 6) !== metaLine) { fail('META 行与生成值不一致'); }
  p = part.indexOf('\n', p) + 1;
  for (let n = 0; n < sections.length; n++) {
    const hEnd = part.indexOf('\n', p);
    const h = part.slice(p, hEnd);
    const m = /^;;;PYODIDE-SECTION (\S+) (text|b64gz) (\d+)$/.exec(h);
    if (!m) { fail('段 ' + n + ' 头不合法: ' + h.slice(0, 80)); return; }
    const len = Number(m[3]);
    const body = part.substr(hEnd + 1, len);
    if (part.charAt(hEnd + 1 + len) !== '\n') { fail('段 ' + m[1] + ' 正文后不是换行'); return; }
    if (body.length !== len) { fail('段 ' + m[1] + ' 长度不符: 头声明 ' + len + ', 实际 ' + body.length); return; }
    if (m[1] !== sections[n].id || m[2] !== sections[n].kind || body !== sections[n].body) { fail('段 ' + n + '(' + m[1] + ') 往返不一致'); return; }
    p = hEnd + 1 + len + 1;
  }
  const tail = part.slice(p, part.indexOf('\n', p));
  if (tail !== FRAME + '-END') { fail('段尾不是 ' + FRAME + '-END: ' + tail); }
  p = part.indexOf('\n', p) + 1;
  if (part.slice(p, part.indexOf('\n', p)) !== '</script>') { fail('END 之后不是 </script>'); }
})();

/* C8:按档的段布局断言(防止轻档"假装完整版" / full 被塞 profile 段) */
const textSections = sections.filter(s => s.kind === 'text');
const textIds = textSections.map(s => s.id);
const wheelSectionCount = wheelSections.length;
if (PROFILE.id === 'full' && PROFILE.profileSection) {
  fail('C8 完整版不得带档位元数据段(profileSection 必须为 false)—— 否则"重建 == 入库产物"的逐字节承诺失效');
}
const expectTextSections = PROFILE.profileSection ? 4 : 3;
if (textSections.length !== expectTextSections) {
  fail('C8 文本段数不符:' + textSections.length + ' ≠ ' + expectTextSections
    + (PROFILE.profileSection ? '(轻档必须含 pyodide-profile.json 段)' : '(full 不得含 profile 段)'));
}
if (PROFILE.profileSection) {
  if (textIds[3] !== 'pyodide-profile.json') { fail('C8 第 4 个文本段必须是 pyodide-profile.json(实际 ' + textIds[3] + ')'); }
  if (sections.length !== 3 + 1 + 2 + wheelSectionCount) {
    fail('C8 段数不符:' + sections.length + ' ≠ 3 文本 + 1 profile + 2 核心 + ' + wheelSectionCount + ' wheel');
  }
  try {
    const doc = JSON.parse(profileSection.body);
    if (doc.profile !== PROFILE.id) { fail('C8 profile 段的 profile 字段=' + doc.profile + ' ≠ ' + PROFILE.id); }
    if (doc.packages !== selected.size) { fail('C8 profile 段的 packages 字段=' + doc.packages + ' ≠ ' + selected.size); }
  } catch (e) { fail('C8 profile 段不是合法 JSON: ' + e.message); }
} else {
  if (textIds.indexOf('pyodide-profile.json') >= 0) { fail('C8 full 档不得含 pyodide-profile.json 段(逐字节零回归失效)'); }
  if (sections.length !== 3 + 2 + wheelSectionCount) {
    fail('C8 段数不符:' + sections.length + ' ≠ 3 文本 + 2 核心 + ' + wheelSectionCount + ' wheel');
  }
}

/* ===========================================================================
 * 6. 体积账(与纸面估算核对 ±5%)
 * ========================================================================= */
const partBuf = Buffer.from(part, 'utf8');
const byGroup = {};
account.forEach(a => {
  const g = byGroup[a.group] || (byGroup[a.group] = { n: 0, raw: 0, gz: 0, out: 0 });
  g.n++; g.raw += a.raw; g.gz += a.gz; g.out += a.out;
});
function gsum(group, key) { const x = byGroup[group]; return x ? x[key] : 0; }
const GROUP_IDS = ['core', 'official', 'author'];
console.log('分段体积账:');
GROUP_IDS.forEach(g => {
  const x = byGroup[g];
  if (!x) { return; }
  console.log('  ' + g.padEnd(9) + ' ' + String(x.n).padStart(3) + ' 段  原始 ' + mb(x.raw).padStart(11) +
    '   gzip ' + mb(x.gz).padStart(11) + '   内嵌 ' + mb(x.out).padStart(11));
});
console.log('  ' + '合计'.padEnd(8) + ' ' + String(account.length).padStart(3) + ' 段  原始 '
  + mb(GROUP_IDS.reduce((a, g) => a + gsum(g, 'raw'), 0)).padStart(11)
  + '   gzip ' + mb(GROUP_IDS.reduce((a, g) => a + gsum(g, 'gz'), 0)).padStart(11)
  + '   内嵌 ' + mb(GROUP_IDS.reduce((a, g) => a + gsum(g, 'out'), 0)).padStart(11));
console.log('  ' + dispOut() + '  ' + fmt(partBuf.length) + ' B (' + mb(partBuf.length) + '),md5 ' + crypto.createHash('md5').update(partBuf).digest('hex'));

/* 纸面估算(口径 1 MB = 1,000,000 B):
 *   本产物 = 核心集内嵌(≈8.9 MB) + 预置包组本体(138.52 MB)× 1.334(仅 base64 膨胀) = ≈193.68 MB
 *   → 产物 = 非 pyodide 部分(≈9.88 MB) + 本产物 ≈ 203.6 MB(定稿值)
 * 这里只用本产物自身的分段账核对,不读已拼好的产物(否则重跑时产物已含载荷,基线会被自己吃掉)。
 * build.js 另有基于产物实测总体的核对。 */
/* C7 对账:expect 来自 scripts/pyodide-profiles.json(段数/文本段/wheel 段恒为强断言;
   字节字段为 null 时=首跑引导态,打印实测值 + 提示写回,不阻断;非 null 时 ±0 强断言) */
const embeddedBytes = gsum('official', 'out') + gsum('author', 'out');   /* 轮组内嵌(b64gz 段长之和,不含核心 2 段) */
const lockBytes = account.filter(a => a.id === 'pyodide-lock.min.json')[0].out;
const actual = {
  sections: sections.length,
  textSections: textSections.length,
  wheels: wheelSectionCount,
  embeddedBytes: embeddedBytes,
  lockBytes: lockBytes,
  partBytes: partBuf.length
};
const exp = PROFILE.expect;
const pendingWriteBack = [];
['sections', 'textSections', 'wheels', 'embeddedBytes', 'lockBytes', 'partBytes'].forEach(k => {
  const e = exp[k], a = actual[k];
  if (k === 'sections' || k === 'textSections' || k === 'wheels') {
    if (e !== a) { fail('C7 对账不符(强断言) ' + k + ': expect ' + e + ' ≠ 实测 ' + a); }
  } else if (e === null || e === undefined) {
    pendingWriteBack.push(k);
  } else if (e !== a) {
    fail('C7 对账不符(±0) ' + k + ': expect ' + fmt(e) + ' ≠ 实测 ' + fmt(a)
      + ' —— 换 Node/zlib 版本会触发;确认无误后把实测值写回 ' + path.relative(ROOT, PROFILES_PATH).replace(/\\/g, '/') + ' 的 expect');
  }
});
console.log('  C7 对账(expect = scripts/pyodide-profiles.json,档位 ' + PROFILE.id + '):');
console.log('    sections ' + actual.sections + ' / textSections ' + actual.textSections + ' / wheels ' + actual.wheels
  + ' / embeddedBytes ' + fmt(actual.embeddedBytes) + ' / lockBytes ' + fmt(actual.lockBytes) + ' / partBytes ' + fmt(actual.partBytes));
if (pendingWriteBack.length) {
  console.log('    [!] expect 的 ' + pendingWriteBack.join(' / ') + ' 为 null(首跑引导态,打印不阻断):'
    + '把上面的实测值写回该档 expect 后重跑,二跑起按 ±0 强断言');
} else {
  console.log('    全部六项与 expect 精确相等(±0)');
}

/* 纸面估算(口径 1 MB = 1,000,000 B):
 *   本产物 = 核心集内嵌(≈8.9 MB) + 预置包组本体(138.52 MB)× 1.334(仅 base64 膨胀) = ≈193.68 MB
 *   → 产物 = 非 pyodide 部分(≈9.88 MB) + 本产物 ≈ 203.6 MB(定稿值)
 * 这里只用本产物自身的分段账核对,不读已拼好的产物(否则重跑时产物已含载荷,基线会被自己吃掉)。
 * build.js 另有基于产物实测总体的核对。
 * 该估算只对 full 有定稿值;轻档的体积判据 = 上面的 C7 ±0 对账(实测写回),不做纸面 ±5%。 */
if (PROFILE.id === 'full') {
  const EST_CORE = 8.9e6;                 /* 核心集内嵌 ≈ 8.9 MB */
  const EST_WHEELS = 138.52e6 * 1.334;    /* 预置包组本体 138.52 MB × 1.334 */
  const EST_PART = EST_CORE + EST_WHEELS;
  const coreOut = gsum('core', 'out');
  const wheelsOut = gsum('official', 'out') + gsum('author', 'out');
  const devCore = (coreOut - EST_CORE) / EST_CORE * 100;
  const devWheels = (wheelsOut - EST_WHEELS) / EST_WHEELS * 100;
  const devPart = (partBuf.length - EST_PART) / EST_PART * 100;
  console.log('  体积核对(口径 1 MB = 1,000,000 B):');
  console.log('    核心集内嵌 ' + mb(coreOut) + ' vs 估算 ' + mb(EST_CORE) + ' → 偏差 ' + devCore.toFixed(2) + '%');
  console.log('    预置包组内嵌 ' + mb(wheelsOut) + ' vs 138.52 MB × 1.334 = ' + mb(EST_WHEELS) + ' → 偏差 ' + devWheels.toFixed(2) + '%');
  console.log('    本产物合计 ' + mb(partBuf.length) + ' vs 估算 ' + mb(EST_PART) + ' → 偏差 ' + devPart.toFixed(2) + '%');
  console.log('    (产物 ≈ 非 pyodide 部分 + 本产物;由 build.js 按实测总体再核一次)');
  if (Math.abs(devPart) > 5) {
    console.log('    [!] 本产物体积偏差超出 ±5% —— 记入交付文档,不阻断生成(纸面估算,非代码不变量)');
  }
} else {
  console.log('  纸面估算核对:定稿估算只对 full 适用;本档以 C7 的 ±0 对账为准(见上)');
}

/* ===========================================================================
 * 7. 落盘
 * ========================================================================= */
if (problems.length) {
  console.error('');
  console.error('自检失败(' + problems.length + ' 条):');
  problems.forEach(p => console.error('  - ' + p));
  process.exit(1);
}
if (cliDry) {
  console.log('');
  console.log('  --dry-run:不落盘(断言与体积账已跑完);目标路径 ' + OUT);
} else {
  fs.mkdirSync(path.dirname(OUT), { recursive: true });
  fs.writeFileSync(OUT, part, 'utf8');
  console.log('');
  console.log('  已写出 ' + path.relative(process.cwd(), OUT).replace(/\\/g, '/') + '  ' + fs.statSync(OUT).size + ' bytes');
}
console.log('  段数 ' + sections.length + '(文本 ' + textSections.length + ' + wasm/stdlib 2 + wheel ' + wheelSections.length + '),包数 '
  + Object.keys(min.packages).length + ',档位 ' + cliProfile
  + (PROFILE.profileSection ? '(含档位元数据段 ' + fmt(profileSection.body.length) + ' B)' : '(不含档位元数据段)'));
