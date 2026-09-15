/* WebBridge 调用助手
   用法:
     node scripts/wb.js eval  <jsfile>    在页面中执行(自动包 try/catch,脚本用 return 返回结果)
     node scripts/wb.js nav   <url>       导航(附带分组)
     node scripts/wb.js shot  <out.png>   截图
     node scripts/wb.js snap              无障碍树
   所有请求经 Node fetch 直发本地守护进程,避免 Windows 命令行中文转义问题。
*/
const fs = require('fs');
const path = require('path');
const SESSION = 'azusaai-webui-verify';
const DAEMON = 'http://127.0.0.1:10086/command';

async function send(action, args) {
  const res = await fetch(DAEMON, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ action, args: args || {}, session: SESSION })
  });
  const txt = await res.text();
  let j = null;
  try { j = JSON.parse(txt); } catch (e) {}
  return j || { ok: false, raw: txt.slice(0, 500) };
}

(async () => {
  const cmd = process.argv[2];
  if (cmd === 'eval') {
    const file = process.argv[3];
    const src = fs.readFileSync(file, 'utf8');
    const code = '(async () => { try { ' + src + '\n } catch (e) { return "ERR: " + (e && (e.stack || e.message || e)); } })()';
    const r = await send('evaluate', { code });
    if (!r.ok) { console.log('FAIL ' + JSON.stringify(r).slice(0, 600)); return; }
    let v = r.data && r.data.value;
    try { v = JSON.parse(v); } catch (e) {}
    console.log(typeof v === 'string' ? v : JSON.stringify(v, null, 1));
  } else if (cmd === 'nav') {
    const r = await send('navigate', { url: process.argv[3], newTab: false, group_title: 'AzusaAI WebUI 验证' });
    console.log(JSON.stringify(r));
  } else if (cmd === 'shot') {
    const r = await send('screenshot', { format: 'png', path: process.argv[3] });
    console.log(JSON.stringify(r));
  } else if (cmd === 'snap') {
    const r = await send('snapshot', {});
    console.log(JSON.stringify(r).slice(0, 3000));
  } else if (cmd === 'pick') {
    const r = await send('find_tab', { url: process.argv[3] || 'http://127.0.0.1:9000/AzusaAI-WebUI-full.html' });
    console.log(JSON.stringify(r).slice(0, 400));
  } else if (cmd === 'tabs') {
    const r = await send('list_tabs', {});
    console.log(JSON.stringify(r).slice(0, 2000));
  } else if (cmd === 'front') {
    const r = await send('cdp', { method: 'Page.bringToFront', params: {} });
    console.log(JSON.stringify(r).slice(0, 300));
  } else if (cmd === 'cdp') {
    let params = {};
    try { params = JSON.parse(process.argv[4] || '{}'); } catch (e) {}
    const r = await send('cdp', { method: process.argv[3], params: params });
    console.log(JSON.stringify(r).slice(0, 800));
  } else {
    console.log('usage: node scripts/wb.js eval|nav|shot|snap ...');
  }
})();
