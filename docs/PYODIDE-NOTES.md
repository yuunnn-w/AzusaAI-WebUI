# pyodide 内嵌产物与运行时笔记

> 状态：现行 —— pyodide 载荷格式、运行时门控与实测数字的唯一记录；改动 `src/pyodide*.part` 或 `scripts/make-pyodide-part.js` 前必读
> 更新：2026-09-14 · 载荷段 `src/pyodide.part` = 195,811,297 字节（186.7 MB，整个载荷只有 **1 个** `<script type="text/plain">` 块，段在块内，见 §1）
> 轻档载荷：`src/pyodide-normal.part`（123,762,373 B）/ `src/pyodide-minimal.part`（42,265,436 B），**不进版本库**（`.gitignore` 的 `src/pyodide-*.part`）
> 上游版本：pyodide 314.0.6（CPython 3.14.2）· 核心集 + **151 / 99 / 33** 个预置包 wheel（full / normal / minimal）· 生成脚本 `scripts/make-pyodide-part.js`（幂等）
> 档位唯一权威：`scripts/pyodide-profiles.json`（档位名单、分组与对账 expect 都在这里）
> 取件：`vendor/pyodide-src/` 需要官方 `pyodide-lock.json`、核心文件（`pyodide.asm.mjs` / `pyodide.js` / `pyodide.asm.wasm` / `python_stdlib.zip`）、`wheels/*.whl` 与选包清单 `packages.json`（可附 `MANIFEST.json` 逐条核对 sha256）；缺件即 `exit 1`

> 本文件只记**已实现的事实**：路径一律写 `scripts/…` / `src/…` / `vendor/…`，行号只写文件名 + 章节名。
> 与 `docs/DEVELOPMENT.md` 的关系：那份是架构与踩坑总览，本文件是 pyodide 的实测口径；两者冲突时以**真实代码**为准并回来更正本文件。

---

## 1. 载荷分段格式(帧头 `len` 是权威)

`src/pyodide.part` 是**纯数据**,不含任何 JS 逻辑。块内按行帧切分:

```
<script type="text/plain" id="pyodide-assets">
;;;PYODIDE-PART 1 <段数>
;;;PYODIDE-META <单行 JSON:版本/段清单/字节账>
;;;PYODIDE-SECTION <id> <text|b64gz> <字符数>
<该段正文,恰好 N 个字符>
... 重复 ...
;;;PYODIDE-END
</script>
```

- **读段只认帧头的 `len`**,不靠寻找下一个帧标记。段正文里原则上不会出现 `;;;PYODIDE`
  (构建期断言,当前 0 命中),但 `pyodide.asm.mjs` / `pyodide.js` 这类上游源码将来可能带上类似序列 ——
  按长度切才不会踩这个雷。
- 段 id 与文件名一一对应,加载器按 `indexURL + file_name` 直接取段,**不做重命名映射**。
- `text` 段是改写后的文本(转义:`</script` → `<\/script`、`<!--` → `<\!--`);
  `b64gz` 段是 `base64(gzip(原始字节))`。文本段的还原**只在确实含有转义序列时**才做 ——
  无条件还原会把正则字面量 `/<\/script/` 里的 `\/` 也吃掉。
- 帧解析与解码都在**主线程**做(worker 里没有那个巨大的文本节点);解码结果按"一次消费一批"交给
  `postMessage(transfer)`。
- **两个解释器同时冷启动时,同一批 ArrayBuffer 只能给一个消费者**:slot-1 的 transfer 会把缓冲 detach,
  slot-2 再 transfer 就抛 `An ArrayBuffer is detached and could not be cloned.`。修法是**消费互斥** ——
  后来者排队,前一个 `release` 后**重新解码**拿到全新缓冲(等待者串行,内存曲线不变)。

