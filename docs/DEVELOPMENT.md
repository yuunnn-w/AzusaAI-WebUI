# 开发文档

面向要改这个项目的人：**架构总览 → 构建链 → 数据与存储 → 踩坑清单 → 数值口径附录**。

- 贡献流程与验证方法（两种起手式的完整模板）见 `CONTRIBUTING.md`；
- 各内嵌库的深水区（取件、改写、`file://` 约束、实测数字）见 `docs/PDFJS-NOTES.md`、`docs/TESS-NOTES.md`、`docs/PYODIDE-NOTES.md`、`docs/OFFICE-NOTES.md`。

---

## 1. 架构总览

### 1.1 单文件构建链

整个应用是**一个 HTML 文件**：`src/` 下的手写分段与内嵌库载荷，经 `scripts/build.js` 拼装成 `AzusaAI-WebUI-<档>.html`（唯一发布物，双击即用）。

```
src/                              scripts/                         产物(代码目录根)
head.part      ─┐
libs.part       │                build.js ──→ AzusaAI-WebUI-full.html
katex.min.js    ├─────────────────┘         ──→ AzusaAI-WebUI-normal.html
pdfjs.part      │   (按固定顺序拼装,统一 LF 归一)
tesseract.part  │
office.part     │
render.part     │
pyodide*.part   │
jupyterlite.part│
appA–appE.part ─┘  (appJ.part 插在 appD 与 appE 之间)
```

`build.js` 的拼装顺序（改动顺序会导致 ID / 函数引用错乱）：

```
head.part        先替换 /*__KATEX_CSS__*/ 占位符为 src/katex-embedded.css
+ libs.part      marked + DOMPurify + highlight.js
+ <script> katex.min.js
+ pdfjs.part     缺文件 → 控制台警告，PDF 功能自动降级
+ tesseract.part 缺文件 → 同上，OCR 自动降级
+ office.part    缺文件 → 同上，办公附件自动降级
+ render.part    缺文件 → 同上，docx 图片档自动降级（window.RenderKit 缺失 ⇒ 能力表 false）
+ pyodide 载荷   按档取 src/pyodide.part（full）或 src/pyodide-{normal,minimal}.part；
                 显式档位缺载荷即 exit 1；默认档缺载荷时整族 Python 工具降级
+ jupyterlite.part  缺文件 → 打印警告并注入 JL_AVAILABLE=false 的极小占位段（不 exit 1），Jupyter 标签页不可用
+ appA.part + appB.part + appC.part + appD.part + appJ.part + appE.part
     ↑ appJ 插在 appD 与 appE 之间（同一 IIFE 的分段，不得提前闭合；appE 仍必须是最后一段）
```

拼装完成后立即用检查链验证：

```bash
node scripts/syntax.js    # 产物里每段内联脚本交给 V8 解析（跳过 text/plain 载荷）
node scripts/lint.js      # 启发式检查「调用了但未声明」的标识符
```

写出策略：每档先写 `<final>.tmp-<pid>`，成功后才 rename 到正式名——进程被杀或中途出错都不留半套产物，也不碰现有正式产物。另有 **LFS 指针守卫**：读取载荷前扫描 `src/*.part`，发现 Git LFS 指针文本（未 `git lfs checkout`）立即中止。

### 1.2 应用分段（同一个 IIFE）

`appA`–`appE` 与插在 `appD` / `appE` 之间的 `appJ` 只是把一万多行 JS 按功能切开的**分段**，拼起来必须是一个完整 IIFE：各段靠函数声明提升与模块级 `var` 共享作用域，拆开单独看会报「未定义」。**只有 `appE.part` 结尾收 IIFE 与 `</script></body></html>`**——新增分段不得提前闭合。

| 分段 | 职责 |
| --- | --- |
| `appA.part` | 环境自检 / 常量 / 设置与状态 / 存储底座 / 主题 / 连接状态 / Markdown 渲染 |
| `appB.part` | 数学公式（抽取 + KaTeX 渲染回填） |
| `appC.part` | 代码块增强 / 消息渲染 / 滚动 / 流式渲染调度 |
| `appD.part` | 会话 / 输入区 / 附件 / API 调用 / 交互 / 弹层 / 设置面板 / 命令面板 / 语音输入 |
| `appJ.part` | JupyterLite 宿主（主页面侧）：独立标签页与子文档引导 / 工作区 ↔ Jupyter Contents 镜像同步与租约 / 右键「通过 JupyterLab 打开」扩展项 |
| `appE.part` | 工具框架（注册表 / 权限 / 限额 / 沙箱 / 审计）+ Python 运行时与任务层 + 初始化入口 |

⚠️ **新增函数名前先全局搜重名**：同名函数后声明者会静默覆盖先声明者（历史 bug「新建会话空白页」的根因就是 `appE` 里的桩函数覆盖了 `appC` 的真实现）。

### 1.3 三档产物

三档**共用同一份 JS 源码**，差别只在 pyodide 载荷里嵌了哪些 wheel（界面与功能完全一致）：

| 档位 | 预置包 | 产物体积（实测） | 载荷来源 |
| --- | --- | --- | --- |
| `full`（默认） | 151 个 wheel | **228,036,277 B**（2026-09-19 终编现取，md5 `9700988b…`） | `src/pyodide.part`（入库） |
| `normal` | 99 个 wheel | **155,987,523 B**（同上口径，md5 `f5794ebc…`） | `src/pyodide-normal.part`（本地生成） |
| `minimal` | 33 个 wheel | **74,490,588 B**（同上口径，md5 `de0143ff…`） | `src/pyodide-minimal.part`（本地生成） |

档位名单 / 分组 / 落盘路径 / 体积对账值的**唯一权威** = `scripts/pyodide-profiles.json`；生成期与构建期断言（C1–C9）保证「名单不许漂移、轻档不缺依赖、被裁包的理由不许撒谎」，未知档位直接 `exit 1`。轻档载荷里带一段 `pyodide-profile.json`，运行时据此显示真实档位与包数（缺段 = 完整版）。

### 1.4 运行时要点

| 面 | 要点 |
| --- | --- |
| 双协议 | OpenAI 兼容 `/v1/chat/completions` 与 Anthropic `/v1/messages`；流式 / 非流式自动降级；`tool_calls` 与 `tool_use` 双适配 |
| 渲染 | marked + DOMPurify 净化后渲染；KaTeX 公式先抽占位符、Markdown 处理完再替换回；代码块 10 行上限 + 块内滚动；思考块与正文各自渲染 |
| 启动 | 异步启动（会话从 IndexedDB 装回）——外部等待的判据是 `document.documentElement.dataset.boot === "done"` |
| 存储 | localStorage 只放设置（键 `chatgpt-webui-v1`）；会话与附件在 IndexedDB `chatgpt-webui-blobs`；不可用时整体回退旧模式 |
| 工具 | `AGENT_TOOLS` 注册表（`appE`）；权限三态（允许 / 询问 / 禁止）+ 限额；执行沙箱是独立 Web Worker，默认网络为抛错桩 |
| Python | pyodide 载荷懒解码；解释器懒启动、**首次使用时阻塞装载本档全部预置包**（装载时间不计入脚本超时）、空闲 **10 分钟**回收（有未完成任务时保活）；停止 = 直接销毁解释器，产物已回写工作区的不受影响 |
| 外观 | 12 套马卡龙主题色：所有跟随主题的颜色都从 `--accent-rgb` 派生；深 / 浅两套 CSS 变量必须成对定义 |

---

## 2. 数据与存储

### 2.1 localStorage（只放设置）

- 键名 `localStorage["chatgpt-webui-v1"]`；`schema: 4` 控制一次性迁移（`mergeSettings`）。
- 只存 `settings` + `activeId` + `promptLib` + `fetchedModels`；会话本体在 IndexedDB。
- 特殊值约定：`temperature` / `topP` 为 `-1` 表示「不发送」（滑块拉到最左）；`reasoningEffort: ""` 表示不发送；`maxTokens: 0` 表示不发送（Anthropic 协议必须带 `max_tokens`，未设置按 256K 发送）。
- `mergeSettings` 的迁移动作（只执行一次）：旧强调色名 → 马卡龙色名、旧硬编码采样参数 → 不发送、`codeWrap` 默认改真；历史版本还做过 `delete ctxBudget`（按 token 截断历史整体移除，只留「最多携带条数」`ctxMsgs`）、删除旧的「对外暴露工具服务」配置、`toolLimits` 改新默认、`run_js` → `ExecuteJavaScript` 的权限键搬迁等；**0.2.1 起**多了一条条件式迁移 —— `pdfMode` → `attachImgMode`（三档附件图片策略）：显式 `"image"` ⇒ `"image"`、其余（含旧 `"text"` / 缺键 / 非法值）⇒ `"both"`，**旧键不删**（单向兼容：老用户盘上的 `pdfMode` 原样保留，旧版本回退仍能读到；新版本只写新键）。
- **Base URL 不做自动迁移**：旧默认曾是内网地址，现不保留其字面量（`DEFAULT_BASE` = 本机回环 `http://127.0.0.1:8080/v1`）；任何已存值（老默认、自定义值）都原样保留，只有「缺 `baseUrl` 字段」的设置才会被合并上默认值。
- `schema` 未升但后来新增的字段（`attachImgMode` / `pdfDpi` / `pdfMaxPages` / `ocrLangs` / `visionForce` / `modelVision` / `pyTaskTimeoutMs` / `genMax` 等）都有默认值，旧库不需要额外动作；`attachImgMode` 是唯一例外 —— 它有一条条件式迁移（见上一条），旧 `pdfMode` 只作读入来源、新版本不再写出。**0.2.1 起** `pyTaskStopKills` 被显式丢弃（D-5 裁决作废该设置项：`delete out.pyTaskStopKills`，旧库载入 / 导入备份都不复活，幂等）。
- 迁移完成后写入当前 `schema`，之后用户手动修改不会被改回。

### 2.2 IndexedDB（会话与附件，v3 五个存储区）

- `convs`：一条会话一条记录（消息正文也在这里，不受 localStorage 5MB 限制）。
- `blobs`：附件 payload（图片 / PDF 页的 base64）。
- `workspaces` / `wsfiles` / `pyocache`（v3 新增）：工作区元数据 + 工作区文件二进制（存 `Blob` 而非 base64）+ pyodide 运行时缓存。
- **加存储区必须升版本号 + 按 `objectStoreNames` 补齐 + 确认会话区就绪后才允许迁移**（历史上真的丢过一次数据）；`gcBlobs()` 每次启动必跑，但**只扫 `blobs` 区**，三区工作区数据不在回收范围。
- 工作区写入的唯一入口是 `WS.*`（内核用 `wsLock(wsId)` 串行化「读改写」），UI 侧不得自己开 IDB 事务写工作区。
- IndexedDB 不可用（含启动 4s 超时）时整体回退：会话与附件都内联在 localStorage，行为照旧、面板写明原因。
- 导出备份时附件内联回 JSON；工作区文件不进备份（只带清单）；「清空全部」连工作区与 pyodide 缓存一起清。

### 2.3 内嵌库总表

| 库 | 版本 | 用途 | 载荷 / 体积 |
| --- | --- | --- | --- |
| marked | 12.0.2 | Markdown → HTML | `src/libs.part` |
| DOMPurify | 3.1.6 | HTML 净化（XSS 防线） | `src/libs.part` |
| highlight.js | 11.9.0 | 代码高亮 | `src/libs.part` |
| KaTeX | 0.16.11 | 数学公式（20 个 woff2 字体 base64 内联） | `src/katex.min.js` + `src/katex-embedded.css` |
| pdf.js（pdfjs-dist legacy） | 6.3.289 | PDF 文字抽取 + 页面渲染（已修 CVE-2024-4367） | `src/pdfjs.part` ≈ 1.80 MiB（1,882,880 B，2026-09-18 现取） |
| Tesseract.js（+ core） | 7.0.0 | 图片 OCR（eng + chi_sim，tessdata_fast） | `src/tesseract.part` ≈ 6.37 MiB |
| mammoth | 1.12.3 | docx 文字 + 正文图片 | `src/office.part` |
| SheetJS CE | 0.20.3 | xlsx / xls → CSV | `src/office.part` |
| @jose.espana/docstream | 0.1.3 | pptx / doc / ppt 文字 + 图像 | `src/office.part` |
| fflate | 0.8.3 | ZIP 结构预扫描（ZIP 炸弹防线） | `src/office.part` ≈ 3.27 MiB 合计（**3,428,845 B** = 7 块合计 / **3,429,087 B** = 文件字节，2026-09-19 现取） |
| docx-preview | 0.4.0 | docx 逐页渲染成页图（图片档） | `src/render.part` |
| jszip | 3.10.2 | docx-preview 的 ZIP 依赖（采 MIT 支） | `src/render.part` |
| modern-screenshot | 4.7.0 | DOM → canvas 光栅化（页图） | `src/render.part` ≈ 203,364 B 合计（2026-09-19 现取） |
| pyodide（+ CPython 3.14.2 标准库） | 314.0.6 | Python 运行时 | `src/pyodide.part` ≈ 186.7 MiB |
| JupyterLite（jupyterlite-core + lab 站点） | 0.8.3 | JupyterLab 站点（独立标签页、工作区为根） | `src/jupyterlite.part` ≈ 16.3 MiB（17,099,161 B，2026-09-18 现取） |
| JupyterLab（含 lumino / CodeMirror 等运行期依赖） | 4.6.3 | lab 站点构建 | 同上 |
| jupyterlite-pyodide-kernel | 0.8.6 | Jupyter 内核（复用 `src/pyodide.part` 的核心集，不重复打包） | 同上 |
| jedi / parso | 0.19.2 / 0.8.6 | 内核属性补全（子批 B 落位） | 同上（`pyodide/*.whl`） |

三份产物内的 SVG 图标为手写图标精灵（**66** 个 symbol；2026-09-18 现取，以取件时刻为准），不依赖图标字体。内嵌的 wasm / 训练数据放在 `<script type="text/plain">` 里（不会被当成 JS 执行，也不会被语法检查误报）。`syntax.js` 必须跳过这类块，否则会报一堆语法错误、把真正的问题淹掉。

---

## 3. 踩坑清单（都踩过）

> 本清单是踩坑的**唯一权威清单**；新踩的坑请追加到这里，不要另开副本。

### 存储与工作区

