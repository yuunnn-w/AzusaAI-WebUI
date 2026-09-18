/* 拼装单文件产物:node scripts/build.js [--profile=<id>|all] [--out=<path>]
   默认档 = full → AzusaAI-WebUI-full.html(三档同落代码目录根,与分发布局一致);
   轻档默认落 profile 的 outFile(同为代码目录根);--out 按 ROOT 解析,覆盖档位默认路径。
   --profile=all = 一次拼装三档(任一档载荷缺失即 exit 1,不写任何产物)。
   写出策略:每档先写 <final>.tmp-<pid>,全部写成功后才统一 rename 到正式名 ——
   缺件之外的失败面(IO / CR 中途出错)也不留半套、不碰任何现有正式产物。 */
const fs = require('fs');
const path = require('path');
const ROOT = path.resolve(__dirname, '..');
const SRC = path.join(ROOT, 'src');

/* ---------------------------------------------------------------------------
 * 源树自检:Git LFS 指针守卫 —— 必须在读取任何载荷之前跑
 * src/pyodide.part 在 .gitattributes 里是 filter=lfs:克隆端未装 git-lfs / 未拉取 LFS
 * 对象时,工作区里它只是 134 B 的指针文本。若放行,构建会把指针当载荷嵌进产物并
 * exit 0 —— 产物"构建成功"、Python 工具族实则全坏(静默坏产物)。这里提前拦下。
 * ------------------------------------------------------------------------- */
const LFS_POINTER_HEAD = 'version https://git-lfs.github.com/spec/v1';
function firstLineOf(file) {
  const fd = fs.openSync(file, 'r');
  try {
    const buf = Buffer.alloc(256);
    const n = fs.readSync(fd, buf, 0, 256, 0);
    return buf.slice(0, n).toString('utf8').split('\n')[0].replace(/\r/g, '').trim();
  } finally {
    fs.closeSync(fd);
  }
}
function lfsPointerParts() {
  const hits = [];
  const walk = (dir) => {
    fs.readdirSync(dir, { withFileTypes: true }).forEach((ent) => {
      const full = path.join(dir, ent.name);
      if (ent.isDirectory()) { walk(full); return; }
      if (!ent.name.endsWith('.part')) return;
      let first;
      try { first = firstLineOf(full); } catch (e) { return; }   /* 读不了的文件交给后面的读取阶段报错 */
      if (first.indexOf(LFS_POINTER_HEAD) === 0) hits.push(path.relative(ROOT, full).replace(/\\/g, '/'));
    });
  };
  walk(SRC);
  return hits;
}
const lfsParts = fs.existsSync(SRC) ? lfsPointerParts() : [];
if (lfsParts.length) {
  lfsParts.forEach((p) => console.error('检测到 Git LFS 指针文件（' + p + '）：请安装 git-lfs 并执行 '
    + '`git lfs checkout`（或 `git lfs pull`）后重试。'));
  console.error('构建已中止:未读取载荷、未写入任何产物。');
  process.exit(1);
}

/* ---------------------------------------------------------------------------
 * CLI + 档位解析(权威 = scripts/pyodide-profiles.json)
 * ------------------------------------------------------------------------- */
const argv = process.argv.slice(2);
function cliVal(name) {
  const hit = argv.filter(a => a.indexOf('--' + name + '=') === 0);
  if (hit.length > 1) { console.error('参数 --' + name + ' 重复'); process.exit(1); }
  return hit.length ? hit[0].slice(name.length + 3) : null;
}
const cliUnknown = argv.filter(a => a.indexOf('--profile=') !== 0 && a.indexOf('--out=') !== 0);
if (cliUnknown.length) {
  console.error('未知参数: ' + cliUnknown.join(' ') + '(可用:--profile=<id|all> --out=<path>)');
  process.exit(1);
}
const cliProfileGiven = argv.some(a => a.indexOf('--profile=') === 0);
/* 空值是「给了但无效」,与「没给」不是一回事:`--profile=$VAR` 里 VAR 为空时不许静默构建完整版
   (未知/无效档位直接 exit 1,不猜测、不回退 full) */
