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
- **pptx 的幻灯号与幻灯标记（P2 的 S11 / S11b 已落地，本节为现行口径）**：
  - **幻灯标记**：office 胶水给 pptx 的输出逐张幻灯写 `----- 第 N 页 -----`（与 PDF **同一字面量**）⇒ pptx 文本具备页标记族。编号**只认 docstream 写在 `metadata.slideNumber` 上的原始序号**（= `slideN.xml` 的文件名序号）；取不到（= 0）时**整篇回退原文本**（宁可没有标记，也不冒标错号的风险，与自检同一取舍）—— **不**退回"数组下标 + 1"：vendor 的 `content` 只 push **有子节点**的幻灯（空幻灯不进数组），下标编号会让空幻灯之后的页号整体前移。**空幻灯不出标记**；「注记页不出标记」当前是**空真**（vendor 把 note 节点挡在 `content` 之外、解析完即丢 ⇒ `pptxIsNote` 是防御性判断，pptx 路径下恒假）。每次仍有自检：去掉标记行后必须与 `ast.toText()` **逐字节相同**，不成立就**整篇回退原文本**。
  - **图片→幻灯号归属**：走 docstream AST 的 slides，用节点 `metadata.attachmentName` ↔ `attachments[].name` **精确相等**建映射，把真幻灯号写进 `sink.images[].page`（1 起）；**匹配不到 ⇒ `page = 0`**，应用层保持整篇序号并如实注记「该图无法定位到幻灯,按整篇顺序编号」（不猜）。
  - **`label` 仍是 docstream 给的「图 N」**（解析层不改文案）；`第 N 页 · 图 k`（`k` = **该幻灯内**的图片序号）由 `appE.part` 的候选构造拼装。
  - **页数**来自 `docProps/app.xml` 的 `<Slides>`，文件没这个属性时 `pages=0`（读数里 calibre 造的 test.pptx 有 9 页）；状态行保留「共 N 页」（它是**解析层声明的幻灯数、含空幻灯**）。
  - **`page_offset` 对 pptx 可用**（`kind` 并入 `pdfPage` 标记族）。**空幻灯没有正文 ⇒ 没有标记 ⇒ 段号会缺号**：段序按标记出现的先后（`page_offset=4` 在"第 4 张幻灯是空幻灯"的样件上会命中「第 5 页」那一段），状态行的「本次 第 a-b 页」写的是**段标签里的真幻灯号**；段数 ≠ 幻灯数（`page_offset` 超过段数报 `EINVAL`「本文件共 N 页/表/幻灯…越界」，**按段数计**）—— 两个数在状态行第一次出现时就写在一起：「共 N 页(M 段有标记)」（只在两数不等时补写段数）。
  - **v1 过渡态（历史，勿当现状）**：P2 之前 pptx **没有**幻灯标记、图片也没有真幻灯号 ⇒ 状态行靠"共 N 页"+`page_offset` 一律 `EINVAL`（`kind = "slides"` 第三态）。该值已随 S15 退役。
- **限额表**：分两层，各自的**单一来源**写在代码注释里（无跨文件副本）——
  结构安全层(核心胶水 `LIMITS`,命中即拒收/跳过)与体积/超时层(OfficeKit 包装层,解析前判上限 + 计时):

  | 项 | 值 | 说明 |
  |---|---|---|
  | ZIP 条目数 / 解压总量 / 单条 / 压缩比 | 2000 / 150MB / 64MB / 120:1 | 预扫描命中即 `zip-bomb` 拒收(`LIMITS`) |
  | 图像张数 / 单图 / 图像总量 | 20 / 8MB / 32MB | 超出跳过并计数(`LIMITS`) |
  | 单文件解析超时 | 60s | 到点 terminate(主线程模式只能对调用方兜底);唯一来源 `officekit.src.js` 的 `DEFAULT_TIMEOUT` |
  | 单文件上限 | 50MB(主线程降级 20MB) | 与附件体系一致;唯一来源 `officekit.src.js` 的 `MAX_BYTES` / `MAIN_THREAD_CAP`(界面侧准入 `appD.part` 的 `officeSizeCap()` 直接读 `OfficeKit.limits.mainThreadSizeCap`,不留同值副本) |
  | 文本上限 | 默认 131,072 字符 | `ATTACH_TEXT_MAX`(`appD.part`),与胶水里的 `TEXT_MAX` 必须一致(目前靠注释承诺,无构建期断言);**可按次覆盖**:`OfficeKit.parse(buf, ext, {textMax: n})`(P2 的 `S12` 已落地,`0` / 非法值 = 不指定 ⇒ 退回默认;`ReadOffice` 工具自己的 schema 里**没有**这个参数,恒用 131,072) |
