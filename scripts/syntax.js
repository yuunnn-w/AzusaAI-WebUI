/* 语法检查:把一个单文件产物里的内联 <script> 逐个交给 V8 解析(不执行)
   用法:node scripts/syntax.js [path]
   不给路径时取三档产物里第一个存在的(顺序 = scripts/pyodide-profiles.json 的 outFile:
   AzusaAI-WebUI-full.html → -normal.html → -minimal.html,三档都落代码目录根)
   跳过 type 非 JS 的块(<script type="text/plain"> 是内嵌库的二进制/文本载荷,不是 JS)

   ⚠ 本脚本除 V8 解析外,另跑一条 **HTML 分词器陷阱守卫**(见下 scriptDataTrap);
   它按 HTML 规范的 script-data 状态机扫每段正文——**type 非 JS 的载荷块同样要扫**
   (分词器不看 type)。 */
const fs = require('fs');
const vm = require('vm');
const path = require('path');
const ROOT = path.resolve(__dirname, '..');

/* ===========================================================================
 * HTML 分词器陷阱守卫(2026-09-17 收口批新增;`[E13]` 四段范式)
 *
 * ① 原判据(现状)= 本脚本只把每段正文交给 V8 解析,对 HTML 分词器层**零检查**。
 * ② 为何不成立 = "V8 能解析" ≠ "浏览器能取到该段"。HTML 规范里 <script> 正文的
 *    script-data 状态机遇到 `<!--` 就进 script-data-escaped;此后再遇到 `<script`
 *    (后随 空白 / `/` / `>`)便进 script-data-double-escaped —— 该状态下 `</script>`
 *    **不再结束元素**,后面整片 HTML 被吞进脚本正文。本项目 Phase 3 真踩过这个坑
 *    (`appJ.part` 注释里的 `<!--` 与之后的 `<script`)。
 * ③ 新判据 = 对每段正文跑一遍 script-data 状态机:**终态不得为 double-escaped**
 *    (终态 = 浏览器读完这段正文时所在的状态)。命中 ⇒ 打印块号 / HTML 行 /
 *    正文内偏移与行 / 上下文,并**非零退出**。
 * ④ 为何仍能抓真缺陷 = 注入式负例(正文内 `<!--` 之后有一段**没有 `-->` 收尾**的
 *    `<script`)实测必须 FAIL,当前产物实测必须 PASS(见收口批报告 §断言负例实测)。
 *
 * ⚠ 不许用"`<!--` 之后出现 `<script` 即 FAIL"的**朴素版**:`libs.part`(highlight.js)
 *   的**正则字面量**里既有 `<!--` 也有 `<script`,但两者之间隔着 `-->`
 *   ⇒ 状态机早已回到 script-data ⇒ 朴素版会对**当前产物误报**(实测记录见收口批报告)。
 * ========================================================================= */
function scriptDataTrap(body) {
  /* 只关心四类记号;`<script`/`</script` 后面必须是 空白 / `/` / `>` 才算标签名结束 */
  const TOK = /<!--|-->|<script(?=[\s/>])|<\/script(?=[\s/>])/gi;
  let state = 'data';           /* data | escaped | double */
  let m, trap = null;
  while ((m = TOK.exec(body))) {
    const t = m[0].toLowerCase();
    if (t === '<!--') { if (state === 'data') { state = 'escaped'; } }
    else if (t === '-->') { if (state !== 'data') { state = 'data'; } }
    else if (t === '</script') { if (state === 'double') { state = 'escaped'; } }
    else if (state === 'escaped') {          /* <script:escaped ⇒ double-escaped(陷阱入口) */
      state = 'double';
      if (!trap) { trap = m.index; }
    }
  }
  return (state === 'double') ? { at: trap } : null;
}

/* 默认目标来自档位文件(唯一权威);档位文件读不到就报错退出,不猜路径 */
function defaultTargets() {
  const profilesPath = path.join(ROOT, 'scripts', 'pyodide-profiles.json');
  let doc;
  try {
    doc = JSON.parse(fs.readFileSync(profilesPath, 'utf8'));
  } catch (e) {
    console.error('读不到档位文件 scripts/pyodide-profiles.json(' + e.message + ')。'
      + '也可以显式传路径:node scripts/syntax.js <file>');
    process.exit(1);
  }
  return (doc.profiles || []).map(p => path.join(ROOT, p.outFile));
}

let target = process.argv[2];
if (!target) {
  const cands = defaultTargets();
  for (let i = 0; i < cands.length; i++) {
    if (fs.existsSync(cands[i])) { target = cands[i]; break; }
  }
  if (!target) {
    console.error('没找到任何一档产物,依次试过:\n  ' + cands.join('\n  ')
      + '\n先跑 node scripts/build.js(或 --profile=normal / --profile=minimal / --profile=all)生成产物,'
      + '或显式传路径:node scripts/syntax.js <file>');
    process.exit(1);
  }
}
if (!fs.existsSync(target)) {
  console.error('目标文件不存在: ' + target);
  process.exit(1);
}
const html = fs.readFileSync(target, 'utf8');
const re = /<script((?:\s[^>]*)?)>([\s\S]*?)<\/script>/g;
let m, i = 0, bad = 0, skipped = 0, traps = 0;
while ((m = re.exec(html))) {
  i++;
  const attrs = m[1] || '';
  const tm = /type\s*=\s*["']?([^"'\s>]+)/i.exec(attrs);
  const type = tm ? tm[1].toLowerCase() : '';
  const src = m[2];
  if (!src.trim()) continue;
  const line = html.slice(0, m.index).split('\n').length;
  /* 分词器陷阱守卫(先于 V8 解析;对**所有**块的正文都扫,分词器不看 type) */
  const trap = scriptDataTrap(src);
  if (trap) {
    traps++;
    const at = trap.at >= 0 ? trap.at : 0;
    const bodyLine = src.slice(0, at).split('\n').length;
    const ctx = src.slice(Math.max(0, at - 60), at + 60).replace(/\n/g, '\\n');
    console.log('SCRIPT #' + i + ' (starts at html line ' + line + ', type=' + (type || 'js') + ')'
      + ' TRAP: 正文进入 script-data-double-escaped(该块的 </script> 不会被识别)');
    console.log('   `<!--` 之后出现 `<script` 且其间无 `-->`:正文内偏移 ' + at
      + '(正文第 ' + bodyLine + ' 行)');
    console.log('   ...' + ctx + '...');
  }
  if (type && ['text/javascript', 'application/javascript', 'module'].indexOf(type) === -1) { skipped++; continue; }
  try {
    new vm.Script(src, { filename: 'inline-' + i + '.js' });
  } catch (e) {
    bad++;
    console.log('SCRIPT #' + i + ' (starts at html line ' + line + ') ERROR: ' + e.message);
    const mm = /inline-\d+\.js:(\d+)/.exec(e.stack || '');
    if (mm) {
      const n = Number(mm[1]);
      const rows = src.split('\n');
      for (let k = Math.max(0, n - 3); k < Math.min(rows.length, n + 2); k++) {
        console.log('   ' + (k + 1) + ': ' + rows[k].slice(0, 160));
      }
    }
  }
}
const rel = path.relative(process.cwd(), target).replace(/\\/g, '/');
console.log((bad === 0 && traps === 0)
  ? ('语法检查通过(' + rel + ':' + i + ' 段内联脚本,跳过 ' + skipped + ' 段非 JS 载荷,分词器陷阱 0 处)')
  : ('语法错误 ' + bad + ' 处 / HTML 分词器陷阱 ' + traps + ' 处(' + rel + ')'));
process.exit((bad || traps) ? 1 : 0);