const rawProfile = cliVal('profile');
const rawOut = cliVal('out');
if (rawProfile === '') {
  console.error('参数 --profile 的值为空(--profile=)。用法:--profile=<id|all>(有效 id 见 '
    + 'scripts/pyodide-profiles.json;缺省 = full)');
  process.exit(1);
}
if (rawOut === '') {
  console.error('参数 --out 的值为空(--out=)。用法:--out=<path>(缺省 = 档位文件里的 outFile)');
  process.exit(1);
}
const cliProfile = rawProfile || 'full';
const cliOut = rawOut;
const cliAll = cliProfile === 'all';
/* all 要写三条路径,--out 只认一条:直接拒掉,不猜(与 make-pyodide-part.js --all 同款语义) */
if (cliAll && cliOut) {
  console.error('--profile=all 不支持 --out(三档要写三个路径)。请逐档构建,或去掉 --out。');
  process.exit(1);
}

const PROFILES_PATH = path.join(ROOT, 'scripts', 'pyodide-profiles.json');
let profileDoc;
try {
  profileDoc = JSON.parse(fs.readFileSync(PROFILES_PATH, 'utf8'));
} catch (e) {
  console.error('读档位文件失败 scripts/pyodide-profiles.json: ' + e.message);
  process.exit(1);
}
const allProfiles = profileDoc.profiles || [];
if (!allProfiles.length) {
  console.error('档位文件里没有任何档位:scripts/pyodide-profiles.json');
  process.exit(1);
}
/* all = 档位文件列出的全部档位(顺序也以文件为准:full → normal → minimal) */
let targets;
if (cliAll) {
  targets = allProfiles;
} else {
  const profile = allProfiles.filter(p => p.id === cliProfile)[0];
  if (!profile) {
    console.error('未知档位 --profile=' + cliProfile + '(有效 id:'
      + allProfiles.map(p => p.id).join(' / ') + ';另有 --profile=all = 一次拼装三档)');
    process.exit(1);
  }
  targets = [profile];
}
/* 显式请求的档位不许静默降级成"没有 Python 的页面";默认 full 缺件时保留旧语义(警告 + 降级) */
function partPathOf(p) { return path.join(ROOT, p.partFile); }
function explicitOf(p) { return cliAll || cliProfileGiven || p.id !== 'full'; }
function partMissingMsg(p) {
  return '构建失败:' + p.partFile + ' 不存在(--profile=' + p.id + ')。'
    + '先跑 node scripts/make-pyodide-part.js --profile=' + p.id + ' 生成该档载荷。';
}
/* all 先把三档载荷的存在性一次查完:缺任一档就一个产物都不写(不做半套) */
if (cliAll) {
  const missing = targets.filter(p => !fs.existsSync(partPathOf(p)));
  if (missing.length) {
    missing.forEach(p => console.error(partMissingMsg(p)));
    console.error('--profile=all 要求三档载荷齐备:未写入任何产物。');
    process.exit(1);
  }
}

/* 统一按 LF 读取,避免各编辑器/工具写入 CRLF 造成混合换行 */
function read(f) {
  return fs.readFileSync(path.join(SRC, f), 'utf8').replace(/\r\n/g, '\n');
}
/* 可选分段:不存在就跳过(先生成 pdfjs.part / tesseract.part 再拼) */
function readIf(f) {
  try {
    return read(f);
  } catch (e) {
    console.log('! 缺少 ' + f + '(该功能会自动降级为不可用)');
    return '';
  }
}

/* 与档位无关的分段只读一次(all 模式下三档共用同一份文本) */
const head = read('head.part').replace('/*__KATEX_CSS__*/', () => read('katex-embedded.css').trim());
const libs = read('libs.part');
const katexJs = '<script>\n/* ===== 内嵌库: KaTeX v0.16.11 ===== */\n'
  + read('katex.min.js') + '\n</script>\n';