- **工作区在 IndexedDB 的 v3 三区里，不在 `blobs` 区**：`IDB_VER` 只能 bump 一次；`onupgradeneeded` 只按 `objectStoreNames.contains` **补建** `workspaces` / `wsfiles` / `pyocache`。`gcBlobs()`（每次启动必跑）**只扫 `blobs` 区**——工作区文件的引用关系在 `workspaces.tree` 里、规则不同，纳进来会被当垃圾清掉。
- **同一个工作区的写操作必须走 `WS.*`（唯一写入口）**：`tree` + `usedBytes` 是「读改写」，并发写必丢文件 → 内核用 `wsLock(wsId)` 串行。上传的 `Blob` 直存 `wsfiles`（不转 base64、不读进内存），逐文件独立事务（允许部分成功）。
- **`Edit` 的事务边界**：`wsEditText` 把「解码文本」放在写事务**之外**（`blob.text()` 不是 IDB 请求，放在事务里会让事务提前提交 → `put` 报 `The transaction has finished`），整段仍包在 `wsLock` 里——改这里要保证 `await` 只接 IDB 请求。
- **IndexedDB 加存储区必须升版本号**：`indexedDB.open(name, 1)` 对已存在的 v1 库不会触发 `onupgradeneeded`，新区建不出来——而代码却因为「库开着」以为数据已在大仓库里，`saveState` 就把 localStorage 里的会话省掉了（实机上真的丢过测试数据）。现在：升级回调按 `objectStoreNames` 补齐缺失的区，并且**只有确认目标区真的存在**才允许把数据从 localStorage 搬走。
- **「清空全部 / 导入备份」之后的下一次打开，第一条消息别再撞上「整库替换」的墓碑语义**：一次性标记必须在**启动采纳 IDB 列表那一步**就消费掉，否则下一次无关的合并会把用户刚新建、还没落库的会话整表丢掉——表现是气泡永远停在「正在生成…」、刷新后消息彻底消失。消费点一共四处、都在「采纳完整库」那一步；合并自身的判据收窄为「整库替换只该丢掉出生时间早于 `wiped` 的未落库副本」。
- **生成中切走会话：收尾与工具轮都必须按「发起这次生成时」那条会话 id 写回**：`selectConv()` 只在「切到同一条」时早退，生成期间可以切走，而模块级 `conv` 会跟着改成另一条——收尾若拿 `state.activeId` 重取目标，`commit(asst)` 在**别的**会话里找不到这条消息、静默 no-op，却照样刷新那一条的 `updatedAt`；组合「生成中切走 + 原会话被删除/整库替换」就是**静默丢答**。做法：`sendMessage()` 开头捕获 `var sendConvId = conv.id`，收尾一律 `commit(asst, sendTarget())`，未命中才如实提示；模块级 `conv` 有意不跟着挪。
- **发送前的两个 await 也算「这一轮的窗口」**：用户消息与助手占位是在 `await hydrateAttachments(...)` + `await externalizeFiles(...)` **之后**才 push 的——附件大时足够切走会话。现在 `sendMessage()` 在**第一个 await 之前**捕获 `sendConvId`，两个 await 之后重取目标；目标为空（会话被删 / 被整库替换拿走）就**根本不发请求**并如实提示，否则 push / 落盘 / 请求体（`payloadMessages(tf.messages)`）全落目标会话。
- **工具轮的写入同样只认「发起时会话」**（长按生成 + 切走 + 工具有调用是最容易踩的组合）：工具卡片消息与续答一律写 `getConv(sendConvId)`；**视图重绘只在「发起会话 == 当前会话」时做**（每个重绘点各自重算，`await` 期间用户可能又切回来）；发起会话消失就收手，由 finally 统一如实提示。
- **主题化确认框是异步的，确认之后要按 id 重新取目标**：原生 `confirm()` 同步返回布尔值，换成自绘确认弹窗后动作落在回调里——弹窗开着时用户可能切走会话 / 目标已被删，所以「确认后要动的东西」必须在回调里重新取一次（删对话 / 清空消息 / 数据面板删与压缩 / 导入 / 清空全部两级 / 删工作区两级 / 删文件等都要按这个口径）。
- **两级确认 + 自动聚焦「确定」必须吞掉 Enter 的自动重复**：按住 Enter 不放时键盘自动重复的每一帧都会激活聚焦按钮，而第二级确认弹窗是在第一级的回调里同步打开、同样自动聚焦「确定」的 ⇒ 只按一次键就能**连过两级**。在 document-capture keydown 里加 `if (e.key === "Enter" && e.repeat) { e.preventDefault(); return; }` 即可（聚焦按钮的 Enter 默认动作挂在 keypress 上，keydown 被取消后连 keypress 都不生成）。
- **动态弹窗里的文本 `<input>` 必须显式写 `type="text"`**：表单控件样式选择器要求属性存在，漏了就完全吃不到样式、落到 UA 默认背景（深色系统偏好下浅色主题会变成「深底 + 深字」）。
- **会话现在在 IndexedDB，自动化脚本别再读 `localStorage.conversations`**：要读会话得打开 `chatgpt-webui-blobs` 的 `convs` 区 `getAll()`。
- **启动是异步的，脚本要等 `data-boot="done"`**：会话从 IndexedDB 装回发生在初始化第二步，只有 `#welcome` / `.msg` 出现不代表初始化完成（早写 localStorage / 点按钮会被随后的保存覆盖）。
- **判「还在生成」要看 `.gen-line` 的 `display`，别看 `textContent`**：该行是**故意不重绘**的（动画才连续），收尾时只把 `style.display` 置 `none`——拿 `textContent.indexOf('正在生成')` 做断言会**永远为真**。
- **下载链只有一条内核（`wsDownloadPath`）**：单文件、目录打包（`wsDirPackPlan` + `wsZipBuildLazy`）、整箱（`dir="/"`）都走它；zip 字节格式只有一份实现（`wsZipWriteOpen`）。要点：`WS_ZIP_MAX` = 128 MB 是**内存硬顶**（超了报 `E2BIG`，不许提高）；`CompressionStream("deflate-raw")` **有则压（method 8）、无则原样存（method 0）**，压不小也退回 store；懒加载**逐文件读**（每文件一个只读事务，读完即压缩入块并释放原始字节）；Blob 只在 `finish()` 时构造 ⇒ 中止/失败**不产半成品**；**只有模型发起的那条路能被 `Ctrl+X` 中断**——`wsDownloadTool` 透传 `ctx.signal` 并带 `deadline`（`toolLimit("timeoutMs")`），而 UI 路径（工具栏「整箱 zip」的 `wsZipWorkspace`、右栏「下载」的 `wsDownloadPathUI`）调内核时只传 `onProg`，**不带 signal/deadline** ⇒ 打包中按 `Ctrl+X` 停不下它（与批前一致的既有行为，不是本批回归）。**打包与上传双向互斥**（`WS_DL.running` ↔ `WS_UPLOAD.running`，冲突报 `EBUSY`）——这是 0.2.0 起的**有意行为变更**：批前「打包中再点单文件下载」会成功，现在会被闸挡住。清单来源必须是 `wsReadTx` 的 `env.meta.tree`（**不要**读 `WS_META_CACHE`，缓存冷/陈旧时会打出缺文件的包）。`wsZipBuild`（全量内存版）产品内**已无调用者**，保留它是为了让装置的 D12 逐字节对拍有"新实现"这一条腿。

### 工作区右键菜单（0.2.0 起）

- **`#pop-menu` 是单例**：新使用者一律 `closeMenus()` 开、自己的 `data-*` 分派分支收，且分支必须插在 `data-convact` 分支**之前**——再往后就是 palette 兜底 `runPaletteAction(null)`，落到那里等于点了空（菜单关了、什么都没发生）。菜单的"上下文"必须跟菜单一起失效：`closeMenus()` 与关浮层的 document click 都清掉 `WS_CTX`，否则点下一次会拿着上一个目标的路径去动作。
- **定点弹出用 `placeMenuAt`，不要复用 `placeMenu`**：后者是"贴着锚点上方"语义、必须有真实锚点元素；右键没有锚点，要的是"以鼠标位置为左上角 + 四边夹紧 + 超高时自身滚动"。两者并列存在，别合并。
- **列表重绘会冲掉行上的临时态**：剪切标记（`.wsr-cut`）与选中态都**只存路径**，重绘时按当前状态重算（`wsrRowExtraClass(wsId, path)` 带 `wsId`）+ 在状态变更处显式 `renderWsFiles`。带 `wsId` 是硬要求：`WS_RAIL_SEL` 有多条写入路径，跨工作区会误标同名路径（同一路径在两个工作区里都存在是常态）。
- **工作区内部剪贴板 ≠ 系统剪贴板**：不进系统剪贴板、刷新即失、跨工作区禁用（换工作区清、删的正是来源工作区才清）。粘贴与"复制副本"一律**先算唯一名**（`name (2).ext`）+ `overwrite:false`／`mode:"create"`，**永不覆盖**用户文件；先算名与执行之间被并发抢先（`EEXIST`）时重算一次再试，仍失败如实报错，不静默。
- **行选中集（多选）只在「同一工作区 + 同一目录」内有效**：键 = `String(wsId) + "|" + WS_CWD`（`WS_SEL_KEY`），**键一变即清** —— 判据在 `renderWsFiles()` 入口判一次，一条判据同时覆盖「换目录」「换工作区」「删掉当前目录后被兜回父目录」三条路径。此外还有**三个明确的清空落点**：① 右键落在空白/特殊行（`wsCtxContextFromEvent` → `wsSelClear()`，全库唯一调用点）；② 侧栏点选换工作区的 `"pick"` 分支（显式置空）；③ **任何动作改动后**（`wsCtxAfterChange` —— 路径可能已被改名/移走，留着只会是过期路径）。注意 `wsSelClear()` 只是"置空 + 就地刷"的封装，**只有 ① 走它**，②③ 直接置空 `WS_SEL` / `WS_SEL_ANCHOR`；新增清空路径时**两条纪律都要守**（清键所指的那一份状态、且必须 `wsSelPaint()` 或触发重绘）。选中集与剪贴板同性质：**纯内存**，不进 settings / IDB。
- **列表行的点击语义是「单击只选中、双击才打开」**：闸门只有一处 —— `wsrRowActivate(e, b)` 返回 `e.detail >= 2`，挂在 `onWsRailClick` 的 `go` / `enter` / `preview` 分支上。**不要把这个闸门搬到别处、也不要给它加"例外"**：行内按钮（`.wsr-ops` / `.wsr-caret`）、面包屑、无 `data-wst` 的特殊行之所以"照旧"，靠的是 `wsrRowActivate` 内部两行提前 `return true`（判 `classList.contains("wsr-row")` 与 `data-wst` 是否存在），不是分支里的额外判断。选中态与剪切标记 `.wsr-cut` **共用** `wsrRowExtraClass(wsId, path)` 这一个渲染出口（重绘后自动重算），只有 `.wsr-sel` 另有一条**就地刷**的路 —— `wsSelPaint()` 只增删类名、**不整列表重绘**（重绘会丢滚动位置与树展开态），所以改选中逻辑时"重绘出口"与"就地刷"两条路都要照顾。
- **批量动作必须「串行 + 如实聚合」**：批量删除 / 下载 / 粘贴一律经 `wsCtxBatchRun(label, items, fn)`（**逐条串行**，进度走既有 `#wsr-prog` 状态行）+ `wsCtxBatchReport(label, st)` 收口。报告纪律写死在 `wsCtxBatchReport` 里：全成 ⇒ success toast；**只要有一条失败 ⇒ error toast（成功数 + 失败数 + 前 3 条明细）＋ 完整明细灌进 `wsSetReport` ⇒ 右栏 `#wsr-report` 可"展开全部 N 条失败"** —— **部分失败绝不静默**，不允许"整批报成功"或"只弹最后一条错误"的写法。状态容器形状 = `{ total, i, ok, done:[], fails:[{path,name,code,msg}], aborted }`（`aborted` 由单条返回的 `r.aborted` 触发，已完成的项按"已写入不回滚"如实登记）。

### 桌面快捷方式（`.url` / `.ico`，0.2.0 起）

- **桌面上的 `.html` 文件本身的图标改不了**：那是操作系统外壳按「扩展名 → 文件类型 → 注册表图标」定的，网页侧没有任何 API 能改写自己这个本地文件的 OS 图标。唯一的可行形态 = 生成 Windows 原生 InternetShortcut（`.url`），把它的 `IconFile=` 指向我们自己生成的 `.ico`。产品文案必须把这条如实说出来，否则用户会以为"设置里开了就生效"。
- **`IconFile=` 必须是绝对路径**：同目录下的相对文件名（`IconFile=azusa.ico`）实测**无效** —— 外壳给的是与"完全没有 IconFile"一样的默认图标；`%VAR%` 形式同样不解析。而 **File System Access API 不暴露所选目录的完整路径**（`FileSystemDirectoryHandle` 只有 `name`，即叶子名）。⇒ 「一键写入」通道的 `.url` 只能引用**本页所在目录**（由 `location` 唯一确定）；用户选中的目录与它不同时，必须给**显式提示**让用户把 `.ico` 搬过去，**不许静默产出一个引用错路径的 `.url`**。
- **`showDirectoryPicker()` 必须在用户手势的同一个任务里同步调用**：它要 *transient user activation*，而一次 `await`（例如"先把图标渲染成 ICO 再选目录"）就会把它耗尽 ⇒ 选择器被 `NotAllowedError` 拒。正确次序 = 点击处理器里**先同步取 picker**，拿到 handle 之后再渲染、再写（handle 上的 `createWritable/write/close` 不需要手势）。
- **原生目录选择框是操作系统 UI，不可自动化**：CDP / PowerShell / UI Automation / SendKeys 都点不了「选择文件夹」那个按钮（实测其 UIA 控件是 `ControlType.Pane`，不支持 `InvokePattern`/`LegacyIAccessiblePattern`）。硬闯的代价 = 反复弹出用户可见的对话框 + 把验证脚本自己搞崩。**这类验证只能走「用户侧手工读数」**：写一份可直接复制给用户的 30 秒步骤（做什么 → 看什么 → 回报什么）。同理适用文件选择框、打印对话框、证书/权限原生弹窗、UAC。
- **`.url` 的编码分支**：内容全 ASCII ⇒ UTF-8 **无 BOM**；含非 ASCII（中文用户名 / 中文目录）⇒ **UTF-16LE + `FF FE` BOM**（逐字节手写，不要借 `TextEncoder`）。实测 UTF-8 / ANSI 都解析不了中文路径。注意验证时 `Buffer.toString("utf16le")` **不会剥掉 BOM** —— 断言要从**字节偏移 2** 起解码，否则必然把 U+FEFF 当成内容而误判。非 ASCII 时提示必须放在状态行**首行**（默认展开），不能在长文案末尾藏一句。
- **ICO 的 6 条目结构**（16/32/48/64/128 = DIB，256 = PNG）：DIB 条目 = `BITMAPINFOHEADER(40B)` + XOR 位图（**自下而上**、BGRA、alpha 直存）+ AND 掩码（同样自下而上，每行 `((w+31)>>5)*4` 字节，**bit=1 表示透明**）；`biHeight` 要写**两倍**高。**别为省体积砍条目**：实测单 16×16 条目的 ICO 外壳不采用，6 条目才全效果（该观察来自方案审查期的 Win11 旧读数，本批未复测；**很可能与下面第 7 条那个 SVG 源缺陷同源** —— 一个 2×2 像素的小点当然不会被外壳采用）。**结构对 ≠ 画对**：实测过「6 条目结构全部合法、图案却被画进左上角一小块」的产物 ⇒ 校验必须带**像素级**判据（**非零 alpha 覆盖率 ≥ 10%·s²** ∧ **外接框四边留白 ≤ `max(2, 10%·s)`**），而 `alpha > 0`（4 个像素也满足）**不算判据**；**两条来源都要跑**（默认 SVG 源 + 用户自定义位图源），并留一份修前产物当负例（证明判据不是恒 PASS）。
- **渲染取「中心正方区域」再缩放，但 SVG 源必须先光栅化成位图**（与 `faviconDraw` 同口径）：默认 favicon 是内嵌 SVG，**没有 `width`/`height` 属性**（只有 `viewBox`），直接按宽高各自拉伸有变形风险。**别把「先中心裁方」当成"不依赖源图固有尺寸"的保证 —— 它对 SVG 源不成立**：对 SVG 源用「显式源矩形」的 9 参 `drawImage(img, dx,dy,sw,sh, 0,0,s,s)` 时，Chromium（实测 122）**先按目标尺寸光栅化、再把源矩形按固有尺寸坐标系解释**，图案因此只占画布左上角 `s²/固有边长` 个像素（实测六档 16→4 / 32→49 / 48→247 / 64→742 / 128→11455，只有 256 正常；**同一个 9 参调用打在位图源上完全正确**）。⇒ 正确做法 = **先按最大档边长（256）用 4 参 `drawImage(img, 0,0,n,n)` 把 SVG 光栅化成真正的位图**（矢量源在这一步按该尺寸重绘，不失真），**之后一律走位图源路径**（`dskIconSource()` 就是这道归一化；非 SVG 源原样返回 ⇒ 位图路径字节级不变）。**别把这一步当成可省的开销**：把 SVG 直接喂给 9 参 = 回到上面那 5 个错档。
- **图标名用内容哈希**（`<base>-icon-<crc8>.ico`，`crc8` = ICO 字节的 CRC32 十六进制）：资源管理器对外壳图标有缓存，换图标而名字不变时可能仍显示旧图标。注意 `padStart` 在本项目的手写段是**禁用**的（ES5 口径），要自己补前导零。
- **`.url` 是危险扩展名**：新版 Chrome 会把下载的 `.url` 改名成 `.download`（目标环境 Thorium 122 实测无此问题）⇒ 界面上给"手动改回 `.url`"的指引；另外 `.url` 双击由**系统默认浏览器**接管，不一定是本浏览器，文案要写清。
- **路径卫生（`.url` 行注入的纵深防御）**：`IconFile` 值里出现 C0/C1 控制字符（含 CR/LF）或 `[` / `]` 时**拒绝生成**（Windows 文件名本不允许这些字符 ⇒ 一旦出现就是有人在构造额外行）；`URL=` 行同样过控制字符检查。
- **非 `file:` 协议一律不出按钮**：dev server（`http://`）下 `location` 给不出本机路径，状态行写明「只对 `file://` 打开的单文件版本有效」并把两个下载按钮置灰。`showDirectoryPicker` 除协议门外还要**能力探测**（`typeof window.showDirectoryPicker === "function"`），缺任一 ⇒ 「写入文件夹…」按钮保持 `hidden`（不交付无法验收的分支）。

