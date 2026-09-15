# 办公文件解析内嵌产物（OfficeKit）笔记

> 状态：现行 —— 办公六格式解析的取件、worker 加载与限额的唯一记录；升级四库版本或改动 `src/office.part` 前必读
> 更新：2026-09-14 · 载荷段 `src/office.part` 文件 = 2,928,414 字节（2.79 MB；其中 6 段载荷合计 2,927,822 B，差额 592 B = 5 个 `<script>` 包装 + 头注释）
> 上游版本：mammoth 1.12.3 / SheetJS CE 0.20.3 / @jose.espana/docstream 0.1.3 / fflate 0.8.3 · 生成脚本 `scripts/make-office-part.js`（幂等 + sha256 断言）
> 取件：按生成脚本 `LIBS` 表里的 URL 下载四个浏览器单包到 `vendor/office-src/`（文件名见 §1 表）；哈希不符即 `exit 1`
> 测试：一次性 CDP 无头脚本（只跑 `file://`）已删除，结论与原始读数见 §5；要复现请按 `CONTRIBUTING.md`「验证」现写

---

## 1. 版本与体积

| 部分 | 来源(上游发行版,落 `vendor/office-src/`) | 原始 | 产物 |
|---|---|---|---|
| mammoth | `mammoth@1.12.3 mammoth.browser.min.js` | 636,898 B | 637,016 B |
| SheetJS CE | `xlsx-0.20.3 xlsx.full.min.js`(官方 CDN 版) | 951,904 B | 952,033 B |
| docstream | `@jose.espana/docstream@0.1.3 dist/officeparser.browser.js` | 1,271,443 B | 1,271,483 B(剥 shebang 1 处 + sourceMappingURL 1 处) |
| fflate | `fflate@0.8.3 umd/index.min.js` | 33,311 B | 33,146 B(剥 jsdelivr 横幅 267 B) |
| 解析核心胶水 | `src/office-worker.src.js` | 16,865 B | 17,005 B |
| OfficeKit 包装层 | `src/officekit.src.js` | 17,139 B | 17,139 B |
| **合计** | | | **2,927,822 B** |

四库来源 URL 与 SHA-256 断言写在 `scripts/make-office-part.js` 的 `LIBS` 表里（哈希不符即 `exit 1` 且不写盘）。
`vendor/office-src/` 不进版本库（`.gitignore` 已排除）；丢了就按 `LIBS` 表重新下载四份发行版文件（文件名见 §1 表的上游来源列）并核对哈希。

**为什么是这四个**:纯浏览器、可离线、`file://` 下可用 ——

- **mammoth** 只做 docx 的 `document.xml` 解析:文本用 `extractRawText`(实测 7,040 字符),图片用 `convertToHtml({convertImage})` 顺手收(HTML 本体丢弃)。实测 `convertToHtml` 全文 249,135 字符(35×),HTML 显然不能进消息(token/体积/注入面三重代价),所以**双 pass**:一份拿纯文本、一份只为了图。
- **SheetJS CE 0.20.3** 是官方 CDN 版。**不要退到 npm 上的 0.18.5**:那个版本有 CVE-2023-30533 / CVE-2024-22363,且官方不给 0.18.x 修。
- **docstream** 是老格式(`doc`/`ppt`/`xls`)唯一能用的纯 JS 方案(`harshankur/officeParser` 的 fork,浏览器单包);成熟度低(单人、长期未更新),所以它的失败路径按「可读错误 + 如实降级」设计,不硬扛。
- **fflate** 只有一个用途:**ZIP 结构预扫描**(`unzipSync(u8, {filter})` 零解压枚举中央目录),在解压任何东西之前就拒掉压缩炸弹。

## 2. `.part` 的结构与拼装

```
<!-- 头注释 -->
<script type="text/plain" id="office-lib-mammoth">   … </script>
<script type="text/plain" id="office-lib-xlsx">      … </script>
<script type="text/plain" id="office-lib-docstream"> … </script>
<script type="text/plain" id="office-lib-fflate">    … </script>
<script type="text/plain" id="office-worker-glue">   … 解析核心(网络桩 + 预扫描 + 三条解析路径) </script>
<script>                                              OfficeKit 包装层(ES5) </script>
```

