# 办公文件解析内嵌产物（OfficeKit）笔记

> 状态：现行 —— 办公六格式解析的取件、worker 加载与限额的唯一记录；升级四库版本或改动 `src/office.part` 前必读
> 更新：2026-09-19 · 载荷段 `src/office.part` 文件 = 3,429,087 字节（3.43 MB；7 块载荷合计 3,428,845 B + 头注释 235 B + 7 处块间换行 7 B；md5 `94ee8e08205bb39c40e148952c68aed6`）
> 上游版本：mammoth 1.12.3 / SheetJS CE 0.20.3 / @jose.espana/docstream 0.1.3 / fflate 0.8.3 · 生成脚本 `scripts/make-office-part.js`（幂等 + sha256 断言）
> 取件：按生成脚本 `LIBS` 表里的 URL 下载四个浏览器单包到 `vendor/office-src/`（文件名见 §1 表）；哈希不符即 `exit 1`
> 测试：一次性 CDP 无头脚本（只跑 `file://`）已删除，结论与原始读数见 §5；要复现请按 `CONTRIBUTING.md`「验证」现写

---

## 1. 版本与体积

| 部分 | 来源(上游发行版,落 `vendor/office-src/`；胶水 / 包装 / 写入层为本项目源文件) | 原始 | 产物(`<script>` 整块) |
|---|---|---|---|
| mammoth | `mammoth@1.12.3 mammoth.browser.min.js` | 636,898 B | 637,080 B |
| SheetJS CE | `xlsx-0.20.3 xlsx.full.min.js`(官方 CDN 版) | 951,904 B | 952,096 B |
| docstream | `@jose.espana/docstream@0.1.3 dist/officeparser.browser.js` | 1,271,443 B | 1,271,548 B(剥 shebang 1 处 + sourceMappingURL 1 处) |
| fflate | `fflate@0.8.3 umd/index.min.js` | 33,311 B | 33,206 B(剥 jsdelivr 横幅 267 B) |
| 解析核心胶水 | `src/office-worker.src.js` | 34,813 B | 35,014 B |
| OfficeKit 包装层 | `src/officekit.src.js` | 20,029 B | 20,091 B |
| OfficeWrite 写入层 | `src/officewrite.src.js` | 479,745 B | 479,810 B |
| **合计** | | | **3,428,845 B** |

`src/office.part` = 上表 7 块 3,428,845 B + 头注释 235 B + 7 处块间换行 7 B = **3,429,087 B**（上表各列均为 2026-09-19 现取；文件 md5 `94ee8e08205bb39c40e148952c68aed6`）。
块结构 = 5 个 `text/plain` 载荷（四个上游库 + 解析核心胶水）+ 2 个可执行块（`OfficeKit` / `OfficeWrite`）。

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

**预算（唯一来源 = `appD.part` 的常量）**：单次附件解析最多 `ATT_IMG_MAX_IMAGES`(10) 张、**OCR 阶段**总预算 `ATT_OCR_TIMEOUT_MS`(60 s，
**整次解析共享**、不是每图)；注意它只覆盖 **OCR 阶段** —— 取图调用 `pageImages` / `renderCrops` / `renderPages` 各自另有一次 60 s 超时，
取图极慢时整条解析的墙钟会超过 60 s。融合后正文超 `ATTACH_TEXT_MAX`(131072 字符) 截断并标 `clipped`。
达到任一预算**立即停**并如实注记（不等满 60 s 再丢图）；`OCRKit.recognize` 单次调用不可中断（只能 `terminate`）⇒ 超时 / 中止的生效点只在两张之间。

**实测（无头 Chrome 152 + 真实夹具，批 B-Ⅱb）**：`code.docx / code.pptx / code.xlsx` 在 `visionForce="no"` 下
`att.text` 命中图片暗号（`AZUSA-IMG-7Q`）、块头与计数正确、请求体里没有 image part；`visionForce="yes"` 下图片按现状以 image part 发出、
请求体不含「本机 OCR」字样；无图文件（`test.xlsx` / `plain.docx`）与视觉格请求体与批前**逐字节相同**。
逐条读数与证据路径见 `shared/progress/netdocs-B2b-done.md`。

## 8. 写入（OfficeWrite）—— docx / pptx 生成与改写