- **轻档多一个文本段**:normal / minimal 的载荷在剪裁锁段之后多一个 `pyodide-profile.json`(text 段;normal 5,593 B / minimal 12,863 B),内容 = 档位 id、包数、被裁顶层包(带分组)/ 被裁闭包项(带 `via`)/ 永久不可用名单。运行期 `pyProfileInfo()`(**同步**、带缓存)据此渲染状态行与 `PythonPackages` 的 ③ 段(中文分组文案常驻 `appE`,载荷段只有 ASCII 组名)。**完整版不带该段**(保住"重建 == 入库产物"的逐字节承诺),运行期**缺段固定解释为完整版** —— 且构建期断言保证轻档必有该段,不会出现"轻档假装完整版"。

## 2. 为什么必须是 classic blob worker + 影子化 `importScripts`

- `file://` 下实测（Chrome 152 无头，能力探针为一次性脚本，已删）：
  **classic blob worker ✅ / module blob worker ❌**(构造成功、运行失败)。pyodide 官方 dist 的
  `pyodide.asm.mjs` 是 ESM,所以走 **classic + 文本改写**(1 处 `export default` → 全局赋值、
  3 处 `import.meta.url` → 字符串字面量),与 pdf.js 完全同一套路。
- pyodide 用「能不能 `importScripts`」来判定 classic worker,**真 classic worker 会命中它的硬检查并
  直接 throw**(`Classic web workers are not supported…`)。所以在 worker 源码最前面**影子化**
  `globalThis.importScripts` 为一个抛错函数:命中后 `isClassicWorker()` 返回 false,
  而 `typeof` 探针仍然是 `"function"`(与真 classic worker 同值)。
- worker 内**自建** blob URL 的 `importScripts` 可用;主线程创建的 blob URL 在 worker 里**不可用**。
- worker 内 IndexedDB 在 `file://` 下不可用(请求既不 success 也不 error)—— 这也是
  `cacheMethod:'none'` 那类绕行在 Tesseract 侧必需的原因,pyodide 侧本来就不走 idb 缓存。

## 3. 内联 tiny-inflate 的 `>= 8` 位对齐修正

没有 `DecompressionStream` 的内核走内联的 `tiny-inflate`(MIT)。上游那份 `inflateRaw` 的位对齐条件
写作 `> 8`,而正确条件是 **`>= 8`**:在该产物上用上游写法 **38/153 段解压失败**,
改成 `>= 8` 后 **153/153 段与 zlib 逐字节一致**(检查方式:逐段 `zlib.inflateRawSync` 比对 sha256)。
修正点与理由在 `src/appE.part` 的 `pyTinyInflateRaw` 头部有注释;`THIRD_PARTY_NOTICES.md` 里如实登记了
"内联的 MIT 代码有 1 处本地修正"。

## 4. 必须显式传的设置(否则静默失败或半途报错)

| 设置 | 为什么必须显式 |
|---|---|
| `packageBaseUrl` | 只给 `lockFileContents` 而不给 `packageBaseUrl` 时,`loadPackage` 会报 `no packageBaseUrl`。**必须显式传**(本项目传的是自建的假目录 URL,实际字节由 worker 内的 fetch 钩子截胡) |
| `indexURL` | 决定 pyodide 自己的资源定位;同样显式传假目录,避免任何真实网络请求 |
| 输出通道用 `write` | `setStdout` / `setStderr` 接收的是**写函数本身**(pyodide 内部 `setOps(opts as Writer)` 把整个 options 对象当 writer 用)。写成 `{ write: fn }` 会在第一次输出时抛 `OSError [Errno 29]`(跨 run 还会泄漏滞留缓冲) |
| 退出/输出上限 | 输出与结果的上限常量**主线程是唯一权威**（注入进 worker），worker 侧不另立数字 |

## 5. 门控:`PY_TOOL_NAMES` + `pyReady()`

- `PY_TOOL_NAMES`(6 名:`ExecutePython` / `TaskList` / `TaskOutput` / `TaskStop` / `WaitFor` / `PythonPackages`)
  是**唯一的运行期门**:`activeTools()` 按它判定,未就绪时整套 Python 工具一起消失(工具面板与
  请求体的 `tools[]` 同时生效),并在 设置 → MCP 工具 里写明原因。