- 五个 `text/plain` 载荷只存文本、不执行(和 pdfjs / tesseract 同款)。
- 所有块都过 `escapeForInline`(`</script` → `<\/script`、`<!--` → `<\!--`),生成脚本断言**逐字节可逆**;四个库 + 胶水**源内** `</script` / `<script` 计数为 0(库内 `<!--` 实测 10 处: mammoth 3 / xlsx 5 / docstream 2,都在字符串里,转义后产物内未转义计数为 0)。
- **URL 中和**:`https://unpkg.com/` 与 `https://cdn.jsdelivr.net/` 在构建期换成 `about:blocked#`(docstream 各 1 处);再抽取全产物 `https?://<host>`、**小写归一后**断言 ⊆ 19 条白名单(四库全量实测并集)。出现新主机即构建失败 —— 新增必须人工判定。
- `build.js` 里的位置:`head + libs + katexJs + pdfjs.part + tesseract.part + **office.part** + pyodide.part + appA…appE`;缺 `office.part` 时只打印警告(办公附件功能自动降级为「不支持 .docx 这类文件」),其余不受影响。

## 3. 运行时到底怎么加载(关键坑)

**worker 源(默认路径)** —— classic blob worker,`file://` 下可用:

```
self.window = self;  self.global = self;  self.__OFFICE_WORKER__ = 1;
<setImmediate 垫片>            ← 见下方「头号性能坑」
<mammoth> ; <xlsx> ; <docstream> ; <fflate>
<解析核心胶水>                 ← 装网络桩 + 挂 self.onmessage
```

- **`self.window = self` 必须最先**:docstream 顶部就是 `if (typeof setImmediate === 'undefined') window.setImmediate = …`,没有 `window` 直接在 worker 里抛错。
- **网络桩**:检测到 `__OFFICE_WORKER__` 时,胶水把 `fetch / XMLHttpRequest / WebSocket / EventSource / importScripts` 换成抛错函数。docstream 内部有 `fetch(` ×21、`new Function("specifier","return import(specifier)")` 之类的路径(都在它自己的 PDF 通道里,本项目不调用);桩让「万一走到」也立刻失败而不是发请求。
- **头号性能坑 —— `setImmediate` 垫片**:docstream 自带的垫片是 `setTimeout(cb, 0)`。它的异步管线会反复 `setImmediate`,`setTimeout` 的 **4ms 最小间隔**被逐次放大:同一个 92KB 的 pptx,实测 **1,302~3,508ms**;换成 `MessageChannel` 宏任务垫片后 **40~66ms**,输出字符数 / 文本哈希 / 附件字节**完全一致**(实验:主线程同款 1,051ms → 48ms)。所以包装层在库体**之前**注入 MessageChannel 版 `setImmediate`(没有 MessageChannel 的老内核退回 `setTimeout`)。⚠️ 改这一行前先复跑一遍耗时对照。
- **主线程降级**(环境没有 `Worker` 时):同一份「库体 + 胶水」经 `<script>` 注入执行 —— **不含** `window/global` 垫片、**不装网络桩**(装了会破坏应用自己的网络层),零请求靠上面的 URL 中和 + 网络审计兜底;此时单文件上限 20MB。
- **`window.pdfjsLib` 不被污染**:docstream 内嵌了一份 pdfjs-dist 5.4.530,但只在它的 PDF 通道里用;库体加载本身不动 `window.pdfjsLib`(实测解析前后都是 `undefined`),而且默认路径跑在 worker 里,全局污染只可能在 worker 内。

## 4. API(`window.OfficeKit`,形态对齐 `PDFKit` / `OCRKit`)

```js
OfficeKit.available / why / version / formats / mode   // mode: "worker" | "main-thread"
OfficeKit.parse(buf, ext, { onProgress(phase), timeout })  // 永不 reject
OfficeKit.cancelCurrent()   // best-effort:terminate 当前 worker(正在跑的解析以 cancelled 结束)
OfficeKit.terminate()       // 释放 worker,下次 parse 懒重建
OfficeKit.limits            // { mainThreadSizeCap, maxBytes, timeoutMs }
```

- 结果:`{ ok, subtype, text, textChars, clipped, images:[{mime,data(base64),bytes,label,page}], imgCount, skippedImgs, pages, unit, ms }`;失败:`{ ok:false, error(中文), errorKind, ms }`,`errorKind ∈ too-big / zip-bomb / encrypted / corrupt / unsupported / timeout / cancelled / worker`。
- **串行队列**:同一时刻只解析一个文件(多文件一起拖进来时不会同时挤爆 worker 内存);`cancelCurrent()` 只终止在跑的那个,队列后面的会用新 worker 继续。
- 图像以 **transferable ArrayBuffer** 从 worker 回传,主线程再转 base64(不 detach 调用方 buffer;`buf` 进 worker 前会先复制)。

