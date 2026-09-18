/* 由 vendor/jupyterlite-src/ + vendor/pyodide-src/ 生成可内联的 src/jupyterlite.part —— 唯一入口:
 *   node scripts/make-jupyterlite-part.js [--dry-run] [--verify[=<part 路径>]] [--part=<路径>]
 *
 * 产物结构:与 src/pyodide.part **完全同构**(共用同一套解析器),一个
 * `<script type="text/plain" id="jupyterlite-assets">` 块,块内行帧切分为多个段:
 *
 *   ;;;JLITE-PART 1 <段数>
 *   ;;;JLITE-META <单行 JSON: 版本/载荷/索引/锁/断言读数>
 *   ;;;JLITE-SECTION <id> <text|b64gz> <字符数>
 *   <该段正文,恰好 N 个字符>
 *   ... 重复 ...
 *   ;;;JLITE-END
 *
 * 一次生成**三件交付物**(勘-七-5.1;后两件物化为 text 段,运行期走同一条资产读取路径):
 *   ① src/jupyterlite.part        站点段(段 id = 站点内相对路径)+ `pyodide/<file>` 例外段
 *   ② Jupyter 专用锁               text 段 `jupyterlite-lock.json`            (勘-七-1.3)
 *   ③ 合并后的 all.json            text 段 `<EXTPREFIX>/static/pypi/all.json` (覆盖站点自带)
 *
 * 关键归属裁定(D-1,依据 勘-七-2.1 ⑤ + 勘-七-1.2):
 *   **本载荷不含 core**(pyodide.asm.mjs / pyodide.asm.wasm / python_stdlib.zip / pyodide.js)
 *   —— 它们由产品的 `#pyodide-assets`(pyodide.part)在运行期复用;断言 16 强断言(负例 ⑤ 配套)。
 *
 * 输入(只读):vendor/jupyterlite-src/{site/**, MANIFEST.json, pyodide.mjs, comm-*.whl, micropip-*.whl,
 *   official-all.json} · vendor/pyodide-src/{pyodide-lock.json, wheels/**} · scripts/pyodide-profiles.json
 *
 * 自检断言 16 条(勘-七-5.6;逐条打印,任一不过 ⇒ exit 1 不落盘;断言 6 按 D1 只打印+对拍):
 *   1 输入齐全 + MANIFEST 逐条 sha256/字节匹配(强制;上游多出仅告警并登记)
 *  1f 专用锁 / 合并索引内每个包的 version == 其来源(pyodide-lock.json / packages.json / wheel 文件名;
 *     修正批新增 —— `comm` 版本号曾误为 `.2.3`,16 条断言无一覆盖 ⇒ 本断言把它锁死)
 *   1e 产品剪裁锁 == wheels/ 名册(**集合相等**,逐件报差集;修正批由"计数相等"升级)
 *   2 结构改造:实打者逐条命中且唯一(逐文件计数);**实际 applied 集合 == 已授权集合**(D1)+ 其余逐条登记
 *   3 raw site 内 CDN 回退串逐条登记(不得声称"已清除";权威判据在网络层;本断言恒 PASS、无咬合力)
 *   4 页面级 native `import(` 白名单;读数落 JLITE-META.l3(修正批修 P2-1 死赋值)
 *   5 段正文无裸 `</script`/`<!--`/帧标记前缀;text 段字符数 == 字节数
 *   6 体积账逐段 + 汇总 + assetsInlined 打印 + 与预算对拍(偏差 >5% 逐项说明,**不 exit 1**)
 *   7 幂等(不含时间戳/随机量;由外部连跑两次字节比对判)
 *   8a site 面:站点内 pyodide 副本 = 0(4 子断言 + 白名单 ≤5 条 ∈ pypi/**)
 *   8b 载荷面:载荷段显式清单进 JLITE-META(pyodide.mjs + 闭包 + micropip;**动态表述**,子批 B 首跑 = 17 件)
 *   9 子资源改写**三分类**(勘-七-5.3 步 2):① 已内联 ② `unresolvedNoForm`(有引用、无可改写形态) ③ 真零引用;
 *     ②/③ 显式登记进 JLITE-META 且**不 exit 1**(Leader 裁定 D3/P1-2);`refs>0 ∧ remain>0`(部分残余)仍 exit 1
 *  10 `--verify` 自校验(读回 part、按帧解析、逐段解码、重扫 `1f` / 3 / 5 / 8a / 9 / 11 / 14 / 15 / 16;**短路构造段,不重跑生成**。
 *     ⚠ 口径(修正批 3 · 复审 F4 点明):短路 ⇒ ① **不打印 16 条断言表**(只打印重扫读数);② `--neg=` 注入
 *     对它**不生效**(注入点全在 §4–§15 的生成段)⇒ 负例一律走 `--dry-run --neg=<id>`(生成期))
 *  11 LF 输出 + CR = 0 + 与 MANIFEST/生成脚本一致(闭包件 sha256 对照来源)
 *  12 站点种子内容:site/**`files/**` 存在即 exit 1;`jupyter-lite.ipynb` 白名单两件
 *  13a 122 术语族扫描(解码后站点资产 + pyodide.mjs);命中 = 0 或逐条理由;落 JLITE-META
 *  14 索引 ⊇ 闭包 ∧ 索引每条 url 有对应段 ∧ 段 sha256 对照来源
 *  15 专用锁 ⊇ 闭包 ∧ **每条 file_name ∈ 本载荷段 ∨ ∈ wheels/ 名册**(主路径) ∧ micropip 在锁内 ∧ sha256 对照来源
 *     (子批 B 复核:附加根 jedi/parso 同走本判据 —— 闭包件 ⇒ file_name ∈ 本载荷段,判据无需放宽)
 *  16 载荷面 dist 重复 = 0(core 段不得出现;闭包段 sha256 对照来源)
 *
 * 必跑负例(§5.1;`--neg=<id>` 真注入缺陷,生成期 FAIL;另 `--verify` 篡改段长一轮):
 *   section-len / drop-comm-index / drop-comm-lock / drop-micropip-lock / core-dup / no-pyodide-url
 *   bad-comm-version(修正批新增:注入错误 version ⇒ 断言 1f 必 FAIL)
 *   **drop-jedi-root / drop-jedi-packages**(子批 B 新增,注入**只改被检一侧**:前者只去闭包队列的附加根
 *   ⇒ 断言 15 FAIL;后者只把 P7 的 packages 支回 [] ⇒ 断言 2c FAIL)
 *   **drop-authorized-p8 / apply-deferred-p3**(修正批 3 新增 —— `勘十-1`(二)的 `N8`/`N9`,断言 2 的两条
 *   必跑负例:applied 缺一条已授权 / 已授权 deferred 被强行登记进 applied ⇒ 集合不等 ⇒ 断言 2 FAIL)
 *   断言 1 三条负例(缺件 / 字节不符 / sha256 不符)由外部装置用 `--jl-src=<副本>` 注入(见 neg 装置)
 */
'use strict';
const fs = require('fs');
const path = require('path');
const zlib = require('zlib');
const crypto = require('crypto');

const ROOT = path.resolve(__dirname, '..');
const FRAME = ';;;JLITE';
const ASSETS_ID = 'jupyterlite-assets';
const PSEUDO_ORIGIN = 'https://azusa-jupyter.invalid/';
const EXTPREFIX = 'extensions/@jupyterlite/pyodide-kernel-extension/';
const PIPY_SEG = EXTPREFIX + 'static/pypi/all.json';
const LOCK_SEG = 'jupyterlite-lock.json';
const LAB_VERSION = '4.6.3';
const JL_VERSION = '0.8.3';
const KERNEL_VERSION = '0.8.6';
const PYODIDE_VERSION = '314.0.6';

/* core 段 basename 集(断言 16;勘-七-2.1 ⑤ 逐字) —— 本载荷**不得**出现任一 */
const CORE_BASENAMES = ['pyodide.asm.mjs', 'pyodide.asm.wasm', 'python_stdlib.zip', 'pyodide.js'];
/* 122 术语族表(§5.5 逐字) */
const TERMS_122 = [
  'for await', '[Symbol.asyncIterator]', 'ReadableStream', 'URL.parse(', 'Promise.try(',
  'Promise.withResolvers(', 'Object.groupBy(', 'Map.groupBy(', 'RegExp.escape(', 'toSorted(',
  'toReversed(', 'Array.fromAsync(', 'Iterator.from(', 'Set.prototype.union', 'Float16Array',
  'Symbol.dispose', 'Uint8Array.fromBase64', 'import.meta', 'structuredClone('
];
/* 122 已实测可用族 ⇒ 命中即给理由,不算缺陷(§5.5 的"命中 = 0 或逐条给出理由")。
   新增命中族若未登记理由 ⇒ 断言 13a FAIL(保证"新面出现必须有人给理由",不是恒真仪式) */
const TERMS_122_KNOWN_OK = {
  'structuredClone(': 'Chromium 98 起可用(远早于 122);§5.5 逐字点名"122 可用,仅记录"',
  'ReadableStream': 'Chromium 43 起可用;此处为读流类型使用,不是异步迭代语法',
  'for await': '异步迭代语法,Chromium 63 起可用;本项目 122 翻车过的是 pdf.js 的运行时 "e is not async iterable"(E6),与语法可用性无关',
  '[Symbol.asyncIterator]': '异步迭代协议,Chromium 63 起可用;同上(与 pdf.js 的运行时缺陷不同层)'
};
/* CDN 回退串族(断言 3;勘-4:该串是第三方 bundle 的字节内容,删它 = 改上游代码) */
const CDN_STRINGS = ['https://cdn.jsdelivr.net/pyodide', 'https://cdn.jsdelivr.net/npm/pyodide', 'cdn.jsdelivr.net'];
/* 页面级 native import( 白名单(断言 4;`*` = 通配段)。**首跑现取回填**(与 pyodide-profiles.json 的
   expect 同一范式):白名单外出现命中 ⇒ exit 1(新增 native import 面 = 新的潜在出站面,必须显式登记)。
   现取(2026-09-16,本批首跑):build/7622.*.js=1 / build/lab/bundle.js=4 / 393.*.js=1 /
   coincident.worker.*.js=1 / comlink.worker.*.js=1 / lab/index.html=1 */
const L3_WHITELIST = [
  'build/7622.*.js', 'build/lab/bundle.js', 'lab/index.html',
  'extensions/@jupyterlite/pyodide-kernel-extension/static/393.*.js',
  'extensions/@jupyterlite/pyodide-kernel-extension/static/coincident.worker.*.js',
  'extensions/@jupyterlite/pyodide-kernel-extension/static/comlink.worker.*.js'
];

/* 子批 B(用户决策 ②):内核运行期预装件。上游 worker 的安装候选清单含 `jedi`,而本档
   载荷/索引/锁历来都不含它 ⇒ Phase 3 起由 appJ 运行期摘除、IPython 属性补全静默降为 0 条。
   本批把它入册(附加闭包根 ∪ 专用锁条目(官方全量锁原样) ∪ loadPyodideOptions.packages),
   走 loadPyodide 的锁+packageBaseUrl 取件路径(与 micropip 同路径,已现役验证)。
   ⚠ 两条已核实装载前提(③.5 独立实验;最易被后续批次无意破坏,勿改装载顺序):
   ① 顺序依赖:jedi 在 `loadPyodide` bootstrap 期装入,**早于** `import pyodide_kernel`
      (内核在 `initKernel` 里才 import)⇒ IPython 模块级 `import jedi` 成功 ⇒ `JEDI_INSTALLED=True`;
   ② 实验路径:`pyodide_kernel` 的 `_use_experimental_60_completion=True` ⇒ 走含 jedi 的实验补全;
      legacy `complete()` 会**主动跳过** jedi(skip_matchers)——①②缺一,补全判据全部读 0。 */
const KERNEL_EXTRA_PACKAGES = ['jedi', 'parso'];

/* ---- 与 §4.2 对拍的预算(勘-八-5 ④ 现取基线) ---- */
const BUDGET = {
  siteRaw: { v: 18776600, src: '勘-八-5 ④ 现取(469 件)' },
  /* 首跑实测回填(2026-09-17 修正批;来源 = 首跑 `out/r5-dryrun.log` 的分组体积账) */
  siteB64gz: { v: 11712248, src: '首跑实测 2026-09-17(站点段内嵌 b64gz)' },
  payloadRaw: { v: 4356963, src: '子批 B 首跑实测 2026-09-18(+jedi 1,563,101 + parso 106,894)' }
};

/* 扩展目录前缀(生成器内多处引用;提前定义,`--verify` 短路块亦需要) */
const extDir = EXTPREFIX + 'static/';

/* wheel 文件名 → version(PEP 427:{dist}-{version}(-{build})?-{py}-{abi}-{platform}.whl;
   dist/version 内的 `-` 已被归一为 `_` ⇒ 按 `-` 切分后 index 1 恒为 version)。
   ⚠ 修正批:旧实现 `file.replace(/-[0-9]/,'|§|')` 的 `/[0-9]/` 无 `g`,把 `-` 与首位数字
   一起吃掉(`comm-0.2.3-…` → `.2.3`);评审建议的 `/^(.+?)-([0-9][^-]*)\.whl$/` 经实测
   **匹配不到真实 wheel 名**(build 段后仍跟 `-py3-none-any`) ⇒ 采用按 `-` 切分的正确口径。 */
function versionFromWheelFile(file) {
  const base = String(file).replace(/\.whl$/i, '');
  const parts = base.split('-');
  return parts.length >= 2 ? parts[1] : '';
}

/* ===========================================================================
 * 0. CLI
 * ========================================================================= */
const argv = process.argv.slice(2);
function cliVals(name) { return argv.filter(a => a.indexOf('--' + name + '=') === 0).map(a => a.slice(name.length + 3)); }
const cliUnknown = argv.filter(a => !/^--(dry-run|verify|help)$/.test(a) && a.indexOf('--part=') !== 0
  && a.indexOf('--out=') !== 0 && a.indexOf('--neg=') !== 0 && a.indexOf('--jl-src=') !== 0);
if (cliUnknown.length) {
  console.error('未知参数: ' + cliUnknown.join(' '));
  console.error('可用参数:--dry-run --verify[=<part 路径>] --part=<路径> --out=<路径> --neg=<id> --jl-src=<目录> --help');
  process.exit(1);
}
const cliDry = argv.indexOf('--dry-run') >= 0;
const cliVerify = argv.filter(a => a === '--verify' || a.indexOf('--verify=') === 0).length > 0;
const cliVerifyVal = cliVals('verify')[0] || null;
const cliPart = cliVals('part')[0] || null;
const cliOut = cliVals('out')[0] || null;
const cliNeq = cliVals('neg');
const cliJlSrc = cliVals('jl-src')[0] || null;
if (argv.indexOf('--help') >= 0) {
  console.log(fs.readFileSync(__filename, 'utf8').split('\n').slice(0, 40).join('\n'));
  process.exit(0);
}
const VALID_NEG = ['section-len', 'drop-comm-index', 'drop-comm-lock', 'drop-micropip-lock', 'core-dup', 'no-pyodide-url', 'bad-comm-version',
  'drop-authorized-p8', 'apply-deferred-p3', 'drop-jedi-root', 'drop-jedi-packages'];