> 本章只记**已实现的事实**。范围 = 方案 `shared/specs/office-tool-plan.md` 的批次 A–H（载荷地基 / docx / pptx / 工具接线 /
> 图表与内嵌工作簿 / 页眉页脚 / 表格·图片·套用版式）；0.2.1 的 docx·pptx 补口（`set_paragraph` / `paragraph_index` / `item_offset` /
> `set_table_cell` / `slide_index` / `move_slide` / `delete_shape` / `set_geometry` / `add_text_box`）与 xlsx·xls 写路径另见 `CHANGELOG.md` 与 §9，本章不再逐 op 补小节。

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
- 参数判据的唯一出口 = `officeSpecCheck()`（现行 **28 项** operation —— docx 12 / pptx 17 / xlsx 11 / xls 4，逐格式 scope 见 §9.1 → 后缀 × `file_type` → 按 `file_type` 选表逐参数 →
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

**边界面（「本章未补小节」≠「未实现」）**：批次 F/G/H 的五个 operation（`add_table` / `add_image` / `add_chart` / `set_header_footer` / `apply_layout`）
**已实现并过审（0.2.0 起）** —— `officeRun` 分派与 `OFFICE_SCOPE` 都在位，本章只是没为它们逐 op 补小节（细节见 `CHANGELOG.md` 与各自批次报告）。**真正未做**：**Word / PowerPoint COM 不可用**（本机 Word 的 `AddChart2` / `SaveAs2` 会挂起、PowerPoint COM 不可用）⇒ docx / pptx 写 op 的产物未在 Word / PowerPoint 里打开复核；docx 侧未走 COM；改变既有图表数据 / 批注 / 公式 / 智能艺术 / 宏 / 加密文档 /
老格式（`.doc`·`.ppt`）写回一律不支持；`normal` / `minimal` 两档的页内验收与 Thorium M122（Win7）真机未实测（分发目标，待目标机复跑装置）。

## 9. xlsx / xls 写路径（Excel 外科编辑与值级重写）

> 本章只记**已实现的事实**（X1-a…X2-b 五批，各批 PASS 后入账）；docx / pptx 写路径见 §8，读路径见 §6 / §7。
> 分层：载荷仍是 `src/officewrite.src.js`（= `src/office.part` 的第二个可执行块，见 §8「文件与分层」），本章只补 Excel 侧。

### 9.1 能力面（工具 `Office` 的 xlsx / xls scope）

- **xlsx 恰 11 项**：`create` / `outline` / `set_cell` / `set_range` / `add_sheet` / `rename_sheet` / `delete_sheet` / `move_sheet` / `insert_rows` / `delete_rows` / `set_properties`。
  `set_cell` 用 `address` + `value` / `formula`（二选一）/ `value_type` / `number_format`；`set_range` 用 `range` + `rows`；**`rename_sheet` / `delete_sheet` / `move_sheet` 强制点名 `sheet`**（不给就默认第一张的误伤面太大）；`insert_rows` / `delete_rows` 用 `at` + `count`；`outline` 用 `cell_offset` 分页（不是 docx / pptx 的 `item_offset`）。
- **xls 恰 4 项**：`create` / `set_cell` / `set_range` / `convert`。**`outline` 已撤下**（BIFF 没有"工作表摘要"这条读语义，列了就是"工具面放行、载荷恒拒"的说谎面）⇒ 读 `.xls` 内容统一走 `ReadOffice`（grid 视图能读）。
- 上限（`OfficeWrite.limits`，与 `make-office-part.js` 断言面同值）：单文件 20 MB、表数 ≤ 20、每表 ≤ 200 行 × 30 列（create / set_range 面）；xls 另有**双闸**（§9.3）。
- 工具层与载荷层的分工：参数判据（键集合 / 必填 / 类型 / 值域 / 后缀与 `file_type` 一致）在工具面 `officeSpecCheck`；**数值与语义规则（地址、表名、格式串、引用同步）的唯一实现在载荷**，工具面不写第二份。

### 9.2 xlsx 外科编辑：机制与保证面

