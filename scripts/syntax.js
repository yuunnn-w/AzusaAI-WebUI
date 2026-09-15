/* 语法检查:把一个单文件产物里的内联 <script> 逐个交给 V8 解析(不执行)
   用法:node scripts/syntax.js [path]
   不给路径时取三档产物里第一个存在的(顺序 = scripts/pyodide-profiles.json 的 outFile:
   AzusaAI-WebUI-full.html → -normal.html → -minimal.html,三档都落代码目录根)
   跳过 type 非 JS 的块(<script type="text/plain"> 是内嵌库的二进制/文本载荷,不是 JS) */
const fs = require('fs');
const vm = require('vm');
const path = require('path');
const ROOT = path.resolve(__dirname, '..');

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
let m, i = 0, bad = 0, skipped = 0;
while ((m = re.exec(html))) {
  i++;
  const attrs = m[1] || '';
  const tm = /type\s*=\s*["']?([^"'\s>]+)/i.exec(attrs);
  const type = tm ? tm[1].toLowerCase() : '';
  const src = m[2];
  if (!src.trim()) continue;
  if (type && ['text/javascript', 'application/javascript', 'module'].indexOf(type) === -1) { skipped++; continue; }
  const line = html.slice(0, m.index).split('\n').length;
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
console.log(bad === 0
  ? ('语法检查通过(' + path.relative(process.cwd(), target).replace(/\\/g, '/') + ':' + i + ' 段内联脚本,跳过 ' + skipped + ' 段非 JS 载荷)')
  : ('语法错误 ' + bad + ' 处(' + path.relative(process.cwd(), target).replace(/\\/g, '/') + ')'));
process.exit(bad ? 1 : 0);