## 5. 实测结果(headless Chrome,`file://`,一次性 CDP 脚本 —— 脚本未随仓库保留,下表是当时的原始读数)

worker 路径(真实 `OfficeKit`)与主线程参照(注入库体 + 胶水直接跑核心)逐项对照:

| 样例 | 主线程 ms | worker 冷/温 ms | 文本字符 | 图像 | 跳过 | 页/表 |
|---|---|---|---|---|---|---|
| test.docx (4.4MB) | 66 | 74 / 50 | 7,040 | 2 (jpeg 102,117 + 75,103 B) | 0 | — |
| test.xlsx (212KB) | 29 | 28 / 18 | 7,633 | 1 (png 182,483 B) | 1 (chart) | 9 表 |
| test.xls (20KB) | 8 | 8 / 5 | 5,553 | 0 | 0 | 1 表 |
| test.pptx (93KB) | 43 | 46 / 25 | 6,816 | 1 (png 18,597 B) | 1 (chart) | 9 页 |
| test.ppt (527KB) | 1 | 1 / 1 | 1,278 | 0 | 0 | — |
| ppt-native.ppt (767KB) | 4 | 159 / 4 | 4,676 | 0 | 0 | — |
| test.doc (1.7MB) | 1 | 2 / 1 | 112 | 0 | 0 | — |

- **两条路径读数完全一致**(文本哈希、图像张数与字节数、跳过数、页/表数),图像 base64 解码长度 == 原始字节数。
- **零外部请求**:CDP `Network.requestWillBeSent` 全量计数 —— 除主文档自身外 **0 条**。
- **`window.pdfjsLib`**:解析前后都是 `undefined`(未污染)。
- **耗时**:稳态 worker ≈ 主线程甚至更快;唯一例外是**冷启动第一个文件 +~160ms**(worker 启动 + 2.9MB 库首次惰性编译),之后复用同一个 worker。
- 载荷自检(`new Function` 编译探针,页面加载时一次)耗时 44ms。
- `onProgress` 阶段顺序:`started → extracting → parsing → done`。
- 老格式 `doc` 的样例文本很短(112 字符)是**样例本身**如此(图表夹具),不是抽取失败。

## 6. 已知限制与坑

- **老格式 `doc` / `ppt` 没有图像提取**(纯 JS 生态里没有可用方案):走降级文案「老格式只提取文字」;`doc` 的**修订痕迹会被一起抽出来**(库不区分)。
- **图像 MIME 白名单**:PNG / JPEG / GIF / WebP / BMP / AVIF;图表(chart)、EMF / WMF / TIFF 一律跳过并计入 `skippedImgs`(卡片上「跳过 N 张」的含义就是这个)。
- **pptx 图像不带页码**:docstream 的附件对象只有 `name / mimeType / data`,`label` 统一是「图 N」;`pptx` 的页数来自 `docProps/app.xml` 的 `<Slides>`,文件没这个属性时 `pages=0`(读数里 calibre 造的 test.pptx 有 9 页)。
- **限额表**：分两层，各自的**单一来源**写在代码注释里（无跨文件副本）——
  结构安全层(核心胶水 `LIMITS`,命中即拒收/跳过)与体积/超时层(OfficeKit 包装层,解析前判上限 + 计时):

  | 项 | 值 | 说明 |
  |---|---|---|
  | ZIP 条目数 / 解压总量 / 单条 / 压缩比 | 2000 / 150MB / 64MB / 120:1 | 预扫描命中即 `zip-bomb` 拒收(`LIMITS`) |
  | 图像张数 / 单图 / 图像总量 | 20 / 8MB / 32MB | 超出跳过并计数(`LIMITS`) |
  | 单文件解析超时 | 60s | 到点 terminate(主线程模式只能对调用方兜底);唯一来源 `officekit.src.js` 的 `DEFAULT_TIMEOUT` |
  | 单文件上限 | 50MB(主线程降级 20MB) | 与附件体系一致;唯一来源 `officekit.src.js` 的 `MAX_BYTES` / `MAIN_THREAD_CAP`(界面侧准入 `appD.part` 的 `officeSizeCap()` 直接读 `OfficeKit.limits.mainThreadSizeCap`,不留同值副本) |
  | 文本上限 | 131,072 字符 | `ATTACH_TEXT_MAX`(`appD.part`),与胶水里的 `TEXT_MAX` 必须一致(目前靠注释承诺,无构建期断言) |