- 编辑一律「全量解包 → 只改被点名的部件 → 重压」；**未点名的 zip 条目逐字节保留**。判据 = 解包后**逐条目内容** 100% 相等，且"预期被改部件集"**每用例运行期现算**（普通值 = 目标 sheet；`formula` = + `xl/workbook.xml`；`number_format` = + `xl/styles.xml`；增 / 删 / 改名 / 移表 = + `xl/workbook.xml` / `xl/_rels/workbook.xml.rels` / `[Content_Types].xml` 等）。
- **`insert_rows` / `delete_rows`**：只动目标表部件的两处机械量 —— 每个 `<row>` / `<c>` 的 `r` 属性 + `dimension@ref`；行的属性（`ht` / `customHeight` / `s` / `spans`）随元素一起走。`dimension` 口径：insert 只扩不缩（与原 ref 取并集）、delete 允许收缩、整表无格时落 `"A1"`。**引用（公式文本 / 条件格式 / 数据验证 / 合并区 / 超链接 / 表对象）一律不重写** ⇒ 成功体**恒带**告知（R-X2 的"确定发生"）；`insert ×n` 后 `delete ×n` 同数可逐字节回到原部件（位移是可逆的纯机械变换）。
- **`number_format`**：三路解析（界面名别名 → 内建格式串 → 受控自定义 ≤ 64 字符）；内建两路**不落 `numFmts`**；自定义走 `numFmts`（同 code **复用**、新 id ≥ 164）且 `cellXfs` 同 `numFmtId` 复用（**不新增 xf** —— 每次新建 xf 会让用户文件的样式表无限膨胀）；显式格式**优先于** `value_type:date` 的默认 numFmt 14；替换既有 `s` 如实进 `warnings`；非法格式拒（`EINVAL`）—— 格式串是写进产物 XML 的**外部输入**，不收口会被 Excel 判"修复"。
- **`move_sheet`**：重排 `<sheets>`；**表身份不动**（`sheetId` / `r:id` / 部件 / rels / CT 全不变，包内只改 `xl/workbook.xml`）；`definedNames/@localSheetId` 与 `bookViews/@activeTab` 是**表序号** ⇒ 按同一置换重映射（不重映射 = 命名区域静默改挂）；`xl/calcChain.xml` 的表序号**未重写 ⇒ 如实进 `warnings`**。
- 写"只有公式没有缓存值"的格会置 `calcPr@fullCalcOnLoad="1"`（仅当原文件没有该属性）让 Excel 自己重算；合并区非左上角、表名非法 / 重名、删到没有可见表、`at` / `count` 非法或越界等一律拒绝并点名。
- 校验：`validateXlsx` 自检 + Excel COM 接受（只读打开无修复提示 + 逐格相符；真值件含 `definedNames` / `calcChain` / `activeTab` / 多表 / 一个表级名字与打印区域）。

### 9.3 xls 路线：值级重写、损失清单与双闸

**读选项固定** `{type:"array", cellStyles:true, cellNF:true}` —— `cellStyles` 才让 SheetJS 读出列宽（`!cols`），默认读法（`!cols === undefined`）重写即丢。SheetJS 经 **CJS 垫片 + 影子 `window` / `self` / `global`** 装载（`sheetjsOf()`）：正常路径 `typeof window.XLSX === "undefined"` ∧ `typeof window.fflate === "undefined"`（全程三处采样）。

**损失清单（成功体 `warnings` 恒带；措辞边界 = 实测，不写没量过的话）**：

- **公式未保留** —— BIFF 写出面不含 `f`（`B4=SUM(B2:B3)` 重写后只剩缓存值 `v=15.5`；**没有缓存值的公式格会整格消失**）；写公式**直接拒**（`ENOTSUP`，请写结果值）。
- **样式 / 行高未保留** —— 字体 / 填充 / 边框 / 对齐读得出、落不下去（`!rows` 写读 = `undefined`）。
- **内嵌对象不复制** —— 图表 / 图片 / 批注 / 透视表等（重写只带 SheetJS 的值层模型）。
- **文档属性（标题 / 作者等）不保留** —— 写出前清 `Props` / `Custprops`（见下"Excel 拒开"）。
- **保留项（实测）**：值（含中文、空格敏感串）/ 数字格式（日期 `z` 与显示值原样）/ 列宽 / 合并单元格。
- `convert`（xls→xlsx）另有自己的口径：值 / 公式 / 日期格式 / 合并 / 列宽全在，样式、行高、内嵌对象不在；产出是**新文件**（原 `.xls` 不动，同名 `.xlsx` 已存在报 `EEXIST`）。