### JupyterLite 宿主与镜像（0.2.0 起）

- **`file://` 下「真实 URL 子页」与 opener 的跨文档访问必被拒（J-TAB-1 的由来）**：Jupyter 标签页的现有形态把子文档承载在 `about:blank` / 伪源站点上（注入式引导，主页面与子页在同一文档上下文里互操作）；一旦改成"可刷新、有真地址"的形态（历史 J1 尝试），`file://` 下子页与 opener 互相读不到对方 ⇒ 镜像 / 内核资产 / 跨页命令**整体断链**，该路线已撤回（`batch-j1-revert`）。⇒ 现状 = 已知限制 **J-TAB-1**（子标签页依赖主页面：`about:blank`、刷新白屏，重新「通过 JupyterLab 打开」即恢复、不丢数据）；根治 = A1 子页内核资产自举 + A2 传输迁 localStorage + `storage` 事件（0.2.1 备选）。**没做 A1/A2 之前，不要动子页的承载形态。**
- **镜像仲裁必须先做时间戳归一，否则「比较」恒为假**：工作区侧记的是数值毫秒（`mtime`），Jupyter Contents 侧给的是 ISO 字符串（`last_modified`）——`ch.lm + 1000` 落在字符串上会变成**字符串拼接**，两侧比较恒 `false` ⇒ 拉方向**无条件获胜**，工作区里刚写的新内容会在 ≤1 s 内被陈旧副本**静默还原**。修法 = 两侧统一走 `jlMirrorLmMs()` 归一 + **平局（同一毫秒）判外部/工作区侧胜**。改仲裁前先读 `specs/jtab3-conflict-fix-plan.md` 的甲/乙清单（行号会漂，按符号锚点找）。
- **冲突轮不更新同步基线**（`conflict-base-hold`）：两侧自基线以来都变过时不选边——限频提示（8 s）点名文件，且**该轮不得写回基线**；否则下一轮比较只剩「单侧较新」，冲突被静默吞掉。计数面在 `data-jl-mirror-stat` 的 `conflicts` 字段。
- **「打开中的文档跟随」的三条硬前提**（缺一即变成"表面通过的死代码"）：① 路径比对**按段归一**——子文档 `context.path` 不带前导斜杠（`seed.txt`）而镜像侧带（`/seed.txt`），直接 `===` 永不成立；② 跟随 = `await c.revert()` 且检查失败，**不得静默假成功**；③ 成功后 `clearUndoHistory()`——否则用户一次 `Ctrl+Z` 就把跟随事务撤掉、静默写回旧内容。dirty（有未保存修改）一律跳过；子窗口已聚焦时也跳过（`hasFocus()` 读不到就保守跳过）。
- **已有标签页的「真打开」= want 投递 + 等落位**：目标文件由 `JL_LAB_PATH` 承载，`JL_LAB_WANT_SEQ` **只增不减**（句柄丢弃不归零，旧回执不得吞掉新 want）；看门狗**只在有目标文件时才允许开**（`if (JL_LAB_PATH) …` + `JL_LAB_WANT_AT > 0` 守卫）——无 want 时开了会误报「标签页没有回应打开请求」（实测 t+1.15 s 假警告）；等待预算 `JL_LAB_WANT_WATCH_MS` 45 s / `JL_LAB_WANT_BOOT_MS` 120 s，超时必须出**可见**横幅（不静默）。

### 布局、主题与样式

- **加右栏要动 `autoFitSidebar()`**：`avail` 必须同时扣掉左栏与右栏（`railWidth()` 用 `offsetWidth` 且**排除 `.collapsed`**），否则会出现「右栏开着但对话区已被挤爆」。**右栏让位判定里左栏要按「收起计 0」**：左栏收起是 `margin-left:-278px`、`offsetWidth` 仍是 278，若按展开计，「先收起左栏再开右栏」这条绕行路走不通；判定不足收掉右栏后当趟 `return`（不同趟重判左栏），改完记得让开合右栏与窗口 resize 都触发一次重判。
- **窄屏两块抽屉互斥**：≤880px 左栏与右栏都是 fixed 抽屉，打开一个要关掉另一个，否则叠在一起互相打架。
- **右栏面板的初始渲染**：`settings.wsRailOpen` 为真时，刷新后右栏是开的但**面板内容还没渲染**——启动链上要补一次渲染，否则用户得点两次按钮才看到内容。
- **主题色**：新增颜色时要在 CSS 里同时加 `html[data-accent=x]` 与 `html[data-theme="light"][data-accent=x]` 两组，并把名字加进 `ACCENTS`、在色块区加一个 `.swatch[data-accent=x]`。
- **不要用 `color-mix()`**：背景光晕、描边等一切跟随主题色的颜色都通过 `rgba(var(--accent-rgb), α)` 派生。
- **主题变量必须两套都定义**：`--ai-bubble` 曾只写进深色主题，日间模式下变量无效 → AI 气泡背景直接变透明。新增主题相关变量时，深色 / 浅色两块都要写一份（浅色底色用 `#ffffff`，深色用 `var(--surface-2)`）。
- **文本型 `<span>` 加省略号**必须显式写 `display: block`（flex 子项会自动块级化，非 flex 子项不会）。
- **全局 `svg { display: block }`**：图标 + 文字的按钮如果不显式 `display:flex; justify-content:center`，图标会独自占一行、文字被挤到别处；带图标的 chip 必须 `inline-flex` + `nowrap`，否则 chip 变两行高、与相邻 chip 对不齐。
- **同一行里的两段文字要用同一套字体字号**：不同字体 / 字号时盒子中心对齐了但**文字基线错开**，肉眼看着就是「两个字高度不齐」。
- **`.range-row` 里的 `<input type=range>` 要 `min-width: 0`**：range 的默认固有宽度约 129px，作为 `flex: 1` 子项不会自动收缩，整行会溢出容器。
- **`.popover` 不设宽度会被长提示文本撑开**：绝对定位 + 只有 `right` 时宽度是 shrink-to-fit，显式写 `width` 或 `max-width`；`.popover` 的位置由 `placePopover()` 计算，不要写死 `right/bottom`。
- **自绘浮层要压过弹窗**：`.modal-mask` 是 `z-index: 200`；所有挂在 body 上的自绘浮层统一用 **z-index 400**，否则在设置弹窗里展开时浮层被盖住、肉眼完全看不见。
- **浮层开关**：打开浮层的按钮要带 `data-menu-opener`，否则文档上的「点别处收起」会把它刚打开就关掉；自绘浮层的开关是「切换」语义——箭头按钮要在 `mousedown` 里 `preventDefault` 防止抢焦点导致浮层刚展开就被失焦延时任务收掉。
- **编辑框里的提示文字要显式设色**：通用提示样式只在特定选择器下生效，放进编辑框会继承正文色，在深色主题下就是刺眼的近白色。
- **浮窗定位按「布局盒」量，不按渲染矩形量**：`placeMenu` 用 `offsetWidth` / `offsetHeight`（与 `transform` 无关）——动画（`popIn` 140 ms）期间量 `getBoundingClientRect()` 拿到的是缩放中的尺寸，首开与稳定态不一致（实测 7 px 级漂移，用户视角就是「第一次点开位置不对，再点一次才对」）。改量法前先看 `shared/progress/u2-fix-done.md` 的仪器陷阱（连点 3 次、每次 +80 ms 采样会落在动画内 ⇒ 假 `distinct=2`）。
- **浮层打开时 `scrollTop` 归零，且落点必须在「cap 之后」**：`placeMenu` / `placeMenuAt` 的 `menu.scrollTop = 0` 必须写在函数**末行**（高度上限 / 溢出设置之后）——写在前面会被浏览器在 cap 落地时还原的旧偏移吃掉（探针实证：中间态重开仍读到 200）。`#cmd-list` / `#ctx-pop` / `#params-pop` 的同类残留同因同修（打开即回 0，键盘选中项才不会落在可视区外）。
- **右栏行高 +2px（36 → 38）是命中区修复的必然导出**：`.wsr-row .wsr-ops button` 22→24、`.wsr-caret` 20→24 后行内容 max = 24，`align-items:center` 上下各 +1。**别用降 `padding` 的方式把它压回 36px**——不含 ops/caret 的行（如「上一级」）会反向缩到 32.25px、引入新的行高不一致。

### 渲染与流式

- **公式处理顺序**：`renderMarkdown()` 先调 `protectMath()` 把公式换成占位符，marked 处理完后由 `substituteMath()` 用 `katex.renderToString` 换回。**不要**把公式直接交给 marked——Markdown 会把 `\\` 当成「转义的反斜杠」吃掉一个，矩阵 / cases / align 会全部塌成一行。
- **思考块内部也有一层 `.msg-content`**：用 `msgEl.querySelector(".msg-content")` 取到的是**思考块里那一个**——结果正式回答被写进思考气泡、思考内容被覆盖。取正文一律用 **`.msg-body > .msg-content`**。
- **思考内容渲染 Markdown 前必须去掉 4 空格缩进**（`deindentOutsideFences`）：推理模型习惯用缩进写子级说明，直接交给 marked 会变成「缩进代码块」。围栏内的缩进必须保留。
- **流式渲染要给未闭合的围栏补一个结束围栏**（`closeOpenFence`）：否则模型刚写出 ```` ``` ```` 还没写结束围栏时，后面的正文会被 marked 全部当成代码块。
- **流式速度不要用服务端 `timings.predicted_per_second`**：它只覆盖纯解码窗口（`predicted_ms`），不含首字延迟与 prompt 处理，短回答时会明显偏高。界面统一用客户端实测吞吐，服务端口径只写进状态栏 tooltip 便于对比。
- **流式拿 token 数必须带 `stream_options: { include_usage: true }`**（详见 §4.1）。
- **按字符估算 token 要分四桶**：CJK 0.804 / 拉丁字母 0.170 / 数字 1.041 / 符号空白 0.319。**不能合并成一个「其它」桶**——英文散文约 0.17、数字接近 1.0，相差一个数量级，混在一起会互相抵消。实测平均绝对误差：四桶 7.9%。
- **流式速度用「首尾相减」会抖**：llama.cpp 会把多个 token 合并进一个 chunk，首尾取样容易落在突发块边界上。改成窗口内最小二乘拟合后，相邻跳变从 5~6% 降到 0.35% 量级。
- **子容器的滚动位置要自己存自己恢复**：逐帧重写 `innerHTML` 时 `<pre>/<code>` 与思考正文是**新元素**，滚动位置会归零。改动重绘逻辑时必须保留滚动状态收集 / 恢复。
- **拖动滚动条时不要重绘**：重绘调度里有交互宽限（420ms），程序化设 `scrollTop` 会先记到元素私有字段上，避免被误判成「用户手动滚动」而关掉粘底。
- **`data-stick` 而不是全局变量**：思考块 / 代码块各自粘底，状态挂在元素上，新出现的容器默认为「跟随」，用户手动滚走后变「不跟随」。
- **行数上限单一来源**：代码块的显示行数由 CSS 变量 `--code-lines`（默认 10）控制，JS 侧的常量直接读取它，不要在两处各写一遍。
- **`String.prototype.replace` 会解释替换串里的 `$`**：`$$`、`` $` ``、`$'`、`$1` 都会被当成特殊模式，导致文本被复制或吞掉。批量替换请用函数形式 `s.replace(a, function () { return b; })`。

### 工具与消息渲染

- **同一轮的工具调用要合并成一个气泡**：数据层仍是「助手（带 tool_calls）→ 工具结果 → 后续助手」多条消息，靠共享 `turnId` 在渲染时合并，协议回填与历史裁剪都不用改。代价是要给每条消息的正文挂 `data-mid`、给组挂 `data-msg-last`，让流式重绘找对元素。
- **工具卡片不在 `.msg-content` 里**：卡片是 `.msg-body > .tool-stack > .tool-slot > .tool-card`，`.msg-content pre …` 这一族选择器匹配不到它——代码块相关样式一律写成**共享选择器**（`.msg-content pre …, .tool-card pre …`），否则卡片里的代码块没表头、没换行、按钮竖着排。
- **工具卡片渲染会先清空宿主**：多张卡片**不能共用一个容器**，每张要有自己的 `.tool-slot`，否则后渲染的会把前面的清掉。
- **气泡里的工具卡片要留间距**：卡片与上下正文之间留 14px，紧贴会显得挤。

### 上下文压缩的界面（0.2.0 起）

- **压缩卡位置 = 原位，永不挪位、不置顶**：三态（运行中 / 完成 / 失败）都挂在同一个工具气泡（`.msg.cx-card`）上、就地插在消息流里。任何"把压缩卡挪到对话最上方"的写法都已被否掉（方案与负例都按「原位」口径建）；改这块先读 `shared/specs/compact-ui-plan.md` 与它的 ③.5/复查报告。
- **运行卡与失败卡不得退化成可折叠工具卡**：运行 / 失败卡撤掉 `data-tool-toggle` / `aria-expanded` / `.tc-arrow`（可折叠的只有「完成卡的摘要」这一件事）。运行的活体标记 `[data-cx-live]` 挂载条件是**互斥**的——`status === "running"` ∧ **非工具入口**（`source !== "tool"`），收敛即消失；漏掉后半句会让模型入口双挂。
- **流式草稿的所有权 = 每次尝试前清空 + 失败 / 取消清空**：草稿由 `cxCallSummary` 的流式分支逐字写，`finally` / 异常路径 / 中止一律清掉——失败**不能**残留半截草稿（用户会把半截当结果），失败与取消对 `conv.messages` 的改动数必须为 0。
- **失败卡正文固定一句「本次压缩没有改动任何消息，原文仍完整保留。」** + 「重试 / 知道了」；手动压缩的 `Ctrl+X` 走 `cxRunActive → cxAbortActive`（**不**调 `stopGenerating`，避免误杀沙箱 / 工具等待 / 自动续跑）。

### Python 执行与装载