- `pyReady()` = `CAP.worker` ∧ `CAP.objectUrl` ∧ 有解压能力 ∧ 页面里有载荷 ∧ `PY_STATE.phase !== "error"`。
  **硬失败(`pyHardFail`)是"锁死"语义**:一旦置上,后续不会再反复重解码。
- **能力 / 构造级失败 = 全局硬失败**:`!CAP.objectUrl`、`URL.createObjectURL` 抛错、`new Worker` 抛错、
  worker 源码拼装失败 —— 这几种一律走 `pyHardFail`:**整池摘除**(每个 slot 的在途请求都以
  `{ok:false, killed:true}` 收尾,另一个 slot 正在跑的任务**会被连带终止**)+ 状态行写明原因
  + 整套 Python 工具隐藏(工具面板与请求体的 `tools[]` 同时消失)。
  恢复方式 = **刷新页面**,或在 设置 → 环境 的运行时状态行旁点「重试」(它只复位状态机,
  下一次执行重新走一遍 boot;能力 / 构建本身缺失时不给这个按钮 —— 那时重试不会有变化)。
  单 slot 级失败只有这几种:启动超时 / worker 崩溃(`onerror`) / 任务被停止 / 任务超时 / 空闲回收。
- 工具面板的分组:文件(6)/ 代码执行(2)/ 任务(5),由 `needsWorkspace` + `PY_TASK_NAMES` 推导
  (`PY_TASK_NAMES` **只用于分组渲染**,不参与门控);外部 MCP 段未接入,整组不渲染。

## 6. 任务层与「不跨刷新」

- 任务记录 `PV_TASKS` **只活在内存**：刷新或关闭页面后任务与记录**全部消失**，已回写工作区的产物仍在。这是拍板结论（不建 `pytasks` 存储区），不是待办。
- 池上限 = **2 个未完成任务**（前台 + 后台合计），第 3 个执行请求直接 `EBUSY` 并列出当前两个 `task_id`，**不排队**。
- 完成回流：生成中 → 下一轮请求的 sys 注入带过去；空闲 → 自动发起一次 Agent Loop。**`notified` 的语义是「模型确实收到过」**：拼 sys 时只登记「在途」，本轮请求**解析成功**（流正常收尾 / 非流式解析通过；**不是** HTTP 2xx —— 服务端可以先回 200 再在 SSE 流里报错）之后才落 `notified`，失败 / 中断 / 流内报错回滚 → 下次请求重新带上。同一会话**连续自动续跑上限 3 次**，触顶改为 toast 提示「需手动继续」，用户下一次真实发送消息时计数复位（防无人看管时烧额度）。
- sys 注入段里**只出现固定枚举文案与结构化字段**：`description` / `reason` / `error` / `traceback` 都是模型或脚本可控的文本，**一律不进**（安全边界）。

## 7. 实测数字(跑之前先看这些,心里有数)

在本机(Windows + Chromium 152/154,`file://` 与本地静态服务)实测:

| 项 | 实测 |
|---|---|
| `file://` 下 `data-boot=done` | **≈2.0 s**(载荷**懒解码**:首屏不碰 186 MB 文本;全选 2013 ms / 仅应用 2004 ms 两组对照证明载荷对首屏近乎零成本)。UI 验收在真实浏览器 + 较重数据下另测到 **4.0 s**(热缓存) |
| 首次执行（装配 CPython + **全量装载本档全部预置包**） | 极简版 ≈ **3.2–3.6 s** 墙钟（其中装载段 **1.1–1.4 s** / 33 个包；本机两次独立复跑 3554 / 3181 ms）；完整版见下「包装载与释放」实测表。装载时间**不计入脚本超时**，同一解释器生命期内只装一次 |
| 主线程 JS 堆峰值 | **≈416 MB**(415.7 MB) |
| 空闲回收 | **10 分钟**无活动回收解释器（连同已装载的包一起释放）;**有未完成任务时保活** |
| 分发产物 `AzusaAI-WebUI-full.html` 总体积(完整版) | **209,165,053 B**(≈199.4 MiB;pyodide 载荷 195,811,297 B 是大头;md5 `c3c06a7f56c22cf2d46cff3ad5fe1e35`)。口径 = `node scripts/build.js` 打印值;**载荷段 `src/pyodide.part` 生成后未变过** |

