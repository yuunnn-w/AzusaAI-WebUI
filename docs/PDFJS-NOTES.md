# pdf.js / PDFKit 内嵌产物笔记

> 状态：现行 —— 内嵌 pdf.js 的取件、ESM→classic 改写与 `file://` 加载约束的唯一记录；升级库版本或改动 `src/pdfjs.part` 前必读
> 更新：2026-09-19 · 载荷段 `src/pdfjs.part`（**1,882,880 B**，三档产物共用；口径 = `stat -c %s src/pdfjs.part`，2026-09-18 现取——该段由 netdocs-B 重生成后未再变）· 生成脚本 `scripts/make-pdfjs-part.js`（幂等 + 自检）
> 上游版本：pdfjs-dist 6.3.289（`legacy` 构建，已修复 CVE-2024-4367）

## 1. 结论速览

| 项 | 值 |
|---|---|
| pdf.js 版本 | **pdfjs-dist 6.3.289**(`legacy` 构建) —— **已修复 CVE-2024-4367**(该漏洞影响 ≤4.1.392) |
| 原始包 | `vendor/pdfjs-src/pdfjs-dist-6.3.289.tgz`(registry.npmjs.org);源文件 `vendor/pdfjs-src/legacy/build/pdf.min.mjs` 518,555 B、`pdf.worker.min.mjs` 1,317,034 B |
| 产物 | **`src/pdfjs.part` = 1,882,880 bytes**（2026-09-18 磁盘实测；md5 `befc7cafea8e5b469df4e3542c7285d9`；该段由 netdocs-B 重生成后未再变） |
| 结构 | 3 个 `<script>` 块:display 库 → `<script type="text/plain" id="pdfjs-worker-src">` worker 源码 → PDFKit 包装层 |
| 体积影响 | 相对增量 **+1.85 MB**(上版 3.11.174 是 +1.53 MB) |
| 测试 | 一次性测试件 → **150 项断言全 PASS**（脚本未随仓库保留，见 §5） |
| API | 与上一版**完全一致**,无任何契约变更(见 §4) |

旧版 3.11.174（含 CVE-2024-4367 漏洞）不保留在 `vendor/pdfjs-src/`。

取件：`npm pack pdfjs-dist@6.3.289`（或直接从 registry.npmjs.org 下载 tgz），解出 `legacy/build/{pdf.min.mjs,pdf.worker.min.mjs}` 与 LICENSE 放到 `vendor/pdfjs-src/`，再跑 `node scripts/make-pdfjs-part.js` 生成载荷。

**CVE-2024-4367 修复的直接证据**(对产物做静态扫描):6.3.289 的 display 与 worker 里
`eval(` **0 处**、`new Function(` **0 处**(唯一命中的 `new FunctionBasedShading(...)` 是类名),
且 v4.2+ 已把 `isEvalSupported` 选项整个删掉(`isEvalSupported` 在产物中出现 0 次)。
该 CVE 的成因就是 pdf.js 用 `Function()` 处理 PDF 里的字体矩阵;6.x 不再有这条路径。
包装层仍照旧传 `isEvalSupported: false`(v6 会忽略,纯属向上/向下兼容的冗余保险)。

### 版本取舍(实测产物大小)

| 版本 | legacy 原始大小 | 生成的 `src/pdfjs.part` | 官方 legacy 浏览器目标 | 结论 |
|---|---|---|---|---|
| 4.10.38(最低可接受 4.2.67 之上的 4.x 末版) | 1.82 MB | 1,832,578 B | Chrome ≥ 98 级 | 不用:比 5.x 大、比 6.x 旧 |
| 5.7.284(5.x 最新) | 1.78 MB | **1,800,792 B**(最小) | Chrome ≥ 118 / Safari ≥ 16.4 | 备选:兼容面更宽 |
| **6.3.289(最新,本次采用)** | 1.84 MB | **1,882,880 B**(2026-09-18 现取;表中另两行为 2026-09-14 读数) | Chrome ≥ 125 / Safari ≥ 18 | 最新;不比 5.x 明显更大(量级结论不变) |

结论:**6.x 并没有比 5.x 明显更大**(远低于“>3MB”的担忧),所以按“最新版优先”选了 6.3.289。
如果要把兼容面从 Safari 18 降到 16.4,只需把 `vendor/pdfjs-src/legacy/build/` 换成 5.7.284 的两个文件再跑一次
`node scripts/make-pdfjs-part.js`(生成器已实测三版都能跑通),产物 ≈1.80 MB(2026-09-14 读数;再生成请以当次打印值为准),其余一切不变。

## 2. v4+ 没有 UMD:ESM → classic 的机械化改写

`pdf.min.mjs` / `pdf.worker.min.mjs` 是 ESM,内联进 classic `<script>` 会直接语法报错(实测
`new Function('export{a as b}')` → `Unexpected token 'export'`)。`scripts/make-pdfjs-part.js` 做确定性改写:

1. **结尾 `export{...}`** → `window.pdfjsLib = {导出名: 局部名, ...}`(display)/ `globalThis.pdfjsWorker = {...}`(worker)。
   6.3.289 的 display 导出 62 项,worker 1 项(`WorkerMessageHandler`)。
2. **`import.meta.url`**(display ×2、worker ×2,都在 Node canvas / emscripten JBig2-OpenJPEG 胶水里)→ 预置变量 `__pdfjsModuleUrl`
   (页面取 `document.baseURI`,worker 取 `location.href`)。classic script 里 `import.meta` 是**语法错误**
   (实测 `new Function('import.meta.url')` → `Cannot use 'import.meta' outside a module`)。
