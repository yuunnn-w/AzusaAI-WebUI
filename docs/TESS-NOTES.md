# Tesseract.js 内嵌产物（OCRKit）笔记

> 状态：现行 —— 内嵌 OCR 的取件、blob worker 加载与 `file://` 约束的唯一记录；升级库版本或改动 `src/tesseract.part` 前必读
> 更新：2026-09-14 · 载荷段 `src/tesseract.part` = 6,678,819 字节（6.37 MB，9 个 `<script>` 块，三档产物共用）
> 上游版本：tesseract.js 7.0.0 + tesseract.js-core 7.0.0 + tessdata_fast（eng / chi_sim）· 生成脚本 `scripts/make-tesseract-part.js`（幂等）
> 取件：`vendor/tess-src/` 下装 `tesseract.js` / `tesseract.js-core` 的 npm 发行版（含 `node_modules/` 里全部 core 变体），训练数据放 `vendor/tess-src/langs/{eng,chi_sim}.traineddata`
> 测试：一次性 CDP 无头脚本（含真实窗口模式，只跑 `file://`）已删除，结论见 §5；要复现请按 `CONTRIBUTING.md`「验证」现写

---

## 1. 版本与体积

| 部分 | 来源 | 原始 | 内联后 |
|---|---|---|---|
| 主线程库 | `tesseract.js@7.0.0 dist/tesseract.min.js` | 62,961 B | 62,920 B(去掉 sourceMappingURL) |
| worker 源码 | `tesseract.js@7.0.0 dist/worker.min.js` | 111,307 B | 111,269 B |
| core glue | `tesseract.js-core@7.0.0 tesseract-core-simd-lstm.js` | 89,271 B | 89,271 B |
| core wasm | `tesseract-core-simd-lstm.wasm` | 2,857,601 B | gzip 1,064,026 → base64 **1,430,526 B** |
| eng 训练数据 | `tessdata_fast/eng.traineddata` | 4,113,088 B | gzip 1,962,155 → base64 **2,638,009 B** |
| chi_sim 训练数据 | `tessdata_fast/chi_sim.traineddata` | 2,469,156 B | gzip 1,722,294 → base64 **2,315,528 B** |
| worker 引导层 | `vendor/tess-src/ocr-worker-boot.js` | 2,452 B | 2,452 B |
| core 适配层 | `vendor/tess-src/ocr-core-adapter.js` | 1,039 B | 1,039 B |
| OCRKit 包装层 | `src/ocrkit.src.js` | ~17 KB | 26,662 B(含 tiny-inflate 9.2 KB) |
| **合计** | | | **6,678,819 B(6.37 MB)** |

语言数据全部来自 **tessdata_fast**(LSTM-only 小模型)。eng 4.11 MB / chi_sim 2.47 MB 已经是 fast 版尺寸,
没有更小的官方替代(不要再放 best/legacy)。

选择 `tesseract-core-simd-lstm` 的理由:与纯 `tesseract-core-lstm` 体积几乎一样(2.857 MB vs 2.855 MB),
但支持 SIMD 更快;tessdata_fast 只有 LSTM 模型,所以 LSTM-only core 够用。

## 2. `.part` 的结构与拼装

```html
<script>/* tesseract.min.js 主线程库(会执行,挂 window.Tesseract) */</script>
<script type="text/plain" id="ocr-embed-boot">      worker 引导层源码        </script>
<script type="text/plain" id="ocr-embed-core">      core glue 源码           </script>
<script type="text/plain" id="ocr-embed-adapter">   core 适配层源码          </script>
<script type="text/plain" id="ocr-embed-worker">    官方 worker.min.js 源码  </script>
<script type="text/plain" id="ocr-embed-wasm">      base64(gzip(wasm))       </script>
<script type="text/plain" id="ocr-embed-lang-eng">  base64(gzip(eng))        </script>
<script type="text/plain" id="ocr-embed-lang-chi_sim"> base64(gzip(chi_sim)) </script>
<script>/* OCRKit 包装层(ES5) */</script>
```

