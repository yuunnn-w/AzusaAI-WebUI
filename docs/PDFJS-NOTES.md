# pdf.js / PDFKit 内嵌产物笔记

> 状态：现行 —— 内嵌 pdf.js 的取件、ESM→classic 改写与 `file://` 加载约束的唯一记录；升级库版本或改动 `src/pdfjs.part` 前必读
> 更新：2026-09-14 · 载荷段 `src/pdfjs.part`（1,853,151 B，三档产物共用）· 生成脚本 `scripts/make-pdfjs-part.js`（幂等 + 自检）
> 上游版本：pdfjs-dist 6.3.289（`legacy` 构建，已修复 CVE-2024-4367）

## 1. 结论速览

| 项 | 值 |
|---|---|
| pdf.js 版本 | **pdfjs-dist 6.3.289**(`legacy` 构建) —— **已修复 CVE-2024-4367**(该漏洞影响 ≤4.1.392) |
| 原始包 | `vendor/pdfjs-src/pdfjs-dist-6.3.289.tgz`(registry.npmjs.org);源文件 `vendor/pdfjs-src/legacy/build/pdf.min.mjs` 518,555 B、`pdf.worker.min.mjs` 1,317,034 B |
| 产物 | **`src/pdfjs.part` = 1,853,151 bytes**(磁盘实测;md5 `b06fc57dbbc44ef6c9042a3723cc1047`) |
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
| **6.3.289(最新,本次采用)** | 1.84 MB | **1,853,151 B** | Chrome ≥ 125 / Safari ≥ 18 | 最新;仅比 5.x 大 52 KB(+2.9%) |

结论:**6.x 并没有比 5.x 明显更大**(远低于“>3MB”的担忧),所以按“最新版优先”选了 6.3.289。
如果要把兼容面从 Safari 18 降到 16.4,只需把 `vendor/pdfjs-src/legacy/build/` 换成 5.7.284 的两个文件再跑一次
`node scripts/make-pdfjs-part.js`(生成器已实测三版都能跑通),产物 1,800,792 B,其余一切不变。

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
| `src/pdfjs.part` | **最终内嵌片段（1,853,151 B）** |
| `scripts/make-pdfjs-part.js` | ESM→classic 机械化改写 + 自检 + 转义，幂等 |

- **`Read` 工具也复用本引擎**：`Read` 对 PDF 先 `extractText`（**文本优先，不吃 `pdfMode()` 设置** —— Read 的契约是"读内容"），只在确实没有文字层（分页标记 `----- 第 N 页 -----` 不算文字）时才 `renderPages`（页数 = `min(pdfMaxPages(), READ_PDF_IMG_MAX_PAGES=4)`，参数与附件图片模式同参）→ 支持视觉就附页图、否则逐页 OCR；渲染与文本抽取都直接调 `PDFKit`，不走 `resolvePdf` 包装。

能力探针（实测 module/classic worker、blob、动态 import、`new Function` 在 http/file 下的差异，§3 的设计依据）与一次性测试件（`mk-test-pdf.js` / `mk-libtest-page.js` / `libtest-pdfjs.js` / `pdfjs-cdp.js` / `libtest-pdfjs.html` / `pdfjs-test-log.txt`）及旧 `.build/` 目录均已删除；§5 的数字是当时的原始结论（仓库不保留回归脚本，理由见 `CONTRIBUTING.md`「验证」）。