3. **动态 `await import(x)`**(display 1 处 = fake worker 兜底加载 workerSrc;worker 1 处 = wasmUrl 下的
   JBIG2/OpenJPEG 模块)→ 预置函数 `__pdfjsFakeWorkerImport(spec)` / `__pdfjsWasmImport(spec)`。前者会
   “把内联 worker 源码在主线程跑一次再交付 WorkerMessageHandler”,后者干净 reject(pdf.js 侧本就 try/catch,
   而且我们用 `useWasm:false` 关掉了这条路径)。
4. **整体包进 `(function () { "use strict"; ... })()`** —— 等价于 ESM 的模块作用域 + 严格模式(ESM 恒为严格模式)。

唯一的“残留”:display 里 `_createCDNWrapper` 内有一句**模板字符串** `` `await import("${t}");` ``(跨源 workerSrc 才会生成的
wrapper,我们的流程永远走不到)。生成器对该串做白名单校验,并断言其它位置 `import(...)` 调用数为 **0**。

### 生成器自检(任一条不过就不写盘)

- 每个文件 `export` 关键字只出现 1 次,且必须是最后一条语句(export 之后除 `;` 外无内容);
- 导出名/局部名必须是普通标识符;display 必须含 `getDocument/version/GlobalWorkerOptions/PDFWorker/AbortException/Util/OPS`,
  worker 必须含 `WorkerMessageHandler`,且产物里必须有 `globalThis.pdfjsWorker =`;
- **长度账**:`改写后长度 === 原文长度 + Σ(替换串长 − 原串长)`,即除 export 尾句外只发生了这些定点替换;
- **上下文窗口**:每个替换点的前 48 字符与后 48 字符在产物中原样存在;正则单次匹配不得超过 160 字符(防越界吞代码);
- 产物 `new Function(script)` 必须能编译(V8;浏览器侧同一结果——页面能正常跑就是证明);
- `</script` / `<!--` 转义检查 + script 开闭标签数一致 + 转义可逆(逐字节还原);
- 幂等:两次运行 md5 相同。

## 3. worker 加载方式(http 与 file 都实测)

- **http(s)://** → `mode = "worker"`。自己做 `new Worker(URL.createObjectURL(new Blob([workerSrc])))`(**classic worker**),
  然后 `new pdfjsLib.PDFWorker({ port: w })` 交给 `getDocument({worker})`,整页复用同一个 worker。
- **file://** → **同样是 `mode = "worker"`**(这次不再是主线程!)。实测结论:file:// 下
  **classic blob worker 可用**,而 **module blob worker 会构造成功但运行失败**
  (`ERR onerror` / Chromium "Not allowed to load local resource")。pdf.js v4+ 内部固定
  `new Worker(workerSrc, {type:"module"})`,所以在 file:// 会踩坑 —— 这正是我们自己建 classic worker 并用
  `workerPort` 绕过它的原因。
- **兜底**:`new Worker(...)` 构造失败(或 `PDFWorker` 构造抛错)→ 把 `#pdfjs-worker-src` 的文本塞进一个临时
  `<script>` 在主线程执行,得到 `window.pdfjsWorker.WorkerMessageHandler`;pdf.js 的
  `#mainThreadWorkerMessageHandler` 检查会自动走官方 fake worker 分支(`mode = "main-thread"`)。
  两条路都失败才 `mode = "none"`,`available` 仍为 true 但调用返回 `{ok:false,error}`。