- **工具结果一旦被截断，方向就只有一种：留尾**。执行器族（`ExecutePython` / `ExecuteJavaScript` / 4 个任务工具）走这条；`Read` / `Grep` / `Glob` / `PythonPackages` 是分页与结构化检查类工具，**保持留头**（否则文件开头 / 摘要头 / ① 段先丢）。族的名单是 `RESULT_TAIL_TOOLS`，白名单外的工具行为必须逐字节不变（改这里等于改四种工具语义）。
- **短而关键的行必须放进不可裁的注记区，且序列化在最末**：档位提示、工作区回写、并发冲突、装载摘要这类几十字符的行如果拼在正文最前，留尾裁剪会把它们**第一个**剪掉（历史上档位提示就是因为这个被放在最前的）。现在的契约是 `pyResultText()` 返回 `{body, notes, cut}`：`notes` 不参与任何裁剪、序列化后位于末尾。
- **整棵 `innerHTML` 重建会让滚动容器失去拖拽与位置**：任务卡片每 1 秒重建行会让鼠标拖拽中断、`scrollTop` 归零（用户看到的是"有滚动条但拖不动"）。改法是"行建一次（Ensure）+ 就地改文本（Paint）"，并且**重建后的首填必须贴底**（否则窗口停在最早几行）。滚动位置用"仅贴底时跟随"语义维护，另加 ≤420ms 的交互宽限期推迟刷新。
- **装载窗口与脚本窗口必须分开计时**：首次执行要先装载本档全部预置包（三档 33 / 99 / 151 个），这段时间若计入脚本超时，第一次执行必然超时。前台是"装载窗口（`PY_LOAD_WINDOW_MS`，只起算一次）+ `stage/exec` 后切脚本窗口"；后台任务的计时钟在装载期间暂停、`stage/exec` 后按 `pyTaskTimeoutMs` 满额重起；收尾不依赖被暂停的计时器（worker 死亡由 run 的 finish 兜底）。
- **"已装"的判据要用归一后的名字**：pyodide 写 `loadedPackages` 的键取的是"被请求时的名字"，依赖表里可能是下划线形态（`jsonschema_specifications`）而锁条目名是连字符（`jsonschema-specifications`）。只按小写比会把已装好的包误判成失败并每次执行重试一遍 —— 两侧都要走 `- _ .` 折叠（PEP 503 口径）。

### 附件与网络

- **一次拖进来的多个附件必须按输入顺序落位**：附件准备是异步的（图片要解码、PDF 要渲染），用 `push()` 收集会让顺序随「谁先完成」而变——表现是图文交错顺序随机。按输入下标占位、`Promise.all` 之后再按序插入。
- **附件的展示元数据挂在 content 部件上，发请求前必须剥掉**：`attach`（id / name / size / pages / ocrOnly / ocrText…）是气泡渲染用的，发送前统一剥掉；忘掉就会让严格的 OpenAI / Anthropic 实现因未知字段报 400。
- **OCR-only 的图片是「显示用」的**：它带 `attach.ocrOnly:1`，发送时被换成 OCR 文本；但气泡里仍然要渲染图片 + 一行 OCR 说明。删附件时按 `attach.id` 整组删，别只删一个部件。
- **含 OCR 说明的元素别用普通 flex 子项**：要独占一行（`flex: 0 0 100%`），否则会跟缩略图挤在同一行。
- **别把「服务端当前加载的模型」当成「你选的那个模型」**：llama.cpp 的 `/props` 只描述它**正在服务**的那个模型。只有 `/props` 自报的模型名（取文件名比较）与所选模型一致、或该模型确实在服务端列表里时才记录窗口长度，其余一律标成「默认值（无法确认）」。
- **窗口来源（优先级从高到低）：服务端 > 手动设置 > 默认值 256K**：服务端返回窗口时一律以服务端为准、手动设置项不显示；服务端拿不到窗口时才可在生成参数浮层（「调节」）/ 设置 → 生成参数 手动设置；两者都没有时按默认 256K（`DEFAULT_CTX_WINDOW`）。
- **`state` 与 `state.settings` 不要放错**：要持久化的数据（如模型窗口记录）必须写在 `state.settings.*`（加载与迁移只认 settings 里的字段），写错位置刷新后就读不到了。
- **弹出 async 函数别忘 `await`**：测试助手改成 async 后漏了 `await`，断言拿到 Promise 直接变成 `{}`，看起来像断言失败。改造助手时一起检查调用点。
- **写含反斜杠的 JS 字符串要用文件工具**：批量替换脚本经过多层工具会吃掉一层反斜杠，把脚本写坏。要么用文件写入工具直接落盘，要么别在字符串里写反斜杠。
- **非视觉模型的「图文按序融合」有三条不能破的线**：① `o.ocr`（「没有文本层才 OCR」）的语义**不得扩展**——ReadOffice 侧用独立标志 `o.fuseImgs`，入口条件唯一 = 非视觉 **且** 有候选图；② 视觉路径一字不变（不探页图、不做 OCR）；③ 无图 / 纯文本文档的输出逐字节不变（融合入口一律在「有图」判定之后）。三批改动都靠负例断言守着（请求体既不含「本机 OCR」也不含夹具暗号）。
- **附件侧 PDF 的融合条件是「非视觉**且有图**」，不是「文本为空」**：`PDFKit.extractText` 对任何 ≥1 页 PDF 都会写页分隔标记（`----- 第 N 页 -----`）⇒ `text.trim()` 永不为空，「有没有文字层」必须用 `realText`（剥掉页分隔标记）判定。ReadOffice 侧早已按 `realText` 修过；附件侧本轮**只加融合、没顺带重构**那条分支——它对 ≥1 页 PDF 本就不可达（批前实测：`scan.pdf` 交付的正文只有 `----- 第 1 页 -----`，**不是**拒收），扫描件于是由融合兜住：页图 OCR 进正文。**0.2.1 起再加一条档位前提**：显式选「纯文本」档时**不做融合**（只发文字）；混合档与「无文字层自动升档」才融合 —— 口径见上「附件三档与图片策略（0.2.1 起）」。
- **图片从「丢弃」改成「OCR 融合」时三处必须同步**：`att.text`（融合后正文）、`att.textChars`（chip 的「文本 N 字」）、`att.ocrImgs`（新计数；旧数据无此键 = 0，不做迁移）。`imgCount` 保持原义（作为图片发出去的张数）——非视觉下融合的图不进 `att.images`，所以它仍是 0；把融合的图算进 `skippedImgs` 同样错（`skippedImgs` 只数解析层跳过的不支持格式 / 超限图）。
- **附件侧 OCR 的时长预算是「整次解析共享」，而且只覆盖 OCR 阶段**：`ATT_OCR_TIMEOUT_MS`(60s) 不是每图预算，达到张数 / 时长 / 字符任一预算**立即停**并如实注记（与 Read 侧 `READ_OCR_TIMEOUT_MS` 同口径）；它**不是整条解析链的墙钟上限** —— 取图调用 `pageImages` / `renderCrops` / `renderPages` 各另带一次 60 s 超时（同一常量），取图极慢时整条解析会超过 60 s。`OCRKit.recognize` 单次调用不可中断（只能 `terminate`）⇒ 中止 / 超时的生效点只在两张之间。
- **CORS 的失败在页面侧一律同形**：预检（OPTIONS）被拒、实际响应缺 `Access-Control-Allow-Origin`、网络不可达，浏览器都只给一条 `TypeError: Failed to fetch` —— 只报「无法连接到服务器」等于让用户猜。要分三层报：① 四条探针（`corsProbe()`：简单 GET / 带应用头 GET / 简单 POST / 真实 POST）只判「浏览器读不读得到响应」（4xx/5xx 也算读到），再用一条 `mode:"no-cors"` 辅助探针把「读不到」分成断网与跨域拦截；② 族文案（`CORS_FAMILY_TEXT`：预检被拒 / 实际响应无 ACAO / 网络不可达 / 服务端不解析 `text/plain` / 服务端未在超时内回话）按族给可执行下一步（这份文案有三条消费路径 —— 诊断面板的结论行、开关**被拒时**的红字 note、开关**开启成功**时的绿字 note（`setCorsBypass` 成功分支）；`friendlyError` / `connectHint` 的族参数分支在产线不可达，见代码注释）；③ 探针只能推出「哪一层被拦」，分不出 OPTIONS 是 500 还是 200-但缺 `Allow-*` 头（两者同形）⇒ 文案不写死单一诊断，细节让用户看 DevTools 的 OPTIONS 状态码。
- **服务端要补的头（诊断面板「CORS 分层」段用的同一份文本）**：`Access-Control-Allow-Origin: *`（**`file://` 下浏览器发的 `Origin` 是 `null`**，用 `*` 最省事）；`Access-Control-Allow-Headers: Content-Type, Authorization, x-api-key, anthropic-version, anthropic-dangerous-direct-browser-access`；`Access-Control-Allow-Methods: POST, GET, OPTIONS`；**OPTIONS 预检直接返回 204**（不要 500、也不要只在 200 里回 JSON）；**错误响应（4xx/5xx）同样要带 `Access-Control-Allow-Origin`** —— nginx 用 `add_header … always;`，Apache / 自研网关在 500 与 `ErrorDocument` 分支同样要补（只给 200 加头是最常见的漏点）。替代方案 = 同源反向代理（页面与接口同源 ⇒ 浏览器不做跨域检查）。
- **「预检规避兼容模式」只能救一种病**（设置 → 模型 → 高级(兼容性)，默认关、带前置探针）：它把请求改成「简单请求」（`Content-Type: text/plain;charset=UTF-8` + 不带任何自定义头）⇒ 浏览器不发 OPTIONS。**三条死路线写在界面上**：① 有自定义头（`Authorization` / `x-api-key` / `anthropic-version` / `application/json`）就必预检；② 服务端实际响应没有 ACAO 时，简单请求同样被拦；③ Anthropic 协议必带 `anthropic-version` 等头 ⇒ 一律禁用。门控 = **协议 × 是否有 Key 双条件**（`corsBypassGate()`），开启前跑四探针、只有「简单 POST 能读到响应」才允许打开（`setCorsBypass()`），否则拒绝并给族文案；开关关闭时请求头 / 体**逐字节不变**（`headersFor()` 首行短路）。
- **外部 MCP 的请求自建头、不走 `headersFor()`**：`mcpPost()`（`src/appE.part`）自己拼 `Content-Type: application/json` + `Accept` +（有则）`Authorization` / `Mcp-Session-Id` / `MCP-Protocol-Version` ⇒ 它恒是非简单请求、必过 OPTIONS 预检 —— 因此**既不被「预检规避兼容模式」覆盖，也不受它保护**（开不开该模式，这一路都一样）。外部 MCP 服务器要么自己放行预检（补齐 `Access-Control-Allow-*`），要么走同源反向代理。

### 附件三档与图片策略（0.2.1 起）

- **三档 = 单键三值**：`attachImgMode ∈ {text,image,both}`、默认 `both`（混合）；读取入口 `attachImgMode()`（缺键 / 非法 ⇒ `both`）。旧 `pdfMode` **单向迁移**（显式 `"image"` ⇒ `image`，其余含旧 `"text"` / 缺键 / 非法 ⇒ `both`；旧键不删）。判定唯一来源 = `decideAttachPlan`（纯函数、FIRST-MATCH，`appD`），渲染能力表 = `attachRenderCap`（`pdf:"pages"` / `xlsx|xls:"drawn"` / `docx`：`ATT_DOCX_RENDER_READY`（唯一开关，S7 起 = `true`）∧ `docxRenderReady()`（`window.RenderKit.available`，render.part 在位）/ 其余 `false`）—— **别处不许重推档位语义**；五处展示面（chip / 气泡卡片 / 预览 meta / 办公预览 / 请求体信息行）共用 `attachModeLabel` 取词。
- **硬不变量：图不进消息 ⇒ `send` 只能是 `text`**（`images==="ocr"` ⇒ `send==="text"`）。自动升档 / 改档（无文字层升混合、图片档 + 非视觉改混合）**不新增选项、不改写用户设置**；降级必须可见（一次性 toast + `degraded` 注记），且**对外不暴露内部档位**（请求体不含 `attach` 元数据 —— 发送前整块剥掉，已断言）。
- **附件图片上限 `ATT_IMG_MAX_IMAGES`(10) 是取图计划与 OCR 融合的同一上限**（批前 `ATT_OCR_MAX_IMAGES`(6) 已并入，旧名**源码全库**归零 —— 别再引用旧名）；PDF 侧三处取图调用点统一传 `attPdfOpts()`（`{maxScan: pdfMaxPages(), maxImages: ATT_IMG_MAX_IMAGES}`），自绘图张数上限同源（`maxSheets`）。
- **交错件的 `attach.blob` 键必须与 `externalizeFiles` 同源（数组下标：`id + "-p" + (下标 + 1)`）**，而 `page` 保留**真页号 / 表序号** —— 两者不是一回事。用「下标数组去找对象」会恒 `-1` ⇒ 键退化成 `-p0` ⇒ 发送期 `blobMemo` 未命中 ⇒ 请求体落「图片数据不在本机」占位（批-2 的真实缺陷）；查找一律走 `attImgIndexOf`（找不到返回 -1 ⇒ 该图退化为内联 data，绝不写无效键）。
- **请求体信息行只在 `f.ocrImgs || f.degraded` 时追加模式词**（`attachmentParts`）—— 默认路径的信息行逐字节不变，别改成恒显。
- **自绘表格图（xlsx / xls 图片档，D12）的交付闸门**：每张过 `attCanvasFitForModel` —— 按 `imgMaxSide()` 下采样 + 单张 `READ_IMG_HARD_MAX`(1 MiB) 硬顶，**可读性优先**（先降质量后降边长；压不进硬顶的那张只跳过该表 + 注记，绝不发半张坏图）；生产参数含 `scale: 2`。
- **docx 页图（图片档，S7 起）的三条硬约束**：① **资源守卫必须 wrap 三方法**（`loadDocumentImage` / `loadNumberingImage` / `loadFont`）—— 缺失部件时 docx-preview 拿到 `null`，而 `null` 在被赋给 `img.src` / 拼进 CSS `url()` 的**瞬间**就发起加载尝试（晚挂载 / 拆 parse+render / 事后属性守卫都拦不住）⇒ 只有"换掉那个值"能阻断；**少包一个 ⇒ 非主文档请求 ≠ 0**（批内实测：漏 `loadFont` ⇒ 2 次）；② **`OPTIONS` 必须钉 `renderAltChunks:false`**（altChunk 的 `<iframe srcdoc>` 会真发请求）与 `useBase64URL:true`（全链 `data:`，免 `blob:` 路径）；③ **挂载必须双容器**（`host` + `styleHost` 两个 `div` 都 `appendChild`）—— `<style>` 未挂 ⇒ `<style>` 不生效 ⇒ 页盒 794×1123 变 1034×1315、页图全变。另：渲染失败的两支回退注记必须落在 **O6 块之后的共同出口**（cap=`pages` 时内嵌图在 O6 复查**之前**就已落进 `images` ⇒ `!images.length` 恒假 ⇒ O6 块不进，注记依赖 O6 会静默 —— 批内实测出来的缺陷）。
- **docx 页图的清洗层是"两层 + 两个计数"**（防御纵深，`cleanNodes`）：**属性面**（`src`/`srcset`/`poster`/`data`/`xlink:href` + 非 `<a>/<area>` 的 `href` 中和；`<a href>` 豁免 = 超链接不发起请求）与**样式文本面**（`<style>` 里的 `url(http(s)://…)` → `url(about:blocked#)`）。两类计数随结果返回（`res.clean = {attr, styleUrl}`），**正常输入下应为 0，非 0 必须进 `notes`**。**该面当前不可由真实 docx 触达**（三库的 CSS `url()` 发射点只有 `@font-face` 与 numbering 变量两处，均已被 wrap 在值级截住）⇒ 实测靠**合成节点实验**：往渲染树塞一条会命中的 `<style>url(http…)` 规则 —— 不中和 ⇒ 请求真的发起（nonDoc=1）；中和 ⇒ 0 请求 + `clean.styleUrl=1`（修正轮 `styleurl`/`styleurln` 两变体）。两条同族健壮性约束：**超时路径的宿主清理由 `work.then(cleanup, cleanup)` 兜底**（超时先于挂载时 race 侧 cleanup 是空刀，work 之后仍会 appendChild ⇒ 必须在自己结算处再清一次；cleanup 幂等）；**逐页处理的 `attCanvasFitForModel`/取样段也必须包在单页 try/catch 内**（同步抛 ⇒ 只跳该页 + 注记，不冒泡成整轮失败）。

### 办公文档写路径（xlsx / xls / docx / pptx，0.2.1 起）