const badNeg = cliNeq.filter(n => VALID_NEG.indexOf(n) < 0);
if (badNeg.length) { console.error('未知 --neg 注入 id: ' + badNeg.join(' ') + '(可用:' + VALID_NEG.join(' / ') + ')'); process.exit(1); }

const VENDOR = path.join(ROOT, 'vendor');
const PY_SRC = path.join(VENDOR, 'pyodide-src');
const JL_SRC = cliJlSrc ? path.resolve(ROOT, cliJlSrc) : path.join(VENDOR, 'jupyterlite-src');
const SITE = path.join(JL_SRC, 'site');
const WHEELS = path.join(PY_SRC, 'wheels');
const DEFAULT_PART = path.join(ROOT, 'src', 'jupyterlite.part');
const PART_PATH = cliOut ? path.resolve(ROOT, cliOut) : (cliPart ? path.resolve(ROOT, cliPart) : DEFAULT_PART);
const VERIFY_PATH = cliVerifyVal ? path.resolve(cliVerifyVal) : PART_PATH;

/* ===========================================================================
 * 1. 通用工具
 * ========================================================================= */
const problems = [];
const ASSERT = [];
function fail(m) { problems.push(m); }
function note(m) { console.log('  ' + m); }
function check(n, name, ok, detail, soft) {
  ASSERT.push({ n: n, name: name, pass: !!ok, soft: !!soft, detail: detail || '' });
  if (!ok) {
    if (soft) { note('[!] 断言 ' + n + '(' + name + ')未过(软):' + (detail || '')); }
    else { fail('断言 ' + n + '(' + name + '):' + (detail || '未过')); }
  }
}
function fmt(n) { return n.toLocaleString('en-US'); }
function mb(n) { return (n / 1000000).toFixed(2) + ' MB'; }
function sha256(b) { return crypto.createHash('sha256').update(b).digest('hex'); }
function sha256File(f) { return sha256(fs.readFileSync(f)); }
function norm(s) { return String(s).toLowerCase().replace(/[._]+/g, '-'); }
function posix(p) { return p.split(path.sep).join('/'); }
const TEXT_EXT = /\.(js|mjs|css|json|html|txt|map|ipynb|svg|webmanifest|md|py|xml|ts|1)$/i;
const BIN_EXT = /\.(woff2|woff|ttf|eot|png|gif|jpg|jpeg|ico|svg)$/i;

function walk(dir, base, acc) {
  fs.readdirSync(dir, { withFileTypes: true }).forEach(e => {
    const abs = path.join(dir, e.name);
    const rel = base ? base + '/' + e.name : e.name;
    if (e.isDirectory()) { walk(abs, rel, acc); } else { acc.push({ rel: rel, abs: abs, bytes: fs.statSync(abs).size }); }
  });
  return acc;
}

/* 最小 ZIP 读取(读 wheel 内 METADATA;不依赖任何 npm 包) */
function zipEntries(buf) {
  let eocd = -1;
  const floor = Math.max(0, buf.length - 22 - 65536);
  for (let i = buf.length - 22; i >= floor; i--) { if (buf.readUInt32LE(i) === 0x06054b50) { eocd = i; break; } }
  if (eocd < 0) { throw new Error('不是有效 ZIP(找不到 EOCD)'); }
  const count = buf.readUInt16LE(eocd + 10), cdOff = buf.readUInt32LE(eocd + 16);
  if (count === 0xffff || cdOff === 0xffffffff) { throw new Error('ZIP64 暂不支持'); }
  const out = [];
  let p = cdOff;
  for (let n = 0; n < count; n++) {
    if (buf.readUInt32LE(p) !== 0x02014b50) { throw new Error('中央目录签名错误 @' + p); }
    const method = buf.readUInt16LE(p + 10), comp = buf.readUInt32LE(p + 20);
    const nl = buf.readUInt16LE(p + 28), el = buf.readUInt16LE(p + 30), cl = buf.readUInt16LE(p + 32);
    const off = buf.readUInt32LE(p + 42);
    out.push({ name: buf.toString('utf8', p + 46, p + 46 + nl), method: method, comp: comp, off: off });
    p += 46 + nl + el + cl;
  }
  return out;
}
function zipRead(buf, e) {
  const nl = buf.readUInt16LE(e.off + 26), el = buf.readUInt16LE(e.off + 28);
  const d = buf.slice(e.off + 30 + nl + el, e.off + 30 + nl + el + e.comp);
  return e.method === 0 ? d : zlib.inflateRawSync(d);
}
function wheelMetadata(buf) {
  const e = zipEntries(buf).filter(x => /\.dist-info\/METADATA$/.test(x.name))[0];
  if (!e) { throw new Error('wheel 内缺 *.dist-info/METADATA'); }
  return zipRead(buf, e).toString('utf8');
}

/* ===========================================================================
 * 2. 输入校验(断言 1)
 * ========================================================================= */
console.log('jupyterlite ' + JL_VERSION + ' —— 由 vendor/jupyterlite-src/ 生成 '
  + posix(path.relative(ROOT, PART_PATH)) + (cliDry ? ' · dry-run' : '') + (cliVerify ? ' · verify' : ''));
console.log('');

function mustExist(p, what) { if (!fs.existsSync(p)) { fail('缺输入:' + what + '(' + posix(path.relative(ROOT, p)) + ')'); return false; } return true; }
mustExist(path.join(PY_SRC, 'pyodide-lock.json'), 'vendor/pyodide-src/pyodide-lock.json');
mustExist(WHEELS, 'vendor/pyodide-src/wheels/');
mustExist(SITE, 'vendor/jupyterlite-src/site/');
mustExist(path.join(JL_SRC, 'MANIFEST.json'), 'vendor/jupyterlite-src/MANIFEST.json');
mustExist(path.join(JL_SRC, 'pyodide.mjs'), 'vendor/jupyterlite-src/pyodide.mjs');
mustExist(path.join(ROOT, 'scripts', 'pyodide-profiles.json'), 'scripts/pyodide-profiles.json');
mustExist(path.join(ROOT, 'scripts', 'jupyterlite-patches.json'), 'scripts/jupyterlite-patches.json');
if (problems.length) { console.error('自检失败:\n  - ' + problems.join('\n  - ')); process.exit(1); }

const manifest = JSON.parse(fs.readFileSync(path.join(JL_SRC, 'MANIFEST.json'), 'utf8'));
const prodLock = JSON.parse(fs.readFileSync(path.join(PY_SRC, 'pyodide-lock.json'), 'utf8'));
const pyWheelNames = fs.readdirSync(WHEELS);