拼进产物 html:放在 `</body>` 前即可(位置任意,`<head>` 里也能工作,包装层只在首次
`recognize` 时才碰 DOM)。**在 `scripts/build.js` 里只要原样拼接 `src/tesseract.part`**,不要做任何转义。

**格式安全**:生成脚本自动 1) 删除 JS 里的 `sourceMappingURL`(否则会多出一次请求);
2) 把脚本块里的 `</script` 替换成 `<\/script`(JS 源码里这个序列只可能出现在字符串/正则/注释中,语义等价),
并统计条数;3) 若出现 `<!--` 或 `<script`(会破坏 HTML 的 script-data 转义状态)则**直接报错中止**,不产出坏文件;
4) 产出后再自检 `<script>` 与结束标记个数是否一致。当前各文件里这三个序列出现次数均为 0。

## 3. 运行时到底怎么加载(关键坑)

1. 首次 `recognize` 时:把 5 段源码(引导层+glue+适配层+worker)拼成**一个** blob URL,`new Worker(blobURL)`
   (`workerBlobURL:false`),再把 wasm / traineddata 的**原始字节**用 `postMessage` 投递进去。
2. worker 里 `self.fetch` 被引导层改写:只应答 `*.traineddata`,其它一律 `reject` 并记进 `self.__OCR_BLOCKED`。
   所以 worker 内**不可能**发生外部请求。