- **加密 OOXML 的识别**靠魔数:加密的 `docx/xlsx/pptx` 会被 Office 另存成 CFB(OLE2)容器 → 见到 `D0 CF 11 E0 A1 B1 1A E1` 直接给「可能已加密或受密码保护」,不去猜密码。
- **`file://` 下 classic blob worker 可用**(与 Tesseract 的 worker 是同一类路径);worker 里 `importScripts(blob:)` / `fetch(blob:)` 会被浏览器拒绝,所以载荷一律**随 worker 源一起塞进去**,不用 `importScripts`。
- **`ReadOffice` 工具复用本引擎**：`ReadOffice` 遇 doc / docx / ppt / pptx / xls / xlsx 时直接调 `OfficeKit.parse`（与工作区预览同一调用式，不走 `resolveOffice` 的 File+vision+toast 包装）；抽取文本走共享分页内核 `wsLinePageResult`（"文件:…"信息并入末尾状态行、不占行号），文档图片按模型视觉能力附带（单次 ≤4 张、累计 ≤1 MiB）或非视觉（文本为空时）走 OCR。**`Read` 自己不再读文档**：遇办公六格式或 PDF 一律返回 `EINVAL` 重定向（按后缀逐类给词 + 可照抄的 `path` JSON），且不产生任何解析副作用。
- **文档读取的文本上限默认 131,072 字符**（本节前文的 `TEXT_MAX` = `appD.part` 的 `ATTACH_TEXT_MAX`）：超限部分被胶水 `raw.slice(0, TEXT_MAX)` 真删掉 ⇒ **任何偏移都取不到**，工具侧只如实注记「仅前 N 字符,剩余内容本工具读不到(原文共 M 字符)」。**上限已可按次覆盖**（P2 的 `S12` 已落地）：`OfficeKit.parse(buf, ext, {textMax: 1048576})` 可取到全文（实测 189,802 字符的 docx：默认档 `clipped=true` / 长度 131,072，传 1 MiB 后 `clipped=false` / 长度 = `textChars`，且前 131,072 字符与默认档**逐字节相同**）；`textMax` 缺省 / `0` / 非法值一律退回默认 —— **`ReadOffice` 工具的 schema 里有意没有这个参数**（它是包装层/胶水的按次能力，应用层恒用 131,072 并如实注记）。
- 只在 **Chrome/Edge** 实测;**Safari / Firefox 未实测**(与项目其它内嵌库同一条边界)。

## 7. 非视觉模型的图片处理（图文按序融合，批 B）

**行为**：模型不支持图像输入（`visionState().vision === false`）时，`docx / xlsx / pptx` 抽出来的图片**不再丢弃**——
用同一套助手（`appD.part` 的 `ocrSeqCands` / `ocrBlockText` / `docFusePages`）逐张本机 OCR 后插进正文。
**两条路径的插入位置口径不同（别混）**：
- **`ReadOffice` 侧（工具结果）**：pptx 的正文自带 `----- 第 N 页 -----` 幻灯标记（见第 6 节）⇒ 走 `layout:"pages"`，
  块插到**对应幻灯段之后**、块头 `【第 N 页 · 图 k · 本机 OCR】`（`k` = **该幻灯内**的图片序号；P2 的 `S15` 已落地，
  此前一律 `flat` ⇒ 图全堆在文末）；docx / xlsx 没有页 / 表标记 ⇒ 仍 `layout:"flat"`（正文之后按序接，块头 `【图 k · 本机 OCR】`）。
  识别不到幻灯的图（`page = 0`）按 `docFusePages` 的既有口径追加到文末、块头标 `(原页未在文本中找到)`，并记一条
  「另有 N 块图片未匹配到页号,已追加到文末」。
- **附件侧（选文件发消息）**：office 三格式一律 `layout:"flat"`（正文之后按序接，块头 `【图 k · 本机 OCR】`）；PDF 走 `layout:"pages"`。
  本批（P2 的 S15）**只改了 `ReadOffice` 侧**，附件侧一字未动。
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

## 8. 写入（OfficeWrite）—— docx / pptx 生成与改写

> 本章只记**已实现的事实**。范围 = 方案 `shared/specs/office-tool-plan.md` 的批次 A–D（载荷地基 / docx / pptx / 工具接线）；
> 批次 F（图表 + 内嵌工作簿）、G（页眉页脚）、H（表格 / 图片 / 套用版式）落地后须回来补本章对应小节。