/* 内嵌库分段:pdf.js(文本抽取 / 页面渲染)与 Tesseract.js(OCR)
   由 make-pdfjs-part.js / make-tesseract-part.js 生成,自带 </script> 转义与自检 */
const pdfjsPart = readIf('pdfjs.part');
const tessPart = readIf('tesseract.part');
/* 内嵌办公文件解析分段(mammoth / SheetJS / docstream / fflate + OfficeKit 包装层)
   由 make-office-part.js 生成,自带 </script> 转义与自检;缺失时同款语义:跳过 + 警告 */
const officePart = readIf('office.part');
/* 内嵌 JupyterLite 分段(lab 站点 + pyodide 依赖闭包 + Jupyter 专用锁 + 合并后的 piplite 索引)
   由 make-jupyterlite-part.js 生成。读取口径与 pyodide 载荷**逐字同款**:按字节读 + **含 CR 即 exit 1**
   (载荷段头按字符数记长,CRLF→LF 归一会让段长错位)⇒ **禁止**走做归一的 readIf。
   缺 src/jupyterlite.part ⇒ 打印警告 + 注入 JL_AVAILABLE=false 的极小占位段,**不 exit 1**
   (与 pdfjs/tesseract 同款降级语义;占位段的 `jl-status.json` 段 = 运行期判定"载荷缺失"的唯一入口,
   由 Phase 2 的 jlAppWaitReady 读取)。该载荷**无档位维度**:三档共用同一份(勘-七-5.8)。 */
const JL_PART_FILE = 'jupyterlite.part';
const JL_PLACEHOLDER_SECTION = 'jl-status.json';
let jlPart = '', jlDegraded = false;
{
  const jlPath = path.join(SRC, JL_PART_FILE);
  if (fs.existsSync(jlPath)) {
    const jlRawBuf = fs.readFileSync(jlPath);
    const crAt = jlRawBuf.indexOf(13);
    if (crAt >= 0) {
      console.error('构建失败:' + JL_PART_FILE + ' 含 CR(0x0D,首个偏移 ' + crAt + ')。'
        + '载荷按字符数记长,CRLF→LF 归一会让段长错位 → 请跑 node scripts/make-jupyterlite-part.js '
        + '重新生成(该脚本保证 LF 输出)。');
      process.exit(1);
    }
    jlPart = jlRawBuf.toString('utf8');
  } else {
    jlDegraded = true;
    const st = 'JL_AVAILABLE=false\n';
    jlPart = '<!-- ===== JupyterLite 载荷缺失(降级占位段;正式载荷由 node scripts/make-jupyterlite-part.js 生成)===== -->\n'
      + '<script type="text/plain" id="jupyterlite-assets">\n'
      + ';;;JLITE-PART 1 1\n'
      + ';;;JLITE-META ' + JSON.stringify({ assetsInlined: 0, pyodideCopies: 0, jlAvailable: false, missing: true, jlVersion: null }) + '\n'
      + ';;;JLITE-SECTION ' + JL_PLACEHOLDER_SECTION + ' text ' + st.length + '\n' + st + '\n'
      + ';;;JLITE-END\n</script>\n';
    console.log('! 缺少 ' + JL_PART_FILE + '(JupyterLab 标签页功能会自动降级为不可用)');
    console.log('   ↳ 跑 node scripts/make-jupyterlite-part.js 生成正式载荷;占位段 ' + JL_PLACEHOLDER_SECTION + ' = JL_AVAILABLE=false');
  }
}
/* appJ.part = JupyterLite 宿主的**主页面侧**(需求 6 · Phase 2)。它插在 appD 与 appE 之间:
   appA–appE(含 appJ)同属**一个 IIFE**,靠函数声明提升共享作用域 ⇒ appJ 不得提前闭合 IIFE,
   appE 必须仍是最后一段(勘-七-5.7)。新增函数名须先全库搜重名(同名后声明者会静默覆盖)。 */
const app = read('appA.part') + read('appB.part') + read('appC.part') + read('appD.part')
  + read('appJ.part') + read('appE.part');

