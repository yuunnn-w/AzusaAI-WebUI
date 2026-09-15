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
pyodide*.part   │
appA–appE.part ─┘
```

`build.js` 的拼装顺序（改动顺序会导致 ID / 函数引用错乱）：

```
head.part        先替换 /*__KATEX_CSS__*/ 占位符为 src/katex-embedded.css
+ libs.part      marked + DOMPurify + highlight.js
+ <script> katex.min.js
+ pdfjs.part     缺文件 → 控制台警告，PDF 功能自动降级
+ tesseract.part 缺文件 → 同上，OCR 自动降级
+ office.part    缺文件 → 同上，办公附件自动降级
+ pyodide 载荷   按档取 src/pyodide.part（full）或 src/pyodide-{normal,minimal}.part；
                 显式档位缺载荷即 exit 1；默认档缺载荷时整族 Python 工具降级
+ appA.part + appB.part + appC.part + appD.part + appE.part
```

拼装完成后立即用检查链验证：

```bash
node scripts/syntax.js    # 产物里每段内联脚本交给 V8 解析（跳过 text/plain 载荷）
node scripts/lint.js      # 启发式检查「调用了但未声明」的标识符
```

写出策略：每档先写 `<final>.tmp-<pid>`，成功后才 rename 到正式名——进程被杀或中途出错都不留半套产物，也不碰现有正式产物。另有 **LFS 指针守卫**：读取载荷前扫描 `src/*.part`，发现 Git LFS 指针文本（未 `git lfs checkout`）立即中止。

### 1.2 应用分段（同一个 IIFE）

`appA`–`appE` 只是把一万多行 JS 按功能切开的**分段**，拼起来必须是一个完整 IIFE：各段靠函数声明提升与模块级 `var` 共享作用域，拆开单独看会报「未定义」。**只有 `appE.part` 结尾收 IIFE 与 `</script></body></html>`**——新增分段不得提前闭合。

| 分段 | 职责 |
| --- | --- |
| `appA.part` | 环境自检 / 常量 / 设置与状态 / 存储底座 / 主题 / 连接状态 / Markdown 渲染 |
| `appB.part` | 数学公式（抽取 + KaTeX 渲染回填） |
| `appC.part` | 代码块增强 / 消息渲染 / 滚动 / 流式渲染调度 |
| `appD.part` | 会话 / 输入区 / 附件 / API 调用 / 交互 / 弹层 / 设置面板 / 命令面板 / 语音输入 |
| `appE.part` | 工具框架（注册表 / 权限 / 限额 / 沙箱 / 审计）+ Python 运行时与任务层 + 初始化入口 |

⚠️ **新增函数名前先全局搜重名**：同名函数后声明者会静默覆盖先声明者（历史 bug「新建会话空白页」的根因就是 `appE` 里的桩函数覆盖了 `appC` 的真实现）。

### 1.3 三档产物

三档**共用同一份 JS 源码**，差别只在 pyodide 载荷里嵌了哪些 wheel（界面与功能完全一致）：

| 档位 | 预置包 | 产物体积（实测） | 载荷来源 |
| --- | --- | --- | --- |
| `full`（默认） | 151 个 wheel | 209,165,053 B（≈199.4 MiB） | `src/pyodide.part`（入库） |
| `normal` | 99 个 wheel | 137,116,299 B（≈130.7 MiB） | `src/pyodide-normal.part`（本地生成） |
| `minimal` | 33 个 wheel | 55,619,364 B（≈53.0 MiB） | `src/pyodide-minimal.part`（本地生成） |

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
- `mergeSettings` 的迁移动作（只执行一次）：旧强调色名 → 马卡龙色名、旧硬编码采样参数 → 不发送、`codeWrap` 默认改真；历史版本还做过 `delete ctxBudget`（按 token 截断历史整体移除，只留「最多携带条数」`ctxMsgs`）、删除旧的「对外暴露工具服务」配置、`toolLimits` 改新默认、`run_js` → `ExecuteJavaScript` 的权限键搬迁等。
- **Base URL 不做自动迁移**：旧默认曾是内网地址，现不保留其字面量（`DEFAULT_BASE` = 本机回环 `http://127.0.0.1:8080/v1`）；任何已存值（老默认、自定义值）都原样保留，只有「缺 `baseUrl` 字段」的设置才会被合并上默认值。
- `schema` 未升但后来新增的字段（`pdfMode` / `pdfDpi` / `pdfMaxPages` / `ocrLangs` / `visionForce` / `modelVision` / `pyTaskTimeoutMs` / `pyTaskStopKills` 等）都有默认值，旧库不需要额外动作。
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
| pdf.js（pdfjs-dist legacy） | 6.3.289 | PDF 文字抽取 + 页面渲染（已修 CVE-2024-4367） | `src/pdfjs.part` ≈ 1.85 MB |
| Tesseract.js（+ core） | 7.0.0 | 图片 OCR（eng + chi_sim，tessdata_fast） | `src/tesseract.part` ≈ 6.37 MB |
| mammoth | 1.12.3 | docx 文字 + 正文图片 | `src/office.part` |
| SheetJS CE | 0.20.3 | xlsx / xls → CSV | `src/office.part` |
| @jose.espana/docstream | 0.1.3 | pptx / doc / ppt 文字 + 图像 | `src/office.part` |
| fflate | 0.8.3 | ZIP 结构预扫描（ZIP 炸弹防线） | `src/office.part` ≈ 2.93 MB 合计 |
| pyodide（+ CPython 3.14.2 标准库） | 314.0.6 | Python 运行时 | `src/pyodide.part` ≈ 186.7 MB |

三份产物内的 SVG 图标为手写图标精灵（62 个 symbol），不依赖图标字体。内嵌的 wasm / 训练数据放在 `<script type="text/plain">` 里（不会被当成 JS 执行，也不会被语法检查误报）。`syntax.js` 必须跳过这类块，否则会报一堆语法错误、把真正的问题淹掉。

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

### `file://` 与浏览器环境

- **`file://` 下不是所有 blob 手段都能用**：`new Worker(blob:…)` 能构造成功，但 worker 里 `importScripts(blob:)` 与 `fetch(blob:)` 都会被拒（`eval` / `new Function` 反而可用）。两个内嵌库都按这个约束选了各自的加载路径（见 `docs/PDFJS-NOTES.md` / `docs/TESS-NOTES.md`），改库版本前先读它们。
- **Tesseract 的 `cacheMethod:'none'` 必须开**：它跑在 blob worker 里，而 `file://` 下 worker 内 IndexedDB 的 open 请求不触发任何事件，默认的「读缓存」会永久挂住。注意区分：`file://` 的**主文档**里 IndexedDB 是正常的，所以附件库在 `file://` 下也能用。
- **`syntax.js` 要跳过 `<script type="text/plain">`**：内嵌的 wasm / 训练数据放在这种块里，当成 JS 解析会报一堆语法错误，把真正的问题淹掉。
- **`requestAnimationFrame` 在后台标签页会被暂停**：真实浏览器验证前必须先把标签页调到前台，否则流式重绘根本不发生（看起来像渲染坏了）。
- **本机 8090–8123 端口段被 Windows 保留**（`listen EACCES`）：无头验证脚本与静态服务器统一用 9000+。
- **无头脚本的就绪判定要盯住「换过文档了」**：`Page.navigate` 之后旧文档还在，只判「有 `.msg` / `#welcome`」会在旧文档上立刻返回 true；带种子数据时旧文档里同样有 `.msg`，所以必须用 `performance.timeOrigin` 变化来确认换过文档。
- **多套件别并行跑**：同一台机器上并行会因负载互相干扰出假失败，串行跑。
- **`file://` 只能自己开无头 Chrome 验**：WebBridge 打不开 `file://`；用 CDP 的 `Page.navigate` 到 `file:///…` 再 `Runtime.evaluate`。

### 构建与产物

- **自写 ZIP 的两个「大小」字段不能混**：本地头（偏移 18/22）与中央目录（偏移 20/24）各有「压缩后大小」与「原始大小」；用 `deflate-raw` 压缩时两者不相等，两处都写压缩后的大小会让解压工具读出半截文件。
- **改 `src/*.part` 后必跑构建链**：`node scripts/build.js && node scripts/syntax.js && node scripts/lint.js`；产物 html 是构建结果，手改会被下一次构建覆盖。
- **JS 代码里的 `</script` 序列会提前终止脚本块**：内嵌载荷的生成脚本统一把它转义成 `<\/script`（语义等价），并断言 `<!--` / `<script` 出现即报错中止。
- **新增分段不得提前闭合 IIFE**：只有 `appE.part` 结尾收 IIFE 与 `</html>`。

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

- 完整版产物 ≈ 199.4 MiB（三档见 §1.3）；三个内嵌库都是**惰性初始化**（第一次调用 API 或第一次识别才建 worker / 解码 wasm；pyodide 载荷同样懒解码，首屏不碰载荷正文）。
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
| `CX_KEEP_RATIO` / `CX_KEEP_MIN_TOK` / `CX_KEEP_MAX_TOK` | 0.15 / 8192 / 20000 | 保留尾部预算 = `clamp(round(win×0.15), 8k, 20k)` |
| `CX_HEAD_TOK` | 2000 | 最早的用户输入保留预算（`keepHead`，整条原文，不截断） |
| `CX_KEEP_MAX_MSGS` | 8 | 保留尾部最多几条消息（`auto` 向左扩展时的约束） |
| `CX_LOOP_TAIL_MIN` | 2 | 「单用户轮有界 tail 档」要求尾部至少含这么多个**完整工具循环** |
| `CX_MIN_MSGS` | 4 | 少于这么多条消息时拒绝压缩（`ECOMPACT_SMALL`） |
| `CX_SUMMARY_TIMEOUT_MS` | 900000 | 摘要请求超时（**每次尝试各计**；全量输入后在长会话可能数分钟，`Ctrl+Alt+Esc` 可随时取消） |
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

**失败/取消零改动**：摘要成功后才动数据；动数据前过两道闸（非工具路径 = `cxHistoryHash` 相等 **且** `cxHistoryPrefixIntact`；工具路径 = 前缀完整、允许尾部增长）；`cxApplyCompaction` 先按 id 重取会话（`getConv(conv.id)`），其后只用重取到的对象。工具路径「只算不落」（`CX_RUN.pending`），应用点是**本轮所有工具调用结束后**的唯一一处（`await cxApplyPending(tf)`，漏 `await` 会让被删消息继续发给模型）。**取消通道 = 运行级 `AbortController`**：`CX_RUN.ctrl` 在整段压缩运行期间持有（请求与退避 `cxSummarySleep` 共用同一 signal；`cxCallSummary` 把它桥进本次请求的局部 ctrl，不再自设/自清 `CX_RUN.ctrl`）；`Ctrl+Alt+Esc`（macOS `Cmd+Esc`）在生成中走 `stopGenerating → cxAbortActive`（既有链路），手动压缩（`generating=false`）走 `cxRunActive → cxAbortActive`（**不**调 `stopGenerating`，避免误杀沙箱 / 工具等待 / 自动续跑）；ladder 另有「成功前 abort 复检」关闭「响应已到、应用未开始」的窄窗。

**摘要与用量环**：摘要请求的 token 消耗**不进**用量环与累计统计（它是管理开销）；压缩成功后目标会话的实测锚点作废（`usage.exact = false`、`lastMsgId = ""`，累计量不动），否则用量环不下降、去重与触发判定失真。

**版本号唯一口径**：`APP_VERSION`（`appA.part`）是用户可见版本号的唯一权威 —— 环境面板、环境信息、导出备份的 `version`、MCP `clientInfo`、关于弹窗、`<meta name="version">`（启动时同步）全部读它；不要在别处再写版本字面量。

### 4.6 文件工具的多媒体读取（Read 图片 / 办公文档 / PDF）

**路径双写（唯一权威句 `WS_PATH_DUAL_NOTE`，`appE.part`）**：工作区 6 个文件工具一律用 `/…`（`wsNormPath` 只认 `/` 开头、禁 `..` 与反斜杠），而 Python 解释器里同一份数据挂载在 `/workspace`（`PY_MOUNT` / `pyContainerPath`）——**两者指向同一份文件**。这句话同时进 `【工作区】` 上下文块（每轮请求的 system，≈100–130 token 成本）、6 个文件工具与两个执行工具的 `modelDesc`、`PV_TASK_COMMON`（4 个任务类工具共用尾段）。两条定向纠错都只改错误文案：工作区工具路径以 `/workspace` 开头且报 ENOENT 时，`wsToolRun` 追加口径提醒，且**只在"去掉前缀后的路径确实存在"**（同步查 `WS_META_CACHE[wsId].tree`，缓存缺失就不确指）时给出「你要的可能是 /x」；Python 侧的 `__pyShimOpen` 在 `FileNotFoundError` 分支里对 `not p.startswith(_KIMI_MOUNT) and os.path.exists(_KIMI_MOUNT + p)` 的情形补一句「这个文件在工作区里是 "/workspace" + 原路径」——**只覆盖 `builtins.open`**：`os.open` 是另一个引用，`pathlib.Path.open/read_text` 走的是 `io.open`（**不在覆盖内**），`pandas.read_csv` / `PIL.Image.open` 等最终走 `builtins.open` 的路径在内；**只加文案、不改异常类型**（仍是 `FileNotFoundError`）。两点实现约束（回归教训）：① 挂载点由 JS 侧 `JSON.stringify(__PY_MOUNT)` 拼成 **Python 字面量** `_KIMI_MOUNT` 注入 —— 裸写 `__PY_MOUNT` 是 worker 的 JS 变量，Python 命名空间里不存在（会 `NameError`，把 `FileNotFoundError` 换成更难读的报错）；② wrapper 的安装与「本次有没有超预载上限文件」**解耦**（工作区挂载即安装，`_KIMI_REMOTE` 每次 run 重写），否则没有超限文件时该提示永不生效；安装失败往 stderr 打一行诊断而不是静默吞掉。

**Read 的类型路由**（`wsReadTool`，顺序即优先级）：文本（含 svg；输出逐字节不变）→ 办公 / PDF（后缀或 magic）→ 图片（magic 命中且 mime 是 `image/*`，或后缀 ∈ 可解码集合 png/jpg/jpeg/gif/webp/bmp/avif/ico/tif/tiff）→ 宏格式单独拒 → 其余二进制给新 EBINARY 文案（`zipfile` 解包再 Read 是新增的真实出路）。

**图片预算（单一权威 `READ_*` / `TOOL_MEDIA_CHAT_*`，`appE.part`）**：最长边 ≤ `READ_IMG_MAX_EDGE`(2000) 且与 `imgMaxSide()` 取小；尽力压到 `READ_IMG_BYTE_BUDGET`(256 KiB)，梯子 = PNG（保 alpha）→ JPEG 0.8/0.6/0.4 → 边长回退 2000/1000/768/512/384/256；**单张硬顶 = 单次总量 = `READ_IMG_HARD_MAX` = 1 MiB**（压不到就报错、**不发原图**）；单次交付 `READ_MEDIA_MAX_IMAGES`(4) 张、累计 ≤ `READ_MEDIA_TOTAL_BYTES`(1 MiB)，超出按读取顺序保留并在状态行写「另有 N 张超出单次上限未附带」；直通（字节 ≤ 256 KiB、边 ≤ 上限、mime ∈ `SAFE_IMG_MIME`）**仍会解码验证一次**再交付原字节——这一层挡住"截断/损坏但 IHDR 还写着小尺寸"的假直通。解码两道门：像素 `READ_IMG_DECODE_MAX_PX`(40M，先用 `wsImgSniffDims` 嗅 PNG/JPEG/GIF/BMP/WebP 尺寸头，**嗅不到尺寸的 avif/ico 只能在解码后复核**，这是已知残差) + 字节 `READ_IMG_DECODE_MAX`(32 MiB)，取与。交付 mime 收敛到 `{image/png,image/jpeg}`（重编码）∪ `SAFE_IMG_MIME`（直通）——**源 mime 不在 SAFE 集时不走「压不动回原字节」**（否则交付 mime 会落在收敛集合之外，例如 1×1 BMP 交付 `image/bmp`），改为交付最佳重编码（仍 ≤ 单张硬顶）。

**非视觉 → OCR**：`wsOcrTextOf` 是唯一出口；输入是**按上表缩放后的画布 dataURL**（与附件 `maybeOcrImage` 同形）；超时 = **整次 Read 的总预算** `min(READ_OCR_TIMEOUT_MS(120 s), toolLimit("timeoutMs"))`，多页共享、页间查 `ctx.signal.aborted`、文本累加到工具结果预算即停；引擎不可用 / 超时 / 无文字各有独立可读文案（**不写"请重试"**）。

**媒体怎么进模型（`m.media` 内联 + 协议双形态）**：消息上唯一新字段 `m.media[]`（`kind/mime/data/w/h/bytes/path/label/from/srcW/srcH/scaled`；`resultText` 只写文本 + 「已附带 N 张」）。payload 项内部字段 `media` 由 `payloadMessages` 带出：**Anthropic** 走 `tool_result.content` 内容块数组（文本 + image）；**OpenAI 兼容**走"tool 文本 + **按工具轮分组**的一条合成 user 消息"（连续 `role==="tool"` 段 = 同一工具轮，组内 media 合并、插在段末最后一条 tool 之后，跨段不合并 ⇒ 不产生 `tool→user→tool` 交错）；合成消息**只存在于请求体**，不落盘、不进界面。工具轮内还有一个 `msgList.push({role:"tool"})`（续答轮），**必须带上同一个 media**，漏了就是"本轮模型看不到刚读的图"。

**严格端点 400 的降级与复位**：`isToolMediaStructureError`（显式状态码 `{400,422}` + 结构类正则）命中且 `msgList` 里仍有带 media 的 tool 项时，工具轮内 `mediaOff = true` 后重跑本轮（**整次 send 只一次**；`buildBody` 收 `noToolMedia` ⇒ 工具文本照发、仅图片不随本次发送），并 toast 说明。复位路径三条：① 本次自动降级（不落盘）；② 设置 → 模型 把该模型图像能力设为「不支持」（`visionForce:"no"`）或换模型；③ 删除 / 压缩掉那条带图片的工具消息。降级后仍失败时，失败文案追加一条指向②的恢复建议。

**两条预算纪律**：文本预算（`toolLimit("maxOut")`）只管 `m.content`，媒体字节走 `m.media` ⇒ 附图不挤状态行、正文过长也不削图；**聊天轮软护栏** `trimToolMediaForRequest` 对非摘要请求保留最近 `TOOL_MEDIA_CHAT_KEEP_ITEMS`(8) 条带媒体工具项、累计 `TOOL_MEDIA_CHAT_MAX_BYTES`(8 MiB，`data` 字符量)，更早的**非破坏性**替换为 `[图像已省略:超出请求体积上限,需要时可重新读取]`（存储一字不动）；摘要请求不走这条护栏（b25 的 `cxSummaryMedia` 阶梯 + 32 MiB 闸自有口径）。工具媒体计入 token 估算的唯一入口是 `estimateToolMediaTokens`（调用点清单：`payloadMessages` 的 token 循环 + `cxPayloadMsgTokens`）。

**存储代价（如实）**：媒体内联在消息里（不新增 blob 键 ⇒ 不触碰释放链），单条消息最大 ≈1.4 MB base64；三条放大面 = 每次 `saveState(true)` 整条会话写、导出备份整份 `JSON.stringify`、历史图片逐轮重发（软护栏封顶）。**「优化存储」可回收**（`stripAttachmentData` 在 `m.content` 早退之前删 `m.media`，`keepIds` 保护口径不变）。**计量口径**：`storageBytes()` 量的是 localStorage 设置串，**不是会话存量** —— 报告占用请用 `navigator.storage.estimate()` / IDB 实测，不要拿它宣称"媒体存储已计量"。

**Read 复用办公 / PDF 引擎的边界**：复用引擎（`OfficeKit.parse` / `PDFKit.extractText|renderPages` / `OCRKit.recognize`）与数值（`ATTACH_TEXT_MAX`、`officeSizeCap()`、`pdfDpi()`、`pdfMaxPages()`、`ocrLangs()`、`SAFE_IMG_MIME`、`IMAGE_EXT/OFFICE_EXT/MACRO_EXT`），**不复用** `resolveOffice/resolvePdf/resolveImage` 的 File+vision+toast 包装（Read 的入参是 Blob + 目标会话，包装会引入"生成期间切走会话"的判定错会话问题）；PDF 走文本优先、不吃 `pdfMode()` 设置。文档状态的"文件:…"信息一律并入**末尾状态行**（不占行号，`line_offset` 语义与文本读一致），`clipped` 在状态行标注。

### 4.7 提示词风格契约（模型可见文本的唯一风格口径）

> 适用面 = 内置 `AGENT_TOOLS` 的 `modelDesc` + 四个系统注入块（`wsContextBlock` / `pvTaskBlockText` / `mcpInstructionsBlock` / `cxContextBlock`）+ 工具结果注记 + 附件部件文本。**外部 MCP 工具描述与 `mcpHistoryPlaceholder` 显式豁免**（内容由服务器直通，不满足本契约属正常）。

- **骨架（S1/S2）**：首行 = 单句总述（6 个文件工具与任务 / 压缩类工具沿用**边界句**首行，功能句紧随其后；`ExecuteJavaScript` / `ExecutePython` / `AskUser` / `PythonPackages` 为功能句首行）→ 空行 → `- ` 要点；要点 >8 条或跨 ≥2 话题时分节（`**短标题**` 独占一行；`ExecutePython` 五节）。**唯一例外**：`WS_PATH_DUAL_NOTE`（b26 路径口径句）在文件工具 `modelDesc` 里保持「首句后独立一行」的原位 —— 它是每轮 system 里唯一的路径口径来源、被 9 处引用，不为空行形态挪位。
- **语气（S3/S4/S13）**：中文祈使管做什么、陈述管是什么；强约束用加粗，**不引英文强调词**（`NEVER` / `DO NOT` / `MUST` 0 命中）；每条「禁止 / 必须」附理由。
- **反例与替代（S5）**：每个「不要用 / 不能」都要给出替代或指路（如 `Read` 的「二进制 → 用 ExecutePython 以 `"rb"` 读」，且明说**不要重试**）。
- **参数权威（S6）**：参数事实（必填性 / 默认 / 单位 / 上限 / 省略行为）的**唯一权威 = `inputSchema.description`**；`modelDesc` 只留「关键参数」要点。凡 `modelDesc` 删掉或弱化的参数事实，必须能在 schema 里逐条找到（审查时对拍，见 4.7 表）。
- **标点与反引号（S7/S8）**：列表前缀一律 `- `（`· ` 只允许行内出现）；参数名 / 错误码 / 路径字面量用反引号包裹（工具名保持裸名，协议 token 例外）；括号统一半角。
- **数字与标签（S9/S10/S11）**：不写「较大 / 适量 / 若干 / 一些」类模糊量词，数字逐处带值（无权威数字时改为可验证的具体口径，如任务列表用 `PV_TASK_LIST_MAX`）；`next_step:` 只加在**本身已含出路**的句子上（截断提示、EBINARY 文案；任务族不加，避免与 `PV_TASK_COMMON` 重复）；注入块统一 `【…】` 头 + `- ` 列表。
- **双面文案（S12）**：同一字符串既进模型又进用户面（工具卡片 / toast）时用**中性措辞** —— 不出现只对用户说的话，也不出现只给模型的语法（`next_step:`）；用户去处（改哪个开关）改为**事实括注**（非祈使、无第二人称）。
- 落地记录：`shared/progress/b27-impl-done.md`（含 S0.5 三段合并表、逐行映射、五维差分与字符量读数）。

### 4.8 生成中的待发送缓冲区（b28）

- **状态**：`SQ = { convId, items }`（`appD`，**内存态、不持久化**；`items[i] = { id, convId, text, at }`）+ `genConvId`（在飞 send 的归属会话）。
- **归属**：`item.convId` 恒 = 入队时的会话 = `SQ.convId`；flush 目标取 `opts.bufItem.convId`（生成期间允许切走，消息必须落回原会话）；浮窗只在 `conv.id === SQ.convId` 时可见。
- **顺序面（三处写死，动它们等于重写本机制）**：① `sendMessage` 的 finally 里 **`genConvId = ""` 在 `sqFlushAfterTurn(sendConvId)` 之前**；② flush **同步**调 `sendMessage`（不 await、不 catch），插在 `pvNotifyCheck()` 之前 ⇒ 缓冲消息优先于 E12 自动续跑；③ 出队点唯一（`SQ.items.shift()`），交接期不可召回/丢弃，`sendMessage` 在 push 用户消息之前的所有早退分支统一 `sqRestoreBuf`（幂等）。
- **入队门**：仅 `generating === true` 且 `conv.id === genConvId` 时入队；附件 `draftFiles` / `office` 解析中一律拒绝 + toast（v1 不支持附件入队）。
- **UI**：`#send-queue`（`head.part`，贴输入框正上方，`#slash-menu` 的 z-70 之下）；行文本一律 `textContent`；按钮走 `#send-queue-rows` 上的**事件委托**（先例 = `ctx-pop`）；hint 四态（默认 / 等待 / 压缩 / 残留）；`↑`（输入框**完全为空**时）召回最后一条 —— **有意偏离 Kimi 的 busy 门**（空闲也允许召回，否则 `ECOMPACT_ABORT` 残留态无法用 ↑ 恢复）。
- **可见性重算**：`sqRender()` 挂在 `renderChat()`（`appC`）的两个出口 ⇒ 会话切换 / 删除 / 新建 / 整库替换 / 分支切换自动跟随。
- **已知缺口（登记）**：`generating = true` 之后、`try` 之前的两处 `await`（`hydrateAttachments` / `cxAutoBeforeSend`）若 reject，条目不回队且 `generating` 卡 true（现码全路径 resolve；加固属 L3 状态机面，另批）。

## 5. 未来方向

- 会话虚拟滚动与全文索引（长会话已有 600 条/会话软上限与压缩工具）。
- OCR 结果按区域裁剪 + 置信度标注；PDF「按页选中」而不只是整份或全部页。
- 再加几个工具：取当前时间 / 读写提示词模板 / 在沙箱里生成图片或图表并内联显示。
- 文件系统工具（先只读）：走 File System Access 授权目录 + 相对路径，兼容性探测不过就整块隐藏；句柄进 IndexedDB，掉权限时在对话里出现「需要重新授权」的卡片。
- 工具的资源管理再细一点：并发上限、内存水位提示、按会话统计调用次数。
- 工具结果的可视化渲染（表格 / 图片 / 链接）而不只是 JSON 代码块。
- 把 Tesseract 的训练数据块在解码后从 DOM 里删掉（现在常驻约 9MB）。
- 附件：图片压缩质量可调、按页选 PDF、OCR 语言按图片自动判断。