**文件与分层**：源码 `src/officewrite.src.js`（IIFE，ES5 + `var` + `function`），由 `scripts/make-office-part.js`
追加为 `src/office.part` 里的**第二个可执行 `<script>` 块**（现盘块结构 = 5 个 `text/plain` 载荷 + 可执行 OfficeKit + 可执行 OfficeWrite = 7 块）。
它**只做纯数据变换**：不碰工作区、不发请求、不写日志、不新增全局（`fflate` 一律走 **CJS 垫片**
`new Function("module","exports",src)(m,m.exports)` 取自 `office-lib-fflate` 载荷文本，**正常路径下 `typeof window.fflate === "undefined"`**）。

**对外契约（`window.OfficeWrite`）**：

```js
OfficeWrite = { available, why, version, formats:["docx","pptx"], limits:{...}, diag(), run(input, spec, opts) }
run(input, spec, opts) -> Promise<Result>     // 永不 reject
input = { bytes: Uint8Array } | null          // 仅编辑类需要;create 传 null
spec  = { op, fileType, ... , props }         // 字段名与工具 inputSchema 同名(snake_case)
opts  = { signal: AbortSignal, onProgress: fn }
成功 = { ok:true, bytes, mime, size, counts, warnings:[String], steps:[String] }
失败 = { ok:false, code, error }              // code ∈ EINVAL / EBADZIP / ENOTSUP / EOFFICE / EABORT / EINTERNAL / E2BIG
```

- **只读 operation（`outline`）的成功体不带 `bytes`**：返回 `{ok:true, readOnly:true, size:0, outline, counts, warnings, steps}`。
  工具层（`officeToolRun`）按 `readOnly === true` **分支**、绝不走写盘（批次 B 审查 P2-1 的接线前置条件）；
  给模型看的 `size` 由工具层填**源文件字节数**（`0` 在那边会被读成"文件是空的"）。
- `create` 不接收输入 ⇒ 不覆盖已存在的文件这件事由**工具层**保证（先 `wsStat` 查一次 + 写盘用 `wsWriteBlob` 的 `mode:"create"`）。
- **`E2BIG` 是合法返回码**（由 ZIP 中央目录预扫描 `zipPrescan` 产生），工具层**原样带回**。

**部件模板**（自备模板 = 参考实现"底稿 pptx"的浏览器等价物；模板串里**不得出现字面 `<script` / `</script`**，
构建期「检测 13」另行断言每个 `var TPL_*` 块**每个部件都带 XML 声明**）：

- **docx**：固定 7 件 —— `[Content_Types].xml` / `_rels/.rels` / `docProps/{core,app}.xml` / `word/{document,styles,numbering}.xml`；
  `word/_rels/document.xml.rels` **按需**（出现页眉页脚 / 图片 / 图表 / 超链接才建）。
- **pptx**：固定 16 件（含 `ppt/theme/theme1.xml`、`ppt/slideMasters/slideMaster1.xml`、`ppt/slideLayouts/slideLayout{1,2}.xml`、`ppt/tableStyles.xml`）
  + **每张幻灯 2 件**（`ppt/slides/slideN.xml` + `ppt/slides/_rels/slideN.xml.rels`；**rels 部件名 = `relsPathOf(slidePart(n))`，全库唯一命名源**，
  批次 C 的 P0 就是写读两侧各算一套命名导致的"真 PowerPoint 打不开"）。

**保真承诺（可判定）**：编辑一律「全量解包 → 只改被点名的部件 → 重压」。

- **未点名的 zip 条目逐字节保留**（判据 = 解包后逐条目 md5 100% 相等）；被点名部件里未被修改的 XML 节点同样不动。
- **整包字节不可复现**：`fflate.zipSync` 未给 `mtime` 时用当前时间 ⇒ 同一输入两次产出的 zip 字节必然不同
  （DOS 时间戳，实测 `3f0a` → `3f0b`）。**任何"同输入同输出"类断言都不适用**，判据一律用逐条目内容。
- `replace_text` 命中**跨多个 run** 时该段退化为「整段重写」（保留 run 节点与其 `rPr`、把文本写回首个 `w:t`、
  清空其余 `w:t`），该段 run 级格式可能变化 —— **一律写进结果 `warnings`**；0 命中不是错误
  （`ok:true` + `counts.replaced=0` + `unchanged:true`，原字节原样回传）。

**限额**（`OfficeWrite.limits`；ZIP 四项与解析层 `src/office-worker.src.js` 的 `LIMITS` **同值**，构建期等值断言）：