- 为什么不用 pdf.js 内部的 workerSrc 路径:它会做 `_isSameOrigin`(用到很新的 `URL.parse`)并在跨源时包一层
  module wrapper(内含 `await import(...)`,file:// 下不工作)。用 `workerPort` 完全绕开。

## 4. API(与上一版逐字一致,没有变化)

```js
window.PDFKit = {
  available, why, version,
  extractText(arrayBuffer, opts) -> Promise<{ok,text,pages,chars,truncated,error?}>,
  renderPages(arrayBuffer, opts) -> Promise<{ok,images:[{dataUrl,width,height,page}],pages,truncated,error?}>,
  mode(), setupWorker()                      // 非契约的附加方法(一直都有)
}
```
`opts` 默认值不变:`{ maxPages:50, dpi:150, format:"image/jpeg", quality:0.85, onProgress, maxChars:400000 }`,
额外支持 `opts.timeout`(默认 60000,超时返回 `{ok:false,error:"处理超时(>Xms)"}` 并 destroy 文档)。
入参除 `ArrayBuffer` 外仍接受 `Uint8Array` 与 base64/dataURL 字符串(内部复制,不会 detach 调用方 buffer)。
返回对象仍带附加字段 `pageCount / ms / mode`。

**唯一的行为变化(对上层是好事)**:file:// 下 `PDFKit.mode()` 从 `"main-thread"` 变成 `"worker"`——不再阻塞 UI 线程。

新增的内部细节(不影响契约):`getDocument` 增加 `useWasm:false`,避免 JBIG2/JPEG2000 去外部抓 `.wasm`
(单文件零请求;这两类图走 pdf.js 内置的 JS 解码器)。

## 5. 测试证据

```
node scripts/make-pdfjs-part.js   # 生成 src/pdfjs.part（幂等 + 自检）
# 以下三个是一次性测试件，已删除（结果见下表，要复现请按 CONTRIBUTING.md「验证」现写）：
#   测试 PDF 生成 / 测试页生成 / CDP 无头测试运行器（6 模式 + degrade）
```

| 模式 | 结果 | 关键数字 |
|---|---|---|
| `http://127.0.0.1:9111/libtest-pdfjs.html` | PASS 27/27 | mode=worker;extract 77 ms;render 79 ms;1241×1753 |
| `file:///…/libtest-pdfjs.html`(一次性测试件的 `.build/` 路径) | PASS 27/27 | mode=worker;extract 68 ms;render 72 ms;1241×1753 |
| http + `Network.emulateNetworkConditions offline:true` 重跑 | PASS 27/27 | 期间请求记录 **0 条** |
| file:// + 页面加载前即 offline:true | PASS 27/27 | 唯一请求 = file:// 页面自身 |
| 集成：part 替换进产物副本（当时的产物名 `index.html`，现为 `AzusaAI-WebUI-full.html`；http / file） | PASS 5/5 ×2 | 应用无报错；PDFKit 6.3.289 抽取 + 渲染正常 |
| degrade:display 块换成语法错误 | PASS 5/5 | 后续脚本照跑;`available=false, why="缺少 pdfjsLib"`;API 返回 `ok:false` 不抛错 |

关键断言(每个页面对话环境都跑一遍):

- `pdf.js >= 4.2.67(CVE-2024-4367 已修复)` → `version=6.3.289`;
- 文本含 `Hello PDF 12345` 与中文 `中文测试`(Chrome 生成的 PDF 带 ToUnicode);
- 150 DPI A4 = **1241×1753**(理论 1240.2×1754.0,**±1%** 断言通过);100 DPI = **827×1169**(理论 826.8×1169.3,±1%);
- dataUrl 前缀 `data:image/jpeg;base64,` / `data:image/png;base64,`;页分隔 `----- 第 N 页 -----`;maxChars/maxPages 截断;
- 坏数据 / `null` / `timeout:1` 都返回 `{ok:false,error}` 不抛错;页面无未捕获错误。

抽出的文本前 120 字(所有环境一致):

```
----- 第 1 页 -----
Hello PDF 12345 中文测试
English line: The quick brown fox jumps over the lazy dog.
中文行:这是一个用于 pdf.js 测试的
```

> 集成测试会先把**产物里**已有的旧 part 整段剥掉再拼新版（实测剥离 1,799,712 B），
> 即模拟“替换”。**主 agent 必须替换、不能追加**:页面上同时存在两份 pdf.js 会出现
> `The API version "3.11.174" does not match the Worker version "6.3.289"` 这种失败。

## 6. 兼容性(重要)

pdf.js 6.3.289 官方 Babel/Browserslist 目标(上游 `gulpfile.mjs` 的 `ENV_TARGETS`):
`last 2 versions, Chrome >= 125, Firefox ESR, Safari >= 18, Node >= 22`。
legacy 构建额外打包 **core-js 3.50.0** 自动打补丁(`Promise.withResolvers`、`Uint8Array.prototype.toHex/toBase64` 等),
所以真正的门槛是**语法**(私有类字段/方法、类 static 块、`?.`/`??`、逻辑赋值等)。

- 实测通过环境:**本机 Chrome(headless,当前版本)**。
- 基线从上一版的 Chrome 88+ 提到 **Chrome ≥125 / Safari ≥18 / Firefox ESR**(按 pdf.js 官方 legacy 目标),这是
  用户明确允许的;若需要更低门槛请换 5.7.284(Chrome ≥118 / Safari ≥16.4)。
- **优雅降级已实测**:把库块换成语法错误(等价于老浏览器解析失败)→ 同页其它脚本照常执行,
  `PDFKit.available === false`、`why === "缺少 pdfjsLib"`,调用返回 `{ok:false,error}`,不产生未捕获错误。
- **未验证**:Chrome 125 之前、Safari、Firefox、移动端**都没有实测**;`PDFKit.available` 门控必须由上层实现
  (低于门槛的浏览器仍能正常用应用其它功能,只是 PDF 功能不可用)。
- **补充(2026-09-15,见 §9)**:缺 `ReadableStream` 异步迭代的宿主(Chromium<124,如用户真机 122)上的
  取文本崩溃已修并**实测可用(垫片 + 手动 reader 泵)**;**基线档位不变** —— 122 **不**升格为官方支持基线,
  仍归上面这条"未验证"档(上面的实测是**能力注入模拟**,不等于真机跑过)。

## 7. 已知限制 / 坑

1. **没有内联 cMap / standard_fonts / wasm**:依赖 Adobe CJK CMap 且无 ToUnicode 的老 PDF 抽取会退化;
   用 14 种标准字体的 PDF 用系统字体替身渲染;`useWasm:false` 让 JBIG2/JPEG2000 走内置 JS 解码器
   (未用这类图做实测)。
2. 扫描件/纯图片 PDF 抽不到文本(无 OCR),属预期。
3. 体积 +1.85 MB;单文件 HTML 相应变大。
4. 超时只是“放弃结果 + destroy 文档”,内部任务不保证立刻中断。
5. worker 为整页复用(一个 document 的 destroy 不会杀掉它);极端情况下 worker 崩了会走
   `__pdfjsFakeWorkerImport` 兜底(把 worker 源码在主线程跑一次),再不行才报错。
6. 严格 CSP(无 `unsafe-inline`)会让主线程兜底与 worker blob 失败 → `mode="none"`,API 返回 `ok:false`(未测 CSP)。
7. 端口:9111(HTTP)/ 9800-9899(CDP);本机 8090-8123 被 Windows 保留。

## 8. 文件清单（现行布局）

| 文件 | 说明 |
|---|---|
| `vendor/pdfjs-src/` | `pdfjs-dist-6.3.289.tgz`、`legacy/build/{pdf.min.mjs,pdf.worker.min.mjs}`、LICENSE、package.json.orig、测试 PDF 及源 HTML（不进版本库） |
| `src/pdfjs.part` | **最终内嵌片段（1,863,174 B；含 §9 的流异步迭代垫片 + 手动 reader 泵）** |
| `scripts/make-pdfjs-part.js` | ESM→classic 机械化改写 + 自检 + 转义，幂等 |

- **`ReadOffice` 工具复用本引擎**：`ReadOffice` 对 PDF 先 `extractText`（**文本优先，不吃附件图片策略（`attachImgMode()`）设置** —— 读文档的契约是"读内容"），只在确实没有文字层（分页标记 `----- 第 N 页 -----` 不算文字）时才 `renderPages`（页数 = `min(pdfMaxPages(), READ_PDF_IMG_MAX_PAGES=4)`，参数与附件图片模式同参）→ 支持视觉就附页图、否则逐页 OCR；渲染与文本抽取都直接调 `PDFKit`，不走 `resolvePdf` 包装。**`Read` 自己不再读 PDF**：遇 `.pdf` 返回 `EINVAL` 重定向到 `ReadOffice`（按后缀给词 + 可照抄的 `path` JSON），不产生任何解析副作用。

能力探针（实测 module/classic worker、blob、动态 import、`new Function` 在 http/file 下的差异，§3 的设计依据）与一次性测试件（`mk-test-pdf.js` / `mk-libtest-page.js` / `libtest-pdfjs.js` / `pdfjs-cdp.js` / `libtest-pdfjs.html` / `pdfjs-test-log.txt`）及旧 `.build/` 目录均已删除；§5 的数字是当时的原始结论（仓库不保留回归脚本，理由见 `CONTRIBUTING.md`「验证」）。

## 9. 流异步迭代兼容（Chromium<124 / 旧 Safari；含实测口径）

**症状**：宿主缺 `ReadableStream.prototype[Symbol.asyncIterator]`（Chrome 124 才有）时，`PDFKit.extractText()` 必抛
`TypeError: e is not async iterable` —— pdf.js 6 的 display `getTextContent()` 内部是
`for await (const t of this.streamTextContent())`。用户真机 Thorium Legacy `M122.0.6261.171`（Chromium 122 / Windows 7）
2026-09-15 报出该错，PDF 附件与 `Read` 的文本抽取全部不可用（当时 PDF 由 `Read` 读；该路径现已属 `ReadOffice`）。

**修法（两处，都在 `make-pdfjs-part.js` 的模板里；`src/pdfjs.part` 只经脚本重生成）**
1. **手动 reader 泵**（热路径，`PDFKIT_JS`）：`extractText` 不再调 `getTextContent()`，改走 `pageTextViaReader()`
   → `page.streamTextContent()` + `drainStream()`（逐个 `reader.read()` 拉完流）。合并语义与 pdf.js 原文逐条等价
   （`items` 顺序 push、`styles` 逐键拷贝、`lang` 对齐 `i.lang ??= t.lang`）；失败/中止先 `reader.cancel()` 再 `releaseLock()`。
2. **原型垫片**（兜底，`DISPLAY_PRELUDE` 与 `WORKER_PRELUDE` 各一份）：宿主缺该能力时给
   `ReadableStream.prototype[Symbol.asyncIterator]` 补一个符合规范的实现（`next`/`return`；有则不动、整段 `try` 包裹）；
   打补丁**前**的原生能力快照写全局 `__pdfStreamIterNative`，供 `PDFKit.diag()` 判定。
3. **两处口径细节**（批 B-Ⅰ 收口）：① 能力快照是**首次写入语义**（`typeof … === "undefined"` 才写）——主线程降级时
   worker 载荷会被二次执行，不守卫就会把「已垫片」谎报成「原生」；② 垫片迭代器已**对齐原生**
   （`[Symbol.asyncIterator]` 自引用可迭代、`return()` 在 `cancel()` 落定后释放读锁）。

**实测口径（2026-09-15，无头 Chrome + 能力注入；注入 = `delete ReadableStream.prototype[Symbol.asyncIterator]`）**

| 场景 | `typeof RS.prototype[Symbol.asyncIterator]` | `extractText(vendor/pdfjs-src/test.pdf)` | `PDFKit.diag().streamIter` |
|---|---|---|---|
| 注入 + **修前**产物 | `"undefined"` | **`ok:false`** · `TypeError: e is not async iterable` | —（当时无 `diag()`） |
| 注入 + **修后**产物 | `"function"`（垫片补上） | **`ok:true`** · 323 字符 / 2 页 / 91 ms | `{native:false, shimmed:true, worker:"static-inferred"}` |
| 不注入 + 修前产物（对照） | `"function"` | `ok:true` · 323 字符 / 2 页 / 98 ms | — |
| 不注入 + 修后产物（对照） | `"function"` | `ok:true` · 323 字符 / 2 页 / 99 ms | `{native:true, shimmed:false, worker:"static-inferred"}` |

- **取文本逐字节不变**：`vendor` 测试 PDF（323 字符）/ 手工「文本+图」PDF（85 字符）/ Word 导出 PDF（130 字符）三份夹具，
  修前与修后（注入与不注入两态）抽出的**整段文本逐字节相同**（FNV-1a：`9272822b` / `e3bc2762` / `c564579a`）。
- **渲染路径**：同一注入环境下 `renderPages()` 与 `page.getOperatorList()` 均 `ok:true`（`vendor` 测试 PDF 页 1 无图像 op）。
- **结论口径**：本引擎在缺该 API 的宿主上**实测可用（含垫片 + 手动 reader 泵）**。**这不等于把 Chromium 122 列为官方
  支持基线** —— pdf.js 6 官方目标仍是 Chrome ≥125 / Safari ≥18 / Firefox ESR（§6），122 上仍可能存在其它未实测差异（见下）。

**覆盖边界（不得当已验证）**
- 垫片覆盖 **pdf.js display realm + pdf.js worker realm**，且作用点是 **realm 的 `ReadableStream.prototype`** 而非某个库 ——
  主 realm 内**任何**库只要碰同一原型就一并被覆盖。因此 `src/office.part` 内 docstream 自带的那份 pdf.js
  **只在 office worker realm 不在覆盖内**（那份跑在自己的 worker 里）；一旦 office 解析降级到主线程
  （无 `Worker` 时），它就在主 realm 里跑，**同受垫片覆盖** —— 别据此推断「主线程降级的 office pdf 路径一定挂」。
  它的唯一同类调用点 `static async decompressSignature` 属注释编辑器/签名墨迹路径，本应用不可达。
- worker realm **注入不到**（CDP `Page.addScriptToEvaluateOnNewDocument` 只作用于主文档）⇒ worker 侧结论是**静态推导**
  （`WORKER_PRELUDE` 与 worker 体同 IIFE ⇒ 同 realm）。`diag().streamIter.worker` 恒为 `"static-inferred"`，面板与文档文案只说
  「宿主（主线程）」，不声称 worker 已实测。
- `Util.applyTransform(p, m)` 在 6.3.289 legacy 里是**原地改写入参并返回 `undefined`**（不是「返回新点」）—— 取四角算几何时
  要传副本、读回传出的数组（写几何实现时踩过一次，报 `Cannot read properties of undefined (reading '0')`）。
- `pageTextViaReader` 旁路了原文首句 `if (this._transport._htmlForXfa) …`；本应用 `openDoc` 不启用 XFA ⇒ 影响可忽略。
- 垫片**未实现 `throw()`**（`for await` 正常路径只用 `next()`/`return()`，有意简化）。
- **实测（真机）**：**Thorium `M122.0.6261.171`（Chromium 122）上可用** —— 启动 / 诊断面板 / `PDFKit.extractText`（`{ok:true,pages:2,chars:323}`）/
  `pageImages`（`mode:"rect"`）/ `renderCrops` / 流式对话 / 附件侧 OCR 融合（请求体含两个暗号、块头与 §10 逐字相符）都在 122 上跑通
  （原始读数 `shared/progress/thorium-122-verify-done.md`）。口径同 `README.md`：**「实测可用（含垫片 + 手动 reader 泵）」，不等于把
  Chromium 122 升格为官方支持基线**（官方目标仍是 Chrome ≥125 / Safari ≥18）。
- **仍未实测**：**Windows 7 本体**（本机 = Win11 + Chromium 122 内核）；Safari / Firefox / 移动端；worker realm 内的实测（仍是静态推导）；
  `normal` / `full` 档的浏览器行为 —— **本条已改准(2026-09-16)**：122 真机上**三档都跑过整轮**（`shared/progress/netdocs-final-122-verify.md`：
  6 轮 × 逐项 = 264 项、FAIL = 0；`full` 42/42、`normal` 同，只有「真实模型端到端」一项只在 `minimal` 跑）。此前记的"122 真机上只抽验过
  `minimal` 档 / `full` 档那次读数取自更早快照且在并发事故中作废、事后未复跑"（`shared/progress/thorium-122-verify-done.md` §15）
  是**那一路上当时**的实况，已被上述独立复测取代。**仍未覆盖**的只剩：那套读数对应 02:05–02:06 那次构建，
  **02:37 之后重编的现盘产物只做了静态标记核**（同报告 §一/§六）。

`PDFKit.diag()` 的读数同时出现在 设置 → 环境 的「诊断」面板（`· PDF 引擎:pdf.js 6.3.289 · worker · 取文本:reader ·
流迭代:原生/已垫片 · 取图:可用/不可用 · 图文融合:<`fuseStateText()` 的返回值>`）；「图文融合」的取值逐字来自 `src/appD.part` 的
`fuseStateText()`，共五种：`不可用(取图能力缺失)` / `已启用(当前模型不支持图像输入)` / `不可用(本机 OCR 引擎不可用)` /
`未启用(当前模型支持图像输入)` / `未启用(图像能力未知,还没探测到该模型;可在 设置 → 模型 把该模型的图像能力设为「不支持」后启用 OCR 融合)`。
`PDFKit.notes` 给出「仅提示、不拦」的环境备注（宿主缺失时会写明已垫片）。
「取图」与「图文融合」是两件事：前者只是 `PDFKit.renderCrops` 的能力在位，后者才表示**现在真的会做** OCR 融合
（判据 = 取图能力 + 本机 OCR 引擎可用 + 当前模型不支持图像输入，见 §10）。

## 10. 图文按序融合（非视觉模型：图 → 本机 OCR → 按文档顺序插进文本）

**需求**：模型不支持图像输入时，文档里的图片内容也要进上下文——既不能只发文字，也不能把图丢掉。

**链路**：`PDFKit.pageImages()` 扫页图位置（Route C 几何：CTM 下的单位方，见 §2.1c 契约与 `shared/progress/netdocs-B1-done.md`）
→ `PDFKit.renderCrops()` 按区取图（失败单调降级到整页 `renderPages`，**必须注记**，不静默）→ 应用层用内嵌 OCRKit 逐张识别
→ 按页插进 PDF 文本。ReadOffice 侧（`ReadOffice("/x.pdf")`）与附件侧（选文件 → `resolvePdf`）**同一套助手、同一套块格式**：
`ocrSeqCands`（逐张 `wsImgFitForModel` → `wsOcrTextOf` + 预算/中止 + 计数，**不做任何文案**）、
`ocrBlockText`（块头唯一来源，空 / 纯空白 ⇒ `""`，不写空块）、`docFusePages`（按页插块；无块 ⇒ 原样返回）——
三个都在 `src/appD.part`，`appE.part` 的 `wsOcrCands`（旧的「整页无文字层 ⇒ OCR」路径）内部改调 `ocrSeqCands`，
**对外文案逐字保留**（旧注记 `【第 N 页】` 与融合注记 `【第 N 页 · 图 k · 本机 OCR】` 双轨有意保留，别顺手统一）。

**块格式（唯一口径）**

```
----- 第 1 页 -----
<该页文本>

【第 1 页 · 图 1 · 本机 OCR】
<识别文本>
```
- 整页兜底（`mode:"full"` 的页，或 `renderCrops` 失败）：`【第 N 页 · 整页图像 OCR(含文字层重复,可能有误差)】`——
  这条**不带**「· 本机 OCR」（label 里已写明），实现上是 `kind:"full"` 的唯一区别。
- 识别为空：不写空块，只累计「N 图 OCR 未识别到文字」。
- 匹配不到页号的块：追加文末并注明 `(原页未在文本中找到)` + 记一条注记（不静默丢弃）。

**预算（单一来源）**

| 常量 | 值 | 语义 |
|---|---|---|
| `READ_DOC_OCR_MAX_IMAGES` | 12 | ReadOffice 侧单次最多 OCR 张数 |
| `READ_DOC_OCR_MAX_PAGES` | 8 | ReadOffice 侧只扫描前 8 页的图片（文本仍按 `extractText` 的 `maxPages` 取） |
| `READ_OCR_TIMEOUT_MS` | 120 s | **整次读取**共享的 OCR 总预算（`Read` 读图片与 `ReadOffice` 读文档图片共用同一常量） |
| `ATT_IMG_MAX_IMAGES` | 10 | **附件侧**单次最多处理张数（0.2.1 起：取图计划与 OCR 融合**共用同一上限**；批前名 `ATT_OCR_MAX_IMAGES`、值 6）—— 见 §11 |
| `ATT_OCR_TIMEOUT_MS` | 60 s | **附件侧 OCR 阶段**总预算——**整次附件解析共享**，不是每图；**不是整条解析链的墙钟上限**（取图调用 `pageImages` / `renderCrops` / `renderPages` 各另带一次 60 s 超时） |

字符预算两边都 = `max(512, toolLimit("maxOut") - 512)`；附件侧融合后超 `ATTACH_TEXT_MAX`(131072) 截断并标 `clipped`。
达到任一预算**立即停**并如实注记（另有 N 图未处理 / 本机 OCR 时间预算用尽 / 结果文本已达单次长度上限）。
`OCRKit.recognize` 单次调用不可中断（只能 `terminate`）⇒ 超时 / 中止的生效点只在**两张之间**。

**落点**
- **ReadOffice 侧**：融合文本进工具结果（`wsFuseOcrText`），状态行加字段「图片 N 张已本机 OCR 并按页插入(当前模型不支持图像输入,OCR 可能有误差)」。
  **同一条路径也服务 pptx**（P2 的 S15 起）：pptx 的正文由 office 胶水写上 `----- 第 N 页 -----`（S11，**与 PDF 同一字面量**），
  `wsDocumentPack` 的融合分支按 `kind` 选布局 —— `pdfPage`（PDF / pptx）⇒ `pages`（块插到对应页 / 幻灯段之后）、
  docx / xlsx ⇒ `flat`（正文之后按序接）。因此 pptx 的幻灯内图片 OCR 后**落在该幻灯段之后**（块头 `【第 N 页 · 图 k · 本机 OCR】`，
  `k` = 该幻灯内序号），而不是文末。判据与读数见 `shared/progress/read-split-p2b-done.md`。
- **附件侧**：进附件对象——`att.text`（融合后正文，`textChars` 同步）、`att.ocrImgs`（**写入过非空块的张数**；未识别的另计；
  旧数据无此键 = 0，不做迁移）、`att.degraded`（上面那些注记）。气泡卡片与附件 chip 显示「N 图已 OCR」。
  **附件侧不受 S15 影响**：office 三格式仍一律 `flat`，只有 PDF 走 `pages`。

**不变量（有负例断言守着）**：视觉模型路径一字不变（不探页图、不做 OCR，请求体既不含「本机 OCR」也不含夹具暗号）；
无图 / 纯文本文档输出逐字节不变；附件 PDF 的融合条件 = 「非视觉**且有图**」（**不是**「文本为空」）。
另有一条既有坑要写在这里：附件侧「文本为空 ⇒ 扫描件」那条分支**实际不会命中**——`PDFKit.extractText` 对任何 ≥1 页 PDF
都写页分隔标记，`text.trim()` 永不为空（实测：批前扫描件 PDF 的附件正文只有 17 字节的 `----- 第 1 页 -----`，没有正文）。
本轮**不改那条分支**：非视觉 + 有图 ⇒ 融合生效，扫描件在文本模式下改由「页图 OCR 进 `att.text`」兜住
（同一夹具实测：从只有页分隔标记 → 含整页图像的识别文本）；视觉模型下仍是既有的「只有页分隔标记」行为
（要按图发就在 设置 → 附件 →「附件图片策略」选「图片」；三档口径见 §11）。

## 11. 附件三档（0.2.1）：图片策略、格式矩阵与自绘表格图

> 口径唯一来源 = `shared/specs/b31-attach-modes-plan.md`（v2.3）与决策档 `shared/decisions/0.2.1-decisions-approved-2026-09-18.md`；本节只记**已实现并已验收**的部分（落地证据 = `shared/progress/b31-main1-done.md` … `b31-main4-done.md`）。

**需求**：附件里的图片「发不发、怎么发」过去只有 PDF 有文本 / 图片两档（旧键 `pdfMode`）；0.2.1 改为 **PDF 与办公附件共用**的单键三档，并给没有渲染器的格式定好如实降级。

**设置口径（`appA` / `appD`）**
- `attachImgMode ∈ {"text","image","both"}`；读取入口 `attachImgMode()`（`appD`）—— 缺键 / 非法值一律 `both`；选择器恒显三档（`ATTACH_MODE_OPTIONS`：纯文本 / 图片 / 混合），**没有第四个「自动」值**；默认 = **混合**。
- **旧 `pdfMode` 单向迁移**（`mergeSettings`，只执行一次）：显式 `"image"` ⇒ `image`；`"text"` / 缺键 / 非法 ⇒ **`both`**（不是 `text` —— 存量 PDF 从纯文本变混合，是**用户可见变化**，进 `CHANGELOG.md`）；合法新键优先、旧键被忽略。**有意不 `delete` 旧键**：老用户盘上的 `pdfMode` 原样保留（旧版本回退仍能读到），新版本只写新键 —— 单向兼容，不是「新旧互读一致」。

**判定枢纽（唯一实现）**：`decideAttachPlan(o)`（`appD`，FIRST-MATCH 七条）⇒ `{send:"text"|"both"|"image", images:"send"|"ocr"|"none", render:"pages"|"embedded"|"drawn"|"none", why, toast}`；渲染能力表 `attachRenderCap(kind, subtype)` = `pdf ⇒ "pages"` · `xlsx|xls ⇒ "drawn"` · `docx ⇒ "pages"`（`ATT_DOCX_RENDER_READY`（唯一开关，S7 起 = `true`）∧ `docxRenderReady()`（`window.RenderKit.available`，render.part 在位）；任一不满足 ⇒ `false`）· `pptx`（D11 本批排除）/ `doc` / `ppt` / 图片附件 ⇒ `false`。**硬不变量**：图不进消息时 `send` 只能是 `text`（`images==="ocr"` ⇒ `send==="text"`）。判据 = 判定矩阵 54 格（6 格式 × 3 档 × 3 视觉态）+ 能力表 7 条 + 文案咬合 18 条（三项 = **B 组 79 条**；连同 A 组 17 + C 组 54 + D 组 3 ⇒ **合计 153 条页内断言**，批-1）。

**格式 × 档位矩阵（已实现事实）**

| 格式 | 纯文本档 | 图片档 | 混合档（默认） |
|---|---|---|---|
| PDF | 只发抽出的文字；**无文字层且有图 ⇒ 自动升档混合**（`why:"scan"`；视觉发页图 / 非视觉走 OCR） | 逐页渲染页图（`pdfDpi()` 72–300、页数 ≤ `pdfMaxPages()`） | 视觉：文字 + 页图**按页交错**；非视觉：页图本机 OCR 后按页并入正文（原图不发） |
| docx | 文字 | **逐页渲染页图**（S7 起；与 PDF 同参数：同一 DPI / 页数上限 + 单张 1 MB / 最长边 2000 闸门）——渲染失败 ⇒ 有内嵌图走混合、否则纯文本（注记 + toast 如实说明；`toastNoRender` 只用于「库不在位 / 开关关闭」） | 文字 + 内嵌图（图在前、单 text 在后；**位置锚 = S6c 已落地**：按文档序 `【图 k】` 标记交错，锚不过 ⇒ 回退 flat + 注记） |
| pptx | 文字（含 `----- 第 N 页 -----` 幻灯标记） | **本批排除（D11）** ⇒ 降级为混合（有内嵌图）/ 纯文本，`toastNoRender` 如实说明 | 文字 + 内嵌图**按幻灯段交错**（认不出幻灯号的图追加文末 + 注记）；非视觉 ⇒ 幻灯内图 OCR 后插在该幻灯段之后 |
| xlsx / xls | 文字（`# 表名` 段头） | **本机自绘表格图（见下）**；非视觉 ⇒ 不画图（落纯文本；有内嵌图仍走 OCR 腿） | 文字 + 文档内嵌图（**自绘图只在图片档出现**，避免与 CSV 文本重复计费） |
| doc / ppt | 文字（老格式） | 无渲染器 ⇒ 降级（同上） | 文字（有内嵌图则按内嵌图处理） |
| 图片附件（png / jpg…） | 按既有图片路径处理（**单图例外**，不受三档影响）：视觉 ⇒ 内联原图；非视觉 ⇒ 本机 OCR | 同左 | 同左 |

**混合档 + 非视觉 = 自动本机 OCR 回退（对外不暴露档位）**
- 图不原样发送：`PDFKit.pageImages` → `renderCrops`（失败单调降级整页并注记）→ 内嵌 Tesseract 逐张识别 → 按位置插块（PDF 按页 / pptx 按幻灯段 / xlsx、docx 接在正文之后）；三个输出读数 = `att.text` / `att.ocrImgs`（**写入过非空块的张数**；旧数据无此键 = 0，不迁移）/ `att.degraded`（注记）。
- 「不暴露档位」两层：① 自动升档 / 改档**不新增选项、不改写用户设置**（枢纽是纯函数，只描述这一次怎么发）；② 请求体不含 `attach` 元数据（`protocolContent` 发送前整块剥掉，已断言），信息行**只在 `ocrImgs || degraded` 时**追加模式词 ⇒ 默认路径（视觉、有图）的信息行逐字节不变。
- 降级必须可见：一次性 toast + `degraded` 注记，逐字来自枢纽（例：「当前模型不支持图像输入;这份PDF已按「混合」处理:图片内容由本机 OCR 提取后随文字发送(原图未发送)」）。

**图片上限**：`ATT_IMG_MAX_IMAGES = 10`（`appD`；D4：原 `ATT_OCR_MAX_IMAGES`(6) 抬到 10 并**改名**，旧名全库归零）—— **取图计划与 OCR 融合共用同一上限**、与默认 `pdfMaxPages()` 对齐；附件侧三处取图调用点统一传 `attPdfOpts()`（`{maxScan: pdfMaxPages(), maxImages: ATT_IMG_MAX_IMAGES}`），自绘图张数上限同源（`{maxSheets: ATT_IMG_MAX_IMAGES}`）。

**交错件的 `blob` 键教训（P1，批-2）**：组装「正文 + 图」有序部件时，图部件带 `attach.blob = f.id + "-p" + (数组下标 + 1)` —— 该键**必须与 `externalizeFiles` 写库的键同源**；`page` 是另一回事（PDF 保留**真页号**、xlsx 为表序号，UI 与断言都靠它）。批-2 首轮真缺陷：拿「下标数组」去找**对象**（`idx.indexOf(obj)` 恒 `-1`）⇒ 下标算成 0 ⇒ 键退化成 `-p0` ⇒ 发送期 `blobMemo` 未命中 ⇒ 请求体里页图变成「图片数据不在本机」占位（模型看不到图）。修法 = `attImgIndexOf(imgs, im)`（`imgs.indexOf(im)`；找不到返回 -1，该图退化为**内联 data**，绝不写无效键）。取证 = `diag3` 探针（判据 = `blobStore.mem` 键集合 ⊇ 部件 `attach.blob`）；修后 35/35 PASS + 两轮负例 `NEG-OK`。

**自绘表格图（S6b / D12；xlsx / xls 的图片档）**
- 零新库：`attachDrawSheetImages(text, opts)` 用 canvas 逐表画「表名 + 行列网格 + 单元格文本 + 列宽自适应（单列 260 px 裁切）」⇒ `toDataURL`；生产参数 **`scale: 2`**（单元格字体 13px ⇒ 26px）。
- 可读性依据（真机）：同夹具 **1× ⇒ 309×126、OCR 置信度 57**（暗号 `RUN|ALPHA` 读花）；**2× ⇒ 619×252、置信度 82**（`AZUSA DRAWN 7Q3` 逐字可读）；交付图尺寸与 2× 读数逐位一致（判据钉的是生产参数）。
- 交付闸门 `attCanvasFitForModel`（R11）：**按 `imgMaxSide()` 下采样 + 单张 1 MiB 硬顶**（`READ_IMG_HARD_MAX`）；质量梯 `q → 0.6 → 0.4`、边长梯 `1400 / 1000 / 768 / 512`，取第一个进硬顶的版本；**取舍 = 可读性优先**（不追 256 KiB 预算 —— 那是给随手拍照片的取舍，表格图缩到 512 px 宽就认不出字）。压不进硬顶的那张**只跳过该表**并记「表 N 的表格图超过单张 1.00 MB 上限,未绘制」，其余表照画。
- 咬合读数：`heavy` 夹具原始 JPEG 5,357,103 B ⇒ 交付 **265,182 B**；`imgMaxSide=1024` ⇒ **1024×406 / 215,694 B**；`overflow` 自然画布 6280×2492 ⇒ **1400×556 / 267,153 B**（负例轮摘掉硬顶后请求体 1,382,906 B → 5,200,470 B ⇒ 上限确有约束力）。
- 上限与注记（逐字）：每表 ≤ 60 行 × 12 列、≤ 10 表；`degraded` = 「本机绘制的表格图(仅数据网格,不含原表格式)」+ 截断注记「表 N 仅绘制前 60 行(另有 M 行未画)」「表 N 仅绘制前 12 列」「仅绘制前 10 张表(共 M 张)」。
- 保真自证：出图后**画回 canvas 采样非空白像素**（dark 像素落在合理区间 ⇒ 不是白板）+ 真机 OCR 读出写死暗号（`AZUSA DRAWN 7Q3` / `WT-2026-0918-01…04`）。同一份 `noimg.xlsx` 两档两图源可判：默认档 `mode=both / renderSrc=embedded / parts=TTIT`；图片档 `mode=image / renderSrc=drawn / parts=II`。

**`file://` 下 canvas 无 taint 的自证方式**：自绘图的像素全部来自 canvas 原生绘制（不加载任何文件 / blob URL 图片）⇒ 不会 taint。自证 = 真机（Thorium 122 + `file://` + minimal 私有副本）对每张产出调 `toDataURL` **断言不抛**（读数 `taint ok:true`），再叠加「画回 canvas 采样 + OCR 暗号」两条独立读数证明交付字节确实是这张图。**边界**：这条只覆盖 **D12 自绘图**；S7 的办公渲染页图若把 `file://` 的 blob URL 画进 canvas，仍需单独取证（R11 原文即为此写）。