/* ---- 暂存写出:先写临时文件,全部成功后才 rename 成正式名 ---- */
/* 暂存路径与正式文件同目录(rename 才是同卷原子替换)+ 进程号(并发构建互不覆盖) */
function tmpPathOf(outPath) { return outPath + '.tmp-' + process.pid; }
let stagedTmps = [];
function cleanupStaged() {
  stagedTmps.forEach(function (f) { try { fs.unlinkSync(f); } catch (e) {} });
  stagedTmps = [];
}
/* 兜底清理:stageProfile 内部还有几条 process.exit(1) 的既有失败路径(缺件 / 载荷含 CR),
   走到它们时前面的档位可能已写过临时文件 —— 在 exit 钩子里无条件删干净,不留残渣 */
process.on('exit', cleanupStaged);

/* 拼一档:校验 + 组装 + 写**临时文件**;返回 {OUT, TMP, logs}。正式文件此时一个都还没动。 */
function stageProfile(profile) {
  const partExists = fs.existsSync(partPathOf(profile));
  if (!partExists && explicitOf(profile)) {
    console.error(partMissingMsg(profile));
    process.exit(1);
  }
  /* pyodide 载荷(核心集 + 本档预置包闭包)由 make-pyodide-part.js 生成;
     路径取自档位文件的 partFile(默认档 full = src/pyodide.part)。
     这里不用 readIf:载荷段头按**字符数**声明长度,而 read() 的 CRLF→LF 归一会让正文"变短"、段长错位,
     故必须在读取阶段就按字节卡死 CR(含 CR 即失败)。CR-free 时 toString('utf8') 与 read() 等价。 */
  const pyRawBuf = partExists ? fs.readFileSync(partPathOf(profile)) : null;
  let pyPart = '';
  if (pyRawBuf) {
    const crAt = pyRawBuf.indexOf(13);
    if (crAt >= 0) {
      console.error('构建失败:' + profile.partFile + ' 含 CR(0x0D,首个偏移 ' + crAt + ')。'
        + '载荷按字符数记长,CRLF→LF 归一会让段长错位 → 请跑 node scripts/make-pyodide-part.js --profile='
        + profile.id + ' 重新生成(该脚本保证 LF 输出)。');
      process.exit(1);
    }
    pyPart = pyRawBuf.toString('utf8');
  } else {
    console.log('! 缺少 ' + profile.partFile + '(该功能会自动降级为不可用)');
    console.log('   ↳ 缺 ' + profile.partFile + ' → Python 工具族(ExecutePython 与任务类)整体不可用,其余功能不受影响');
  }
  /* --out 按 ROOT(代码目录)解析;默认 = 档位文件的 outFile(三档同落代码目录根) */
  const OUT = cliOut ? path.resolve(ROOT, cliOut) : path.join(ROOT, profile.outFile);
  const TMP = tmpPathOf(OUT);
  /* 轻档产物前置一行档位注释(首行来自 head.part,本身不含档位;full 不插 → 默认产物与历史逐字节一致) */
  const wheelCount = profile.expect && typeof profile.expect.wheels === 'number' ? profile.expect.wheels : null;
  const profileBanner = profile.id === 'full' ? ''
    : ('<!-- AzusaAI WebUI · ' + profile.label + ' edition (profile=' + profile.id + ', '
      + (wheelCount === null ? '?' : wheelCount) + ' 个预置包) · 由 node scripts/build.js --profile='
      + profile.id + ' 生成 · 完整版 = AzusaAI-WebUI-full.html -->\n');
  const out = (profileBanner + head + libs + katexJs + pdfjsPart + tessPart + officePart + pyPart + jlPart + app).replace(/\r\n/g, '\n');
  fs.mkdirSync(path.dirname(OUT), { recursive: true });
  stagedTmps.push(TMP);          /* 先登记后写:写到一半失败也能被 cleanup 掉 */
  fs.writeFileSync(TMP, out);
  /* 日志先收集、rename 成功后才打印(避免"写完失败却已宣告完成"的错序输出) */
  const logs = [];
  /* 降级态(默认档缺 part,见上)不许宣称"预置包 N 个" —— 载荷没带进来,包数无从谈起 */
  const pyBytes = Buffer.byteLength(pyPart);
  logs.push(path.relative(ROOT, OUT).replace(/\\/g, '/') + '  ' + Buffer.byteLength(out) + ' bytes, '
    + out.split('\n').length + ' lines, CRLF:' + (out.indexOf('\r') >= 0 ? 'yes' : 'no')
    + '  [profile ' + profile.id + ' · pyodide ' + pyBytes + ' B · '
    + (pyBytes ? ('预置包 ' + (wheelCount === null ? '?' : wheelCount) + ' 个') : '载荷缺失(Python 工具族不可用)')
    + ']');
  logs.push('  pdfjs ' + pdfjsPart.length + ' B / tesseract ' + tessPart.length + ' B / office '
    + Buffer.byteLength(officePart) + ' B / jupyterlite ' + Buffer.byteLength(jlPart) + ' B'
    + (jlDegraded ? '(降级占位段:载荷缺失)' : ''));

  /* 附加核对:打印 pyodide 分段体积并与档位期望值核对(±5%)
     期望总量 = 档位 expect.partBytes(实测写回) + 本产物实测的非 pyodide 部分(不再写死 203,600,000) */
  const totalBytes = Buffer.byteLength(out);
  const nonPyBytes = totalBytes - pyBytes;
  const expPart = profile.expect && typeof profile.expect.partBytes === 'number' ? profile.expect.partBytes : null;
  if (pyBytes) {
    if (expPart === null) {
      logs.push('pyodide  ' + pyBytes + ' B  [体积核对] 未对账(档位 ' + profile.id
        + ' 的 expect.partBytes = null;首跑后写回 scripts/pyodide-profiles.json)');
    } else {
      const pyEst = expPart;   /* = 期望总量 − 实测非 pyodide 部分 */
      const pyDev = (pyBytes - pyEst) / pyEst * 100;
      logs.push('pyodide  ' + pyBytes + ' B  [体积核对] 本档期望载荷 ' + pyEst + ' B → 偏差 '
        + pyDev.toFixed(2) + '%' + (Math.abs(pyDev) <= 5 ? '(±5% 判据:通过)' : '(±5% 判据:超出,请核对 scripts/pyodide-profiles.json 的 expect 与取件字节)'));
    }
  } else {
    logs.push('pyodide  0 B  [体积核对] 载荷缺失,无法核对(降级态)');
  }
  return { OUT: OUT, TMP: TMP, logs: logs };
}