- **加密 OOXML 的识别**靠魔数:加密的 `docx/xlsx/pptx` 会被 Office 另存成 CFB(OLE2)容器 → 见到 `D0 CF 11 E0 A1 B1 1A E1` 直接给「可能已加密或受密码保护」,不去猜密码。
- **`file://` 下 classic blob worker 可用**(与 Tesseract 的 worker 是同一类路径);worker 里 `importScripts(blob:)` / `fetch(blob:)` 会被浏览器拒绝,所以载荷一律**随 worker 源一起塞进去**,不用 `importScripts`。
- **`Read` 工具也复用本引擎**：`Read` 遇 doc / docx / ppt / pptx / xls / xlsx 时直接调 `OfficeKit.parse`（与工作区预览同一调用式，不走 `resolveOffice` 的 File+vision+toast 包装）；抽取文本走共享分页内核 `wsLinePageResult`（"文件:…"信息并入末尾状态行、不占行号），文档图片按模型视觉能力附带（单次 ≤4 张、累计 ≤1 MiB）或非视觉（文本为空时）走 OCR。
- 只在 **Chrome/Edge** 实测;**Safari / Firefox 未实测**(与项目其它内嵌库同一条边界)。

## 7. 非视觉模型的图片处理（图文按序融合，批 B）

**行为**：模型不支持图像输入（`visionState().vision === false`）时，`docx / xlsx / pptx` 抽出来的图片**不再丢弃**——
用与 `Read` 侧同一套助手（`appD.part` 的 `ocrSeqCands` / `ocrBlockText`）逐张本机 OCR，按文档顺序接在正文之后，
块格式 `【图 k · 本机 OCR】`（`k` 与 `label` 就是解析层给的那个，原样沿用；按第 6 节，pptx 目前也是「图 k」）。
附件对象上同步三个字段：`att.text`（融合后正文）、`att.textChars`、`att.ocrImgs` = **写入过非空块的张数**
（未识别出文字的另计，不计入 `ocrImgs`）；`att.degraded` 写明「图片 N 张已本机 OCR 并按序插入(当前模型不支持图像输入,OCR 可能有误差)」
与预算注记（另有 N 图未处理 / 时间预算用尽 / 未识别 / 引擎不可用）。气泡卡片与附件 chip 显示「N 图已 OCR」。
每会话只提示一次「正在用本机 OCR 识别…」（复用既有 `officeVisionToastShown` 机制），解析进度仍走 `OfficeKit.parse` 的 `onProgress`。

**行为边界**：
- **纯图文档**（没有正文）在非视觉下仍是既有拒收（「没有提取到可发送的内容」）——融合只对「有正文 + 有图」生效；
- 老格式 `doc / ppt` 本来就不抽图，不受影响；
- **视觉模型路径一字不变**：图片照旧作为 image part 发出（`att.mode === "both"`、`imgCount` = 发出张数、无 `ocrImgs`）；
- `skippedImgs` 只数解析层跳过的（图表 / EMF / WMF / TIFF / 超限），**不**把融合的图算进去；融合的图也不进 `att.images`。

**预算（唯一来源 = `appD.part` 的常量）**：单次附件解析最多 `ATT_OCR_MAX_IMAGES`(6) 张、**OCR 阶段**总预算 `ATT_OCR_TIMEOUT_MS`(60 s，
**整次解析共享**、不是每图)；注意它只覆盖 **OCR 阶段** —— 取图调用 `pageImages` / `renderCrops` / `renderPages` 各自另有一次 60 s 超时，
取图极慢时整条解析的墙钟会超过 60 s。融合后正文超 `ATTACH_TEXT_MAX`(131072 字符) 截断并标 `clipped`。
达到任一预算**立即停**并如实注记（不等满 60 s 再丢图）；`OCRKit.recognize` 单次调用不可中断（只能 `terminate`）⇒ 超时 / 中止的生效点只在两张之间。

**实测（无头 Chrome 152 + 真实夹具，批 B-Ⅱb）**：`code.docx / code.pptx / code.xlsx` 在 `visionForce="no"` 下
`att.text` 命中图片暗号（`AZUSA-IMG-7Q`）、块头与计数正确、请求体里没有 image part；`visionForce="yes"` 下图片按现状以 image part 发出、
请求体不含「本机 OCR」字样；无图文件（`test.xlsx` / `plain.docx`）与视觉格请求体与批前**逐字节相同**。
逐条读数与证据路径见 `shared/progress/netdocs-B2b-done.md`。