3. core 不走 `importScripts`:glue 直接拼在同一份 worker 脚本里执行,`global.TesseractCore` 已存在,
   官方 `getCore()` 里那段 `importScripts(corePath)` 被整段跳过(在 file:// 下那一步必然失败)。
4. core 适配层把 `TesseractCore` 换成"先等资源到位再初始化,并注入 `wasmBinary`"的包装函数,
   避开一个**消息顺序竞态**:`createWorker()` 在同步阶段就发出了 `load` 包,而我们的资源包只能之后发。
   wasm 用 `wasmBinary` 注入后,emscripten 不会再去 fetch/locateFile 找 `.wasm`。
5. `createWorker` 的第三个参数:`corePath:'ocrkit-core.js'`(只是为了别触发 simd 探测)、
   `langPath:'ocrkit-embedded'`(假目录,实际被 fetch 钩子截胡)、`gzip:false`、
   **`cacheMethod:'none'`**、`logger`、`errorHandler`。
6. 因为 `createWorker` 自己 `new Worker()`,我们拿不到实例,所以在调用它的那一瞬间**短暂劫持
   `window.Worker`**(同步、调用后立刻还原)来捕获 worker,再 `postMessage` 资源包。

### 为什么必须这么做(file:// 实测,Chrome 152.0.7977.83 / Windows)

| 能力 | http:// | file:// |
|---|---|---|
| `new Worker(blob:URL)` | ✅ | ✅ |
| worker 内 `importScripts(blob:URL)` | ✅ | ❌ `The script at 'blob:null/...' failed to load` |
| worker 内 `fetch(blob:URL)` | ✅ | ❌ `Failed to fetch` |
| worker 内相对路径 fetch | ✅ | ❌(blob worker 没有 base URL) |
| worker 内 `eval` / `new Function` | ✅ | ✅ |
| `new Worker('data:...')` | ✅ | ✅ |
| IndexedDB `open()` | ✅ | ⚠️ **请求永不触发任何事件**(既不 success 也不 error) |

最后一行是第二个大坑:tesseract.js 默认用 idb-keyval 读缓存,
`await readCache(...)` 在 file:// 下会**永久挂住**,所以必须 `cacheMethod:'none'`。

## 4. API(`window.OCRKit`)

```js
OCRKit.available  // true/false:环境是否满足(Promise/Blob/FileReader/createObjectURL/Worker/WebAssembly/atob/内嵌库)
OCRKit.why        // available=false 时的原因,例如 "环境缺少: Worker"
OCRKit.version    // "7.0.0"(tesseract.js 版本)
OCRKit.langs      // ["eng","chi_sim"]
OCRKit.mode       // "worker"(额外字段:实际走的是 worker,没有主线程降级分支)
OCRKit.decode     // 额外字段/诊断:实际用了 "DecompressionStream" 还是 "inflate(内联 tiny-inflate)"
OCRKit.recognize(input, opts) -> Promise<{ok, text, confidence, ms, words?, error?}>
OCRKit.terminate()  // 释放 worker,返回 Promise(true)(契约没规定返回值)
```

- `recognize` **永不 reject**:失败一律 `{ok:false, error:"..."}`。
- `text` 已去掉首尾空白;`confidence` 是 tesseract 的 0–100 均值置信度;`ms` 是本次调用墙钟毫秒。
- `words` 只在 `opts.words === true` 时出现(内部把 `blocks` 摊平成 `[{text,confidence,bbox}]`)。
- `opts`:`langs`(默认 `"chi_sim+eng"`,`+` 分隔)、`onProgress(p)`(0–1,粗粒度、单调)、
  `timeout`(默认 120000 ms)、`words`;其余键(**除这 4 个以外**)原样透传给 tesseract 当识别参数,
  例如 `{ tessedit_pageseg_mode: '7' }`。
- 输入:`data:image/...` 字符串、`blob:` 字符串、`Blob`/`File`、`ArrayBuffer`/TypedArray、
  `HTMLCanvasElement`、`OffscreenCanvas`、`HTMLImageElement`。
  普通 http(s) 字符串会被**拒绝**(保证零网络的语义)。
- 并发:内部串行队列,同时多次调用会排队依次执行,不会互相打断。
- 首次调用懒加载(解码 + 起 worker);之后复用;`terminate()` 或超时后会自动重建。

## 5. 实测结果(headless Chrome,一次性 CDP 脚本 —— 脚本未随仓库保留,下表是当时的原始结论)

7 种场景,**PASS 76 / FAIL 0**:http 在线 / http 离线 / file 在线 / file 离线(导航前就断网)/
file 离线且禁用 `DecompressionStream`(强制走内联 inflate)/ 无 Worker(应 available:false)/
真实产物（`AzusaAI-WebUI-full.html`）+ part 的集成副本。全部场景 `Network.emulateNetworkConditions({offline:true})` 下
**零外部请求**(只出现 data: / blob: / file: / 127.0.0.1)。每个完整场景都断言:无未捕获异常、
零外部请求、`available`、**API 形状符合契约**(keys/类型/langs 数组)、英文识别、onProgress、
Blob 识别、中文识别、words、并发排队、langs 切换、超时自愈、非法输入、terminate 后重识别。

- 英文(canvas 白底黑字 `HELLO OCR 12345`,780×140,bold 76px):
  `"HELLO OCR 12345"`,confidence **92**,首字延迟 **218–275 ms**(含全部初始化:base64 解码 + gunzip +
  worker 启动 + eng+chi_sim 加载 + 识别),热调用 **30–46 ms**。
- 中文(`中文识别测试`,Microsoft YaHei 可画):`"中文识别测试"`,confidence **94**,热调用 **20–27 ms**。
  中文用例同时证明 chi_sim 训练数据加载与 `initialize('chi_sim+eng')` 成功。
- `words:true` → 3 个词 `["HELLO","OCR","12345"]`。
- 并发两次、`langs` 切换(reinitialize)、非法输入、`terminate` 后重识别、超时(1 ms 必触发 → `ok:false` +
  之后自愈)均通过。
- 真实窗口 Chrome(`--windowed`,非 headless,file:// 用例):同样全绿,首字延迟 0.7–1.9 s(窗口渲染开销)。
  唯一差异:超时用例在后台窗口里因为 `setTimeout(1)` 被浏览器节流到 ≥1 s 而未触发(测试脚本标为跳过,
  headless 下为严格断言)。
- 集成副本：`vendor/tess-src/index-ocr-integration.html` = 产物 + part 的集成快照，7,912,015 字节；
  页面关键 DOM 全在、未进 legacy、marked/katex 正常、`OCRKit.available === true`、识别成功。
  （**没有改动产物本身**）

## 6. 已知限制与坑

- **体积**:part 6.37 MB,拼进产物 html 后约 7.6 MB。base64 解码/解压在**首次 recognize 时同步占用主线程
  约 150–250 ms**(headless 实测;真实窗口更慢),之后不再重复。不想要中文可删 chi_sim(-2.2 MB)。
- **内存**:主线程常驻 ~9.4 MB(wasm+两个语言数据解码后),worker 内还有一份副本;`terminate()` 只释放 worker
  那份。DOM 里的 base64 文本**不删除**(便于 terminate 后重建,代价是 ~6.4 MB 文本常驻)。
- **压缩依赖**:wasm/语言数据是 gzip+base64。优先用 `DecompressionStream`(Chrome 80+/Edge 80+/
  Safari 16.4+/Firefox 113+);没有时回退到内联的 `tiny-inflate 1.0.3`(MIT,已带 LICENSE),
  该回退路径已实测(禁用 `DecompressionStream` 后首字延迟 312 ms,结果一致)。所以**不强制** DecompressionStream。
- **只有 worker 一条路**:没有主线程降级分支;没有 Worker 的环境直接 `available:false`。
- **file:// 的 blob worker 是有代价的**:见 §3 表格。如果哪天要换成"多文件加载",file:// 会立刻失效。
- `onProgress` 是**粗粒度**估算(5%/15%/30%/40% 四个阶梯线性插值),不是 tesseract 的原始百分比。
- 识别质量只用了干净的合成图验证;**照片/截图/小字号/倾斜/多栏**没有测,那是 tesseract 自身能力问题,
  与包装层无关。
- 只在 **Chrome 152.0.7977.83(headless=new + 真实窗口,Windows)** 上验证过;Firefox/Safari/Edge 未测。
  没有使用 `--allow-file-access-from-files`,即默认的 file:// 安全策略。
- 未测:超大图片(几千万像素)、`OffscreenCanvas`/`HTMLImageElement` 输入分支、多标签页并发争用。
- tesseract.js 的 `dataPath/dataFromCache` 等高级选项没有透传;`cacheMethod` 被固定为 `none`(file:// 必需)。

## 7. 目录（现行布局）

```
src/tesseract.part                    最终产物(6.37 MB,9 个 script 块)
scripts/make-tesseract-part.js        生成脚本(幂等,md5 稳定)
src/ocrkit.src.js                     包装层源码(ES5) —— 生成脚本会插入版本号/语言表/inflate
vendor/tess-src/                      下载/解包原始发行版文件(含 node_modules 里全部 core 变体)
vendor/tess-src/ocr-worker-boot.js    worker 引导层源码
vendor/tess-src/ocr-core-adapter.js   core 适配层源码
vendor/tess-src/probe-worker.html     file:// 下 blob worker 能力探针(结论见 §3 表格)
vendor/tess-src/probe-run.js          探针运行器
vendor/tess-src/index-ocr-integration.html  集成验证副本(产物 + part 的集成快照),可随时删除
docs/TESS-NOTES.md                    本文件
```

一次性测试件（`libtest-ocr.js` / `libtest-ocr.tmpl.html` / `libtest-ocr.result.json`）
与旧 `.build/` 目录已删除（仓库不保留回归脚本，理由见 `CONTRIBUTING.md`「验证」）。
`vendor/tess-src/libtest-ocr.html` 只在模板 `libtest-ocr.tmpl.html` 存在时产出 ——
模板未随仓库保留,所以现行布局里**没有**这个文件。

第三方许可:tesseract.js / tesseract.js-core —— Apache-2.0;语言数据 tessdata_fast —— Apache-2.0;
tiny-inflate —— MIT(版权与许可文本已内联在 `.part` 的包装层里)。