/* 阶段一:全部目标档位先写临时文件;任一失败 → 清临时 + exit 1,正式产物零触碰 */
const staged = [];
targets.forEach(function (p, i) {
  if (cliAll) console.log('=== [' + (i + 1) + '/' + targets.length + '] --profile=' + p.id + ' ===');
  try {
    staged.push(stageProfile(p));
  } catch (e) {
    cleanupStaged();
    console.error('构建失败(写出阶段):' + ((e && e.message) || e) + ' —— 已清理本次临时文件,正式产物未被替换。');
    process.exit(1);
  }
});
/* 阶段二:全部写成功后才统一替换正式文件(rename 同目录 = 原子) */
staged.forEach(function (one) {
  try {
    fs.renameSync(one.TMP, one.OUT);
  } catch (e) {
    cleanupStaged();   /* 尚未 rename 的临时文件全部清掉;已 rename 的档位保留新产物 */
    console.error('构建失败(替换阶段):' + ((e && e.message) || e) + ' [' + one.OUT + '] —— '
      + '该档之前已完成的档位是新产物,其余档位正式文件未动。');
    process.exit(1);
  }
  stagedTmps = stagedTmps.filter(function (f) { return f !== one.TMP; });
  one.logs.forEach(function (l) { console.log(l); });
});
if (cliAll) {
  console.log('--profile=all:三档全部完成 → ' + targets.map(p => p.outFile).join(' / '));
}