- **手写 sheet XML 的两处机械量 = `r` 属性位移 + `<dimension>` 更新**（`insert_rows` / `delete_rows` 的由来）：单元格与行的地址写在 `<c r="B4">` / `<row r="4">` 的 `r` 属性上，改结构要**逐元素**位移（只改 `<row>` 不改 `<c>` 就是"值搬家一半"）；行属性（`ht` / `customHeight` / `s` / `spans`）随元素一起走。`<dimension ref>` 的口径：**insert 只扩不缩**（与原 ref 取并集）、**delete 允许收缩**、整表无格时落 `"A1"`。判据必须打到原始 XML —— 值级回读对"漏改 `<c@r>`"和"忘改 dimension"都可能是绿的。**同族教训：表序号也是 index** —— `localSheetId` / `activeTab` 是"第几张表"，`move_sheet` 不按同一置换重映射就会把命名区域**静默挂到别的表**。
- **办公产物不能拿整包 md5 当可复现判据**：`fflate.zipSync` 未指定 `mtime` 时用当前时间写 DOS 时间戳（实测同一输入两次为 `3f0a` / `3f0b`），`docProps/core.xml` 还带**秒级**时间戳 ⇒ "同输入同输出"类断言会假红（W-docx / W-pptx 的 R1 / R5 各实测过一次跨秒假红）。统一口径 = **逐条目内容比较**（解包后逐条 md5 / sha256；"预期被改部件集"**每用例运行期现算**，不写死），严格逐字节仍是主判据，只在严格不等时给"仅 `core.xml` 时间戳不同（其余 N−1 件逐字节相同）"的**等价分支**，并配**双向自检**（只改 1 秒 ⇒ 等价接受；给任一部件加一段注释 ⇒ 等价拒绝），防等价分支恒真。
- **pptx 的两套号源（P1-1）：读侧页码 ≠ 写侧索引**：`ReadOffice` 的 `----- 第 N 页 -----` 号 = `slideN.xml` 的**文件名序号**（docstream 显式按 `slide(\d+)` 文件名排序），`Office` 的 `slide_index` = 幻灯在 `p:sldIdLst` 里的**位次**。实测形态：**删第 2 张后读侧跳号 `1 / 3 / 4`**；**`add_slide@k` 的新页文件名号最大（读侧落在最末）**；`move_slide` 只重排 `p:sldIdLst` ⇒ 读侧顺序不变。⇒ 写坐标一律以 `outline` 的 `i` 为准（`items[i].part` 提供「读侧页码 ↔ `i` ↔ 实际部件」三向对齐）；"按页码 − 1 当索引"在 delete 与 add_slide 两臂都必错（T-W3 的负例实测咬住）。改读侧编号属判据变更，不在本批。
- **`set_paragraph` 的两条硬约束**：① docx 侧**必须保住 `w:pPr`**（标题级别 / 编号 / 对齐都在这里），且判据要打到物理面（`w:pPr` 首子元素 + `pStyle` 值）—— 只看"文本被换了"抓不住（丢 `pPr` 的负例里文本类断言仍是绿，红的是"`pPr` 保留面"那 5 条）；清旧 `w:r` 前先取**静态快照**（`getElementsByTagNameNS` 是活动 NodeList，边删边遍历会漏）。② pptx 侧新 run 必须插在 `a:endParaRPr` **之前**：CT_TextParagraph 的次序是 `a:pPr?, EG_TextRun*, a:endParaRPr?`，空段直接 `appendChild` 会产出 `[pPr, endParaRPr, r]` 这种**违 CT 次序**的产物（真 PowerPoint 严格；本机 PowerPoint COM 不可用 ⇒ 由次序判据 + 单点负例守着）。
- **上层"新造 spec"会静默漏掉写侧字段**：xlsx 的 `number_format` 就这样丢过一回 —— 接线层只把 `{sheet: …}` 传给写入内核 ⇒ 显式格式**不报错、不生效**（值级回读天然看不见）。修法 = 让上层把调用方 spec **原样带上**再只覆盖目标表（`xlsxSpecWithSheet` 一类收口），凡"内核字段被上层漏掉"都用这条根治。


- **`file://` 下不是所有 blob 手段都能用**：`new Worker(blob:…)` 能构造成功，但 worker 里 `importScripts(blob:)` 与 `fetch(blob:)` 都会被拒（`eval` / `new Function` 反而可用）。两个内嵌库都按这个约束选了各自的加载路径（见 `docs/PDFJS-NOTES.md` / `docs/TESS-NOTES.md`），改库版本前先读它们。
- **Tesseract 的 `cacheMethod:'none'` 必须开**：它跑在 blob worker 里，而 `file://` 下 worker 内 IndexedDB 的 open 请求不触发任何事件，默认的「读缓存」会永久挂住。注意区分：`file://` 的**主文档**里 IndexedDB 是正常的，所以附件库在 `file://` 下也能用。
- **`syntax.js` 要跳过 `<script type="text/plain">`**：内嵌的 wasm / 训练数据放在这种块里，当成 JS 解析会报一堆语法错误，把真正的问题淹掉。
- **`requestAnimationFrame` 在后台标签页会被暂停**：真实浏览器验证前必须先把标签页调到前台，否则流式重绘根本不发生（看起来像渲染坏了）。
- **本机 8090–8123 端口段被 Windows 保留**（`listen EACCES`）：无头验证脚本与静态服务器统一用 9000+。
- **无头脚本的就绪判定要盯住「换过文档了」**：`Page.navigate` 之后旧文档还在，只判「有 `.msg` / `#welcome`」会在旧文档上立刻返回 true；带种子数据时旧文档里同样有 `.msg`，所以必须用 `performance.timeOrigin` 变化来确认换过文档。
- **多套件别并行跑**：同一台机器上并行会因负载互相干扰出假失败，串行跑。
- **看门狗超时必须大于「正常总耗时 + 充足余量」**：看门狗到点只是**驱动侧放弃等待**——被掐断的套件在页面里**仍会继续跑**（`main()` 的 promise 被丢弃但没停），随即污染下一套件的读数。实测（需求 9）：预算从 540 s 提到 1200 s 后同一套件给出干净读数 `卡片数=1 / files:9 / bytes:50331659`，而"被掐断"轮留下的是 `files:11 / bytes:50331723` ⇒ 差值 `64 B = 20 B + 44 B`，恰是后台残跑写的两个小文件（算术坐实：不是判据松，是读数脏）。
- **中止类断言必须配负例对照，且要防空转式假假**：只断言"中止之后没有成功"会与"压根没跑起来"混为一谈。做法是另跑一轮**撤掉中止动作**的对照（夹具要足够大），让**同一条判据**对着它求值 ⇒ 必须为 `false`（证明这条判据对"中止没生效"有咬合力）；负例轮还要独立断言"确实跑起来过"（出现进行中状态、只留一张卡、结果文本含 `"ok":true`），否则判假的原因是空转而非缺陷。
- **`file://` 只能自己开无头 Chrome 验**：WebBridge 打不开 `file://`；用 CDP 的 `Page.navigate` 到 `file:///…` 再 `Runtime.evaluate`。

### 构建与产物

- **自写 ZIP 的两个「大小」字段不能混**：本地头（偏移 18/22）与中央目录（偏移 20/24）各有「压缩后大小」与「原始大小」；用 `deflate-raw` 压缩时两者不相等，两处都写压缩后的大小会让解压工具读出半截文件。
- **改 `src/*.part` 后必跑构建链**：`node scripts/build.js && node scripts/syntax.js && node scripts/lint.js`；产物 html 是构建结果，手改会被下一次构建覆盖。
- **JS 代码里的 `</script` 序列会提前终止脚本块**：内嵌载荷的生成脚本统一把它转义成 `<\/script`（语义等价），并断言 `<!--` / `<script` 出现即报错中止。
- **新增分段不得提前闭合 IIFE**：只有 `appE.part` 结尾收 IIFE 与 `</html>`。
- **`jupyterlite.part` 的读取口径与 pyodide 载荷逐字同款、无捷径**：按字节读 + **含 CR 即 `exit 1`**（该载荷按**字符数**记长，CRLF→LF 归一会让段长整体错位——不要"顺手统一行尾"）；**缺 `src/jupyterlite.part` 不是错误**：`build.js` 打印警告 + 注入 `JL_AVAILABLE=false` 的**极小占位段**（不 `exit 1`，只让 Jupyter 入口整体不可用）。`appJ.part` 插在 `appD` 与 `appE` 之间，`appE` 仍必须是最后一段。
- **`make-jupyterlite-part.js` 的载荷不含 pyodide core**：`pyodide.asm.mjs` / `pyodide.asm.wasm` / `python_stdlib.zip` / `pyodide.js` 由主页面 `#pyodide-assets` 在运行期复用（有强断言守着，重复打包会让三档体积白涨）；一次生成三件交付物（站点段 + Jupyter 专用锁 + 合并后的 `all.json`）；站点 5 处程序化改写的**唯一权威表** = `scripts/jupyterlite-patches.json`（要改站点行为改它，不在 `vendor/` 上手改——`vendor/` 是只读取件区）。
- **`src/*.part` 有超长行：Bash `grep`/`cat` 直扫会把工具层打崩**：`src/*.part` 最长行 **18,426,328 字符**（`pyodide.part`，`pyodide-normal.part` 同值），`≥128 KiB`（131,072 B）的行共 **175 条**。⇒ **禁止** Bash `grep`/`cat`/`sed -n p` 直扫 `src/*.part`；**一律**用 `Grep` 工具（"全库"语义**须 `include_ignored=true`**：它默认遵守 `.gitignore`，会静默跳过 `src/pyodide-{normal,minimal}.part` 与 `AzusaAI-WebUI-*.html`）或 `grep -c`；必须在 Bash 里扫时，输出**必须 `| head -c N` 字节封顶** —— **`head -20` 只封行数、不封字节**（实测 `grep -n … src/*.part | head -20` 命中 3 行、输出 **15,122,637 B**，`head -20` 一行都没滤掉）。**由来**：同一类缺陷（grep 判据的**作用域 / 形状 / 期望值**）在本项目**第三次复发** —— `desktop-icon` 方案 §3 S0 ① 的 `# 期望：空` 实测 **3 行 / 15,122,637 B** ⇒ **连崩三任 Worker**（工具层 `RangeError: Maximum call stack size exceeded`；`grep` 自身退出码正常 ⇒ 报错不指向凶手）；取证报告 = `shared/progress/desktop-icon-s0-crash-rca.md`；方案审查口径见 `shared/decisions/plans-decision-gates.md`「常设审查项 · grep 判据三查（E-10）」。

### 验证取证纪律（0.2.0 起固化）

- **多轮跑必须逐轮落「轮次子目录」**：`shared/tmp/{批}/out/r1/`、`out/r2/`… —— **共用一个 `out/` 会被末轮覆盖**，于是报告里「全部留档 / 中间 FAIL 轮已留档」的说法与磁盘实际不符（本项目**真的发生过一次**：某批第 2 任的 3 个 FAIL 轮被末轮覆盖，全库 grep 无留存，审查只能记一条「声明与磁盘不符」）。
- **报告里凡写「已留档」的，落笔前必须 `ls` 复核一次**。这是同一条纪律的另一半：光有子目录还不够，"我以为留了"不算留。
- **证据三件套**：① **stdout 全量日志**（不得只留摘要）② 读数 JSON ③ 截图；且**权威交付路径只能有一份**（`shared/progress/{批}-done.md`），并发的第二实例必须写进自己的私有子目录（`shared/tmp/{批}/mine/`），报告头部写明**自己的实例标识（端口 / profile）**。
- **判据文件的 mtime 必须早于末次运行**：改了判据不复跑 ⇒ 结论作废。改判据（断言、期望值、预算/超时、计数口径）必须写全**四段范式** = 原判据是什么 / 为何不成立 / 新判据是什么 / **新判据为何仍能抓住真缺陷**；**改动方向是「放宽」时**还必须配**负例注入**证明新判据会 FAIL。
- **看门狗预算要大于「正常总耗时 + 充足余量」**（理由与实测见上面 `file://` 段的同名条目：被掐断的套件会在后台继续跑并污染下一轮读数）。
- **改判据装置必须留档「改前原文 + 装置自身 md5」**（D3 装置留档纪律）：每次改验证装置（`*-inpage.js` / 运行器 / `lib.js` 一类）都要落**当轮**的 `out/rN/criteria-before.md`（改前原文 + 四段范式），并把**装置自身的 md5** 写进**当轮** `env.json`。只记文件名会让后续审计无法判定"这条读数属于哪一版装置"—— 本项目实际发生过两回：① 某一轮装置改判据既**没有** `criteria-before.md`、源码注释还把版本归属标错，审计者按注释去查的是一份**不存在**的留档；② `env.json` 的装置 md5 只自第 13 轮起才有，前 12 轮一律 `undefined`，该段读数**无法归属**。
- **"翻转型负例"必须给 `ORIG_MD5` 守卫 + `.bak` 字节级还原证明**：证明"某一条款承重"最省的做法是一次性翻转器（形态 = `node patch-*.js apply | revert | status`，**批内临时件、用完即删，不进 `scripts/`**），它至少要满足：① `apply` 前**现取**源码 md5 并与脚本内的 `ORIG_MD5` 比对，不符即 `REFUSE`（防在非终态 / 非本批交付态上误翻转）；② **锚点唯一性检查**（目标串命中数 ≠ 1 即 `REFUSE`）；③ `apply` 先把原文写 `.bak`、**只改一行**；④ 翻完必须 `build` → 跑完 → `revert` → **再 `build`**，且 `revert` 用 `.bak` 做**字节级还原**并回报 `md5 / 字节数 / ok`；⑤ 负例轮与对照轮用**同一装置、同一臂**，只差那一个开关 —— 否则无法把"判据有判别力"与"装置/环境变了"分开。样板见本项目 `patch-d2-bite.js`（D2 分离双向轮：`apply` ⇒ 删除传播臂 `C6DEL` FAIL（件仍在、状态行"本轮不判任何删除"）/ `revert` ⇒ 逐位回原值 ⇒ PASS）。

---

## 4. 数值口径附录

### 4.1 流式 chunk 里有 token 数吗（实测结论）

**默认没有。** 只有显式传 `stream_options: { include_usage: true }`，服务端才会在流的最后**额外发一个 chunk**，把整次请求的 `usage` 带上（该 chunk 的 `choices` 是空数组）；中间任何一个 chunk 都不含 token 数。

| 请求 | chunk 数 | 带 usage 的 chunk | usage |
| --- | --- | --- | --- |
| 流式，不带 `stream_options` | 49 | **0** | 无 |
| 流式，`stream_options: {}` | 60 | **0** | 无 |
| 流式，`include_usage: false` | 47 | **0** | 无 |
| 流式，`include_usage: true` | 58 | **1**（最后一块） | `completion_tokens: 58`、`prompt_tokens: 59` |
| 非流式 | — | — | `completion_tokens: 46`、`prompt_tokens: 59` |

容易踩的点：

1. **不能用 chunk 个数当 token 数**（llama.cpp 会把若干 token 合并进一个 chunk）。
2. 带 usage 的 chunk 的 `choices` 是空数组（OpenAI 规范如此），解析时不能假设每个 chunk 都有 `choices[0]`。
3. 有的实现把 usage 挂在最后一个内容 chunk 上（vLLM 某些版本），也有实现没带该参数时每个 chunk 回 `usage: null`——本项目的读法是「每个 chunk 都看一眼 `j.usage`」，不依赖它出现在哪一块。
4. 个别实现不认 `stream_options`，会直接 400——本项目会自动去掉该字段重试一次。
5. `usage.completion_tokens` **包含思考 token**；OpenAI 官方会给 `completion_tokens_details.reasoning_tokens` 用于拆分，llama.cpp 目前不给。
6. 拿不到 usage 且拿不到 `timings` 时，才退回「按文本长度估算」并加 `≈` 前缀。

### 4.2 速度与 token 估算

