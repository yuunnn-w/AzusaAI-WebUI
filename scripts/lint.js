/* 静态检查:找出调用了但未声明的标识符(启发式) */
const fs = require('fs');
const path = require('path');
const dir = path.join(__dirname, '..', 'src');
/* 作用域分组:必须逐组独立扫描。
   · appA–appE 是**同一个 IIFE 的分段**(靠函数声明提升共享作用域),合并扫才是对的;
   · officekit.src.js / office-worker.src.js 各自是独立的 IIFE —— 与上面拼成一份文本时,
     A 组里「调用了但未声明」的名字会被 B 组里的同名声明顶替而不报(跨组同名实测 83 个,
     当前无被掩盖的真缺陷,但这是结构性的假阴性通道,所以按组隔离声明集合)。
   两个 office 源文件另有构建期保障(make-office-part.js 的块结构编译探针 + 十条断言),两者互补 */
const GROUPS = [
  { label: 'appA-appJ', files: ['appA.part', 'appB.part', 'appC.part', 'appD.part', 'appJ.part', 'appE.part'] },
  { label: 'officekit.src.js', files: ['officekit.src.js'] },
  { label: 'officewrite.src.js', files: ['officewrite.src.js'] },
  { label: 'office-worker.src.js', files: ['office-worker.src.js'] }
];

const BUILT = new Set(('if,for,while,switch,catch,return,typeof,new,delete,void,in,of,instanceof,do,else,'
  + 'try,function,var,let,const,break,continue,throw,case,default,this,null,true,false,undefined,NaN,Infinity,'
  + 'JSON,Math,Date,Object,Array,String,Number,Boolean,RegExp,Error,Promise,Set,Map,WeakMap,Symbol,'
  + 'parseInt,parseFloat,isNaN,isFinite,encodeURIComponent,decodeURIComponent,encodeURI,decodeURI,'
  + 'alert,confirm,prompt,setTimeout,setInterval,clearTimeout,clearInterval,fetch,btoa,atob,'
  + 'document,window,navigator,localStorage,sessionStorage,console,performance,requestAnimationFrame,'
  + 'matchMedia,URL,Blob,FileReader,FormData,AbortController,AbortSignal,TextDecoder,TextEncoder,'
  + 'ReadableStream,BroadcastChannel,SpeechSynthesisUtterance,NodeFilter,CustomEvent,Event,KeyboardEvent,'
  + 'MouseEvent,screen,DOMPurify,marked,hljs,katex,renderMathInElement,Image,Node,speechSynthesis,'
  + 'Uint8Array,ArrayBuffer,exports,module,define,globalThis,'
  /* 下列是标准浏览器 / JS 全局(启发式白名单:原先漏列,靠"字符串剥离后恰好被吞掉"
     才没报出来 —— HEAD 里的 `new Worker(url)` 同理)。补白名单不是放行真实缺陷:
     Worker 早已在本项目使用(runJsInSandbox),其余都是规范里的全局构造器。
     同因补 `Function`(沙箱语法预检 `new Function(code)`)与
     `Int32Array`(wsCrc32 的查表) —— 同样是剥离逻辑修正后露出来的规范全局 */
  + 'Worker,WebAssembly,DecompressionStream,CompressionStream,Response,'
  + 'Int16Array,Uint16Array,Int32Array,DataView,Function').split(','));

/* 注释与字面量必须**按下标一趟扫描**，各自扫到自己的结束符为止：
   · 分四趟 replace（先剥注释再剥字符串）会让字符串里的 `//`（如 "http://127.0.0.1:3344/mcp"）
     被当成行注释起点，该行引号被截断、后续引号配对整体错位 —— 选择器字符串里的 `:not(`
     与工具说明里的 `Edit(` 之类会被当成「未定义调用」，同时另一些真实代码被误剥掉；
   · 反过来先剥字符串也不行：注释里出现一个引号（`// 别写 "x`）同样会错位；
   · 正则字面量（如 `/'/g`、`/[.*+?^${}()|[\]\\]/`）自带引号与斜杠，必须一并识别，
     否则从它的引号起就再次错位。`/` 是除号还是正则起点按「上一个有意义的字符/词」判定。 */