/* MANIFEST 逐条强制(勘-七-3.4):缺件/字节不符/sha256 不符 ⇒ exit 1 */
{
  let okN = 0; const bad = [];
  manifest.entries.forEach(e => {
    const abs = path.join(JL_SRC, e.path);
    if (!fs.existsSync(abs)) { bad.push('缺件 ' + e.path); return; }
    const b = fs.readFileSync(abs);
    if (b.length !== e.bytes) { bad.push('字节不符 ' + e.path + '(' + b.length + ' vs ' + e.bytes + ')'); return; }
    if (sha256(b) !== e.sha256) { bad.push('sha256 不符 ' + e.path); return; }
    okN++;
  });
  /* 上游多出的文件(不在 entries 内)= 告警 + 登记,不 exit 1 */
  const declared = new Set(manifest.entries.filter(e => /^site\//.test(e.path)).map(e => e.path));
  const upstreamExtra = [];
  walk(SITE, '', []).forEach(f => { if (!declared.has('site/' + f.rel)) { upstreamExtra.push(f.rel); } });
  check(1, 'MANIFEST 逐条强制 + 上游多出登记', bad.length === 0,
    bad.length ? bad.slice(0, 5).join('; ') + (bad.length > 5 ? ' …共 ' + bad.length + ' 条' : '')
      : okN + '/' + manifest.entries.length + ' 条逐条匹配;上游多出 ' + upstreamExtra.length + ' 件',
    false);
  var upstreamExtraList = upstreamExtra;
}
note('MANIFEST 现取:' + manifest.entries.length + ' 条,' + fmt(manifest.totalBytes || 0) + ' B');

/* ===========================================================================
 * 3. MANIFEST 索引(sha256 对照来源的唯一权威之一)
 * ========================================================================= */
const manByPath = {};
manifest.entries.forEach(e => { manByPath[e.path] = e; });

/* ---- 闭包根的磁盘侧推导(§8.1 与 `--verify` 短路块共用;不依赖站点内存树) ---- */
function deriveRoots() {
  const pypiWheels = [];
  walk(SITE, '', []).forEach(f => {
    if (new RegExp('^' + reEsc(extDir) + 'pypi/.*\\.whl$').test(f.rel)) { pypiWheels.push(f.rel); }
  });
  pypiWheels.sort();
  const siteProvided = new Set(pypiWheels.map(rel => norm(wheelPkgName(path.posix.basename(rel)))));
  const rootsAll = new Set();
  const rootDecl = [];
  pypiWheels.forEach(rel => {
    const meta = wheelMetadata(fs.readFileSync(path.join(SITE, rel)));
    meta.split('\n').filter(l => /^Requires-Dist:/.test(l)).forEach(l => {
      const spec = l.slice('Requires-Dist:'.length).trim().split(';')[0].trim();
      const nm = spec.split(/[<>=!~\[\s]/)[0].trim();
      if (!nm) { return; }
      rootsAll.add(norm(nm));
      rootDecl.push({ wheel: rel, req: nm });
    });
  });
  const roots = new Set(Array.from(rootsAll).filter(n => !siteProvided.has(n)));
  /* 子批 B:`--verify` 用本函数重推闭包 ⇒ **必须**并入附加根,否则 jedi/parso 不在 verify 闭包里,
     而锁里在 ⇒ 断言 15 的"非载荷件必须 ∈ wheels/ 名册"误报 FAIL(方案 §四 S2-6 / 风险 R9)。 */
  const rootsExtra = KERNEL_EXTRA_PACKAGES.filter(n => !siteProvided.has(n));
  return {
    pypiWheels: pypiWheels, siteProvided: siteProvided, rootsAll: rootsAll,
    roots: new Set([...roots, ...rootsExtra]), rootDecl: rootDecl,
    rootsSiteProvided: Array.from(rootsAll).filter(n => siteProvided.has(n))
  };
}

/* `--verify` 短路(勘-七-5.5「**不重跑生成**」):直接读回盘上 part 校验,跳过 §4–§15 的构造/生成段。
   runVerify 以函数声明形式定义于文件末(声明提升 ⇒ 此处可先调用)。
   ⚠ 口径(修正批 3 · 复审 F4 点明;该口径此前只存于 `done.md` §10.5 末注):
   ① 短路后**不再打印 16 条断言表**(§15 的 `自检断言(N 条)…` 段根本不执行),只打印 runVerify 内的重扫读数;
   ② `--neg=<id>` 注入**对 `--verify` 不再生效** —— 全部注入点都在 §4–§15 的生成段里,短路后它们不执行
      ⇒ `--verify` 只读盘上 part 的既有字节;负例证明一律走 `--dry-run --neg=<id>`(生成期 FAIL)。 */
if (cliVerify) { runVerify(); }

/* ===========================================================================
 * 4. 载入站点树
 * ========================================================================= */
const site = new Map();      /* rel(POSIX) -> Buffer */
let siteDroppedMaps = 0;
walk(SITE, '', []).forEach(f => { site.set(f.rel, fs.readFileSync(f.abs)); });
const siteFilesRaw = site.size, siteBytesRaw = Array.from(site.values()).reduce((a, b) => a + b.length, 0);
note('站点载入:' + siteFilesRaw + ' 件 / ' + fmt(siteBytesRaw) + ' B');

/* ===========================================================================
 * 5. 结构改造(断言 2;勘-七-2.3 的 patches 表 —— 唯一权威表的机械序列化)
 *    本批「实打」= P1 / P2 / P7 / P8 + all_federated.json 形状断言;
 *    「登记 deferred」= P3 / P3b / P4 / P5 / P6 / asm(理由逐条写死,见 D-5/D-1)
 * ========================================================================= */
const PATCH_APPLIED = {};    /* counterKey -> 计数 */
function bump(k, n) { PATCH_APPLIED[k] = (PATCH_APPLIED[k] || 0) + n; }

const anchor393 = Array.from(site.keys()).filter(k => new RegExp('^' + extDir + '393\\.[0-9a-f]+\\.js$').test(k));
const anchorWorker = Array.from(site.keys()).filter(k => new RegExp('^' + extDir + '(comlink|coincident)\\.worker\\.[0-9a-f]+\\.js$').test(k));
const anchorRemote = Array.from(site.keys()).filter(k => new RegExp('^' + extDir + 'remoteEntry\\.[0-9a-f]+\\.js$').test(k));

let patchHits = 0, patchUniqueOk = true;
anchor393.forEach(rel => {
  let t = site.get(rel).toString('utf8');
  const nb = (t.match(/\{type:"module"\}/g) || []).length;
  bump('p1_type_module_before', nb);
  if (nb !== 1) { patchUniqueOk = false; fail('P1 锚点 {type:"module"} 在 ' + rel + ' 命中 ' + nb + ' 次(预期 1)'); }
  t = t.split('{type:"module"}').join('{}');
  bump('p1_patched', 1);
  const na = (t.match(/\{type:"module"\}/g) || []).length;
  bump('p1_type_module_after', na);
  if (na !== 0) { patchUniqueOk = false; fail('P1 改后仍有 {type:"module"} ×' + na); }
  site.set(rel, Buffer.from(t, 'utf8'));
  patchHits++;
});
anchorWorker.forEach(rel => {
  let t = site.get(rel).toString('utf8');
  const m = t.match(/export\{([^}]*)\};?\s*$/);
  const nb = (t.match(/export\{/g) || []).length;
  if (!m || nb !== 1) { patchUniqueOk = false; fail('P2 尾部 export{ 在 ' + rel + ' 未唯一命中(' + nb + ')'); return; }
  const assigns = m[1].split(',').map(pair => {
    const mm = pair.trim().split(/\s+as\s+/);
    return mm.length === 2 ? ('self.' + mm[1] + '=' + mm[0] + ';') : ('self.' + mm[0] + '=' + mm[0] + ';');
  }).join('');
  t = t.slice(0, m.index) + assigns;
  bump('p2_tail_export_stripped_' + path.basename(rel), 1);
  site.set(rel, Buffer.from(t, 'utf8'));
  patchHits++;
});
anchorRemote.forEach(rel => {
  let t = site.get(rel).toString('utf8');
  const anchor = ',$.consumesLoadingData={';
  const hits = t.split(anchor).length - 1;
  bump('p8_anchor_hits', hits);
  const am = t.match(/\$\.p=([A-Za-z_$][\w$]*)=/);
  if (am) { bump('p8_autopath_var', 0); PATCH_APPLIED.p8_autopath_var = am[1]; }
  if (hits !== 1) { patchUniqueOk = false; fail('P8 锚点 ' + anchor + ' 在 ' + rel + ' 命中 ' + hits + ' 次(预期 1)'); return; }
  const pinned = PSEUDO_ORIGIN + extDir;
  t = t.replace(anchor, ',$.p="' + pinned + '";' + anchor.slice(1));
  bump('p8_pinned', 1);
  PATCH_APPLIED.p8_pinned_to = pinned;
  const na = t.split(anchor).length - 1;
  if (na !== 0) { patchUniqueOk = false; fail('P8 改后锚点仍命中 ' + na + ' 次'); }
  site.set(rel, Buffer.from(t, 'utf8'));
  patchHits++;
});

/* P7 —— 配置键集合(勘-七-3.2 的唯一权威键表;嵌套写法)。
   勘九-4:断言 2 的 P7 项 = **注入键集合比较**(基数 9);`p7_config_keys` 计数键**删除**
   (计数漂移的 exit 1 语义作废);读数改 `JLITE-META.p7ConfigKeys = 9`(仅留证,不作判据)。 */
const P7_KERNEL_KEYS = ['pyodideUrl', 'pipliteWheelUrl', 'pipliteUrls', 'disablePyPIFallback', 'loadPyodideOptions'];
const P7_TOP_KEYS = ['exposeAppInBrowser', 'contentsStorageName', 'licensesUrl', 'defaultKernelName'];
const P7_LOADPYODIDE_KEYS = ['indexURL', 'packageBaseUrl', 'packages'];
const P7_INJECTED = P7_KERNEL_KEYS.concat(P7_TOP_KEYS);
let p7Keys = 0, p7RootKeys = 0;
const P7_PIPLITE_WHEEL = PSEUDO_ORIGIN + PIPY_SEG.replace('/all.json', '/piplite-0.8.6-py3-none-any.whl');
{
  const rel = 'jupyter-lite.json';
  const doc = JSON.parse(site.get(rel).toString('utf8'));
  const cfg = doc['jupyter-config-data'] || (doc['jupyter-config-data'] = {});
  cfg.exposeAppInBrowser = true;
  cfg.contentsStorageName = 'AzusaAI-JupyterLab';
  cfg.defaultKernelName = cfg.defaultKernelName || 'python';
  cfg.litePluginSettings = cfg.litePluginSettings || {};
  cfg.litePluginSettings['@jupyterlite/pyodide-kernel-extension:kernel'] = {
    pyodideUrl: PSEUDO_ORIGIN + 'pyodide/pyodide.mjs',
    pipliteWheelUrl: P7_PIPLITE_WHEEL,
    pipliteUrls: [PSEUDO_ORIGIN + PIPY_SEG],
    disablePyPIFallback: true,
    loadPyodideOptions: {
      indexURL: PSEUDO_ORIGIN + 'pyodide/',
      packageBaseUrl: PSEUDO_ORIGIN + 'pyodide/',
      packages: KERNEL_EXTRA_PACKAGES
    }
  };
  if (cliNeq.indexOf('no-pyodide-url') >= 0) {
    delete cfg.litePluginSettings['@jupyterlite/pyodide-kernel-extension:kernel'].pyodideUrl;
  }
  /* 负例 `drop-jedi-packages`:只把 P7 注入值支回 [] —— 闭包根/锁块/KERNEL_EXTRA_PACKAGES 逐字不动
     (只改被检一侧;注入形态沿现法 `no-pyodide-url` 的就地分支;方案 §四 S2-7)。 */
  if (cliNeq.indexOf('drop-jedi-packages') >= 0) {
    cfg.litePluginSettings['@jupyterlite/pyodide-kernel-extension:kernel'].loadPyodideOptions.packages = [];
  }
  site.set(rel, Buffer.from(JSON.stringify(doc, null, 2) + '\n', 'utf8'));
  p7RootKeys = Object.keys(cfg).length;
  const k = cfg.litePluginSettings['@jupyterlite/pyodide-kernel-extension:kernel'];
  p7Keys = P7_KERNEL_KEYS.filter(x => k[x] !== undefined).length + P7_TOP_KEYS.filter(x => cfg[x] !== undefined).length;
  bump('p7_root_config_keys', p7RootKeys);
  patchHits++;
  /* 断言 2 的 P7 项:**集合比较**(逐键命中 ∧ 值匹配 ∧ **无多余键**;勘九-4)。
     旧口径只断 2 个值 ⇒ 漏掉 3 个 URL 值 / `packages: []` / "少一个键、多一个别的键"的形状。 */
  const p7Missing = P7_KERNEL_KEYS.filter(x => k[x] === undefined).map(x => 'litePluginSettings["…:kernel"].' + x)
    .concat(P7_TOP_KEYS.filter(x => cfg[x] === undefined));
  const p7Extra = Object.keys(k).filter(x => P7_KERNEL_KEYS.indexOf(x) < 0);
  const p7LpoExtra = Object.keys(k.loadPyodideOptions || {}).filter(x => P7_LOADPYODIDE_KEYS.indexOf(x) < 0);
  const lpo = k.loadPyodideOptions || {};
  const p7ValBad = [];
  if (k.pyodideUrl !== PSEUDO_ORIGIN + 'pyodide/pyodide.mjs') { p7ValBad.push('pyodideUrl'); }
  if (k.pipliteWheelUrl !== P7_PIPLITE_WHEEL) { p7ValBad.push('pipliteWheelUrl'); }
  if (!(Array.isArray(k.pipliteUrls) && k.pipliteUrls.length === 1 && k.pipliteUrls[0] === PSEUDO_ORIGIN + PIPY_SEG)) { p7ValBad.push('pipliteUrls'); }
  if (k.disablePyPIFallback !== true) { p7ValBad.push('disablePyPIFallback'); }
  if (lpo.indexURL !== PSEUDO_ORIGIN + 'pyodide/') { p7ValBad.push('loadPyodideOptions.indexURL'); }
  if (lpo.packageBaseUrl !== lpo.indexURL) { p7ValBad.push('packageBaseUrl==indexURL'); }
  /* [E13] 判据改动(子批 B):`packages == []` ⇒ **与 KERNEL_EXTRA_PACKAGES 逐元素相等**;
     咬合力 = 负例 `drop-jedi-packages`(只把注入值支回 [] ⇒ 本项 FAIL)。 */
  if (!(Array.isArray(lpo.packages) && lpo.packages.length === KERNEL_EXTRA_PACKAGES.length
    && lpo.packages.every((x, i) => x === KERNEL_EXTRA_PACKAGES[i]))) {
    p7ValBad.push('loadPyodideOptions.packages==KERNEL_EXTRA_PACKAGES');
  }
  if (cfg.exposeAppInBrowser !== true) { p7ValBad.push('exposeAppInBrowser'); }
  if (cfg.contentsStorageName !== 'AzusaAI-JupyterLab') { p7ValBad.push('contentsStorageName'); }
  if (cfg.defaultKernelName !== 'python') { p7ValBad.push('defaultKernelName'); }
  if (!(typeof cfg.licensesUrl === 'string' && cfg.licensesUrl.length > 0)) { p7ValBad.push('licensesUrl'); }
  check('2c', 'P7 注入键集合比较(勘九-4:9 键逐键 ∧ 值匹配 ∧ 无多余键)',
    p7Keys === 9 && p7Missing.length === 0 && p7Extra.length === 0 && p7LpoExtra.length === 0 && p7ValBad.length === 0,
    (p7Missing.length ? '缺 ' + p7Missing.join(', ') + ';' : '')
    + (p7Extra.length ? '多余(内核) ' + p7Extra.join(', ') + ';' : '')
    + (p7LpoExtra.length ? '多余(loadPyodideOptions) ' + p7LpoExtra.join(', ') + ';' : '')
    + (p7ValBad.length ? '值不符 ' + p7ValBad.join(', ')
      : '9 键全在(集合基数 9);4 条 URL/packages 值断言 + 2 条值断言全过;无多余键'), false);
}

/* all_federated.json —— 上游 build 已生成 [];只做形状断言,不改写(勘-八-5 ②) */
{
  const rel = 'build/schemas/all_federated.json';
  const ok = site.has(rel);
  let isArr = false, val = null;
  if (ok) { try { val = JSON.parse(site.get(rel).toString('utf8')); isArr = Array.isArray(val); } catch (e) { isArr = false; } }
  check(2 + 0.1, 'all_federated.json 形状断言(存在 ∧ Array ∧ == [])', ok && isArr && val.length === 0,
    ok ? JSON.stringify(val) : '文件缺失', false);
}

/* 改造点表(唯一权威 = `scripts/jupyterlite-patches.json`,schema 1;勘-七-2.3 —— 机械序列化,禁止内联副本) */
const PATCH_DOC = JSON.parse(fs.readFileSync(path.join(ROOT, 'scripts', 'jupyterlite-patches.json'), 'utf8'));
if (PATCH_DOC.schema !== 1) { fail('scripts/jupyterlite-patches.json 的 schema ≠ 1(勘-七-2.3)'); }
/* ---- 断言 2 的两条必跑负例(修正批 3;`勘十-1`(二)的 `N8`/`N9`,本实现 `--neg` id 见下)----
   为何必须有:断言 2 的判据本批由「计数」改成「**集合相等**」⇒ 按 `[E13]` ③「判据改动 ⇒ 负例证明」,
   必须有注入式 FAIL 读数。复审 `F1`(P1 必修)实测:原装置的注入**无一打向断言 2**
   (`fix-done.md` §3.3 当时只有正例 + 文字论证)⇒ 两条负例是本条的补件。
   注入法 = 在 `patches.json` 的**内存副本**上注入(盘上文件与授权集 `AUTHORIZED_*` **都不动** ——
   授权集是**独立参照**,正是本判据咬合力的来源):
     `drop-authorized-p8` ⇒ 删掉 `P8` 行(模拟"漏做已授权项")⇒ `applied` 少一条;
     `apply-deferred-p3` ⇒ 把已授权 deferred 的 `P3` 行 `status` 改 `applied`(模拟"占位冒充实打")
                            ⇒ `applied` 多一条 + `deferred` 少一条。
   两条都让实际集合 ≠ 授权集合 ⇒ 断言 2 FAIL / exit 1。 */
const PATCH_TABLE = (function () {
  if (cliNeq.indexOf('drop-authorized-p8') >= 0) {
    return PATCH_DOC.patches.filter(p => p.id !== 'P8');
  }
  if (cliNeq.indexOf('apply-deferred-p3') >= 0) {
    return PATCH_DOC.patches.map(p => p.id === 'P3' ? Object.assign({}, p, { status: 'applied' }) : p);
  }
  return PATCH_DOC.patches;
})();
/* D1 授权集(Leader 裁定 2026-09-16;`shared/decisions/jupyterlite-phase1-scope-and-fixes-2026-09-16.md`):
   Phase 1 **只**要求「P1 / P2 / P7 / P8 + 资产 / 索引」实打;P3 / P5 / P6 的延后**获授权**
   (与 P3b / P4 / asm 同列)。断言 2 因此改为「**实际 applied 集合 == 已授权集合**」——
   deferred 行不再由"被检对象自己声明"说了算(评审 P1-3:旧判据对 deferred 行零独立咬合力)。 */
/* 子批 B(用户决策 ②,2026-09-17):P9 入 applied、P3c 入 deferred —— 与 scripts/jupyterlite-patches.json
   同批登记(P2-5:`patches.json` 不可单方面补登,集合不等 ⇒ 断言 2 exit 1)。 */
const AUTHORIZED_APPLIED = ['P1', 'P2', 'P7', 'P8', '资产', '索引', 'P9'];
const AUTHORIZED_DEFERRED = ['P3', 'P3b', 'P4', 'P5', 'P6', 'asm', 'P3c'];
{
  const ids = PATCH_TABLE.map(p => p.id);
  const applied = PATCH_TABLE.filter(p => p.status === 'applied').map(p => p.id);
  const deferred = PATCH_TABLE.filter(p => p.status === 'deferred');
  const deferredIds = deferred.map(d => d.id);
  const setEq = (a, b) => a.length === b.length && a.every(x => b.indexOf(x) >= 0);
  const only = (a, b) => a.filter(x => b.indexOf(x) < 0);
  const appliedSetOk = setEq(applied, AUTHORIZED_APPLIED);
  const deferredSetOk = setEq(deferredIds, AUTHORIZED_DEFERRED);
  check('2', '结构改造:实打者逐条命中且唯一 + 实际 applied 集合 == 已授权集合(D1)',
    patchUniqueOk && appliedSetOk && deferredSetOk && deferred.every(d => d.owner && d.reason),
    '实打 ' + applied.join('/') + '(' + patchHits + ' 处);授权 ' + AUTHORIZED_APPLIED.join('/')
    + (appliedSetOk ? ' ✓(集合相等)' : ' ✗ 差集 = 实际多 [' + only(applied, AUTHORIZED_APPLIED).join(',') + '] / 实际少 [' + only(AUTHORIZED_APPLIED, applied).join(',') + ']')
    + ';登记 deferred ' + deferred.length + ' 条:' + deferred.map(d => d.id + '→' + d.owner).join(', ')
    + (deferredSetOk ? '' : ' ✗ 与授权 deferred 集不符:多 [' + only(deferredIds, AUTHORIZED_DEFERRED).join(',') + '] 少 [' + only(AUTHORIZED_DEFERRED, deferredIds).join(',') + ']'),
    false);
  console.log('  改造点表(' + ids.length + ' 行;实打 ' + applied.length + ' / deferred ' + deferred.length + '):');
  PATCH_TABLE.forEach(p => {
    console.log('    ' + String(p.id).padEnd(5) + p.status.padEnd(9) + (p.owner || '-').padEnd(24)
      + (p.counterKey ? (p.counterKey + '=' + JSON.stringify(PATCH_APPLIED[p.counterKey] !== undefined ? PATCH_APPLIED[p.counterKey] : '(见展开键)')) : ''));
  });
}
note('改造点展开计数:' + JSON.stringify(PATCH_APPLIED));

/* ===========================================================================
 * 6. 断言 3(CDN 回退串登记 —— 在 raw site 上扫)
 * ========================================================================= */
const cdnFallbacks = [];
{
  const raw = new Map();
  walk(SITE, '', []).forEach(f => { raw.set(f.rel, fs.readFileSync(f.abs)); });
  raw.forEach((buf, rel) => {
    if (!TEXT_EXT.test(rel)) { return; }
    const t = buf.toString('utf8');
    CDN_STRINGS.forEach(s => {
      const n = t.split(s).length - 1;
      if (n) { cdnFallbacks.push({ file: rel, needle: s, hits: n }); }
    });
  });
  const covered = cdnFallbacks.filter(x => x.needle === 'cdn.jsdelivr.net').length;
  /* ⚠ 断言 3 **恒 PASS、无咬合力、不得当回归门**(P3-1):判据字面量 `true`,只做逐条登记。
     规格本意如此(勘-4:权威判据在网络层 = Phase 2 的 A9;本断言只要求"逐条登记、不得声称已清除")。 */
  check(3, 'raw site 内 CDN 回退串逐条登记(不断言"已清除";恒 PASS·无咬合力·非回归门)', true,
    '命中 ' + cdnFallbacks.length + ' 条记录 / ' + covered + ' 个文件(覆盖 ' + CDN_STRINGS.length + ' 个特征串)', true);
  console.log('  CDN 回退串登记(' + cdnFallbacks.length + ' 条;权威判据在网络层 = Phase 2 的 A9):');
  cdnFallbacks.slice(0, 10).forEach(x => console.log('    ' + x.file + '  "' + x.needle + '" ×' + x.hits));
  if (cdnFallbacks.length > 10) { console.log('    …共 ' + cdnFallbacks.length + ' 条(全量进 JLITE-META)'); }
}

/* ===========================================================================
 * 7. 子资源 data: 改写(勘-6 五形态)+ 删原文件(断言 9)
 * ========================================================================= */
const MIME = { woff2: 'font/woff2', woff: 'font/woff', ttf: 'font/ttf', eot: 'application/vnd.ms-fontobject', png: 'image/png', gif: 'image/gif', jpg: 'image/jpeg', jpeg: 'image/jpeg', ico: 'image/x-icon', svg: 'image/svg+xml' };
const escaped = {};
function reEsc(s) { return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'); }
const binAssets = Array.from(site.keys()).filter(k => BIN_EXT.test(k)).sort();
const inlinedDetail = [];
let assetsInlinedBytes = 0, inlinedCount = 0;
const unresolved = [];        /* refs>0 ∧ remain>0:已内联但别处仍有引用残留 ⇒ 真缺陷,exit 1 */
const unresolvedNoForm = [];  /* refs==0 ∧ remain>0:有引用但无可改写形态 ⇒ 登记,不 exit 1(Leader 裁定 D3/P1-2) */
binAssets.forEach(rel => {
  const buf = site.get(rel);
  const ext = rel.split('.').pop().toLowerCase();
  const dataUrl = 'data:' + (MIME[ext] || 'application/octet-stream') + ';base64,' + buf.toString('base64');
  const nameRe = reEsc(path.posix.basename(rel));
  let hits = 0; const forms = [];
  Array.from(site.keys()).forEach(rel2 => {
    if (!TEXT_EXT.test(rel2) || rel2 === rel) { return; }
    const before = site.get(rel2).toString('utf8');
    if (before.indexOf(path.posix.basename(rel)) < 0) { return; }
    let t = before;
    /* 形态①(rspack asset/resource 工厂):`<表达式>+"<文件名>"` ⇒ **整段**替换(含前缀表达式)。
       表达式字符类必须**单层**(`[\w$.]*`)—— 写成 `[\w$]*(?:\.[\w$]+)*` 是嵌套量词,
       在"有 `+` 但后面不是该文件名"的位置上会灾难性回溯(实测首跑数分钟)。 */
    t = t.replace(new RegExp('([A-Za-z_$][\\w$.]*)\\+"(' + nameRe + ')"', 'g'), '"$2"');
    /* 形态②~④:url(...) 相对路径 / `~包名/路径` / `%%URL%%` 前缀,可带引号与 ?# 后缀 */
    t = t.replace(new RegExp('url\\((?:[\'"])?(?:[^()\'"]*\\/)?(' + nameRe + ')(?:[?#][^()\'"]*)?(?:[\'"])?\\)', 'g'), 'url(' + dataUrl + ')');
    t = t.replace(new RegExp('%%URL%%/(' + nameRe + ')', 'g'), dataUrl);
    /* 形态⑤:纯字符串字面量 */
    t = t.split('"' + path.posix.basename(rel) + '"').join('"' + dataUrl + '"');
    t = t.split("'" + path.posix.basename(rel) + "'").join("'" + dataUrl + "'");
    if (t !== before) { hits++; forms.push(rel2); site.set(rel2, Buffer.from(t, 'utf8')); }
  });
  /* 剩余出现次数(未内联的引用 ⇒ 未匹配到可改写形态) */
  const nameRe2 = new RegExp('(^|[^\\w.\\-])' + nameRe + '($|[^\\w])');
  const remainWhere = [];
  Array.from(site.keys()).forEach(rel2 => {
    if (!TEXT_EXT.test(rel2) || rel2 === rel) { return; }
    if (nameRe2.test(site.get(rel2).toString('utf8'))) { remainWhere.push(rel2); }
  });
  if (hits > 0) { inlinedCount++; assetsInlinedBytes += buf.length; site.delete(rel); }
  inlinedDetail.push({ file: rel, bytes: buf.length, refs: hits, remainFiles: remainWhere.length });
  if (hits > 0 && remainWhere.length) { unresolved.push({ file: rel, where: remainWhere.slice(0, 4) }); }
  if (hits === 0 && remainWhere.length) { unresolvedNoForm.push({ file: rel, bytes: buf.length, where: remainWhere.slice(0, 4) }); }
});
/* 三分类(勘-七-5.3 步 2;Leader 裁定 D3/P1-2 —— 口径与依据写死在此):
   ① refs>0 ∧ remain==0 ⇒ 已内联(删原文件);
   ② refs==0 ∧ remain>0 ⇒ **有引用、无可改写形态**(形态①–⑤均不吃,如带 `./` 前缀的裸引号串)——
      规格字面要求 exit 1;Leader 裁定改为**显式归类 + 登记 + 进 JLITE-META,不 exit 1**:
      这 3 件(icon-120x120.png / icon-512x512.png ← manifest.webmanifest;lab/favicon.ico ←
      jupyter-lite.json + jupyterlite.schema.v0.json)**确被引用**,只是没有可改写形态 ⇒
      保留为独立段、原路径仍可解析(今日良性)。旧实现把它们按 refs===0 误记为"零引用"并落
      `unreferencedBinaries`(**与事实不符**,评审 P1-2 的决定性证据);本批改准字段名与措辞。
   ③ refs==0 ∧ remain==0 ⇒ 真零引用(保留 + 登记进 JLITE-META.zeroRefBinaries)。
   refs>0 ∧ remain>0(部分残余,已内联而别处仍引用)⇒ 仍判真缺陷、exit 1(`unresolved`)。 */
const zeroRef = inlinedDetail.filter(x => x.refs === 0 && x.remainFiles === 0);
check(9, '子资源改写三分类(已内联 / 有引用无可改写形态 / 真零引用;勘-七-5.3 步 2)',
  unresolved.length === 0,
  unresolved.length ? JSON.stringify(unresolved).slice(0, 400)
    : inlinedCount + ' 件已内联 / ' + fmt(assetsInlinedBytes) + ' B;有引用无可改写形态 ' + unresolvedNoForm.length
      + ' 件(登记不阻断);真零引用 ' + zeroRef.length + ' 件', false);
console.log('  子资源:候选二进制 ' + binAssets.length + ' 件 → ① 已内联 ' + inlinedCount + ' 件 / ' + fmt(assetsInlinedBytes)
  + ' B;② 有引用无可改写形态 ' + unresolvedNoForm.length + ' 件' + (unresolvedNoForm.length ? '(' + unresolvedNoForm.map(x => x.file).join(', ') + ')' : '')
  + ';③ 真零引用 ' + zeroRef.length + ' 件' + (zeroRef.length ? '(' + zeroRef.map(x => x.file).join(', ') + ')' : ''));

/* ===========================================================================
 * 8. 关闭包 + Jupyter 专用锁(交付物②)+ 合并 all.json(交付物③)
 * ========================================================================= */
/* 8.1 闭包根 = 站点 pypi/** 的 wheel METADATA 的 Requires-Dist(不硬编码)。
   站点自带件(例:`pyodide_kernel-0.8.6-*.whl` 的 METADATA 声明 comm + ipython,它自己在 pypi/** 里)
   由站点段直接服务 ⇒ **不进载荷闭包**(否则会多出一份同字节的 `pyodide/pyodide_kernel-*.whl`)。 */
function wheelPkgName(file) { return file.replace(/-[0-9].*$/, ''); }
function importsFromWheel(names) {
  const top = [];
  names.forEach(n => {
    if (/^[^/]+\/__init__\.py$/.test(n)) { top.push(n.split('/')[0]); }
    else if (/^[^/]+\.py$/.test(n) && n !== 'setup.py') { top.push(n.slice(0, -3)); }
  });
  return [...new Set(top)].sort();
}
const pypiWheels = Array.from(site.keys()).filter(k => new RegExp('^' + extDir + 'pypi/.*\\.whl$').test(k)).sort();
const siteProvided = new Set(pypiWheels.map(rel => norm(wheelPkgName(path.posix.basename(rel)))));
const rootsAll = new Set();
const rootDecl = [];
pypiWheels.forEach(rel => {
  let meta;
  try { meta = wheelMetadata(site.get(rel)); } catch (e) { fail('站点轮 METADATA 读取失败 ' + rel + ':' + e.message); return; }
  meta.split('\n').filter(l => /^Requires-Dist:/.test(l)).forEach(l => {
    const spec = l.slice('Requires-Dist:'.length).trim().split(';')[0].trim();
    const nm = spec.split(/[<>=!~\[\s]/)[0].trim();
    if (!nm) { return; }
    rootsAll.add(norm(nm));
    rootDecl.push({ wheel: rel, req: nm });
  });
});
const roots = new Set(Array.from(rootsAll).filter(n => !siteProvided.has(n)));
/* 子批 B:附加闭包根(内核预装件)。**不混进 rootsAll** —— 那是"来自 METADATA"的语义位(方案 §四 S2-3)。 */
const rootsExtra = KERNEL_EXTRA_PACKAGES.filter(n => !siteProvided.has(n));
const rootsSiteProvided = Array.from(rootsAll).filter(n => siteProvided.has(n));
check('1c', '闭包根来自站点轮 METADATA(不硬编码)', rootsAll.size > 0,
  'METADATA 根 = [' + Array.from(rootsAll).join(', ') + '];进载荷闭包 ' + roots.size
  + ';站点自带(跳过)[' + rootsSiteProvided.join(', ') + ']'
  + ';附加根(子批 B)[' + rootsExtra.join(', ') + ']', false);
/* 附加根必须成立:任一被站点自带 ⇒ 闭包/锁/载荷三面同时错位(方案 §四 S2-3 的显式检查)。 */
check('1g', '附加闭包根 KERNEL_EXTRA_PACKAGES 与站点自带件不重叠(子批 B)',
  rootsExtra.length === KERNEL_EXTRA_PACKAGES.length,
  '附加根 [' + KERNEL_EXTRA_PACKAGES.join(', ') + '] ⇒ 进闭包 [' + rootsExtra.join(', ') + ']'
  + (rootsExtra.length === KERNEL_EXTRA_PACKAGES.length ? ''
    : ';✗ 与站点自带重叠 [' + KERNEL_EXTRA_PACKAGES.filter(n => siteProvided.has(n)).join(', ') + ']'), false);
console.log('  闭包根(站点轮 METADATA 现取):全部 [' + Array.from(rootsAll).join(', ') + '] ← '
  + pypiWheels.length + ' 个站点轮 / ' + rootDecl.length + ' 条 Requires-Dist;'
  + '站点自带跳过 [' + rootsSiteProvided.join(', ') + '] ⇒ 载荷闭包根 [' + Array.from(roots).join(', ') + ']'
  + ';附加根 [' + rootsExtra.join(', ') + '](子批 B:内核预装件)');

/* 8.2 闭包展开(PEP 503 归一;lock depends 递归;未命中 ⇒ 缺件) */
function findWheelFile(file) {
  const a = path.join(WHEELS, file);
  if (fs.existsSync(a)) { return a; }
  const b = path.join(JL_SRC, file);
  if (fs.existsSync(b)) { return b; }
  return null;
}
const jlWheelByPkg = {};
fs.readdirSync(JL_SRC).filter(f => /\.whl$/.test(f)).forEach(f => { jlWheelByPkg[norm(wheelPkgName(f))] = f; });
const closure = new Map();     /* norm 名 -> {name, version, file_name, sha256, src, source, origin} */
const closureMissing = [];
(function expandClosure() {
  const seen = new Set();
  /* 子批 B:闭包队列 = METADATA 根 ∪ 附加根。负例 `drop-jedi-root` **只**去掉附加根(只此一处一行;方案 §四 S2-7)。 */
  const queue = cliNeq.indexOf('drop-jedi-root') >= 0
    ? Array.from(new Set([...roots]))
    : Array.from(new Set([...roots, ...rootsExtra]));
  while (queue.length) {
    const n = queue.shift();
    if (seen.has(n)) { continue; }
    seen.add(n);
    if (siteProvided.has(n)) { continue; }      /* 站点段已服务 ⇒ 不进载荷 */
    const e = prodLock.packages[n];
    if (e) {
      const f = findWheelFile(e.file_name);
      if (!f) { closureMissing.push(n + '=>' + e.file_name); continue; }
      closure.set(n, {
        name: n, version: String(e.version), file_name: e.file_name, sha256: e.sha256,
        src: f, source: 'pyodide-lock.json', origin: 'closure'
      });
      (e.depends || []).forEach(d => queue.push(norm(d)));
      continue;
    }
    /* 不在 lock:自造条目(comm)—— 必须能在 jupyterlite-src 找到同名 wheel */
    const file = jlWheelByPkg[n];
    if (!file) { closureMissing.push(n + '(不在 lock,也没有自造 wheel)'); continue; }
    const man = manByPath[file];
    closure.set(n, {
      name: n,
      /* 版本取自 wheel 文件名(PEP 427;修正批 P1-1:旧 `/-[0-9]/` 无 `g` 把 `-` 与首位数字
         一起吃掉 ⇒ `comm-0.2.3-…` 得到 `.2.3`)。`--neg=bad-comm-version` 注入错误值以证明断言 1f 有咬合力。 */
      version: (cliNeq.indexOf('bad-comm-version') >= 0 && n === 'comm') ? '9.9.9' : versionFromWheelFile(file),
      file_name: file, sha256: man ? man.sha256 : null,
      src: path.join(JL_SRC, file), source: man ? 'MANIFEST.json' : '(缺 MANIFEST 条目)', origin: 'closure-external'
    });
  }
})();
check('1d', '闭包完备(缺件 = 0)', closureMissing.length === 0 && closure.size >= roots.size,
  closureMissing.length ? closureMissing.join(', ') : closure.size + ' 件(根 ' + roots.size + ')', false);
const closureLocal = Array.from(closure.values()).filter(c => c.origin === 'closure');
const closureExternal = Array.from(closure.values()).filter(c => c.origin === 'closure-external');
console.log('  闭包:' + closure.size + ' 件(本地 ' + closureLocal.length + ' + 自造 ' + closureExternal.length + ')');

/* 8.3 专用锁 = 产品剪裁锁(档位无关的唯一解 = full 的 151 条快照)∪ micropip ∪ 闭包(含 comm)
   151 = packages.json.fromOfficialLock(115,取自 vendor lock)+ selfAuthored(36,取件轮,lock 里没有)
   ⚠ **D2(Leader 裁定 2026-09-16):维持 151 超集口径、不改行为** —— `勘-七-1.3` 公式含"本档"
   而 `勘-七-5.8` 明写载荷无档位维度 ⇒ 取档位无关超集;normal/minimal 下 139 条不可解析属
   **已定义行为**(`勘-七-2.1`:由索引解析失败并**可见报错**)。改 14 无收益且牵动断言 15/11 与
   已登记读数 ⇒ 本批仅加注释注明口径,不改实现。 */
if (!fs.existsSync(path.join(PY_SRC, 'packages.json'))) { fail('缺输入:vendor/pyodide-src/packages.json'); }
const pkgsJson = JSON.parse(fs.readFileSync(path.join(PY_SRC, 'packages.json'), 'utf8'));
const minLock = { info: prodLock.info, packages: {} };
const lockBad = [];
pkgsJson.fromOfficialLock.forEach(nm => {
  const e = prodLock.packages[nm];
  if (!e) { lockBad.push('fromOfficialLock 的 ' + nm + ' 不在 pyodide-lock.json'); return; }
  minLock.packages[nm] = e;
});
pkgsJson.selfAuthored.forEach(s => {
  const abs = path.join(WHEELS, s.file_name);
  if (!fs.existsSync(abs)) { lockBad.push('缺自造件 ' + s.file_name); return; }
  const buf = fs.readFileSync(abs);
  if (sha256(buf) !== s.sha256) { lockBad.push('自造件 sha256 不符 ' + s.file_name); return; }
  let tops = [];
  try { tops = importsFromWheel(zipEntries(buf).map(x => x.name)); } catch (e) { lockBad.push('自造件解析失败 ' + s.file_name + ':' + e.message); }
  minLock.packages[norm(s.canonical)] = {
    name: s.name, version: s.version, file_name: s.file_name, package_type: 'package',
    install_dir: 'site', sha256: s.sha256, imports: tops.length ? tops : [norm(s.canonical)],
    depends: s.depends
  };
});
const baseLockCount = Object.keys(minLock.packages).length;
/* 断言 1e:**集合相等**(P2-9) —— 旧口径只比"计数相等",锁里少一件 + 盘上多一件同 sha256 时仍误过;
   参照实现 make-pyodide-part.js 是集合级判据。逐件报差集。 */
const baseLockFiles = new Set(Object.keys(minLock.packages).map(k => minLock.packages[k].file_name));
const wheelsSet = new Set(pyWheelNames);
const onlyLock = Array.from(baseLockFiles).filter(f => !wheelsSet.has(f));
const onlyDisk = pyWheelNames.filter(f => !baseLockFiles.has(f));
check('1e', '产品剪裁锁现取 == wheels/ 名册(**集合相等**;官方 115 + 自造 36)',
  lockBad.length === 0 && baseLockCount === pyWheelNames.length && onlyLock.length === 0 && onlyDisk.length === 0,
  lockBad.length ? lockBad.slice(0, 4).join('; ')
    : baseLockCount + ' 条(去重文件 ' + baseLockFiles.size + ') == wheels/ ' + pyWheelNames.length + ' 件'
      + (onlyLock.length ? ' ✗ 锁内有而盘上无:[' + onlyLock.slice(0, 5).join(',') + ']' : '')
      + (onlyDisk.length ? ' ✗ 盘上有而锁内无:[' + onlyDisk.slice(0, 5).join(',') + ']' : ''), false);
/* micropip:lock 在册、wheels/ 缺件 ⇒ 从 jupyterlite-src 取件 */
const micropipFile = fs.readdirSync(JL_SRC).filter(f => /^micropip-.*\.whl$/.test(f))[0];
if (!micropipFile) { fail('缺 micropip 件(vendor/jupyterlite-src/micropip-*.whl)'); }
const micropipEntry = prodLock.packages.micropip;
if (!micropipEntry) { fail('pyodide-lock.json 里没有 micropip(专用锁必须含它;勘-七-1.3)'); }
const micropipSrc = micropipFile ? path.join(JL_SRC, micropipFile) : null;
if (micropipSrc && micropipEntry) {
  const b = fs.readFileSync(micropipSrc);
  if (sha256(b) !== micropipEntry.sha256) {
    fail('micropip 件 sha256 与 pyodide-lock.json 不符(' + sha256(b) + ' vs ' + micropipEntry.sha256 + ')');
  }
  const manMp = manByPath[micropipFile];
  if (!manMp || manMp.sha256 !== micropipEntry.sha256) { fail('micropip 的 MANIFEST 条目缺失或 sha256 不符(来源 = pyodide-dist)'); }
  minLock.packages.micropip = micropipEntry;
}
/* comm 自造条目(格式逐字段对齐 make-pyodide-part.js 的自造条目) */
closureExternal.forEach(c => {
  const b = fs.readFileSync(c.src);
  if (!c.sha256) { fail('comm 缺 MANIFEST 条目(sha256 对照来源缺失)'); return; }
  if (sha256(b) !== c.sha256) { fail('comm 件 sha256 与 MANIFEST 不符(' + sha256(b) + ' vs ' + c.sha256 + ')'); }
  minLock.packages[c.name] = {
    name: c.name, version: c.version, file_name: c.file_name, package_type: 'package',
    install_dir: 'site', sha256: c.sha256, imports: [c.name], depends: [],
    unvendored_tests: false
  };
});
/* 附加根 KERNEL_EXTRA_PACKAGES 的专用锁条目(子批 B;**装载前提,非"顺带登记"** —— ③.5 核):
   `appJ` 把本锁当 `lockFileContents` 喂给 `loadPyodide` ⇒ jedi/parso **不在专用锁就取不到件**;
   插入位置承重:必须在 `closureLocal` 检查**之前**,否则会以"本地闭包件 jedi 不在产品剪裁锁里"误 FAIL。
   条目 = 官方全量锁(vendor/pyodide-src/pyodide-lock.json)原样复制;轮件 sha256 必须与锁一致。 */
KERNEL_EXTRA_PACKAGES.forEach(nm => {
  const e = prodLock.packages[nm];
  if (!e) { fail('附加根 ' + nm + ' 不在 pyodide 官方全量锁(pyodide-lock.json)里'); return; }
  const f = findWheelFile(e.file_name);
  if (!f) { fail('附加根 ' + nm + ' 的轮件缺失:' + e.file_name); return; }
  const got = sha256File(f);
  if (got !== e.sha256) { fail('附加根 ' + nm + ' 轮件 sha256 ≠ 官方全量锁(' + got + ' vs ' + e.sha256 + ')'); return; }
  minLock.packages[nm] = e;
});
/* 本地闭包件:lock 条目原样保留(已在 151 内),但 sha256 必须与来源一致 */
closureLocal.forEach(c => {
  const e = minLock.packages[c.name];
  if (!e) { fail('本地闭包件 ' + c.name + ' 不在产品剪裁锁里(应属 151 之内)'); return; }
  if (e.sha256 !== c.sha256) { fail('闭包件 ' + c.name + ' 的锁 sha256 ≠ 来源 sha256'); }
});
let lockEntries = Object.keys(minLock.packages).length;
if (cliNeq.indexOf('drop-comm-lock') >= 0) { closureExternal.forEach(c => { delete minLock.packages[c.name]; }); lockEntries = Object.keys(minLock.packages).length; }
if (cliNeq.indexOf('drop-micropip-lock') >= 0) { delete minLock.packages.micropip; lockEntries = Object.keys(minLock.packages).length; }
const lockJson = JSON.stringify(minLock);
if (/[\u2028\u2029]/.test(lockJson)) { fail('锁文本含 U+2028/U+2029(转义口径外)'); }
/* 断言 15 主路径(P2-7):「每条 file_name 有段」不再只活在 `--verify`。
   判据 = 锁内每条 file_name **∈ 本载荷段**(闭包 + micropip) **∨ ∈ wheels/ 名册**(运行期由
   `#pyodide-assets` 提供,勘-七-2.1 映射表第 7 行)。失败 detail 打印**具体缺键**(区分 N3/N4)。 */
const lockPayloadFiles = new Set();
closure.forEach(c => lockPayloadFiles.add(c.file_name));
if (micropipFile) { lockPayloadFiles.add(micropipFile); }
const lockMissingClosure = Array.from(closure.keys()).filter(k => !minLock.packages[k]);
const lockNoSeg = [];
Object.keys(minLock.packages).forEach(k => {
  const e = minLock.packages[k];
  const fn = e && e.file_name;
  if (!fn || (!lockPayloadFiles.has(fn) && !wheelsSet.has(fn))) { lockNoSeg.push(k + '→' + (fn || '(无 file_name)')); }
});
check(15, '专用锁 ⊇ 闭包 ∧ 每条 file_name ∈ 载荷段 ∨ ∈ wheels/ 名册(主路径) ∧ micropip 在锁内 ∧ sha256 对照来源',
  Object.keys(minLock.packages).length === lockEntries
  && lockMissingClosure.length === 0
  && !!minLock.packages.micropip
  && lockNoSeg.length === 0
  && Array.from(closure.values()).every(c => c.sha256),
  '锁 ' + lockEntries + ' 条(闭包 ' + closure.size + ' / micropip ' + (!!minLock.packages.micropip) + ')'
  + (lockMissingClosure.length ? '; ✗ 缺闭包键 [' + lockMissingClosure.join(', ') + ']' : '')
  + (lockNoSeg.length ? '; ✗ file_name 无段亦不在 wheels/: [' + lockNoSeg.slice(0, 5).join(', ') + ']' : '')
  + (!lockMissingClosure.length && !lockNoSeg.length ? '; 全部 file_name ∈ 载荷段 ∨ wheels/ 名册' : ''), false);
console.log('  专用锁:' + lockEntries + ' 条(产品剪裁 ' + baseLockCount + ' + micropip + comm) / JSON '
  + fmt(lockJson.length) + ' 字符(原 ' + fmt(JSON.stringify(prodLock).length) + ')');

/* 8.4 合并 all.json(覆盖站点自带;URL 指向虚拟源 /pyodide/) */
const siteIdxRaw = site.get(PIPY_SEG);
if (!siteIdxRaw) { fail('站点缺 ' + PIPY_SEG + '(合并索引的底)'); }
const mergedIdx = JSON.parse(siteIdxRaw.toString('utf8'));
const siteIdxKeys = Object.keys(mergedIdx).length;
const indexUrlToSeg = {};     /* index url -> 段 id */
Object.keys(mergedIdx).forEach(pkg => {
  Object.keys(mergedIdx[pkg].releases || {}).forEach(v => {
    (mergedIdx[pkg].releases[v] || []).forEach(rel => {
      const u = String(rel.url || '');
      const bn = u.split('/').pop().split('?')[0];
      indexUrlToSeg[pkg] = u.indexOf('http') === 0 ? u.slice(PSEUDO_ORIGIN.length) : (extDir + 'pypi/' + bn);
    });
  });
});
/* 索引键一律 **PEP 503 归一**(micropip/piplite 按归一名查表;现状 4 个站点键本已归一) */
const payloadWheelList = [];
closure.forEach(c => {
  const b = fs.readFileSync(c.src);
  const md5 = crypto.createHash('md5').update(b).digest('hex');
  const key = norm(c.name);
  mergedIdx[key] = { releases: {} };
  mergedIdx[key].releases[c.version] = [{
    comment_text: '', digests: { md5: md5, sha256: c.sha256 }, downloads: -1, filename: c.file_name,
    has_sig: false, md5_digest: md5, packagetype: 'bdist_wheel', python_version: 'py3', size: b.length,
    url: PSEUDO_ORIGIN + 'pyodide/' + c.file_name, yanked: false
  }];
  indexUrlToSeg[key] = 'pyodide/' + c.file_name;
  payloadWheelList.push({ id: 'pyodide/' + c.file_name, sha256: c.sha256, source: c.source, pkg: key });
});
if (micropipSrc && micropipEntry) {
  const b = fs.readFileSync(micropipSrc);
  const md5 = crypto.createHash('md5').update(b).digest('hex');
  mergedIdx.micropip = { releases: {} };
  mergedIdx.micropip.releases[micropipEntry.version] = [{
    comment_text: '', digests: { md5: md5, sha256: micropipEntry.sha256 }, downloads: -1,
    filename: micropipFile, has_sig: false, md5_digest: md5, packagetype: 'bdist_wheel',
    python_version: 'py3', size: b.length, url: PSEUDO_ORIGIN + 'pyodide/' + micropipFile, yanked: false
  }];
  indexUrlToSeg.micropip = 'pyodide/' + micropipFile;
}
if (cliNeq.indexOf('drop-comm-index') >= 0) { closureExternal.forEach(c => { delete mergedIdx[c.name]; }); }
const mergedIdxText = JSON.stringify(mergedIdx, null, 2);
if (/[^\x00-\x7F]/.test(mergedIdxText)) { fail('合并索引含非 ASCII(不可进 text 段)'); }
const allJsonPkgs = Object.keys(mergedIdx).length;
site.set(PIPY_SEG, Buffer.from(mergedIdxText, 'utf8'));
check(14, '索引 ⊇ 闭包 ∧ 索引每条 url 有对应段 ∧ 段 sha256 对照来源',
  Array.from(closure.keys()).every(k => !!mergedIdx[k]) && !!mergedIdx.micropip,
  '索引 ' + allJsonPkgs + ' 键(站点底 ' + siteIdxKeys + ' + 闭包 ' + closure.size + ' + micropip)', false);
console.log('  合并索引:' + siteIdxKeys + ' → ' + allJsonPkgs + ' 键(' + fmt(mergedIdxText.length) + ' 字符)');
Object.keys(mergedIdx).forEach(k => { console.log('    ' + k.padEnd(22) + ' → ' + indexUrlToSeg[k]); });

/* 断言 1f(修正批新增,P1-1;`[E13]` 四段范式):
   原判据 = **无**(16 条断言无一比较 version 与其来源)⇒ `comm` 误为 `.2.3` 全绿通过。
   为何不成立:`勘-七-1.3` 逐字写死 `commVersion:"0.2.3"`;version 是 pip/piplite 解析索引的键,
   错误值在带版本约束的需求下会抛 `InvalidVersion`,或经 `ProjectInfo.from_json_api` 被静默跳过 ⇒
   `Can't find a pure Python 3 wheel: 'comm'`(Phase 0 追了三轮的失败形状)。
   新判据:专用锁每条 `version` == 其来源(pyodide-lock.json / packages.json.selfAuthored / wheel 文件名),
   且合并索引(闭包 + micropip)的 release 键 == 锁内 version。
   为何仍能抓真缺陷:`--neg=bad-comm-version`(comm 版本改 9.9.9)⇒ 与文件名 `0.2.3` 不符 ⇒ 本断言 FAIL。 */
{
  const verBad = [];
  Object.keys(minLock.packages).forEach(nm => {
    const e = minLock.packages[nm];
    if (!e || !e.file_name) { return; }
    const wn = versionFromWheelFile(e.file_name);
    let src = null, srcDesc = '';
    if (prodLock.packages[nm]) { src = String(prodLock.packages[nm].version); srcDesc = 'pyodide-lock.json'; }
    else {
      const sa = (pkgsJson.selfAuthored || []).filter(s => norm(s.canonical) === nm)[0];
      if (sa) { src = String(sa.version); srcDesc = 'packages.json.selfAuthored'; }
    }
    if (src !== null && String(e.version) !== src) {
      verBad.push('锁 ' + nm + '.version=' + JSON.stringify(e.version) + ' ≠ ' + srcDesc + ' ' + JSON.stringify(src));
    }
    if (wn && String(e.version) !== wn) {
      verBad.push('锁 ' + nm + '.version=' + JSON.stringify(e.version) + ' ≠ wheel 文件名 ' + e.file_name + ' ⇒ ' + JSON.stringify(wn));
    }
  });
  const idxCheck = (key, want) => {
    const ent = mergedIdx[key];
    if (!ent) { return; }   /* 索引缺条目由断言 14 负责,不在本断言串台 */
    const rels = Object.keys(ent.releases || {});
    if (rels.length !== 1 || rels[0] !== String(want)) {
      verBad.push('索引 ' + key + '.releases 键 ' + JSON.stringify(rels) + ' ≠ version ' + JSON.stringify(String(want)));
    }
  };
  closure.forEach(c => idxCheck(norm(c.name), c.version));
  if (micropipEntry) { idxCheck('micropip', micropipEntry.version); }
  check('1f', '专用锁 / 合并索引内每个包的 version == 其来源(pyodide-lock.json / packages.json / wheel 文件名)',
    verBad.length === 0,
    verBad.length ? verBad.slice(0, 6).join('; ') + (verBad.length > 6 ? ' …共 ' + verBad.length + ' 条' : '')
      : Object.keys(minLock.packages).length + ' 锁条目 + ' + allJsonPkgs + ' 索引键 逐条对源一致(comm=' + JSON.stringify((closure.get('comm') || {}).version) + ')', false);
}

/* ===========================================================================
 * 9. 载荷段(勘-七-5.3 第 4 步;D-1:不含 core)
 * ========================================================================= */
const payload = [];      /* {id, buf, group, source, sha256decl, file} */
payload.push({ id: 'pyodide/pyodide.mjs', buf: fs.readFileSync(path.join(JL_SRC, 'pyodide.mjs')), group: 'payload', file: 'pyodide.mjs' });
closure.forEach(c => {
  payload.push({ id: 'pyodide/' + c.file_name, buf: fs.readFileSync(c.src), group: 'closure', file: c.file_name, source: c.source, sha256decl: c.sha256 });
});
if (micropipSrc) { payload.push({ id: 'pyodide/' + micropipFile, buf: fs.readFileSync(micropipSrc), group: 'closure', file: micropipFile, source: 'pyodide-lock.json+MANIFEST(pyodide-dist)', sha256decl: micropipEntry.sha256 }); }

/* ===========================================================================
 * 10. 段表组装 + 帧序列化
 * ========================================================================= */
const sections = [];
const account = [];
/* P3-3:`escText` 原为死变量(只写不读)⇒ 改为**聚合计数**,落 JLITE-META.escCounts(证据面) */
const escCounts = { sections: 0, nScript: 0, nComment: 0 };
function escForPart(s) {
  const a = (s.match(/<\/script/gi) || []).length, b = (s.match(/<!--/g) || []).length;
  const o = s.replace(/<\/script/gi, '<\\/script').replace(/<!--/g, '<\\!--');
  escCounts.nScript += a; escCounts.nComment += b; if (a || b) { escCounts.sections++; }
  return o;
}
function addText(id, text, group, noteStr) {
  const body = escForPart(text);
  if (body.indexOf(FRAME) >= 0) { fail('段 ' + id + ' 正文含帧标记 ' + FRAME); }
  if (body.length !== text.length) { fail('段 ' + id + ' 转义改变了长度(文本段需逐字符对齐)'); }
  if (/[^\x00-\x7F]/.test(body)) { fail('段 ' + id + ' 是 text 段但含非 ASCII(字符数 ≠ 字节数)'); }
  sections.push({ id: id, kind: 'text', body: body });
  account.push({ id: id, group: group, raw: Buffer.byteLength(text), gz: 0, out: body.length, note: noteStr });
}
function addB64gz(id, buf, group, noteStr) {
  const gz = zlib.gzipSync(buf, { level: 6 });
  const body = gz.toString('base64');
  if (body.indexOf(FRAME) >= 0 || body.indexOf('</script') >= 0 || body.indexOf('<!--') >= 0) { fail('段 ' + id + ' 的 base64 正文含非法子串'); }
  sections.push({ id: id, kind: 'b64gz', body: body });
  account.push({ id: id, group: group, raw: buf.length, gz: gz.length, out: body.length, note: noteStr });
}
/* 站点段(数据改写后的树;含被覆盖后的合并索引) */
Array.from(site.keys()).sort().forEach(rel => {
  if (rel === PIPY_SEG) { return; }   /* 合并索引单独作 text 段,见下 */
  addB64gz(rel, site.get(rel), 'site', rel);
});
/* 两个 text 段(交付物②③;纯 ASCII 载体,勘-七-3.6 的口径) */
addText(LOCK_SEG, lockJson, 'meta', 'Jupyter 专用锁(交付物②)');
addText(PIPY_SEG, mergedIdxText, 'meta', '合并后的 all.json(交付物③,覆盖站点自带)');
/* 载荷段 */
payload.forEach(p => { addB64gz(p.id, p.buf, p.group, 'pyodide/<file> 例外段: ' + p.file); });
/* --neg=core-dup 注入:把 core 复制成一段(负例⑤;断言 16 必须 FAIL) */
if (cliNeq.indexOf('core-dup') >= 0) {
  addB64gz('pyodide/pyodide.asm.wasm', fs.readFileSync(path.join(PY_SRC, 'pyodide.asm.wasm')), 'neg', '负例注入:core 重复');
}

const byGroup = {};
account.forEach(a => { const g = byGroup[a.group] || (byGroup[a.group] = { n: 0, raw: 0, gz: 0, out: 0 }); g.n++; g.raw += a.raw; g.gz += a.gz; g.out += a.out; });

/* 页面级 native import( 扫描(断言 4)。修正批 P2-1:原 `META_READOUT.l3 = hits` 发生在 `meta`
   构造**之后** ⇒ 死赋值,`l3` 从未进 META。此处前移到 META 构造**之前**,使读数真正落盘。 */
function scanNativeImports() {
  const hits = {};
  sections.forEach(s => {
    if (!/\.(js|html)$/i.test(s.id)) { return; }
    if (!/^(build|lab|extensions)\//.test(s.id)) { return; }
    const t = s.kind === 'b64gz' ? zlib.gunzipSync(Buffer.from(s.body, 'base64')).toString('utf8') : s.body;
    const n = (t.match(/\bimport\s*\(/g) || []).length;
    if (n) { hits[s.id] = n; }
  });
  return hits;
}

/* JLITE-META(固定字段集 + 断言表强制落盘项) */
const META_FIXED = {
  assetsInlined: inlinedCount,
  assetsInlinedBytes: assetsInlinedBytes,
  pyodideCopies: 0,
  labVersion: LAB_VERSION,
  jlVersion: JL_VERSION,
  kernelVersion: KERNEL_VERSION,
  pyodideVersion: PYODIDE_VERSION,
  allJsonPkgs: allJsonPkgs,
  p7ConfigKeys: p7Keys,
  jlLock: {
    entries: lockEntries, closureEntries: closure.size, micropip: !!minLock.packages.micropip,
    commVersion: (closure.get('comm') || {}).version || null, baseEntries: baseLockCount
  },
  /* 载荷段显式清单(pyodide.mjs + 闭包 + micropip;**动态表述**,子批 B 首跑 = 17 件;
     P2-4:旧 filter `group === 'closure'` 漏掉 pyodide.mjs ⇒ 14。 */
  payloadWheels: payload.filter(p => p.group !== 'site' && p.group !== 'meta')
    .map(p => ({ id: p.id, sha256: p.sha256decl || sha256(p.buf), source: p.source || 'vendor/jupyterlite-src/pyodide.mjs' }))
};
const META_READOUT = {
  siteFiles: sections.filter(s => s.id.indexOf('pyodide/') !== 0 && s.id !== LOCK_SEG && s.id !== PIPY_SEG).length,
  sections: sections.length,
  closureRoots: Array.from(roots),
  closureMissing: closureMissing,
  cdnFallbacks: cdnFallbacks,
  upstreamExtra: upstreamExtraList,
  assetsInlinedDetail: inlinedDetail.filter(x => x.refs > 0).map(x => ({ file: x.file, bytes: x.bytes })),
  /* P1-2 三分类:② 有引用无可改写形态(字段名改准 —— 旧 `unreferencedBinaries` 把 3 件"有引用"误称"未引用");
     ③ 真零引用(zeroRefBinaries)。 */
  unresolvedNoForm: unresolvedNoForm.map(x => ({ file: x.file, bytes: x.bytes, where: x.where })),
  zeroRefBinaries: zeroRef.map(x => ({ file: x.file, bytes: x.bytes })),
  jlLockKeys: Object.keys(minLock.packages).length,
  locksha256: sha256(Buffer.from(lockJson, 'utf8')),
  indexSha256: sha256(Buffer.from(mergedIdxText, 'utf8')),
  esc: 'backslash',
  escCounts: escCounts,
  l3: scanNativeImports(),
  /* META 必须全 ASCII(勘-七-5.4)⇒ 改造点表的展示 id 用 ASCII 别名(对应 勘-七-2.3 的「资产」/「索引」行)。
     P2-3:patchTable 与 `scripts/jupyterlite-patches.json` **同源序列化**,并带上 schema 1 字段。 */
  patchTable: PATCH_TABLE.map(p => ({
    id: ({ '资产': 'ASSET', '索引': 'INDEX' })[p.id] || p.id, status: p.status, owner: p.owner || null,
    targetGlob: p.targetGlob || null, anchor: p.anchor || null, mode: p.mode || null,
    expectBefore: (p.expectBefore === undefined ? null : p.expectBefore),
    expectAfter: (p.expectAfter === undefined ? null : p.expectAfter),
    replacement: p.replacement || null, source: p.source || null, counterKey: p.counterKey || null
  })),
  patchCounts: PATCH_APPLIED
};

/* 122 术语族扫描(断言 13a;对**解码后的站点资产 + pyodide.mjs** 跑) */
const terms122 = {};
{
  const scan = (id, text) => {
    TERMS_122.forEach(t => {
      const n = text.split(t).length - 1;
      if (n) { (terms122[t] = terms122[t] || []).push(id + '×' + n); }
    });
  };
  sections.forEach(s => {
    if (s.id === LOCK_SEG || s.id === PIPY_SEG) { return; }
    if (!TEXT_EXT.test(s.id)) { return; }
    scan(s.id, s.kind === 'b64gz' ? zlib.gunzipSync(Buffer.from(s.body, 'base64')).toString('utf8') : s.body);
  });
  scan('pyodide/pyodide.mjs', fs.readFileSync(path.join(JL_SRC, 'pyodide.mjs'), 'utf8'));
  const bad = Object.keys(terms122).filter(t => !TERMS_122_KNOWN_OK[t]);
  check('13a', '122 术语族扫描(13a;命中 = 0 或逐条理由)',
    bad.length === 0,
    bad.length ? '未登记理由的命中族: ' + bad.map(t => t + '→' + terms122[t].length + ' 处').join('; ')
      : '命中 ' + Object.keys(terms122).length + ' 族,全部已登记理由(' + Object.keys(terms122).join('/') + ')', false);
  console.log('  122 术语族逐族理由:');
  Object.keys(terms122).forEach(t => { console.log('    ' + t.padEnd(24) + '×' + terms122[t].length + '  ' + (TERMS_122_KNOWN_OK[t] || '(未登记理由)')); });
  console.log('  122 术语族扫描:' + (Object.keys(terms122).length
    ? Object.keys(terms122).map(t => t + '(' + terms122[t].length + ')').join(' ')
    : '0 命中'));
  META_READOUT.terms122 = {}; Object.keys(terms122).forEach(t => { META_READOUT.terms122[t] = terms122[t].length; });
}

const meta = Object.assign({}, META_FIXED, META_READOUT);
/* META 必须全 ASCII(勘-七-5.4):META 行落在 `<script type="text/plain" id="jupyterlite-assets">`
   的正文内(text 载荷)⇒ 非 ASCII(如 patchTable 的 source/anchor 中的中文)一律转义为 `\uXXXX`
   (合法 JSON;`JSON.parse` 解回原值,故 `--verify` 的 META 往返比对不受影响)。 */
const metaLine = JSON.stringify(meta).replace(/[\u007f-\uffff]/g, c => '\\u' + c.charCodeAt(0).toString(16).padStart(4, '0'));
if (metaLine.indexOf('\n') >= 0 || metaLine.indexOf(FRAME) >= 0) { fail('META 行不合法'); }

let part = '<!-- ===== 内联 JupyterLite ' + JL_VERSION + '(lab 站点 + pyodide 依赖闭包,由 scripts/make-jupyterlite-part.js 生成,请勿手改)===== -->\n'
  + '<script type="text/plain" id="' + ASSETS_ID + '">\n'
  + FRAME + '-PART 1 ' + sections.length + '\n'
  + FRAME + '-META ' + metaLine + '\n';
sections.forEach(s => { part += FRAME + '-SECTION ' + s.id + ' ' + s.kind + ' ' + s.body.length + '\n' + s.body + '\n'; });
part += FRAME + '-END\n</script>\n';
if (cliNeq.indexOf('section-len') >= 0) {
  const m = new RegExp('(' + reEsc(FRAME) + '-SECTION \\S+ \\S+ )(\\d+)').exec(part);
  part = part.slice(0, m.index + m[1].length) + (Number(m[2]) + 1) + part.slice(m.index + m[1].length + m[2].length);
}

/* ===========================================================================
 * 11. 产物结构断言(断言 5)+ 帧往返
 * ========================================================================= */
function parsePart(text) {
  const lines = [];
  let p = text.indexOf('\n') + 1;
  const head0 = text.slice(0, text.indexOf('\n'));
  const l2 = text.slice(p, text.indexOf('\n', p));
  p = text.indexOf('\n', p) + 1;
  const l3 = text.slice(p, text.indexOf('\n', p));
  p = text.indexOf('\n', p) + 1;
  const l4 = text.slice(p, text.indexOf('\n', p));
  p = text.indexOf('\n', p) + 1;
  const mPart = new RegExp('^' + reEsc(FRAME) + '-PART 1 (\\d+)$').exec(l3);
  if (!mPart) { throw new Error('第 3 行不是 PART 头: ' + l3.slice(0, 60)); }
  const mMeta = new RegExp('^' + reEsc(FRAME) + '-META (.*)$').exec(l4);
  if (!mMeta) { throw new Error('第 4 行不是 META 行'); }
  const n = Number(mPart[1]);
  const secs = [];
  for (let i = 0; i < n; i++) {
    const hEnd = text.indexOf('\n', p);
    const h = text.slice(p, hEnd);
    const m = new RegExp('^' + reEsc(FRAME) + '-SECTION (\\S+) (text|b64gz) (\\d+)$').exec(h);
    if (!m) { throw new Error('段 ' + i + ' 头不合法: ' + h.slice(0, 80)); }
    const len = Number(m[3]);
    const body = text.substr(hEnd + 1, len);
    if (body.length !== len) { throw new Error('段 ' + m[1] + ' 长度不符: 头声明 ' + len + ' 实际 ' + body.length); }
    if (text.charAt(hEnd + 1 + len) !== '\n') { throw new Error('段 ' + m[1] + ' 正文后不是换行'); }
    secs.push({ id: m[1], kind: m[2], len: len, body: body });
    p = hEnd + 1 + len + 1;
  }
  const tail = text.slice(p, text.indexOf('\n', p));
  if (tail !== FRAME + '-END') { throw new Error('段尾不是 ' + FRAME + '-END: ' + tail.slice(0, 60)); }
  p = text.indexOf('\n', p) + 1;
  if (text.slice(p, text.indexOf('\n', p)) !== '</script>') { throw new Error('END 之后不是 </script>'); }
  return { head: head0, open: l2, meta: JSON.parse(mMeta[1]), sections: secs, metaLine: mMeta[1] };
}
let partParsed = null, partParseErr = null;
try { partParsed = parsePart(part); } catch (e) { partParseErr = e.message; fail('产物帧结构/往返失败:' + e.message); }
{
  const openTag = '<script type="text/plain" id="' + ASSETS_ID + '">';
  const bodyStart = part.indexOf(openTag);
  const bodyEnd = part.lastIndexOf('</script>');
  const body = bodyStart < 0 ? '' : part.slice(bodyStart + openTag.length, bodyEnd);
  const asciiIdx = body.search(/[^\x00-\x7F]/);
  const c = {
    openTagOk: part.indexOf(openTag) >= 0 && part.indexOf(openTag) === bodyStart,
    tagCount: (part.match(/<script/g) || []).length === 1 && (part.match(/<\/script>/g) || []).length === 1,
    bodyStartOk: bodyStart >= 0 && bodyEnd > bodyStart,
    noScriptClose: !/<\/script/i.test(body),
    noCommentOpen: !/<!--/.test(body),
    ascii: asciiIdx < 0,
    textAscii: partParsed ? partParsed.sections.every(s => s.kind !== 'text' || (!/[^\x00-\x7F]/.test(s.body) && Buffer.byteLength(s.body) === s.body.length)) : false,
    roundTrip: !!partParsed
  };
  const allOk = Object.keys(c).every(k => c[k]);
  check('5', '段正文无裸 </script/<!--/帧标记;text 段字符数 == 字节数', allOk,
    JSON.stringify(c) + (partParseErr ? ' 帧解析错:' + partParseErr : ' 段数 ' + (partParsed ? partParsed.sections.length : '?'))
    + (asciiIdx < 0 ? '' : ' 首个非 ASCII @' + asciiIdx + ' 邻域:' + JSON.stringify(body.slice(Math.max(0, asciiIdx - 70), asciiIdx + 70))),
    false);
  console.log('  断言 5 逐项:' + JSON.stringify(c) + (asciiIdx < 0 ? '' : ' 非 ASCII@' + asciiIdx + ':' + JSON.stringify(body.slice(Math.max(0, asciiIdx - 70), asciiIdx + 70))));
}

/* ===========================================================================
 * 12. 断言 8 / 12 / 11 / 16 / 4
 * ========================================================================= */
/* 8a:site 面 dist 副本 = 0 */
{
  const siteIds = sections.filter(s => s.id.indexOf('pyodide/') !== 0 && s.id !== LOCK_SEG && s.id !== PIPY_SEG).map(s => s.id);
  const wasmInSite = siteIds.filter(k => /\.wasm$/i.test(k));
  const coreInSite = siteIds.filter(k => CORE_BASENAMES.indexOf(path.posix.basename(k)) >= 0);
  const lockInSite = siteIds.filter(k => /pyodide-lock|pyodide\.asm\./i.test(k));
  const wheelSha = new Set(pyWheelNames.map(f => sha256(fs.readFileSync(path.join(WHEELS, f)))));
  const copies = [];
  sections.forEach(s => {
    if (s.id.indexOf('pyodide/') === 0 || s.id === LOCK_SEG || s.id === PIPY_SEG) { return; }
    if (wheelSha.has(sha256(Buffer.from(s.body, 'base64').length ? zlib.gunzipSync(Buffer.from(s.body, 'base64')) : Buffer.alloc(0)))) { copies.push(s.id); }
  });
  const wl = siteIds.filter(k => new RegExp('^' + reEsc(extDir) + 'pypi/.*\\.whl$').test(k));
  check(8, '8a site 面:站点内 pyodide 副本 = 0(4 子断言 + 白名单 ≤5)',
    wasmInSite.length === 0 && coreInSite.length === 0 && lockInSite.length === 0
    && copies.length === 0 && wl.length <= 5,
    '.wasm ' + wasmInSite.length + ' / core 名 ' + coreInSite.length + ' / lock 名 ' + lockInSite.length
    + ' / 151 同 sha256 副本 ' + copies.length + ' / 白名单 ' + wl.length + ' 条', false);
  console.log('  8a:站点段 ' + siteIds.length + ' 段;.wasm ' + wasmInSite.length + ' core 名 ' + coreInSite.length
    + ' 副本 ' + copies.length + ';内核 pypi 白名单 ' + wl.length + ' 条');
}
/* 8b:载荷面显式清单(P2-4:期望 = 完整载荷段 = pyodide.mjs + 闭包 + micropip;**动态表述**,子批 B
   首跑 = 1 + 15(含 jedi/parso) + 1 = 17;旧口径 `payloadWheelList.length + micropip` = 14 漏掉 pyodide.mjs) */
const PAYLOAD_EXPECT = 1 + closure.size + (micropipSrc ? 1 : 0);
check('8b', '8b 载荷面:载荷段显式清单进 JLITE-META(pyodide.mjs + 闭包 + micropip)',
  META_FIXED.payloadWheels.length === PAYLOAD_EXPECT
  && META_FIXED.payloadWheels.length === payload.length
  && META_FIXED.payloadWheels.every(w => w.id && w.sha256 && w.source),
  META_FIXED.payloadWheels.length + ' 条(每含 id/sha256/source;= pyodide.mjs + 闭包 ' + closure.size
  + ' + micropip;期望 ' + PAYLOAD_EXPECT + ')', false);
/* 12:files/** + ipynb 白名单两件 */
{
  const filesHits = sections.filter(s => /(^|\/)files\//.test(s.id));
  const ipynb = sections.filter(s => /\.ipynb$/.test(s.id)).map(s => s.id).sort();
  const wl = ['jupyter-lite.ipynb', 'lab/jupyter-lite.ipynb'];
  check(12, 'site/**files/** 存在即 exit 1;ipynb 白名单两件',
    filesHits.length === 0 && ipynb.length === 2 && ipynb.every(x => wl.indexOf(x) >= 0),
    'files/** ' + filesHits.length + ' 命中;ipynb = [' + ipynb.join(', ') + ']', false);
}
/* 11:LF + CR = 0 + 载荷 ↔ MANIFEST/生成脚本一致(闭包件 sha256 对照来源) */
{
  const partBuf = Buffer.from(part, 'utf8');
  const cr = partBuf.indexOf(13);
  const payloadShaOk = payload.filter(p => p.sha256decl).every(p => sha256(p.buf) === p.sha256decl);
  const metaOk = partParsed && partParsed.metaLine === metaLine;
  check(11, 'LF 输出 + CR = 0 + 与 MANIFEST/生成脚本一致(闭包件 sha256 对照来源)',
    cr < 0 && payloadShaOk && metaOk,
    '首个 CR 偏移 ' + cr + ';载荷 sha256 对照来源 ' + (payloadShaOk ? 'OK' : 'FAIL')
    + ';META 往返 ' + metaOk, false);
  var partBufOut = partBuf;
}
/* 16:载荷面 dist 重复 = 0 */
{
  const coreHits = sections.filter(s => CORE_BASENAMES.indexOf(path.posix.basename(s.id)) >= 0
    || (s.id.indexOf('pyodide/') === 0 && CORE_BASENAMES.indexOf(s.id.slice('pyodide/'.length)) >= 0));
  const closureShaOk = payload.filter(p => p.sha256decl).every(p => sha256(p.buf) === p.sha256decl);
  check(16, '载荷面 dist 重复 = 0(core 段不得出现;闭包段 sha256 对照来源)',
    coreHits.length === 0 && closureShaOk,
    coreHits.length ? 'core 段命中: ' + coreHits.map(s => s.id).join(', ')
      : 'core 段 0/' + CORE_BASENAMES.length + ' 名;闭包段 sha256 对照来源 ' + (closureShaOk ? 'OK' : 'FAIL'), false);
}
/* 4:页面级 native import( 白名单(读数 = `meta.l3`,已在 META 构造前算好并落盘;P2-1) */
{
  const hits = meta.l3;
  const outside = Object.keys(hits).filter(k => !L3_WHITELIST.some(w => new RegExp('^' + reEsc(w).replace(/\\\*/g, '.*') + '$').test(k)));
  check(4, '页面级 native import( 白名单', outside.length === 0,
    outside.length ? '白名单外命中: ' + JSON.stringify(outside.map(k => k + '×' + hits[k]))
      : JSON.stringify(hits), false);
  console.log('  页面级 native import( 现取:' + JSON.stringify(hits) + '(已落 JLITE-META.l3)');
}

/* ===========================================================================
 * 13. 断言 6(体积账;按 D1:打印 + 对拍 + 偏差说明,**不 exit 1**)
 * ========================================================================= */
console.log('');
console.log('分段体积账(raw / gzip / 内嵌 b64gz):');
const GROUPS = [['site', '站点段'], ['closure', '闭包轮'], ['payload', '载荷(pyodide.mjs)'], ['meta', '锁+索引(text)'], ['neg', '负例注入']];
GROUPS.forEach(g => {
  const x = byGroup[g[0]]; if (!x) { return; }
  console.log('  ' + g[1].padEnd(16) + String(x.n).padStart(4) + ' 段  原始 ' + mb(x.raw).padStart(11)
    + '   gzip ' + mb(x.gz).padStart(11) + '   内嵌 ' + mb(x.out).padStart(11));
});
const tot = account.reduce((a, x) => ({ n: a.n + 1, raw: a.raw + x.raw, gz: a.gz + x.gz, out: a.out + x.out }), { n: 0, raw: 0, gz: 0, out: 0 });
console.log('  ' + '合计'.padEnd(15) + String(tot.n).padStart(4) + ' 段  原始 ' + mb(tot.raw).padStart(11)
  + '   gzip ' + mb(tot.gz).padStart(11) + '   内嵌 ' + mb(tot.out).padStart(11));
console.log('  assetsInlined = ' + inlinedCount + ' 件 / ' + fmt(assetsInlinedBytes) + ' B(子资源内联增量)');
{
  const budget = [
    { k: 'site raw', got: byGroup.site.raw, b: BUDGET.siteRaw },
    { k: 'site b64gz', got: byGroup.site.out, b: BUDGET.siteB64gz },
    { k: 'payload raw', got: (byGroup.closure ? byGroup.closure.raw : 0) + (byGroup.payload ? byGroup.payload.raw : 0), b: BUDGET.payloadRaw },
    { k: '本产物 b64gz 合计', got: tot.out, b: null }
  ];
  budget.forEach(x => {
    if (!x.b || x.b.v === null) { console.log('  [对拍] ' + x.k.padEnd(18) + fmt(x.got).padStart(12) + ' B   预算 = 待回填(' + (x.b ? x.b.src : '首跑实测') + ')'); return; }
    const dev = (x.got - x.b.v) / x.b.v * 100;
    console.log('  [对拍] ' + x.k.padEnd(18) + fmt(x.got).padStart(12) + ' B   预算 ' + fmt(x.b.v)
      + '(' + x.b.src + ') → 偏差 ' + dev.toFixed(2) + '%' + (Math.abs(dev) > 5 ? '  [!] >5%:需逐项说明(见 done 报告 §体积账)' : ''));
  });
  check(6, '体积账逐段 + 汇总 + assetsInlined 打印 + 与预算对拍(不阻断,D1)',
    account.length > 0 && tot.out > 0 && byGroup.site && byGroup.site.raw > 0,
    '合计 ' + fmt(tot.out) + ' B / ' + account.length + ' 段 / assetsInlined ' + inlinedCount, true);
}
const payloadBytes = (byGroup.closure ? byGroup.closure.raw : 0) + (byGroup.payload ? byGroup.payload.raw : 0);

/* ===========================================================================
 * 14. 断言 7(幂等:不含时间戳/随机量)
 * ========================================================================= */
{
  const timeKeyHits = Object.keys(meta).filter(k => /time|date|now|generated|stamp|random|uuid/i.test(k));
  const isoHit = /\b20\d\d-\d\d-\d\dT\d\d:/.exec(metaLine);
  const randHit = /"?(?:Math\.random|Date\.now|new Date)\b/.test(metaLine);
  check('7', '幂等:产物不含时间戳/随机量(外部连跑两次字节比对为权威)',
    timeKeyHits.length === 0 && !isoHit && !randHit,
    (timeKeyHits.length ? 'META 顶层时间样键:' + timeKeyHits.join(',') + ';' : 'META 无时间样键;')
    + (isoHit ? '正文含 ISO 时间串:' + isoHit[0] + ';' : '')
    + (randHit ? '含随机/时钟调用;' : '')
    + fmt(partBufOut.length) + ' B', false);
  console.log('  产物 ' + posix(path.relative(ROOT, PART_PATH)) + '  ' + fmt(partBufOut.length) + ' B(' + mb(partBufOut.length)
    + '),md5 ' + crypto.createHash('md5').update(partBufOut).digest('hex')
    + ',sha256 ' + sha256(partBufOut).slice(0, 16) + '…');
}

/* ===========================================================================
 * 15. 断言表打印 + 落盘 / --verify
 * ========================================================================= */
console.log('');
console.log('自检断言(' + ASSERT.length + ' 条;编号按 勘-七-5.6 的断言表):');
const ASSERT_ORDER = ['1', '1c', '1d', '1e', '1g', '2', '2b', '3', '4', '5', '6', '7', '8a', '8b', '9', '12', '13a', '14', '15', '16'];
const LABEL = { 2.1: '2b' };
function lab(n) { return LABEL[n] !== undefined ? LABEL[n] : String(n); }
ASSERT.slice().sort((a, b) => {
  const ia = ASSERT_ORDER.indexOf(lab(a.n)), ib = ASSERT_ORDER.indexOf(lab(b.n));
  return (ia < 0 ? 99 : ia) - (ib < 0 ? 99 : ib);
}).forEach(a => {
  console.log('  ' + (lab(a.n) + ')').padEnd(6) + (a.pass ? 'PASS' : (a.soft ? 'WARN' : 'FAIL')) + '  ' + a.name
    + (a.detail ? '  —— ' + a.detail.slice(0, 220) : ''));
});
console.log('');

/* `--verify`(断言 10)。函数声明提升 ⇒ §3 后的 `if (cliVerify) runVerify();` 即调用它。
   短路构造段(不重跑生成)是 P2-6 的核心:`勘-七-5.5` 要求 `--verify` **不重跑生成**;
   旧实现把校验块放在全文生成之后 ⇒ 每次 `--verify` 先跑完整生成(实测 113 s)且退出码不看生成期
   `problems`(会打印 `FAIL` 又给 `PASS`)。本实现改为**读回盘上 part 校验**,并把非 soft `problems`
   并入失败集(短路下 `problems` 仅含输入面问题,合并为防御性冗余)。 */
function runVerify() {
  /* 断言 10:读回已落盘 part、按帧解析、逐段解码、重扫 `1f` / 3 / 5 / 8a / 9 / 11 / 14 / 15 / 16
     (1f 由修正批 3 纳入 —— 复审 F4:`勘十-3` 括注写「正例 ⇒ PASS(`--verify` 重扫断言 17)」,
      而当时的重扫面不含它 ⇒ `--verify` 对"版本==来源"零覆盖) */
  const vp = fs.existsSync(VERIFY_PATH) ? VERIFY_PATH : PART_PATH;
  if (!fs.existsSync(vp)) { console.error('--verify 失败:找不到 ' + posix(path.relative(ROOT, vp))); process.exit(1); }
  const vb = fs.readFileSync(vp);
  const vprobs = [];
  if (vb.indexOf(13) >= 0) { vprobs.push('--verify:文件含 CR(断言 11)'); }
  let vp2 = null;
  try { vp2 = parsePart(vb.toString('utf8')); } catch (e) { vprobs.push('--verify:帧解析/段长往返失败 —— ' + e.message + '(断言 10/5)'); }
  if (vp2) {
    const vm = vp2.meta;
    const seg = {};
    vp2.sections.forEach(s => { seg[s.id] = s; });
    const textOf = s => s.kind === 'b64gz' ? zlib.gunzipSync(Buffer.from(s.body, 'base64')).toString('utf8') : s.body;
    /* 5 */
    vp2.sections.forEach(s => {
      if (s.kind === 'text' && /[^\x00-\x7F]/.test(s.body)) { vprobs.push('--verify:段 ' + s.id + ' 是 text 但含非 ASCII(断言 5)'); }
      if (s.body.indexOf(FRAME) >= 0) { vprobs.push('--verify:段 ' + s.id + ' 含帧标记(断言 5)'); }
    });
    /* 11 */
    if (!vm || !seg[LOCK_SEG] || !seg[PIPY_SEG]) { vprobs.push('--verify:缺锁/索引 text 段(断言 11)'); }
    else {
      const l = JSON.parse(seg[LOCK_SEG].body);
      if (Object.keys(l.packages).length !== vm.jlLock.entries) { vprobs.push('--verify:锁段条目数与 META 不符(断言 11)'); }
      const ix = JSON.parse(seg[PIPY_SEG].body);
      if (Object.keys(ix).length !== vm.allJsonPkgs) { vprobs.push('--verify:索引键数与 META 不符(断言 11)'); }
      if (sha256(Buffer.from(seg[LOCK_SEG].body, 'utf8')) !== vm.locksha256) { vprobs.push('--verify:锁段 sha256 与 META 不符(断言 11)'); }
      if (sha256(Buffer.from(seg[PIPY_SEG].body, 'utf8')) !== vm.indexSha256) { vprobs.push('--verify:索引段 sha256 与 META 不符(断言 11)'); }
    }
    /* 8a */
    const siteIds = vp2.sections.filter(s => s.id.indexOf('pyodide/') !== 0 && s.id !== LOCK_SEG && s.id !== PIPY_SEG).map(s => s.id);
    if (siteIds.some(k => /\.wasm$/i.test(k))) { vprobs.push('--verify:站点面含 .wasm(断言 8a)'); }
    if (siteIds.some(k => CORE_BASENAMES.indexOf(path.posix.basename(k)) >= 0)) { vprobs.push('--verify:站点面含 core 名(断言 8a)'); }
    const wlSeg = siteIds.filter(k => new RegExp('^' + reEsc(extDir) + 'pypi/.*\\.whl$').test(k));
    if (wlSeg.length > 5) { vprobs.push('--verify:内核 pypi 白名单 ' + wlSeg.length + ' > 5(断言 8a)'); }
    /* 16 */
    const coreSegs = vp2.sections.filter(s => CORE_BASENAMES.indexOf(path.posix.basename(s.id)) >= 0);
    if (coreSegs.length) { vprobs.push('--verify:载荷面出现 core 段 ' + coreSegs.map(s => s.id).join(',') + '(断言 16)'); }
    /* 9(用 META 的 assetsInlinedDetail) */
    (vm.assetsInlinedDetail || []).forEach(a => {
      const bn = path.posix.basename(a.file);
      const nameRe = new RegExp('(^|[^\\w.\\-])' + reEsc(bn) + '($|[^\\w])');
      const where = vp2.sections.filter(s => s.id !== a.file && TEXT_EXT.test(s.id) && nameRe.test(textOf(s))).map(s => s.id);
      if (where.length) { vprobs.push('--verify:已内联资产 ' + bn + ' 仍有引用残留(断言 9): ' + where.slice(0, 3).join(',')); }
    });
    /* 3(CDN 串,在解码后的站点文本上) */
    const cdnNow = [];
    vp2.sections.forEach(s => {
      if (!TEXT_EXT.test(s.id) || s.id.indexOf('pyodide/') === 0 || s.id === LOCK_SEG || s.id === PIPY_SEG) { return; }
      const t = textOf(s);
      CDN_STRINGS.forEach(cs => { if (t.indexOf(cs) >= 0) { cdnNow.push(s.id + '"' + cs + '"'); } });
    });
    console.log('  --verify 断言 3 现取:CDN 串命中 ' + cdnNow.length + ' 处(登记 ' + (vm.cdnFallbacks || []).length + ' 条)');
    /* 14 / 15 / 16(需要来源 sha256 ⇒ 从**输入面**现算闭包;
       闭包根由磁盘侧 deriveRoots() 现取 —— 短路下主流程的 `roots` 未初始化) */
    const rootsNow = deriveRoots().roots;
    const wheelsNameSet = new Set(fs.readdirSync(WHEELS));
    const closureNow = new Map();
    (function () {
      const q = Array.from(rootsNow); const seen = new Set();
      while (q.length) {
        const n = q.shift(); if (seen.has(n)) { continue; } seen.add(n);
        const e = prodLock.packages[n];
        if (e) { closureNow.set(n, { file_name: e.file_name, sha256: e.sha256 }); (e.depends || []).forEach(d => q.push(norm(d))); continue; }
        const cand = fs.readdirSync(JL_SRC).filter(f => new RegExp('^' + reEsc(n) + '-.*\\.whl$').test(f));
        if (cand.length) { const mp = manByPath[cand[0]]; closureNow.set(n, { file_name: cand[0], sha256: mp ? mp.sha256 : null }); }
      }
    })();
    /* 15 */
    /* 「每条 file_name 有对应段」按 勘-七-2.1 的映射表读:只有**闭包件 + micropip** 必须在本载荷
       里有段;其余锁条目由 `#pyodide-assets` 在运行期提供(P2-7:旧口径把"其余 N 条由 #pyodide-assets
       提供"写成**未核的声明** ⇒ 此轮升级为**逐件核 ∈ wheels/ 名册**)。 */
    const lockDoc = JSON.parse(seg[LOCK_SEG].body);
    const mustHaveSeg = new Set(['micropip']);
    closureNow.forEach((c, k) => { mustHaveSeg.add(k); });
    let lockSegByAssets = 0, lockAssetsUnchecked = 0;
    Object.keys(lockDoc.packages).forEach(k => {
      const fn = lockDoc.packages[k].file_name;
      if (!mustHaveSeg.has(k)) {
        lockSegByAssets++;
        if (!fn || !wheelsNameSet.has(fn)) { lockAssetsUnchecked++; vprobs.push('--verify:锁条目 ' + k + ' 非载荷件且不在 wheels/ 名册(' + fn + ')(断言 15 映射表读)'); }
        return;
      }
      if (!seg['pyodide/' + fn]) { vprobs.push('--verify:锁条目 ' + k + ' 的 file_name 没有对应段(断言 15): ' + fn); }
    });
    console.log('  --verify 断言 15:锁 ' + Object.keys(lockDoc.packages).length + ' 条;须有段的 ' + mustHaveSeg.size
      + ' 条(闭包 ' + closureNow.size + ' + micropip)全部命中;其余 ' + lockSegByAssets
      + ' 条**已逐件核 ∈ wheels/ 名册**(未命中 ' + lockAssetsUnchecked + ';勘-七-2.1 映射表读)');
    Array.from(closureNow.keys()).forEach(k => {
      if (!lockDoc.packages[k]) { vprobs.push('--verify:专用锁 ⊉ 闭包(缺 ' + k + ')(断言 15)'); }
    });
    if (!lockDoc.packages.micropip) { vprobs.push('--verify:专用锁缺 micropip(断言 15)'); }
    closureNow.forEach((c, k) => {
      const s = seg['pyodide/' + c.file_name];
      if (!s) { vprobs.push('--verify:闭包件 ' + c.file_name + ' 没有对应段(断言 15/16)'); return; }
      const got = sha256(zlib.gunzipSync(Buffer.from(s.body, 'base64')));
      if (got !== c.sha256) { vprobs.push('--verify:段 ' + s.id + ' sha256 ≠ 来源 ' + c.sha256 + '(断言 15/16)'); }
    });
    /* 14 */
    const idxDoc = JSON.parse(seg[PIPY_SEG].body);
    Array.from(closureNow.keys()).forEach(k => { if (!idxDoc[k]) { vprobs.push('--verify:索引 ⊉ 闭包(缺 ' + k + ')(断言 14)'); } });
    Object.keys(idxDoc).forEach(pkg => {
      Object.keys(idxDoc[pkg].releases || {}).forEach(v => {
        (idxDoc[pkg].releases[v] || []).forEach(rel => {
          const u = String(rel.url || '');
          const id = u.indexOf('http') === 0 ? u.slice(PSEUDO_ORIGIN.length) : (extDir + 'pypi/' + u.split('/').pop().split('?')[0]);
          if (!seg[id]) { vprobs.push('--verify:索引条目 ' + pkg + ' 的 url 无对应段(' + id + ')(断言 14)'); }
        });
      });
    });
    /* 1f(修正批 3 纳入,复审 F4)—— 判据与主路径同源:
       ① 锁段**每条** `version` == 其来源(vendor `pyodide-lock.json` / `packages.json.selfAuthored` / **wheel 文件名**);
       ② 合并索引里**闭包 + micropip** 的 release 键 == 锁内 version。
       短路下主流程的 `minLock`/`mergedIdx`/`closure` 未构造 ⇒ 全部从**盘上 part 的两段**现取
       (`lockDoc` 在断言 15 处已解析;`idxDoc` 见上;`closureNow` 由磁盘侧 `deriveRoots()` 现推)。
       覆盖面:**① 覆盖锁段全部条目;② 覆盖闭包 + micropip(不含站点底 4 键 —— 那 4 键不属于闭包面)**。 */
    {
      let pkgsJsonV = null;
      try { pkgsJsonV = JSON.parse(fs.readFileSync(path.join(PY_SRC, 'packages.json'), 'utf8')); } catch (e) { pkgsJsonV = null; }
      const vVerBad = [];
      Object.keys(lockDoc.packages).forEach(nm => {
        const e = lockDoc.packages[nm];
        const fn = e && e.file_name;
        if (!fn) { return; }
        const wn = versionFromWheelFile(fn);
        let src = null, srcDesc = '';
        if (prodLock.packages[nm]) { src = String(prodLock.packages[nm].version); srcDesc = 'pyodide-lock.json'; }
        else if (pkgsJsonV) {
          const sa = (pkgsJsonV.selfAuthored || []).filter(s => norm(s.canonical) === nm)[0];
          if (sa) { src = String(sa.version); srcDesc = 'packages.json.selfAuthored'; }
        }
        if (src !== null && String(e.version) !== src) {
          vVerBad.push('锁 ' + nm + '.version=' + JSON.stringify(e.version) + ' ≠ ' + srcDesc + ' ' + JSON.stringify(src));
        }
        if (wn && String(e.version) !== wn) {
          vVerBad.push('锁 ' + nm + '.version=' + JSON.stringify(e.version) + ' ≠ wheel 文件名 ' + fn + ' ⇒ ' + JSON.stringify(wn));
        }
      });
      Array.from(closureNow.keys()).concat(['micropip']).forEach(k => {
        const lk = lockDoc.packages[k];
        if (!lk) { return; }          /* 缺锁条目由断言 15 负责,不在本断言串台 */
        const ent = idxDoc[k] || idxDoc[norm(k)];
        if (!ent) { return; }         /* 缺索引条目由断言 14 负责 */
        const rels = Object.keys(ent.releases || {});
        if (rels.length !== 1 || rels[0] !== String(lk.version)) {
          vVerBad.push('索引 ' + k + '.releases 键 ' + JSON.stringify(rels) + ' ≠ 锁内 version ' + JSON.stringify(String(lk.version)));
        }
      });
      console.log('  --verify 断言 1f:锁 ' + Object.keys(lockDoc.packages).length + ' 条 + 索引闭包面 '
        + (closureNow.size + 1) + ' 键逐条对源一致(comm=' + JSON.stringify((lockDoc.packages.comm || {}).version)
        + ';不合 ' + vVerBad.length + ')');
      if (vVerBad.length) {
        vprobs.push('--verify:断言 1f 未过 ' + vVerBad.slice(0, 4).join('; ') + (vVerBad.length > 4 ? ' …共 ' + vVerBad.length + ' 条' : ''));
      }
    }
  }
  console.log('');
  const allProbs = vprobs.concat(problems);   /* P2-6:非 soft 生成期 problems 并入失败集 */
  if (allProbs.length) {
    console.error('--verify 未过(' + allProbs.length + ' 条):');
    allProbs.slice(0, 20).forEach(p => console.error('  - ' + p));
    process.exit(1);
  }
  console.log('--verify PASS:帧/段长往返 + 重扫 1f/3/5/8a/9/11/14/15/16 全过('
    + posix(path.relative(ROOT, vp)) + ')');
  process.exit(0);
}

if (problems.length) {
  console.error('自检失败(' + problems.length + ' 条):');
  problems.forEach(p => console.error('  - ' + p));
  process.exit(1);
}
if (cliDry) {
  console.log('  --dry-run:不落盘(断言与体积账已跑完);目标路径 ' + posix(path.relative(ROOT, PART_PATH)));
  process.exit(0);
}
fs.mkdirSync(path.dirname(PART_PATH), { recursive: true });
fs.writeFileSync(PART_PATH, partBufOut);
console.log('  已写出 ' + posix(path.relative(process.cwd(), PART_PATH)).replace(/\\/g, '/') + '  ' + fs.statSync(PART_PATH).size + ' bytes');
console.log('  段数 ' + sections.length + '(站点 ' + (byGroup.site ? byGroup.site.n : 0)
  + ' + 锁/索引 2 + pyodide 段 ' + payload.length + (byGroup.neg ? ' + 负例 ' + byGroup.neg.n : '') + ')'
  + ',锁 ' + lockEntries + ' 条,索引 ' + allJsonPkgs + ' 键,载荷 ' + fmt(payloadBytes) + ' B');