- `maxBytes` 20 MB（**工具层**的输入上限 `OFFICE_MAX_BYTES`；工作区层不设 —— `wsWriteBlob` 只判配额）、`maxSlides` 200；
  `maxEntries` 2000 / `maxTotalUncompressed` 150 MB / `maxSingleEntry` 64 MB / `maxRatio` 120（**中央目录预扫描，先于任何解压**）；
  `maxImageBytes` 8 MB / `maxImagesPerCall` 4（批次 H 用；`4 × 8 MB == LIMITS.maxImagesTotal` 是构建期的一条算式断言）。

**写盘前自检 `validatePackage`**：批次 B 落 ①–④（必需件齐备 / Content_Types 覆盖 / rels 目标在包内 / 每个 `r:id` 有自己的 rels），
⑤–⑦（图表链、内嵌工作簿可解、宿主要素与 `a:graphicData/@uri` 白名单）随批次 F/H 落地 —— 现返回
`checked` / `pending` 两个数组**显式带出**，免得被读成"七条都过了"。失败 ⇒ `ok:false`（`EOFFICE`）且**不产出字节**
（`finishDraft` 里 `validateEntries` 排在 `zipBuild` 之前、不过即 `return`）⇒ 调用方拿不到可写字节。

**工具接线（`src/appE.part`，方案批次 D / S19–S22）**：

- `AGENT_TOOLS` 追加 `Office`（`scope:"write"`、`cap:"office"`、`defaultPerm:"deny"`、`needsWorkspace:true`、
  `wsPathArgs:["path","image_path"]`、`icon:"file"`）。默认「禁止」的**可观测语义 = 模型看不到它**
  （`toolPermOf` 回落 `defaultPerm` ⇒ `activeTools()` 对 `deny` 直接跳过 ⇒ 请求体 `tools[]` 不含 `Office`），
  但设置 → MCP 工具的「文件」组**默认态就显示这一行**（组标题计数由运行时的 `g.list.length` 得出）。
- `CAP.office = !!(OfficeKit.available && OfficeWrite.available)`：office 载荷缺失时整条工具自动隐藏。
- 参数判据的唯一出口 = `officeSpecCheck()`（13 项 operation → 后缀 × `file_type` → 按 `file_type` 选表逐参数 →
  必填 → 值域 → 文件类型绑定（pptx 传 `header` 一类 ⇒ `EINVAL` 点名句）→ 交叉一致性（`chart.series[].values` 长度 ==
  `categories` 长度、`table.rows` 各行列数一致）→ `image_path` 后缀 / 存在性 / 大小）。**失败一律 `EINVAL` + 点名句，不碰工作区**；
  `properties.page` 不在 schema 内（v1.3 已移出 v1）⇒ 按"未知字段"报 `EINVAL`。
- 结果 JSON = `{path, file_type, operation, size, sizeDelta, counts, warnings}`（`outline` 另带 `readOnly:true` 与 `outline`），
  **`warnings` 放在末尾**且 `Office` 已加入 `RESULT_TAIL_TOOLS`（工具结果按**保尾**截断）⇒ 结果超预算时 `warnings` 必然存活。
- 错误码全链路原样带回：`wsToolRun` 只负责把 `{code, error}` 包成 `错误文案(错误码 X)`。

**实测覆盖（批次 A–D）**：`zipSync→unzipSync` 往返、四个夹具「解包→原样重打包→再解包」逐条目 md5 全等、
`zipPrescan` 对 bomb / fake 夹具的负例、docx / pptx 的 create 与逐 op 编辑（含跨 run 降级与 0 命中）、
`validatePackage` 四条注入负例、**真实 PowerPoint COM** 双向复现（正名能开 / 只改回错名 `0x80070570` / 修前件打不开而仅改名即能开）。
读数与脚本存档在 `shared/tmp/office-tool/{a,b,c,d}/`（临时装置，按方案 `E` 批收口）；逐批摘要见 `shared/progress/office-tool-*-done.md`。

**边界（未做，别当已验）**：批次 F/G/H 的五个 operation（`add_table` / `add_image` / `add_chart` / `set_header_footer` / `apply_layout`）
只有 schema 与参数判据，**载荷未实现**（调用返回 `EINVAL` 并说明）；docx 侧未走 COM；改变既有图表数据 / 批注 / 公式 / 智能艺术 / 宏 / 加密文档 /
老格式（`.doc`·`.ppt`）写回一律不支持；`normal` / `minimal` 两档的页内验收与 Thorium M122（Win7）真机未实测（分发目标，待目标机复跑装置）。