**分档实测**(同机、Node v24.16.0;`node scripts/make-pyodide-part.js --profile=…` 的打印值):

| 档位 | wheel 段 | 段数(文本段) | 轮组内嵌 | 剪裁锁 | 载荷 part | 分发产物 |
|---|---:|---:|---:|---:|---:|---:|
| `full`(默认,载荷入库) | 151 | 156(3) | 186,333,848 B | 45,212 B | `src/pyodide.part` 195,811,297 B(md5 `19b48756a17808cd2474ee78bf6f922b`) | `AzusaAI-WebUI-full.html` 209,165,053 B(md5 `c3c06a7f56c22cf2d46cff3ad5fe1e35`) |
| `normal` | 99 | 105(4) | 114,302,412 B | 29,764 B | `src/pyodide-normal.part` 123,762,373 B(md5 `5639fb31bfa39c2538aff656ce90dbcb`) | `AzusaAI-WebUI-normal.html` 137,116,299 B(md5 `b31c2770f9eacf6e3bdb3383074742b2`) |
| `minimal` | 33 | 39(4) | 32,827,748 B | 10,134 B | `src/pyodide-minimal.part` 42,265,436 B(md5 `41abf8f3ceb9eb65c02927196ea38c4c`) | `AzusaAI-WebUI-minimal.html` 55,619,364 B(md5 `eaa6f62d4ff7675d84543b950f1527f3`) |

- 轻档比完整版多 1 个文本段（`pyodide-profile.json`，见 §1）；**完整版的重建与入库产物逐字节相同**（md5 同上，两次独立复跑）；三档的 `expect` 已写回 `scripts/pyodide-profiles.json`，此后每次生成按 **±0 强断言**核对（换 Node / zlib 版本会触发，属有意的摩擦）。
- `--all` 一次生成三档载荷：实测 **13 s**（同机，热缓存），三档 md5 与逐档生成一致。
- **当前三档实测**（2026-09-14；「Python/JS 执行体验修复」批后重建，`node scripts/build.js --profile=all` 两次独立复跑逐字节一致）：`AzusaAI-WebUI-full.html` 209,165,053 B / md5 `c3c06a7f56c22cf2d46cff3ad5fe1e35`；`normal` 137,116,299 B / `b31c2770f9eacf6e3bdb3383074742b2`；`minimal` 55,619,364 B / `eaa6f62d4ff7675d84543b950f1527f3`。（该批之前的 0.1.0 稳版读数 209,130,781 / `fe717287118cb5cac3eb59847fa9333e`、137,082,027 / `78935b1848754982d4a19b3f862c886e`、55,585,092 / `8fa9ce593d47f20fed37c0c9766db90e` 已作废——差异只来自源码改动，载荷段逐字节未变。）
- 「分发产物」三格落位 = 代码目录根（三档 `AzusaAI-WebUI-{full,normal,minimal}.html`、被 `.gitignore` 排除；`build.js --profile=all` 一次出三档）：字节与 md5 见上表与上方「当前三档实测」。

> 首屏 / 内存阈值属**待确认**项；上面的数字都是实测量，不是承诺。

## 8. 包装载与释放（现行：一使用 Python 即全量装载）

**模型**：每个解释器槽位在自己生命期内、**第一次执行任何用户代码之前**，阻塞装载本档**全部**预置包；装完才跑用户代码。同一解释器生命期内只装一次，之后每次执行只做一次廉价的「比对」（不重复装载）。