**双闸（为什么按格数设闸）**：

- 内存的真正驱动量是**格数**而非字节（实测最坏 **≈ 1.2 KB/格**；Excel 真 BIFF8 的密度跨度约 **6–26 B/格** —— 23 件真值件实测）⇒ 只按字节设闸在 2 MiB 附近**天生对不齐**：旧的 2 MiB 单闸内实测到 **217.8 / 224.8 / 239.7 MB** 三件反例（宽表形态）。
- 现口径 = 双闸：`maxLegacyXlsBytes = 1,572,864`（**1.5 MiB 输入尺寸粗闸**：读相位与全流程时间上界 —— 最坏形态 1.5 MiB 读 ≈ 85 MB / 0.4 s）+ `maxLegacyXlsCells = 150,000`（**内存对齐闸**：读盘后按实际格数判 —— 15 万格 × 1.2 KB ≈ 181 MB，留 ~10% 余量）。两条闸都在 `xlsBookOf()` 里、create / set_cell / set_range / convert 全部经它（无旁路）；超任一闸 ⇒ `E2BIG` + 引导「先在 Excel / WPS 里另存为 `.xlsx`」。
- 读数（23 件真值件，含宽表 / 密集 / 字符串三种形态）：**双闸内最坏 163.9 MB / 432 ms**（审查者独立复测同件 163.7 MB）；超阈值件（605 MB / 995 MB 两件 ≥5 MB）**全在闸外**；首次 xls 操作含 SheetJS 懒加载 ≈ 47–77 ms。
- **代价（如实登记）**：尺寸粗闸会拒掉一部分内存上安全的件（如 2.09 MB / 8 万格 / 实测 143.9 MB）。优化项 = 读前扫 BIFF `DIMENSIONS` 记录、把拒绝提前到读盘前（**未做**）。

**两处"值级回读看不见"的真缺陷（X2-a 抓到；均已修 + 成为常规判据）**：

1. `x.write(wb, {bookType:"xls", type:"array"})` 在 SheetJS 0.20.3 回的是 **ArrayBuffer**（node 与浏览器同）⇒ 取不到 `length`、写出为空 ⇒ create / set_cell / set_range / convert 全挂。收口 = `xlsBytesOf()`（Uint8Array / Node Buffer / 带 `byteLength` 的视图三种形态）。
2. **带任何文档属性的 BIFF8 会被 Excel 16.0 的"文件验证"拒开**（"Office 检测到此文件存在一个问题…不能打开此文件"；最小对照：全新小簿 + 任一属性（Title / Author / Company / 字符串日期）即拒，`Props={}` 或 `undefined` 即开）。而 **BIFF8 的 read 恒把 SummaryInformation 填进 `wb.Props`（且 `Props === Custprops` 同一对象）** ⇒ **所有"读→改→写"的 .xls 在 Excel 里都打不开**；SheetJS 自己能读回 ⇒ **值级回读判据天然看不见**。修法 = 写出前清 `wb.Props` / `wb.Custprops`（`xlsWriteBook()`）。两面守护：`make-office-part.js` **检测 12c**（**顺序**断言：清属性语句必须出现在 `x.write(wb, {bookType:"xls"` **之前**）+ 套件判据 **X2**（产物字节里 `\u0005SummaryInformation` / `\u0005DocumentSummaryInformation` **都不出现**，且先断言输入件带这两条流 ⇒ 判据非空转）。

**回归纪律**：凡改到 xls 写出面（`xlsWriteBook` / `xlsBytesOf` / 损失文案）的批次**必须**跑一次 Excel COM 只读抽验（正例 = 逐格读数与工具自报一致）；**且必须在批末按 PID 复核并清理 `EXCEL.EXE`** —— 实测即使 `Quit` + `ReleaseComObject` 都执行了，进程仍可能滞留（不能只依赖脚本内的释放）。

### 9.4 未覆盖边界（不得当已验证）