- **速度口径**：界面显示的是**端到端吞吐**（首字 → 最后一帧），比服务端日志里的纯解码速度低一些；两者都能在状态栏「最近一次生成统计」的悬浮提示里看到。
- **流式期间的速度**：3 秒滑动窗口内做最小二乘拟合（斜率即 tok/s）+ 指数平滑 0.15，带 `≈`；实测相邻读数跳变平均 0.35% / 最大 2.3%。结束后换成精确值：`completion_tokens ÷ (最后一帧 − 首帧)`。
- **token 估算四桶系数**：CJK 0.804 / 拉丁字母 0.170 / 数字 1.041 / 符号空白 0.319（token/字符的倒数换算后按字符数加权）。实测平均绝对误差 7.9%（三桶 17.7%，朴素系数 16.0%）。
- **自校准**：程序会记住每个模型上一次「估算 vs 真实」的偏差倍数（`settings.tokCalib`），下次自动校正；短回答（几百 token 以内）误差偏大——每次响应都有一份固定的模板开销，字符统计看不到。
- **上下文用量环**：优先显示服务端实测 token（最近一次请求的 `prompt_tokens` + `completion_tokens`，再加上之后新增消息与草稿的估算，图片按 ~900/张粗算）；从没发过请求时才整体显示估算值（带 `≈`）。
- **存储上限口径**：`storageBytes()` 用 `JSON.stringify(state).length`（字符数）；Chrome 的 localStorage 配额按字符计，实测 5.00 MB（`5 * 1024 * 1024` 字符）。

### 4.3 性能基线（换库或大改之前先看这些数字）

- 完整版产物 ≈ **217.5 MiB**（228,036,277 B，2026-09-19 现取；三档见 §1.3）；三个内嵌库都是**惰性初始化**（第一次调用 API 或第一次识别才建 worker / 解码 wasm；pyodide 载荷同样懒解码，首屏不碰载荷正文）。
- 真实 Chrome（热缓存）：DOMContentLoaded ≈ 280ms、load ≈ 400ms（pdf.js + Tesseract 时期的实测；内嵌 pyodide 后首屏仍几乎不受影响，`file://` 下 `data-boot=done` 实测 ≈ 2.0s）。
- PDF：2 页 A4 文本模式 ≈ 70–110ms；150 DPI 渲染 ≈ 70–140ms。
- OCR：首次（含解码 wasm + 训练数据）≈ 0.2–1.9s；热调用 30–150ms。
- Python：首次执行 = 装配 CPython（含解码 + 起 worker）+ **全量装载本档全部预置包**（装载耗时在结果注记与任务快照里各有一行读数；三档实测见 `docs/PYODIDE-NOTES.md` 的「包装载与释放」一节）；空闲 **10 分钟**回收解释器（连已装载的包一起释放）。
- 办公解析（worker 路径）：4.4MB docx 74ms（冷）/ 50ms（温）；xlsx / pptx / ppt / doc 见 `docs/OFFICE-NOTES.md` 实测表。

---

### 4.4 输出截断口径（执行器族三方统一）

**方向**：执行器族（`ExecutePython` / `ExecuteJavaScript` / `TaskList` / `TaskOutput` / `TaskStop` / `WaitFor`）一律**保留末尾**；非执行器族（`Read` / `Grep` / `Glob` / `PythonPackages` / 工作区工具 / 外部 MCP）保持"留头 + 尾注"不变。

| 层 | 位置 / 常量 | 阈值 | 方向 | 注记 |
|---|---|---|---|---|
| ① worker 环形缓冲 | `PY_OUT_CAP` | 64 KB / 流 | 留尾 | 丢弃时在段首注明丢弃量 |
| ② worker → 模型单流 | `PY_MODEL_TAIL` | 8 KB / 流 | 留尾 | 截断即视为"被截断"（结果里会出现补取提示） |
| ③ traceback | `PY_TB_CAP` | 8 KB | 留尾 | 段首注明截断量 |
| ④ 主线程外层（决定模型最终看到什么） | `toolLimit("maxOut")`，默认 16384（可配 1000–131072） | 同上 | 执行器族留尾；其余工具留头 | 留尾时段首注明；留头时沿用"原始长度 N 字符"尾注 |
| 任务快照（`TaskOutput`/`TaskStop`/`WaitFor`） | `PV_TASK_SNAP_TAIL` | 8 KB / 流 | 留尾 | 超限时注明"只保留末尾 8 KB" |
| 卡片日志行 | `PV_ROW_TAIL` | 8 KB | 留尾 | 环形丢弃量另以徽标显示 |
| JS `console` | 沙箱 120 条 / 成功取末 60、失败取末 20 | 单条 >4 KB 也留尾 | 留尾 | 丢条数写进**唯一一条**说明（主线程侧生成，不重复） |

**可裁区与不可裁区**：可裁区 = `stdout` / `stderr` / `返回` / `exitCode` / `traceback`（逐段裁）；不可裁区 = `notes`（档位提示、工作区回写与跳过、并发冲突、装载摘要、截断提示）—— 不裁剪且序列化在末尾。

**预算（水填充）**：`budget = maxOut − Σnotes − PY_ENV_RESERVE(160) − pySecReserve(max)`，`pySecReserve(max) = min(500, floor(max×0.05))`；初始份额 `PY_SEC_SHARE = {stdout .40, stderr .20, result .25, tb .15}` 只是**上限**，未用份额按 `PY_SEC_ORDER = [result, stdout, stderr, tb]` 回填（`返回` 优先，因为它是用户投诉面）。预算下限 `max(256, floor(maxOut×0.40))`。默认 16 KB 下，只有一个大 `返回` 时它可拿到 ≈15.7 KB（远高于旧实现"固定份额 3.2 KB"）。

**提示语**：只有 `cut` 为真（worker 单流截断或本机任一段被裁）时才追加一句"本次输出被截断（只保留末尾）…"；未截断时结果里不多一个字符。

### 4.5 上下文压缩（Compact）

**一句话**：把「较早的消息」换成一份模型写的交接总结，然后**真删**那些消息与它们的附件（用户明确要求"没有保留的消息彻底删除"；与 deepseek 的 shadow/soft-delete 路线相反）。

**数值口径（`CX_*`，`appA.part` 常量区）**

| 常量 | 值 | 含义 |
|---|---|---|
| `CX_TRIGGER_DEF` / `CX_TRIGGER_MIN` / `CX_TRIGGER_MAX` | 85 / 60 / 95 | 自动压缩阈值（%），设置面板可调；`settings.compactTriggerPct` |
| `CX_WARN_GAP` | 15 | 近极限系统提醒阈值 = 触发阈值 − 15（下限 50），与自动压缩同源 |
| `CX_RESERVE_MIN` | 16384 | 冗余触发项：`reserve = min(max(本值, win×0.05), win×0.5)`（照 Kimi 的 `reserved < max` 守卫） |
| `CX_OVERFLOW_PER_TURN_MAX` | 3 | 溢出恢复**每次生成轮**的次数上限（借鉴-1；达到后不再自动重试，交由用户处置） |
| `CX_KEEP_RATIO` / `CX_KEEP_MIN_TOK` / `CX_KEEP_MAX_TOK` | 0.15 / 8192 / 20000 | 保留尾部预算 = `clamp(round(win×0.15), 8k, 20k)` |
| `CX_HEAD_TOK` | 2000 | 最早的用户输入保留预算（`keepHead`，整条原文，不截断） |
| `CX_KEEP_MAX_MSGS` | 8 | 保留尾部最多几条消息（`auto` 向左扩展时的约束） |
| `CX_LOOP_TAIL_MIN` | 2 | 「单用户轮有界 tail 档」要求尾部至少含这么多个**完整工具循环** |
| `CX_MIN_MSGS` | 4 | 少于这么多条消息时拒绝压缩（`ECOMPACT_SMALL`） |
| `CX_SUMMARY_TIMEOUT_MS` | 900000 | 摘要请求超时（**每次尝试各计**；全量输入后在长会话可能数分钟，`Ctrl+X` 可随时取消） |
| `CX_SUMMARY_OUT_CAP` / `CX_SUMMARY_OUT_MIN` / `CX_SUMMARY_FIT_MARGIN_RATIO` | 131072 / 4096 / 0.05 | 摘要输出预算：封顶（Kimi `128*1024`）/ 适配收窄下限 / 输入估算余量（配套下限 1024） |
| `CX_SUMMARY_ATTEMPTS_MAX` / `CX_SUMMARY_SHRINK_RATIOS` / `CX_SUMMARY_SHRINK_MAX` | 5 / `[0.7,0.5,0.35]` / 3 | 重试链：总尝试上限 / 溢出缩窗比例 / 缩窗次数上限（Kimi 同值） |
| `CX_SUMMARY_BACKOFF_BASE_MS` / `_MAX_MS` / `_JITTER` | 500 / 32000 / 0.25 | 可重试错误的指数退避（`min(500×2^i, 32s)`，+0~25% 抖动） |
| `CX_SUMMARY_MEDIA_KEEP_RECENT` / `CX_SUMMARY_MEDIA_MAX_BYTES` | 2 / 32 MiB | 媒体降级档保留最近几张图（Kimi 同值）/ 媒体体积闸（本项目扩展） |

**切分语义**：`split = min(kimiSplit, lastUser)`，其中 `kimiSplit` = 从末尾向前第一个满足 `cxCanSplitAfter` 的位置 + 1（Kimi 语义，单独求出，不与 tail 起点混用），`lastUser` = 最后一条**真实**用户输入（`m.compact` 非空的压缩产物不算）。⇒ **默认档** tail 非空且必含「最后一条真实用户输入 → 末尾」整段；随后 `while (split > 0 && !cxCanSplitAfter(list, split-1)) split--` 只向左回退（保留更多，永不越过 `lastUser`）。`mode="auto"` 只向更早方向扩展（`CX_KEEP_MAX_MSGS` / `tailBudget` 内）；`mode="manual"` 不扩展。

**单用户轮有界 tail 档**（`cxLoopSplitBound`，唯一允许 `split > lastUser` 的情形）：全列表恰有 1 条真实用户输入 + 切点落在**完整工具循环**边界（`list[p-1].role === "tool"` 且 `cxCanSplitAfter(list, p-1)`）+ tail 含 ≥ `CX_LOOP_TAIL_MIN` 个完整循环 + `tailTokens ≤ tailBudget`；命中时那条用户消息由 `keepHead` 原样保住（`elided = false`），切点永不落在进行中的循环上。该形态（一条用户消息 + 长工具循环 = 长时间自主工作）在默认档下 `split ≡ 0`、压不动。

**摘要请求**：`stream:false`，组装一律走 `buildBody(list + [指令], model, false, c, {maxTokens: budget, summary: true})` 单一来源（OpenAI 侧 `[system] + list + [user 指令]`；Anthropic 侧 `system` + `normalizeAnthropic`）；消息体 = **完整历史**（`payloadMessages(conv.messages, {noCap:true})` 的全部消息，角色 / 工具调用 / 工具结果原样，**发送前不做任何单条/总长裁剪** —— 对齐 Kimi「投影截断 = 压缩无意义」的裁定）；工具集与聊天轮同源（history-union），采样 / 思考挡位同源；`sys` 使用 `pvTaskBlockText` **只读变体**（文本与 `pvTaskBlock` 逐字节相同，但不登记 `PV_NOTIFY_INFLIGHT` —— 摘要请求没有配套的 `pvNotifyCommit/Rollback`，登记会静默吞掉后台任务完成回流）。

**全量优先、不得已才降级（写死顺序）**：① 先压**输出预算**（无损）：`budget = min(win, 128K)` 按 `room = win − estInput − margin`（`margin = max(1024, win×5%)`）三分支收窄到不低于 `CX_SUMMARY_OUT_MIN`（`estInput` 用四桶估算现算，不用 `ctxSnapshot`）；② 再缩**消息集合**（有损）：溢出缩窗 `[0.7,0.5,0.35]`（≤3 次，对象 = 当前集合总估算 token，尾部装满、丢前导 tool 结果）、截断/空响应丢最旧一条 + 前导 tool 结果；③ **媒体两档**：默认全量进请求 → `degraded`（保留最近 2 图、更早换 `[图像已省略…]` 占位；`request_too_large` 或媒体载荷 > 32 MiB 触发）→ `stripped`（全部占位；`image_format` 或 degraded 后仍超限触发）。预算在缩窗后**不重算**（偏保守）。可重试错误（超时 / 网络层 / 408·409·429·5xx）指数退避 500 ms→32 s + 抖动，总尝试 ≤ 5；`cxClassifySummaryFailure` 十类（溢出 / 体积 / 图片格式 / 截断 / 过滤 / 空 / 超时 / 可重试 / 致命 / 取消）——**500 不判溢出**（不触发有损缩窗）。

**`compactMeta` 与界面读数**：数值键 = `at / dropped / droppedTokens / before / after / released / freedBytes / ms` + 重试链五键 `attempts / shrinks / inputDropped / inputShrunk / mediaLevel`（**0 = 未发生**；`sanitizeConv` 的 `CM_NUMS` 只做"非有限数归 0"校验，不给旧数据补默认值对象）。`inputShrunk === 1` 时摘要卡显示「摘要输入曾缩窗（丢 N 条）」（纯媒体降级且未丢消息时显示「图片降级，未丢消息」）、`attempts > 1` 时显示「尝试 N 次」；`/compact` 完成 toast 与 Compact 工具回执同步追加一句 —— 摘要覆盖范围不完整必须如实标注。

**释放后置与共享 blob 陷阱（改这块之前必读）**：被丢弃消息的附件**不在 splice 里删**——`cxDropMessages(conv, from, to)` 只做「收集键 + 估算 token + splice」，释放必须由调用方在**保留集重插回数组之后**执行 `cxReleasePlan(keys)`（`keep = cxSurvivingBlobKeys()` 是**全库引用重算**，因此 `duplicateConv` 副本共享的键、以及 head 保留消息自己引用的键都会被保护），唯一的异步出口是 `cxReleaseBlobs(todo)`。三条顺序契约：① 先移除 + 重插保留集，再释放；② `keep` 重算必须在同步块结束、head 回位之后取；③ 同步块内不得 `await`（双标签页的 `pullConvsFromIdb`/`mergeConvs*` 可能在整个 await 之后替换数组）。
**统一口径的五个调用点**（此前四处既有缺陷 + 一处同源）：`clearConvMessagesNow`、`trimConversation`、`compressOneConv`、清理空间的内联 80 条裁剪、`stripAttachmentData` —— 一律 `var p = cxReleasePlan(keys); if (p.todo.length) cxReleaseBlobs(p.todo);`。

**失败/取消零改动**：摘要成功后才动数据；动数据前过两道闸（非工具路径 = `cxHistoryHash` 相等 **且** `cxHistoryPrefixIntact`；工具路径 = 前缀完整、允许尾部增长）；`cxApplyCompaction` 先按 id 重取会话（`getConv(conv.id)`），其后只用重取到的对象。工具路径「只算不落」（`CX_RUN.pending`），应用点是**本轮所有工具调用结束后**的唯一一处（`await cxApplyPending(tf)`，漏 `await` 会让被删消息继续发给模型）。**取消通道 = 运行级 `AbortController`**：`CX_RUN.ctrl` 在整段压缩运行期间持有（请求与退避 `cxSummarySleep` 共用同一 signal；`cxCallSummary` 把它桥进本次请求的局部 ctrl，不再自设/自清 `CX_RUN.ctrl`）；`Ctrl+X`（macOS `Cmd+X`）在生成中走 `stopGenerating → cxAbortActive`（既有链路），手动压缩（`generating=false`）走 `cxRunActive → cxAbortActive`（**不**调 `stopGenerating`，避免误杀沙箱 / 工具等待 / 自动续跑）；ladder 另有「成功前 abort 复检」关闭「响应已到、应用未开始」的窄窗。

**摘要与用量环**：摘要请求的 token 消耗**不进**用量环与累计统计（它是管理开销）；压缩成功后目标会话的实测锚点作废（`usage.exact = false`、`lastMsgId = ""`，累计量不动），否则用量环不下降、去重与触发判定失真。