- **名单来源**：worker init 收到的 `a.lockJson`（= 传给 `loadPyodide` 的那份**档位剪裁锁**）的 `packages` 键集 ⇒ 三档 = 151 / 99 / 33。可用 `PY_ALL_PKGS` 的解析结果反查（`__pyLockIndex()`）。
- **判「已装」**：`__pyPy.loadedPackages` 的**键**（活引用：vendor 安装成功时就地写 `ye[e.name]`），比较时**小写 + 折叠 `- _ .`**（PEP 503 口径）。为什么必须折叠：pyodide 写键用的是**被请求时的名字** —— 例如 `jsonschema` 的依赖写成 `jsonschema_specifications`（下划线），而锁条目名是 `jsonschema-specifications`（连字符），只按小写比会把**已装好的包误判成失败**并每次执行都重试一遍。
- **逐包顺序装载**（`__pyLoadAll()`）：不是一次 `loadPackage(数组)`—— ① 单包失败在 vendor 内部被吞掉（`downloadAndInstall` 把错误塞进内部映射、Promise 照常 resolve），**失败归属只能靠逐包差分**；② 进度粒度 = 单包（`stage` 消息 `i/N`）；③ 循环遇失败**不中断**，失败项下次执行自动再试（自愈），成功项永不复装。
- **失败口径**：失败清单**只报包名、不报原因**（原因只存在于被拒收的 vendor 日志里），一行注记进结果；判定为失败的条件是「调用返回后该包仍不在 `loadedPackages` 里」。`catch` 只兜基础设施错误（如包名不在锁里）。
- **不污染脚本输出**：装载期间 `__pyOutHold = true` 拒收 stdout/stderr（pyodide 的 "Loading pandas, …" 自诊断不进入脚本输出语义面；用户代码不在该窗口内）。
- **装载时间不计入脚本超时**：前台两段计时 —— 装载窗口 `PY_LOAD_WINDOW_MS`（默认 **10 分钟**，从 run 消息发出起算，**只起算一次**：151 条 `stage/load` 不续期）→ 收到 `stage/exec` 才切成脚本窗口（`timeout_ms`）。后台任务的计时钟在收到首条 `stage` 时暂停、`stage/exec` 后按 `pyTaskTimeoutMs` 满额重起。装载窗口到期 → "包装载超时"（与"执行超时"文案可区分）。
- **界面可见性**：状态行与任务卡片显示「装载 i/N」（终态更新绕过节流立即重绘，装载结束即消失）；模型侧只有**一行摘要**（`load.n` = 成功数，`attempted` = 尝试数）。
- **降级路径**：`a.lockJson` 解析失败（`__PY_ALL_PKGS` 为空）时退回旧行为（显式 `packages` + `loadPackagesFromImports` 扫描），并在结果注记里写明 —— 正常路径**不再调用** `loadPackagesFromImports`。
- **释放**：解释器空闲 **10 分钟**整机回收（`pyMaybeReclaim` → `pySlotKill` → `terminate()`），已装载的包随之释放；下次调用自动重启并重新全量装载。**没有单包卸载 API**（`unloadPackage` 在运行期 0 命中）⇒ 不新增任何释放路径。
- **双槽口径**：池上限 2，每个槽是独立解释器、各自全量装载 ⇒ 两槽同时活跃时装载量**双份**（内存如实翻倍，用户已明示接受）。
- **`register_module_not_found_hook` 不再自动装包**：本档 vendor 只给 `ModuleNotFoundError` 追加一句 `await micropip.install(…)` 的建议，而本项目**没有 micropip** ⇒ 界面侧在 `__pyHint()` 里加了更正行（"本项目没有 pip / micropip……换完整版产物"）。

**实测（本机 Windows + 无头 Chrome，`file://`，每档全新 profile 冷启动）**：