- **真模型工具调用链未跑**：没有发起过一次真模型的 `Office` 调用；"模型侧可达"= `enum` / `OFFICE_SCOPE` / 文案逐条读回 + `officeSpecCheck` **运行期**收 / 拒读数 + **执行级 `officeToolRun`**（真工具实现 × 内存工作区）。
- **只测本机 Excel 16.0**：其它 Office 版本 / LibreOffice / WPS 对 SheetJS xls 的接受度未验；`.xlsm` / `.xlsb` / ODS 与 xls 批注读面未涉及。
- `number_format` 在 Excel 里的**显示值**渲染核对（断言只到 `numFmtId` / `formatCode` / 格上的 `s` 指向）；`move_sheet` / `insert_rows` 的 COM 接受度；合并区 / CF / DV 在位移后的语义正确性（只保证"如实告知未重写"，不保证 Excel 侧的最终解释）。
- **Word / PowerPoint COM 不可用**（本机无 PowerPoint COM）：pptx 的 `move_slide` 顺序、`add_slide@k` 落点、段级 `set_paragraph` 在真 PowerPoint 里的观感未实测；docx 产物未用 Word 打开（域 / 修订 / 内容控件包裹的段只验"文本被换 + warning 如实"）。
- `full` / `normal` 两档的浏览器真机未跑（沿用既有口径只验 `minimal` 档）；Win7 本体未测。

## 10. docx 渲染页图（S7 批：docx-preview + jszip + modern-screenshot）

> 本章只记**已实现的事实**（b31-S7 批；三档产物均在 `src/render.part` 内联三库）。读路径总览见 §6/§7，写路径见 §8。

### 10.1 库、生成链与加载约束