**界面口径（0.2.0 · U-4）**：压缩在对话里是**工具气泡原位 + 实时流式**——运行中卡带阶段行（去重、≤8 行）与摘要草稿逐字增长（活体标记 `[data-cx-live]`，收敛即消失）；完成卡带 chips（丢弃条数 / 时间范围 / 释放量 / 尝试次数；输入曾缩窗另标「摘要输入曾缩窗（丢 N 条）」）；失败卡正文固定「本次压缩没有改动任何消息，原文仍完整保留。」+「重试 / 知道了」。**位置 = 原位、永不挪位（不置顶）**；运行 / 失败卡**不得**退化为可折叠工具卡（无 `data-tool-toggle` / `aria-expanded` / 箭头）。失败与取消对 `conv.messages` 的改动数必须为 0、不残留半截草稿。三入口（`/compact`、`Compact` 工具、设置「立即压缩」）共用同一条运行链；`Ctrl+X` 取消走 `cxRunActive → cxAbortActive`（**不**调 `stopGenerating`）。触发侧两条口径：溢出恢复每轮至多 `CX_OVERFLOW_PER_TURN_MAX`(3) 次；冗余预留 `CX_RESERVE_MIN` 参与 `reserve` 计算（上限 `win×0.5`）。

**版本号唯一口径**：`APP_VERSION`（`appA.part`）是用户可见版本号的唯一权威 —— 环境面板、环境信息、导出备份的 `version`、MCP `clientInfo`、关于弹窗、`<meta name="version">`（启动时同步）全部读它；不要在别处再写版本字面量。

### 4.6 文件工具的多媒体读取（Read 文本 / 图片 · ReadOffice 办公文档 / PDF）

**路径双写（唯一权威句 `WS_PATH_DUAL_NOTE`，`appE.part`）**：工作区 9 个文件工具一律用 `/…`（`wsNormPath` 只认 `/` 开头、禁 `..` 与反斜杠），而 Python 解释器里同一份数据挂载在 `/workspace`（`PY_MOUNT` / `pyContainerPath`）——**两者指向同一份文件**。这句话同时进 `【工作区】` 上下文块（每轮请求的 system，≈100–130 token 成本）、其中 8 个文件工具（第 9 个 `DownloadForUser` 用**引用式**一句「同其它工作区工具」，不复制原文）与 `ExecutePython` 的 `modelDesc`（`ExecuteJavaScript` 为手抄同句）、`PV_TASK_COMMON`（4 个任务类工具共用尾段）。两条定向纠错都只改错误文案：工作区工具路径以 `/workspace` 开头且报 ENOENT 时，`wsToolRun` 追加口径提醒，且**只在"去掉前缀后的路径确实存在"**（同步查 `WS_META_CACHE[wsId].tree`，缓存缺失就不确指）时给出「你要的可能是 /x」；Python 侧的 `__pyShimOpen` 在 `FileNotFoundError` 分支里对 `not p.startswith(_KIMI_MOUNT) and os.path.exists(_KIMI_MOUNT + p)` 的情形补一句「这个文件在工作区里是 "/workspace" + 原路径」——**只覆盖 `builtins.open`**：`os.open` 是另一个引用，`pathlib.Path.open/read_text` 走的是 `io.open`（**不在覆盖内**），`pandas.read_csv` / `PIL.Image.open` 等最终走 `builtins.open` 的路径在内；**只加文案、不改异常类型**（仍是 `FileNotFoundError`）。两点实现约束（回归教训）：① 挂载点由 JS 侧 `JSON.stringify(__PY_MOUNT)` 拼成 **Python 字面量** `_KIMI_MOUNT` 注入 —— 裸写 `__PY_MOUNT` 是 worker 的 JS 变量，Python 命名空间里不存在（会 `NameError`，把 `FileNotFoundError` 换成更难读的报错）；② wrapper 的安装与「本次有没有超预载上限文件」**解耦**（工作区挂载即安装，`_KIMI_REMOTE` 每次 run 重写），否则没有超限文件时该提示永不生效；安装失败往 stderr 打一行诊断而不是静默吞掉。

**Read 的类型路由**（`wsReadTool`，顺序即优先级）：文本（含 svg；输出逐字节不变）→ 办公 / PDF（后缀或 magic）⇒ **不再代读，直接返回 `EINVAL` 重定向**（按后缀逐类给词「办公文档 / 演示文稿 / 表格 / PDF」+ 可照抄的 `path` JSON 指向 `ReadOffice`；判据用 `wsDocKindOf` 的返回值而非后缀 ⇒ magic 兜底时不会误判）→ 图片（magic 命中且 mime 是 `image/*`，或后缀 ∈ 可解码集合 png/jpg/jpeg/gif/webp/bmp/avif/ico/tif/tiff（= `WS_READ_IMG_EXT`，与 `README.md` 的枚举同集；tif/tiff 的真 TIFF 本机浏览器解不开，进图片路径只为拿到「无法解码 + 提示先转换」的可读错误））→ 宏格式单独拒 → 其余二进制给新 EBINARY 文案（`zipfile` 解包再 Read 是新增的真实出路）。重定向分支**不产生任何解析副作用**（不调解析库、不 OCR）；对称地，`ReadOffice` 遇纯文本 / 图片也返回 `EINVAL` 指向 `Read`。
**ReadOffice 的类型路由**（`wsReadOfficeTool`，校验与路由都在**解析之前**完成）：`path` 必填 → 三条参数互斥校验（`page_offset` 与 `line_offset` 互斥 / `column_offset` 不能与 `page_offset` 同给 / `column_offset` 不能与负数 `line_offset` 同给，均为 `EINVAL`）→ 纯文本指 `Read` → 办公六格式或 PDF 走既有解析链 → 图片指 `Read` → 宏格式沿用 EBINARY（「请另存为 .docx / .xlsx / .pptx 后再用 ReadOffice 读」）→ 其余二进制走 `wsBinaryErr`。

**图片预算（单一权威 `READ_*` / `TOOL_MEDIA_CHAT_*`，`appE.part`）**：最长边 ≤ `READ_IMG_MAX_EDGE`(2000) 且与 `imgMaxSide()` 取小；尽力压到 `READ_IMG_BYTE_BUDGET`(256 KiB)，梯子 = PNG（保 alpha）→ JPEG 0.8/0.6/0.4 → 边长回退 2000/1000/768/512/384/256；**单张硬顶 = 单次总量 = `READ_IMG_HARD_MAX` = 1 MiB**（压不到就报错、**不发原图**）；单次交付 `READ_MEDIA_MAX_IMAGES`(4) 张、累计 ≤ `READ_MEDIA_TOTAL_BYTES`(1 MiB)，超出按读取顺序保留并在状态行写「另有 N 张超出单次上限未附带」；直通（字节 ≤ 256 KiB、边 ≤ 上限、mime ∈ `SAFE_IMG_MIME`）**仍会解码验证一次**再交付原字节——这一层挡住"截断/损坏但 IHDR 还写着小尺寸"的假直通。解码两道门：像素 `READ_IMG_DECODE_MAX_PX`(40M，先用 `wsImgSniffDims` 嗅 PNG/JPEG/GIF/BMP/WebP 尺寸头，**嗅不到尺寸的 avif/ico 只能在解码后复核**，这是已知残差) + 字节 `READ_IMG_DECODE_MAX`(32 MiB)，取与。交付 mime 收敛到 `{image/png,image/jpeg}`（重编码）∪ `SAFE_IMG_MIME`（直通）——**源 mime 不在 SAFE 集时不走「压不动回原字节」**（否则交付 mime 会落在收敛集合之外，例如 1×1 BMP 交付 `image/bmp`），改为交付最佳重编码（仍 ≤ 单张硬顶）。

**非视觉 → OCR**：`wsOcrTextOf` 是唯一出口；输入是**按上表缩放后的画布 dataURL**（与附件 `maybeOcrImage` 同形）；超时 = **整次读取的总预算** `min(READ_OCR_TIMEOUT_MS(120 s), toolLimit("timeoutMs"))`（`Read` 读图片与 `ReadOffice` 读文档图片共用同一常量），多页共享、页间查 `ctx.signal.aborted`、文本累加到工具结果预算即停；引擎不可用 / 超时 / 无文字各有独立可读文案（**不写"请重试"**）。

**图文按序融合（非视觉模型的 PDF / 办公文档，批 B；落地记录 `shared/progress/netdocs-B2b-done.md`）**：ReadOffice 侧与附件侧共用 `appD.part` 的三个助手——`ocrSeqCands`（逐张 `wsImgFitForModel` → `wsOcrTextOf` + 预算 / 中止 + 计数，**不做任何文案**）、`ocrBlockText`（块头唯一来源，空 / 纯空白 ⇒ `""`）、`docFusePages`（按页插块；无块 ⇒ 原样返回）。块格式（唯一口径）：`【第 N 页 · 图 k · 本机 OCR】`；没有页 / 表标记的 office 段内不分区 ⇒ `【图 k · 本机 OCR】`（label 由 OfficeKit 给）；整页兜底 ⇒ `【第 N 页 · 整页图像 OCR(含文字层重复,可能有误差)】`（这条**不带**「· 本机 OCR」）。**插入位置（P2 的 S15 起）**：`wsDocumentPack` 的融合分支按 `kind` 选布局 —— `kind === "pdfPage"`（PDF，以及 **pptx**——它的正文自带 `----- 第 N 页 -----` 幻灯标记）⇒ `layout:"pages"`，块插到**对应页 / 幻灯段之后**（`docFusePages`；未匹配到页号的块追加文末并注明 `(原页未在文本中找到)`）；其余 office（docx / xlsx）⇒ `layout:"flat"`（正文之后按序接）。**附件侧（`attOcrFuse`）不受 S15 影响**：office 三格式仍一律 `flat`，只有 PDF 用 `pages`。预算：ReadOffice 侧 `READ_DOC_OCR_MAX_IMAGES`(12) / `READ_DOC_OCR_MAX_PAGES`(8) / `READ_OCR_TIMEOUT_MS`(120 s)；附件侧 `ATT_IMG_MAX_IMAGES`(10) / `ATT_OCR_TIMEOUT_MS`(60 s，**整次解析共享、只覆盖 OCR 阶段**；取图调用各另带 60 s 超时)/ 扫描页数 = `min(pdfMaxPages(), READ_DOC_OCR_MAX_PAGES)`；字符预算两边都 = `max(512, toolLimit("maxOut") - 512)`，附件侧另受 `ATTACH_TEXT_MAX` 截断。取图 = `PDFKit.pageImages`（**只有 `mode:"rect"` 的页用 rect**，`mode:"full"` 走整页护栏）→ `renderCrops`，失败单调降级到整页 `renderPages` 并注记。落点：ReadOffice 侧进工具结果 + 状态行；附件侧进 `att.text` / `att.ocrImgs` / `att.degraded`（视觉路径与无图文档逐字节不变）。

**媒体怎么进模型（`m.media` 内联 + 协议双形态）**：消息上唯一新字段 `m.media[]`（`kind/mime/data/w/h/bytes/path/label/from/srcW/srcH/scaled`；`resultText` 只写文本 + 「已附带 N 张」）。payload 项内部字段 `media` 由 `payloadMessages` 带出：**Anthropic** 走 `tool_result.content` 内容块数组（文本 + image）；**OpenAI 兼容**走"tool 文本 + **按工具轮分组**的一条合成 user 消息"（连续 `role==="tool"` 段 = 同一工具轮，组内 media 合并、插在段末最后一条 tool 之后，跨段不合并 ⇒ 不产生 `tool→user→tool` 交错）；合成消息**只存在于请求体**，不落盘、不进界面。工具轮内还有一个 `msgList.push({role:"tool"})`（续答轮），**必须带上同一个 media**，漏了就是"本轮模型看不到刚读的图"。

**严格端点 400 的降级与复位**：`isToolMediaStructureError`（显式状态码 `{400,422}` + 结构类正则）命中且 `msgList` 里仍有带 media 的 tool 项时，工具轮内 `mediaOff = true` 后重跑本轮（**整次 send 只一次**；`buildBody` 收 `noToolMedia` ⇒ 工具文本照发、仅图片不随本次发送），并 toast 说明。复位路径三条：① 本次自动降级（不落盘）；② 设置 → 模型 把该模型图像能力设为「不支持」（`visionForce:"no"`）或换模型；③ 删除 / 压缩掉那条带图片的工具消息。降级后仍失败时，失败文案追加一条指向②的恢复建议。

**两条预算纪律**：文本预算（`toolLimit("maxOut")`）只管 `m.content`，媒体字节走 `m.media` ⇒ 附图不挤状态行、正文过长也不削图；**聊天轮软护栏** `trimToolMediaForRequest` 对非摘要请求保留最近 `TOOL_MEDIA_CHAT_KEEP_ITEMS`(8) 条带媒体工具项、累计 `TOOL_MEDIA_CHAT_MAX_BYTES`(8 MiB，`data` 字符量)，更早的**非破坏性**替换为 `[图像已省略:超出请求体积上限,需要时可重新读取]`（存储一字不动）；摘要请求不走这条护栏（b25 的 `cxSummaryMedia` 阶梯 + 32 MiB 闸自有口径）。工具媒体计入 token 估算的唯一入口是 `estimateToolMediaTokens`（调用点清单：`payloadMessages` 的 token 循环 + `cxPayloadMsgTokens`）。

**存储代价（如实）**：媒体内联在消息里（不新增 blob 键 ⇒ 不触碰释放链），单条消息最大 ≈1.4 MB base64；三条放大面 = 每次 `saveState(true)` 整条会话写、导出备份整份 `JSON.stringify`、历史图片逐轮重发（软护栏封顶）。**「优化存储」可回收**（`stripAttachmentData` 在 `m.content` 早退之前删 `m.media`，`keepIds` 保护口径不变）。**计量口径**：`storageBytes()` 量的是 localStorage 设置串，**不是会话存量** —— 报告占用请用 `navigator.storage.estimate()` / IDB 实测，不要拿它宣称"媒体存储已计量"。

**ReadOffice 复用办公 / PDF 引擎的边界**：复用引擎（`OfficeKit.parse` / `PDFKit.extractText|renderPages` / `OCRKit.recognize`）与数值（`ATTACH_TEXT_MAX`、`officeSizeCap()`、`pdfDpi()`、`pdfMaxPages()`、`ocrLangs()`、`SAFE_IMG_MIME`、`IMAGE_EXT/OFFICE_EXT/MACRO_EXT`），**不复用** `resolveOffice/resolvePdf/resolveImage` 的 File+vision+toast 包装（ReadOffice 的入参是 Blob + 目标会话，包装会引入"生成期间切走会话"的判定错会话问题）；PDF 走文本优先、不吃附件图片策略（`attachImgMode()`）设置。文档状态的"文件:…"信息一律并入**末尾状态行**（不占行号，`line_offset` 语义与文本读一致），`clipped` 在状态行标注。

**ReadOffice 的两级偏移坐标（页 / 表 / 幻灯 + 图片序号；本节为正式口径）**

| 槽位 | 参数 | 语义（唯一权威 = 工具条目的 `inputSchema.description`） |
|---|---|---|
| 第一级 · 页 / 表 / 幻灯 | `page_offset` + `n_pages` | 1 起；负数从末尾倒数（-1 = 最后一页）；省略即从头；省略 `n_pages` 即到文档末尾（仍受 `max_chars` 约束）。定位靠**文档自带标记**：PDF 与 **pptx** 都用 `----- 第 N 页 -----`（`PDF_PAGE_SEP_RE`，与附件侧同一字面量；pptx 标记由 office 胶水写，见 `docs/OFFICE-NOTES.md §6`）、xlsx / xls 用胶水写的 `# <表名>` 段头 |
| 第二级 · 全局行号 | `line_offset` + `n_lines` | 行号是**整篇的全局行号**（与文本读同一坐标系、含尾换行的计法一致）⇒ 可与页 / 表坐标互相换算；给出 `page_offset` 时不得同给 `line_offset` |
| 续读超长行 | `column_offset` | 只用于按行读的续读；与 `page_offset` **或**负数 `line_offset` 同给都报 `EINVAL` |
| 图片序号 | `image_offset` + `n_images` | 1 起、按文档顺序编号（**pptx** 的候选图带真幻灯号 ⇒ 可直接用于块头标注与 `page=0` 判别；docx / xlsx 的解析层页码只是"拿不到真页码时的兜底序号"—— **两者都只按 `image_offset` 取窗口**，pptx 的真幻灯号**不参与**页窗口过滤：该过滤只存在于 `wsPdfImgCands`、也只由 PDF 调用点传 `pageFrom/pageTo`）；给出 `image_offset` 时**只回图片不回正文** |
| 单次返回量 | `max_chars` | 默认取本次可用预算上限，硬上限 65536 |