| 档位 | 首次执行墙钟（含装配） | 其中装载段 | 备注 |
|---|---:|---:|---|
| 极简版（33 包） | **≈3.2–3.6 s**（两次：3554 / 3181 ms） | **1.1–1.4 s**（两次：1.1 / 1.4 s） | 第二次执行 ≈0.9 s（不再装载） |
| 完整版（151 包） | 见下方「完整版实测」 | 见下方 | 第二次执行 ≈1.0 s |
| 正常版（99 包） | 未单独跑 | — | 介于两者之间（同机制，包数与时间近似线性） |

> 上面是**冷启动**读数（含 pyodide 引擎装配）；同一解释器后续执行不装载，故只有第一次付这笔时间。装载窗口默认 10 分钟、远高于实测，如需校准只改 `PY_LOAD_WINDOW_MS` 一处。
> 测量口径：无头 Chrome + CDP + `file://` 真产物、每档全新 profile；墙钟 = 发送 → 整轮空闲（含模型桩往返）；装载段取结果注记里的 `耗时`（worker 侧计时）。首次执行读数会随机器负载波动（同机换时重测可能差 1 s 级），以当次实测为准；本节与 §7 的数字来自同一轮测量（原始读数保存在开发工作区的进度文档里，不随仓库分发）。

**完整版实测**（同一 harness、全新 profile；一次抽验，非两次复跑）：首次执行墙钟 **≈38.7 s**，其中装载段 **≈34.8 s**（151 个包）；第二次执行 **≈1.1 s**（不再装载）。

## 9. 维护纪律

- 生成物**只经生成脚本产出**：改 `src/pyodide.part` 不行（会被下次生成覆盖），改 `AzusaAI-WebUI-*.html` 更不行（那是构建产物）。载荷**不得**出现 CR（构建期有专门断言，命中即 `exit 1`）。
- `vendor/` 不进版本库；换件后**必须**重跑 `scripts/make-pyodide-part.js` 并核对它自己的自检段（段数 / 逐段 sha256 / 字节账 / 剪裁锁条目数 / 依赖闭包 / wheel 内 METADATA 与顶层导入名）。改动不涉及载荷时，`src/pyodide.part` 应**逐字节未变**（用 md5 反证）。
- `src/pyodide.part`（186 MB）经 **Git LFS** 入库；三份分发产物（完整版 209 MB / 轻档 137 MB、55 MB）**不进版本库**，分发走 Release 附件。GitHub 单文件 100 MB 硬限对入库件同样生效——LFS 是唯一通道，不要把载荷直接 `git add` 成全量对象。
- **分档纪律**：档位名单 / 分组 / 落盘路径只改 `scripts/pyodide-profiles.json`（构建期 C1–C9 断言挡住「名单漂移、轻档缺依赖、分组覆盖不全、完整版被塞 profile 段」）；改了 `topLevel`（启用旋钮）后必须先用 `--dry-run` 重取实测值并写回 `expect`，否则下一次生成会按 ±0 直接 `exit 1`。**只有完整版载荷 `src/pyodide.part` 入库**，轻档载荷与三份分发产物由 `.gitignore` 忽略、按需本地生成（`make-pyodide-part.js --all` 一次出三档载荷；`build.js --profile=all` 一次出三档产物）。
- **`unavailable` 名单是双份，改动必须两边同步**：界面侧权威 = `src/appE.part` 的 `PY_UNAVAILABLE`（运行期渲染 `PythonPackages` ③ 段），构建侧 = `scripts/pyodide-profiles.json` 的 `unavailable`（C1–C9 断言与轻档载荷段使用）。新增 / 移除不可用包时**两份都要改**——构建期**不**做「二者相等」的断言（有意如此）；轻档载荷段里的 `label` / `topLevel` / `unavailable` 三个字段运行期无人读取，新增消费点前不要依赖它们。
- 与 Python 有关的文档：本文件 + `docs/DEVELOPMENT.md`（架构与存储）+ `README.md` 的「代码执行」一节。