- **三库**（`vendor/render-src/MANIFEST.json` 锁定字节 / sha256 / tgz 哈希）：[docx-preview](https://github.com/VolodymyrBaydalka/docxjs) 0.4.0（Apache-2.0）· [jszip](https://github.com/Stuk/jszip) 3.10.2（双许可，**采 MIT 支**）· [modern-screenshot](https://github.com/qq15725/modern-screenshot) 4.7.0（MIT，dist 无许可头 = 上游事实）。生成 = `node scripts/make-render-part.js`（断言 1–12 + `--verify` + 幂等；主机白名单并集恰 8 个）；产物 `src/render.part` ≈ **203,364 B**（4 个可执行块：jszip → docx-preview → modern-screenshot → `window.RenderKit`），由 `build.js` 拼在 **office 之后、pyodide 之前**；缺件 ⇒ 警告 + 跳过（能力表自动 false，走既有降级文案）。
- **加载顺序硬约束**：docx-preview 的 UMD 在**加载时**读 `globalThis.JSZip` ⇒ jszip 必须在前（`render.part` 块序即依赖序）。
- **OPTIONS 钉死**（parse 与 render 传同一对象）：`useBase64URL:true`（图 / 内嵌字体全 `data:` URL，无 `blob:` 路径）· **`renderAltChunks:false`**（斩断 altChunk 的 `<iframe srcdoc>` 取网面）· **`ignoreLastRenderedPageBreak:false`**（按 Word 存的 `<w:lastRenderedPageBreak/>` 分页 —— 贴 Word 页数的关键一项）· `breakPages:true` / `renderHeaders/Footers:true` / `inWrapper:true` 等。
- **资源守卫 = wrap 三方法（值级阻断；唯一有效层）**：把 `doc.loadDocumentImage` / `loadNumberingImage` / `loadFont` 三个装载口一并包装，原实现返回空或抛错时改返回 1×1 透明 GIF 的 `data:` 哨兵，计数 `{img,numbering,font,errors}`。**为什么必须是值级**：缺失部件 ⇒ 三方法返回 `null`，而 `null` 被赋给 `img.src` / 拼进 CSS `url()` 的**那一刻**就发起加载尝试 —— 晚挂载、拆 parse+render、事后属性守卫**都拦不住**（对照实验：晚挂载 = 1 次、去 wrap = 1 次、只包 `loadDocumentImage` 而漏 numbering/font ⇒ 0→2 次；三方法 wrap = 0 次）。游离态资源清洗是**两层 + 两个计数**（防御纵深）：**属性面**（`src/srcset/poster/data/xlink:href` + 非 `<a>/<area>` 的 `href` 中和，`<a href>` 豁免）+ **样式文本面**（`<style>` 文本里的 `url(http(s)://…)` → `url(about:blocked#)`）；计数 `clean = {attr, styleUrl}` 随渲染结果返回，**正常输入下应为 0、非 0 进 `notes`**。样式文本面**当前不可由真实 docx 触达**（三库 CSS `url()` 发射点只有 `@font-face` 与 numbering 变量两处，均已被 wrap 值级截住）⇒ 用**合成节点实验**验证其承重：合入一条会命中的 `<style>url(http…)` 规则，不中和 ⇒ 请求真的发起（nonDoc=1）、中和 ⇒ 0 请求 + `clean.styleUrl=1`（修正轮 `styleurl`/`styleurln` 两变体）。
- **挂载 = 双容器（`host` + `styleHost` 两个都挂）**：`renderDocument` 的产物里 `<style>` 进 `styleHost`、其余进 `host`，两者都 `appendChild(document.body)` 后才开始布局；**只挂 `host` ⇒ `<style>` 不生效 ⇒ 页盒 794×1123 变 1034×1315、页图全变**（`file://` 实测，批内咬合对）。宿主用后即删（成功 / 失败 / 超时三条路径）。
- **零请求审计口径**：`file://` + CDP `Network.requestWillBeSent` 全量计数 ⇒「**外部主机请求 = 0 ∧ 非主文档请求 = 0**」（主文档自身、`data:`/`blob:` 不计）。产品级真机复跑（Thorium 122 非 headless，附着 + 渲染全程）实测 = 0；对照注入（去 wrap / 漏一个装载口）必红。

### 10.2 参数与闸门（与 PDF 图片档同口径 + 附加闸）

- 光栅化：`modern-screenshot.domToCanvas(section, {scale: dpi/96, backgroundColor:"#fff"})` ⇒ A4@150 DPI = **1240×1753**（与 PDF 侧 1240×1754 差 1 px = docx-preview 的 cm→px 取整，参数逐项同源）；JPEG **q0.86 首选档**；页数 ≤ `pdfMaxPages()`；超限 ⇒ `clipped=true` + 注记「文档共 N 页,按页数上限仅渲染前 M 页」。
- **附加闸门**：每张过 `attCanvasFitForModel(cv, "image/jpeg", 0.86, {maxEdge: Math.min(2000, imgMaxSide())})`（单张 **1 MiB 硬顶** + 最长边 ≤ `min(2000, imgMaxSide())`）。超硬顶 / 单页抛错 ⇒ 该页不发 + 注记（不静默）。DPI=300 边界实测：2830×4000 原生画布经闸门降为 **1414×2000 / 48,738 B** 交付。
- **空页守卫**：出图后按原生分辨率步长 8 抽样算 `inkRatio`；`=== 0` 且该 section 有非空文本 ⇒ 跳过该页 + 注记「第 N 页渲染为空,已跳过」（防 SVG foreignObject 路线的静默空白）。
- 预算：整体 `Promise.race` 上限 **60 s**（超时 ⇒ `ok:false` + 中文 error；在跑的渲染无法中止，属已知取舍）· 挂载后等 `<img>` 解码上限 **5 s**（超时继续，不失败）。
- **外部引用 / 占位注记（来源可信 = 解析层 rels）**：外部图引用 N ⇒ 「文档含 N 个外部引用(未随文档打包),已按离线模式忽略」；`wrap.img > N` ⇒ 「另有 M 张图片…按空白占位」；`wrap.numbering/font > 0`、altChunk 条数 A 各有一条对应注记；第 1 条同时以 toast 呈现一次，全文进 `att.degraded`。

### 10.3 接线与失败回退（`src/appD.part`）

- **唯一渲染出口** `attachRenderPageImages(f, ext, buf, opts)`（`ext` 非 `"docx"` / 库不在位 ⇒ `{ok:false, error:"渲染器未内嵌"}`，不抛；永不 reject）。`resolveOffice` 里以**前置链判别**接入：`isDocxImg = (pre.send==="image" && pre.render==="pages" && sub==="docx")` ⇒ 渲染成功 ⇒ 直接产出图片档 `att`（`mode:"image"` / `renderSrc:"pages"` / `text:""` —— **只发页图、不带正文，与 PDF 图片档同形**）；失败 ⇒ 既有链照走。
- **渲染失败两支注记必须落在「共同出口」**（O6 块之后再补，不许依赖 O6 块）：`cap="pages"` 时 `pre.images==="send"` ⇒ 内嵌图在 O6 复查**之前**就已落进 `images` ⇒ `!images.length` 恒假 ⇒ **O6 块根本不进**（批内实测出来的静默面）。两支 = 有内嵌图 ⇒ 「本机渲染失败(…),已改用「混合」:发送文档内嵌图」+ toast；无内嵌图 + 有正文 ⇒ 「…已改用「纯文本」:只发送文字」+ toast。
- 能力表：`ATT_DOCX_RENDER_READY`（唯一开关，S7 起 = `true`）∧ `docxRenderReady()`（`window.RenderKit.available === true`）⇒ `attachRenderCap("office","docx") === "pages"`；库缺失 / 开关翻回 ⇒ `false` + 既有 `toastNoRender` 文案（「文档暂不支持图片档(本机没有可信的离线渲染器):已改用「…」」）。
- 展示：chip 相位新增 `rendering: "渲染页面…"`；环境面板 `pdfEngineLine()` 追加「· 文档渲染器:docx-preview 0.4.0」；档位 desc 与页数 hint 已改准（`head.part` 两处 + `appD` 档位 desc）。
- **非视觉 / 混合档 / 工具侧不变**：`vision=false` ⇒ `pre.send!=="image"` ⇒ 不渲染（走内嵌图 OCR 融合腿，实测 `ocrImgs=2` / 无页图）；混合档（默认）仍 `renderSrc="embedded"`（内嵌图，逐字节回归）；`ReadOffice`/`Read` 不吃档位、不进此路径。

### 10.4 批内读数（真机 Thorium `122.0.6261.171` 非 headless / `file://`；证据 = `shared/tmp/b31-s7/{out/r1,out/r1-chrome,out/r2,prod/out/r4,prod/out/r6}`，批末归档到 `shared/archive/b31-s7-evidence-2026-09-19/`）

- **烟测门（装置臂）**：f-batch 强夹具（4 节 = 硬分页 ×2 + LRPB ×1；表格 / CJK / 页眉脚 / 纯图页 / 图片项目符号 / 内嵌字体 / altChunk 三面全中）⇒ 页盒 794×1123、canvas 1240×1753、`taint ok`、每页 ink ≥0.0019、**非主文档请求 = 0**、无异常；三组咬合对（wrap 有无 0↔1 / 双挂载 794×1123↔1034×1315 / 字体面 wrap 全 vs 漏 0↔2）在 Thorium 122 与 Chrome 153 双臂一致复现。
- **产品级（探针副本）**：docx + 图片档 + 视觉 ⇒ `mode=image / renderSrc=pages / imgs=4 / pages=4 / textChars=0`；页图 4×1240×1753 / **28.0–38.7 KB**；请求部件形状 `IIII`（四张页图，无正文部件）；chip 相位含「渲染页面…」；**逐页保真自证**（对交付页图 OCR）：页 1/2/3 的暗号 `AZUSA-P1/P2/P3` 各现在本页（conf 86–94），交换页文本 ⇒ 判据必红；页 4 只剩页眉页脚文本、无正文暗号。
- **负例（批内实跑）**：开关翻回 false / `RenderKit.available=false` ⇒ `toastNoRender` 文案逐字；渲染失败 + 有内嵌图 ⇒ 回退「混合」+ 注记（**修复前实测静默 ⇒ 已修**）；渲染失败 + 无内嵌图 + 有正文 ⇒ 回退「纯文本」+ 注记；`maxPages:1` ⇒ 截断注记 + 1 张；非视觉 ⇒ 不渲染页图（走内嵌图 OCR）。

### 10.5 未覆盖边界（不得当已验证）

- 超大 / 复杂真文档（页数 ≫ 上限、巨量表格、内嵌字体实件）的耗时与保真**未实测**；DPI=300 只取到「页 1 / maxPages=1」的闸门读数（全页全 DPI 的批量读数未做）。
- **Word 逐页对照未做**：保真判据 = 逐页暗号 OCR + 像素统计（非 Word COM 另存 PDF 的像素比对）；分页只保证「硬分页 / LRPB / 节变更 + 本库排版」，与 Word 的自动分页可能不一致（UI 已如实注明）。
- CJK 字体缺口（系统缺字时回退）与内嵌字体实件的观感差异未验；`file://` 下已验证（`taint` 不抛 / `data:` 链），Win7 本体未测（分发目标声明，验收走用户侧清单）。
- 产品级读数只在 `minimal` 档的探针副本上取（三档同码；`full`/`normal` 未真机）。
