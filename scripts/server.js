/* 极简静态服务器,供 WebBridge 在真实浏览器中访问 */
const http = require('http');
const fs = require('fs');
const path = require('path');
const ROOT = path.resolve(__dirname, '..');
/* 默认 9000;8090–8123 是本机 Windows 保留端口段,别用 */
const PORT = Number(process.argv[2] || 9000);
/* 访问 / 时给完整版产物:**名字的单一权威 = scripts/pyodide-profiles.json 里 full 档的 outFile**
   (改名后这里曾写死第二份字面量,再改名就会静默 404)。
   读不到就置 null:启动时报错、请求 / 回 500 —— 不内置回退字面量,否则又出现一处产品名副本,
   改产物名时会继续埋同一个坑。 */
let DEFAULT_FILE = null, DEFAULT_FILE_ERR = '';
try {
  const doc = JSON.parse(fs.readFileSync(path.join(ROOT, 'scripts', 'pyodide-profiles.json'), 'utf8'));
  const full = (doc.profiles || []).filter((p) => p.id === 'full')[0];
  if (full && typeof full.outFile === 'string' && full.outFile) DEFAULT_FILE = full.outFile;
  else DEFAULT_FILE_ERR = '档位文件里没有 full 档的 outFile';
} catch (e) {
  DEFAULT_FILE_ERR = '读档位文件失败: ' + e.message;
}
if (!DEFAULT_FILE) {
  console.error('! ' + DEFAULT_FILE_ERR + ' —— 访问 / 会回 500;其他产物仍可按完整路径访问。');
}
const MIME = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8',
               '.css': 'text/css; charset=utf-8', '.json': 'application/json', '.png': 'image/png',
               '.svg': 'image/svg+xml', '.woff2': 'font/woff2', '.ico': 'image/x-icon' };
http.createServer((req, res) => {
  /* 畸形百分号转义(如 /%zz)会让 decodeURIComponent 抛 URIError;本地验证服务器没有外层
     try/catch,一个坏请求就能把进程带走(重跑脚本时才看得出来)——先挡成 400 */
  let p;
  try { p = decodeURIComponent(req.url.split('?')[0]); }
  catch (e) { res.writeHead(400); res.end('bad request'); return; }
  if (p === '/' && !DEFAULT_FILE) {
    res.writeHead(500, { 'Content-Type': 'text/plain; charset=utf-8' });
    res.end('server.js: ' + DEFAULT_FILE_ERR);
    return;
  }
  const f = path.join(ROOT, p === '/' ? DEFAULT_FILE : p);
  /* 前缀比对要带路径边界:ROOT=C:\a\b 时 p=../bc/x 会解析成 C:\a\bc\x,只判 startsWith 会放行 */
  if (f !== ROOT && !f.startsWith(ROOT + path.sep)) { res.writeHead(403); res.end('forbidden'); return; }
  fs.readFile(f, (e, buf) => {
    if (e) { res.writeHead(404); res.end('not found'); return; }
    res.writeHead(200, { 'Content-Type': MIME[path.extname(f).toLowerCase()] || 'application/octet-stream',
                         'Cache-Control': 'no-store' });
    res.end(buf);
  });
}).listen(PORT, '127.0.0.1', () => console.log('serving ' + ROOT + ' at http://127.0.0.1:' + PORT + '/'));