function stripLiterals(code) {
  let out = '';
  let i = 0;
  const n = code.length;
  let last = '';        // 上一个有意义的非空白字符
  let word = '';        // 上一个标识符（判断 return / typeof 之后的正则）
  let mode = 'code';    // code | tpl
  let brace = 0;        // 当前代码段里未闭合的 {
  const tplStack = [];  // 模板插值 `${` 的层：进 `${` 时压入外层 brace
  const WORD_RE = /[A-Za-z0-9_$]/;
  /* 这些词后面出现的 `/` 是正则字面量的起点，不是除号 */
  const REGEX_AFTER_WORD = /^(?:return|typeof|instanceof|in|of|new|delete|void|do|else|case|throw|yield|await)$/;
  const regexOk = () => {
    if (!last) return true;
    if ('(,=:[!&|?{};+-*%~^<>'.includes(last)) return true;
    return !!word && REGEX_AFTER_WORD.test(word);
  };
  while (i < n) {
    const c = code[i], c2 = code[i + 1];
    if (mode === 'tpl') {
      if (c === '\\') { i += 2; continue; }
      if (c === '`') { mode = 'code'; i++; last = '`'; word = ''; continue; }
      if (c === '$' && c2 === '{') { tplStack.push(brace); brace = 0; mode = 'code'; i += 2; last = '{'; word = ''; continue; }
      i++;
      continue;
    }
    if (c === '/' && c2 === '/') { while (i < n && code[i] !== '\n') i++; out += ' '; continue; }
    if (c === '/' && c2 === '*') {
      i += 2;
      while (i < n && !(code[i] === '*' && code[i + 1] === '/')) i++;
      i = Math.min(n, i + 2);
      out += ' ';
      continue;
    }
    if (c === '"' || c === "'") {
      i++;
      while (i < n) {
        const d = code[i];
        if (d === '\\') { i += 2; continue; }
        if (d === c) { i++; break; }
        if (d === '\n' || d === '\r') break;   // 未闭合：不吞掉后面的代码
        i++;
      }
      out += ' '; last = c; word = '';
      continue;
    }
    if (c === '`') { mode = 'tpl'; i++; last = '`'; word = ''; continue; }
    if (c === '/' && regexOk()) {
      let j = i + 1, inClass = false, closed = false;
      while (j < n) {
        const g = code[j];
        if (g === '\\') { j += 2; continue; }
        if (g === '\n' || g === '\r') break;
        if (inClass) { if (g === ']') inClass = false; }
        else if (g === '[') inClass = true;
        else if (g === '/') { closed = true; break; }
        j++;
      }
      if (closed) {
        i = j + 1;
        while (i < n && /[a-z]/.test(code[i])) i++;
        out += ' '; last = '/'; word = '';
        continue;
      }
    }
    if (c === '{') brace++;
    else if (c === '}' && tplStack.length) {
      if (brace > 0) brace--;
      else { brace = tplStack.pop(); mode = 'tpl'; i++; last = '}'; word = ''; continue; }
    } else if (c === '}' && brace > 0) brace--;
    if (WORD_RE.test(c)) word += c;
    else if (c.trim()) word = '';
    if (c.trim()) last = c;
    out += c;
    i++;
  }
  return out;
}

/* 逐组独立分析(见文件头 GROUPS 的说明):声明集合按组隔离,跨组同名不互相背书。
   所有分析都跑在「去掉注释与字面量」的 clean 上：字面量内容既不该被算成声明，
   也不该被算成调用（否则字符串里一句 `function f()` 就能把真实缺陷掩盖掉）。 */
const declared = new Set();
const unknown = new Map();
GROUPS.forEach(g => {
  const clean = stripLiterals(g.files.map(f => fs.readFileSync(path.join(dir, f), 'utf8')).join('\n'));
  const decl = new Set();
  let m;
  let re = /\bfunction\s+([A-Za-z_$][\w$]*)/g;
  while ((m = re.exec(clean))) decl.add(m[1]);
  re = /\bvar\s+([A-Za-z_$][\w$]*)/g;
  while ((m = re.exec(clean))) decl.add(m[1]);
  re = /function(?:\s+[A-Za-z_$][\w$]*)?\s*\(([^)]*)\)/g;   /* 具名函数的形参也要算已声明 */
  while ((m = re.exec(clean))) {
    m[1].split(',').forEach(a => {
      a = a.trim();
      if (/^[A-Za-z_$][\w$]*$/.test(a)) decl.add(a);
    });
  }
  re = /\bcatch\s*\(\s*([A-Za-z_$][\w$]*)/g;
  while ((m = re.exec(clean))) decl.add(m[1]);
  decl.forEach(n => declared.add(n));
  re = /(?:^|[^.\w$])([A-Za-z_$][\w$]*)\s*\(/g;
  let mm;
  while ((mm = re.exec(clean))) {
    const n = mm[1];
    if (decl.has(n) || BUILT.has(n)) continue;
    /* 输出带作用域前缀:同名标识符在不同组里是两个彼此独立的判定 */
    const key = g.label + ' | ' + n;
    unknown.set(key, (unknown.get(key) || 0) + 1);
  }
});
console.log('声明数量:', declared.size);
console.log('可能的未定义调用:');
[...unknown.entries()].sort((a, b) => b[1] - a[1]).forEach(([k, v]) => console.log('  ' + k + ' x' + v));