- **三种 `kind`**（`docSectionIndex` / `wsDocPageView`）：`pdfPage`（自带页标记：PDF 与 **pptx**）、`sheet`（xlsx / xls 的表段头）、`none`（docx / doc / ppt **没有任何页 / 表信息** ⇒ 给 `page_offset` 一律 `EINVAL`「本格式没有页/表标记…请用 line_offset 按行读」，状态行写「页标记:无」）。**pptx 在 P2 之前是第四种 `slides`**（有真实幻灯数但没有幻灯标记 ⇒ 保留「共 N 页」、按页定位报错）；S11 给 pptx 补上幻灯标记后它**并入 `pdfPage` 族**（S15 把 `appE` 的 `kind` 映射改到该族，`slides` 这个值已退役）。**空幻灯没有正文 ⇒ 没有标记 ⇒ 段号会缺号**：段序按标记出现先后（在"第 4 张为空幻灯"的样件上 `page_offset=4` 命中「第 5 页」段），段数 ≠ 幻灯数。
- **窗口透传**：页 / 表窗口（`meta.window = {start,end}` 的全局行闭区间）由 `wsDocPageView` → `wsLinePageResult` 一路透传；漏传 ⇒ 窗口不滑动（表读仍从第 1 行起）。`Read` 的文本路径**永不**传 `meta.window` ⇒ 恒走旧语义。
- **状态行（正文视图）**：`[file 路径 · 格式 · 共 N 页/表 · 本次 第 a-b 页 · total 总行数 · shown 显示范围 · eof 是否到末尾 · size 字节数 · mtime 修改时间 · next 续读参数]`；图片视图以「图片 共 N 张 · 本次 第 a-b 张」开头。`next {…}` 由 `wsDocNextArgs` 生成，是**紧凑 JSON、不含 `path`、可直接照抄**进下一次调用的增量参数，且**仅在续读坐标存在时生成**（只被 `truncatedExtra` 置真而无续读坐标的例子不算截断例）。其中「共 N 页/表」是**解析层声明的总数**（pptx = `docProps/app.xml` 的幻灯数，**含空幻灯**）；有标记的**段数**与之不等时（空幻灯没有正文 ⇒ 没有标记）补写段数，形如「共 6 页(5 段有标记)」—— 让它与越界文案（按**段数**计）的两个数在第一次读到时就在一起。
- **超范围不静默**（S3b）：office 抽取文本超过胶水的 `TEXT_MAX`（**默认 131,072 字符**，P2 的 `S12` 起可由包装层按次覆盖 —— `OfficeKit.parse(buf, ext, {textMax: n})`，缺省 / `0` / 非法值退回默认；`ReadOffice` 工具自己没有这个参数、恒用默认）时，超限部分已被 `slice` 掉（**任何偏移都取不到**）⇒ 状态行如实注记「仅前 N 字符,剩余内容本工具读不到」；PDF 正文抽取上限 = `READ_PDF_TEXT_MAX_PAGES`(200 页) 与 `READ_PDF_MAX_CHARS`(400,000 字符)，命中则给「仅前 N 页 / 仅前 N 字符…（分窗读属后续批次）」，两条原因可同时成立。**两条硬边界都不许用 `ExecutePython` 去拆**，如实告诉用户即可。

### 4.7 提示词风格契约（模型可见文本的唯一风格口径）

> 适用面 = 内置 `AGENT_TOOLS` 的 `modelDesc` + 四个系统注入块（`wsContextBlock` / `pvTaskBlockText` / `mcpInstructionsBlock` / `cxContextBlock`）+ 工具结果注记 + 附件部件文本。**外部 MCP 工具描述与 `mcpHistoryPlaceholder` 显式豁免**（内容由服务器直通，不满足本契约属正常）。

- **骨架（S1/S2）**：首行 = 单句总述（9 个文件工具与任务 / 压缩类工具沿用**边界句**首行，功能句紧随其后；`ExecuteJavaScript` / `ExecutePython` / `AskUser` / `PythonPackages` 为功能句首行）→ 空行 → `- ` 要点；要点 >8 条或跨 ≥2 话题时分节（`**短标题**` 独占一行；`ExecutePython` 五节）。**唯一例外**：`WS_PATH_DUAL_NOTE`（b26 路径口径句）在文件工具 `modelDesc` 里保持「首句后独立一行」的原位 —— 它是每轮 system 里唯一的路径口径来源、被 10 处引用，不为空行形态挪位。
- **语气（S3/S4/S13）**：中文祈使管做什么、陈述管是什么；强约束用加粗，**不引英文强调词**（`NEVER` / `DO NOT` / `MUST` 0 命中）；每条「禁止 / 必须」附理由。
- **反例与替代（S5）**：每个「不要用 / 不能」都要给出替代或指路（如 `Read` 的「二进制 → 用 ExecutePython 以 `"rb"` 读」，且明说**不要重试**）。
- **参数权威（S6）**：参数事实（必填性 / 默认 / 单位 / 上限 / 省略行为）的**唯一权威 = `inputSchema.description`**；`modelDesc` 只留「关键参数」要点。凡 `modelDesc` 删掉或弱化的参数事实，必须能在 schema 里逐条找到（审查时对拍，见 4.7 表）。
- **标点与反引号（S7/S8）**：列表前缀一律 `- `（`· ` 只允许行内出现）；参数名 / 错误码 / 路径字面量用反引号包裹（工具名保持裸名，协议 token 例外）；括号统一半角。
- **数字与标签（S9/S10/S11）**：不写「较大 / 适量 / 若干 / 一些」类模糊量词，数字逐处带值（无权威数字时改为可验证的具体口径，如任务列表用 `PV_TASK_LIST_MAX`）；`next_step:` 只加在**本身已含出路**的句子上（截断提示、EBINARY 文案；任务族不加，避免与 `PV_TASK_COMMON` 重复）；注入块统一 `【…】` 头 + `- ` 列表。
- **双面文案（S12）**：同一字符串既进模型又进用户面（工具卡片 / toast）时用**中性措辞** —— 不出现只对用户说的话，也不出现只给模型的语法（`next_step:`）；用户去处（改哪个开关）改为**事实括注**（非祈使、无第二人称）。
- 落地记录：`shared/progress/b27-impl-done.md`（含 S0.5 三段合并表、逐行映射、五维差分与字符量读数）。

### 4.8 生成中的待发送缓冲区（b28；b30 起**按会话多槽**）

- **状态**：`SQ = {}`（convId → `{ items: [{ id, convId, text, at }] }`，`appD` 的 `var SQ`，**内存态、不持久化**）。b30 之前是单槽 `{ convId, items }` + 模块级 `genConvId`；"同一时刻只有一条在飞 send"的前提作废 ⇒ 槽按会话分开。
- **归属**：每条会话一个队列（`sqSlot(convId, make)`）；浮窗只显示当前会话的槽；`sqInsertAtToolBoundary` / `sqFlushAfterTurn` 只动目标会话的槽；`item.convId` 必填（`sqRestoreBuf` 按它定位）。
- **顺序面（写死，动它们等于重写本机制）**：① `sendMessage` 的 finally 里 **`genDrop(sendConvId)` 是第一句**（记录先消失、缓冲唤醒在其末尾触发）；② `sqFlushAfterTurn(endedConvId, wakeSent)` 在 `pvNotifyCheck(sendConvId)` 之前，且 `wakeSent` 为真时**让位**（一次收尾至多补发一条）；③ 出队点唯一（`shift()`），交接期不可召回/丢弃，`sendMessage` 在写用户消息之前的所有早退分支统一 `sqRestoreBuf`（幂等；`retryAt = now + 4s` 冷却防"补发 → 失败 → 再补发"即时循环）。
- **入队门**：`genOn(conv.id)` 为真才可入队（当前会话自己的生成）；附件 `draftFiles` / `office` 解析中一律拒绝 + toast（v1 不支持附件入队）。**超限转缓冲**：并发已达上限的发送（`genNew` 返回 `null`）转入该会话的槽并提示；任一会话 `genDrop` 后按"槽首项最旧"补发 1 条。
- **UI**：`#send-queue`（`head.part`，贴输入框正上方，`#slash-menu` 的 z-70 之下）；行文本一律 `textContent`；按钮走 `#send-queue-rows` 上的**事件委托**；hint 五态（默认 / 超限「待前序会话空出槽位后自动发送」/ 等放行提问 / 压缩中 / 残留「按 Enter 立即发送」）；`↑`（输入框**完全为空**时）召回**当前会话槽**的最后一条 —— **有意偏离 Kimi 的 busy 门**（空闲也允许召回，否则 `ECOMPACT_ABORT` 残留态无法用 ↑ 恢复）。
- **可见性重算**：`sqRender()` 挂在 `renderChat()`（`appC`）的两个出口 ⇒ 会话切换 / 删除 / 新建 / 整库替换 / 分支切换自动跟随。
- **结构保证（b30 补齐，替代旧「已知缺口」）**：`try` 起点已上移到 `genNew` 之后、`genDrop` 是 `finally` 第一句 ⇒ 记录不会永久残留；前置段（附件 hydrate / 压缩窗口）的早退与异常统一"回队 + 如实提示"。

### 4.9 附件三档与图片上限（0.2.1）

**常量（唯一来源）**：`ATT_IMG_MAX_IMAGES` = 10（`appD`；取图计划与 OCR 融合**共用**；批前 `ATT_OCR_MAX_IMAGES` = 6，已改名并抬值）。自绘表格图另有：每表 ≤ 60 行 × 12 列、一次 ≤ 10 表（= `ATT_IMG_MAX_IMAGES`）、每表一张图。

**token 估算（`draftAttachTokens`；草稿环 / 预览 / 上下文用量浮层同一口径）**：文本走四桶估算；**一张图 ≈ 900**（图片档 = 900 × 张数；混合档 = 正文 + 900 × 张数；`drawn` 自绘图同口径；图片附件 900）。

**自绘表格图的交付数值（`attCanvasFitForModel`，R11）**：单张硬顶 = `READ_IMG_HARD_MAX`(1 MiB)；最长边 ≤ `imgMaxSide()`（默认 4096）；质量梯 `0.86 → 0.6 → 0.4`；边长梯 `1400 / 1000 / 768 / 512`（首个进硬顶者胜）。实测咬合：原始 5,357,103 B ⇒ 交付 265,182 B；`imgMaxSide=1024` ⇒ 1024×406 / 215,694 B。

**可读性旋钮（生产参数）**：自绘 `scale: 2` —— 1× 时单元格字高约 13 px，真机 OCR 置信度 57 且暗号读花；2× ⇒ 82 且逐字可读（交付尺寸 = 2× 读数）。

**PDF 侧取图参数（三处调用点统一）**：`attPdfOpts()` = `{maxScan: pdfMaxPages(), maxImages: ATT_IMG_MAX_IMAGES}`。

### 4.10 多会话并发（b30）

- **唯一权威 = `GEN`**（`appA`「2.5 生成上下文注册表」）：`convId → { convId, seq, ctrl, startedAt, phase, turn, regen, inflight }`。模块级 `generating / abortCtrl / genConvId / lastStats` 已**整体删除**，读点一律走 `genOf / genOn / genAny / genCur / genCount`（机械闸 = 词边界计数脚本；`lint.js` 对裸变量读写**不工作**，只兜函数类）。记录随 `genNew` 建立、随 `genDrop` 销毁；`ctrl` 创建点 = `genNew`（压缩窗口内按 Ctrl+X 也可中止）。
- **不变量（摘要）**：I1 每会话至多一条记录；I2 一切写入按 `sendConvId`（`commit` 的第二参数是唯一入口）；I3 "停止"只影响目标会话（工具等待 / 沙箱 / 续跑静默窗全按 convId；**不含**后台 Python 任务，见下）；I5 渲染调度槽只服务当前会话；I8 `genCount() <= genMax`（超限转缓冲，不静默丢弃）；I9 删会话当场中止其生成、不留幽灵；I11 记录必被同一 `finally` 覆盖（`try` 起点在 `genNew` 之后、`genDrop` 是 finally 第一句）；I12 自动续跑必须显式 `opts.targetConvId`。
- **并发上限**：设置项 `genMax`（1~6，默认 3）；`genDrop` 末尾 `genWakeBuffers()` 按"槽首项最旧"补发 1 条（`retryAt` 冷却 4 s 防"补发→失败→再补发"即时循环）。
- **压缩仍单实例**：并发下第二条会话的自动压缩遇 `ECOMPACT_BUSY` **让位**（本轮不压、照常发送 —— 有意取舍）；`CX_RUN.regen` 已下沉为 `GEN[id].regen`（读点 `genOf(c.id)`；**不得**用 `CX_RUN.convId` —— 那在该处是已复位的旧值/别的会话）。停止静默窗 `PV_STOP_AT[convId]` 是**独立 per-conv 表**（不能放进 GEN 记录：`genDrop` 会删记录，而静默窗必须在收尾之后仍有效）。
- **Python 池（上限 2）与并发的关系**：池是全局资源，跨会话共用；并发条数 > 2 时第三条进 Python 会 `EBUSY`（既有语义）—— 设置项 hint 已写明。
- **流式渲染**：气泡元素**每帧重解**（复用 `msgElementFor` + "仅发起会话 = 当前视图"守卫），不再持有一次性 `msgEl` 快照 —— 历史踩坑 =「切走再切回后气泡冻结」；`cancelRender` 也只由当前会话的收尾调用。
- **`window.__GEN_STAT`（对外面登记）**：只读诊断钩子（无参数 / 无副作用 / 只回 `{max,count,cur,list[]}` 结构与计数，不含正文 / 设置 / API Key；不新增任何通信面）；先例 = 主页面 `window.__mcpRefresh`。
- **D-5（0.2.1 裁决）**：**停止生成不杀后台 Python 任务**；`pyTaskStopKills` 设置项与 `pyTaskStopAll` 已整体作废 / 删除；任务中止的唯一手动收口 = `pvStopTaskFromUI`（`/tasks` 弹窗 / 卡片行「停止」/ `TaskStop` 工具）。

## 5. 未来方向

- 会话虚拟滚动与全文索引（长会话已有 600 条/会话软上限与压缩工具）。
- OCR 结果按区域裁剪 + 置信度标注；PDF「按页选中」而不只是整份或全部页。
- 再加几个工具：取当前时间 / 读写提示词模板 / 在沙箱里生成图片或图表并内联显示。
- 文件系统工具（先只读）：走 File System Access 授权目录 + 相对路径，兼容性探测不过就整块隐藏；句柄进 IndexedDB，掉权限时在对话里出现「需要重新授权」的卡片。
- 工具的资源管理再细一点：并发上限、内存水位提示、按会话统计调用次数。
- 工具结果的可视化渲染（表格 / 图片 / 链接）而不只是 JSON 代码块。
- 把 Tesseract 的训练数据块在解码后从 DOM 里删掉（现在常驻约 9MB）。
- 附件：图片压缩质量可调、按页选 PDF、OCR 语言按图片自动判断。
- **Jupyter 子页的独立性（0.2.1 备选）**：现形态子标签页依赖主页面（`about:blank`、刷新白屏 = J-TAB-1）；解法 = A1 子页内核资产自举 + A2 传输迁 localStorage + `storage` 事件。落地前不要动子页的承载形态（理由见 §3「JupyterLite 宿主与镜像」）。
