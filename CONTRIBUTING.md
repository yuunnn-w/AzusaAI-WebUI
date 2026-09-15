# 贡献指南

感谢有兴趣改进 AzusaAI WebUI。这是一个单文件、离线优先的应用：源码全部在 `src/`，构建脚本在 `scripts/`，产物是代码目录根的三份 `AzusaAI-WebUI-*.html`。改动请改源码分段、重新构建、在浏览器里实测——三条都做完再提 PR。

## 环境要求

- **Node.js**：构建脚本无 npm 依赖，装了 node 即可运行（开发时用的是 Node 24）。
- **git-lfs**：`src/pyodide.part`（约 186 MB）走 Git LFS。克隆后请先拉取：

```bash
git lfs install
git lfs checkout          # 或 git lfs pull
```

未拉取时该文件只是 134 B 的 LFS 指针，`node scripts/build.js` 会在读取载荷前明确中止（不会产出「构建成功但 Python 工具全坏」的坏产物）。

## 构建

```bash
node scripts/build.js                     # 默认档 full → AzusaAI-WebUI-full.html
node scripts/build.js --profile=normal    # → AzusaAI-WebUI-normal.html（需先生成对应载荷）
node scripts/build.js --profile=minimal   # → AzusaAI-WebUI-minimal.html
node scripts/build.js --profile=all       # 三档一次拼装（任一档载荷缺失即 exit 1，不写半套）
```

轻档载荷（`src/pyodide-normal.part` / `src/pyodide-minimal.part`）不在版本库中，先生成再构建：

```bash
node scripts/make-pyodide-part.js --all           # 一次生成三档载荷（实测约 13 s，热缓存）
```

检查链（改动后至少跑前两条）：

```bash
node scripts/syntax.js    # 把产物里每段内联脚本交给 V8 解析（跳过 text/plain 载荷）
node scripts/lint.js      # 启发式检查「调用了但未声明」的标识符
```

产物 html 是构建结果，**不要直接改**——会被下一次构建覆盖；改 `src/` 下的 `.part` 才作数。

## 重新生成内嵌库载荷（vendor/ 取件）

`vendor/` 是各上游发行版的本地副本（不进版本库，最新取件后约 205 MB）。装到位后运行对应的生成脚本，产物即 `src/*.part`（全部幂等：输入不变则输出字节级一致）：

| 载荷 | 生成命令 | 上游来源（放 `vendor/` 下） | 细节 |
| --- | --- | --- | --- |
| `src/pyodide.part` | `node scripts/make-pyodide-part.js --all` | pyodide 314.0.6 官方发行：`pyodide-lock.json`、核心文件、`wheels/*.whl` → `vendor/pyodide-src/` | `docs/PYODIDE-NOTES.md` |
| `src/pdfjs.part` | `node scripts/make-pdfjs-part.js` | pdfjs-dist 6.3.289 legacy（npm 发行 tgz 解出） → `vendor/pdfjs-src/` | `docs/PDFJS-NOTES.md` |
| `src/tesseract.part` | `node scripts/make-tesseract-part.js` | tesseract.js 7.0.0 + tesseract.js-core + tessdata_fast 训练数据 → `vendor/tess-src/` | `docs/TESS-NOTES.md` |
| `src/office.part` | `node scripts/make-office-part.js` | mammoth / SheetJS CE / docstream / fflate 四个浏览器单包 → `vendor/office-src/` | `docs/OFFICE-NOTES.md` |

pyodide 载荷的取件目录最讲究，`vendor/pyodide-src/` 应包含（缺了脚本会直接报错退出）：

```
pyodide-lock.json      官方全量锁文件
packages.json          选包清单（fromOfficialLock / selfAuthored / depExclusions）
pyodide.asm.mjs  pyodide.js  pyodide.asm.wasm  python_stdlib.zip   官方核心文件
wheels/*.whl           官方闭包 + 自造条目的全部 wheel
MANIFEST.json          可选：取件清单，存在时逐条核对 sha256
```

其余三份载荷的取件就是「按上表来源下载 → 放入对应目录」；每份 NOTES 里都记录了当时取到的文件名、字节数、SHA-256 断言位置与生成脚本的自检规则。**换任何库版本前先读对应 NOTES**——`file://` 下的 worker 加载约束、转义规则、ESM 改写都在里面。

## 代码风格（刻意保守）

目标是跑在 **Chrome / Edge 88+、Firefox 85+、Safari 14.1+** 上，语法一律保守：

- **ES5 + `var` + `function`**；仅网络层与工具执行允许 `async/await`。
- 禁用：可选链 `?.`、空值合并 `??`、`??=`、`.at()`、`structuredClone` 等新语法。
- 命名：函数与变量 `camelCase`，常量 `SCREAMING_SNAKE_CASE`，CSS 变量与类名 kebab-case。
- **新增函数名前先全局搜重名**：`appA`–`appE` 是同一个 IIFE 的分段，靠函数声明提升共享作用域——同名函数后声明者会静默覆盖先声明者（历史上出现过「新建会话空白页」）。只有 `appE.part` 收尾，新增分段不得提前闭合。
- 新增主题色要同时补深 / 浅两组 CSS 变量、`ACCENTS` 数组与色块；图标加进 SVG 精灵，不引图标字体。
- 批量替换用函数形式 `s.replace(a, function () { return b; })`——替换串里的 `$` 会被解释成特殊模式。
- 所有 Markdown / HTML 渲染必须过 DOMPurify（或等价转义），不得直插未净化内容。
- **一切资源内联**：不允许 CDN、外链、远程字体或任何外部请求——这是产品红线，也是离线能力与数据不出本机的保证。

> 改代码会踩的坑（都踩过）整理在 `docs/DEVELOPMENT.md`「踩坑清单」，动手前建议通读。

## 验证（脚本按需现写）

本项目**不保留常驻回归 / 测试脚本**：验证脚本按需现写、用完即删。两种起手式：

**A. 无头端到端（自己起静态服务 + 无头 Chrome）**

```bash
# serve(9001) → spawn chrome --headless=new --remote-debugging-port=9300+
# → 轮询 /json/list 拿 ws → Runtime.enable / Log.enable / Page.enable
# → Page.navigate → 等 dataset.boot === "done" → 断言 + 截图
```

**B. 真实浏览器（WebBridge，能连真实模型）**

```bash
node scripts/server.js 9000     # 另开终端常驻（/ 默认给完整版产物）
node scripts/wb.js nav http://127.0.0.1:9000/AzusaAI-WebUI-full.html
sleep 8 && node scripts/wb.js front     # 必做：后台标签页 rAF 被暂停
node scripts/wb.js eval my-check.js     # 自己写的探针脚本（路径相对当前目录）
node scripts/wb.js shot "docs/screenshots/xx.png"
node scripts/wb.js snap                 # 无障碍树
```

### 写验证脚本时必须知道的坑

- **先 `node scripts/build.js`**：产物是构建结果，直接改会被覆盖；改 `src/*.part` 才作数。
- **等启动完成再操作 DOM**：应用异步启动（会话要从 IndexedDB 装回），无头脚本必须轮询 `document.documentElement.dataset.boot === "done"`；只看 `.msg` / `#welcome` 会在启动未完成时误判。
- **真实浏览器必须先 `node scripts/wb.js front`**：后台标签页的 `requestAnimationFrame` 被暂停，流式重绘看起来像「渲染坏了」。
- **CDP 改视口（`Emulation.setDeviceMetricsOverride`）前先 `Page.bringToFront`**：后台标签页不派发 `resize`，会让「自动收回 / 恢复」这类响应式行为看起来失灵——先 `Page.bringToFront` 再改视口。
- **`wb.js eval` 只吃文件路径**（不能传进程替换，Windows 下 ENOENT）；探针写进临时目录再用相对路径传入。
- **无头 Chrome 的 profile 用完即删**（`--headless=new --user-data-dir=<临时目录>`），别留在仓库里。
- **端口用 9000+**：本机 8090–8123 被 Windows 保留（`listen EACCES`）。
- **`file://` 只能自己开无头 Chrome 验**：WebBridge 打不开 `file://`；用 CDP 的 `Page.navigate` 到 `file:///…` 再 `Runtime.evaluate`。
- **多套件别并行跑**：同机并行会因负载互相干扰出假失败，串行跑。
- **Windows 上别用 shell heredoc 写含反斜杠的 JS/JSON**：会被吞掉一层反斜杠，用文件写入工具直接落盘。
- **会话在 IndexedDB**：脚本读 localStorage 只能拿到设置；要读会话得打开 `chatgpt-webui-blobs` 库的 `convs` 存储区 `getAll()`。

## 提交与 PR

- 一次提交只做一件事；提交信息写清「改了什么、为什么」。
- 提交前必跑：`node scripts/build.js && node scripts/syntax.js && node scripts/lint.js`。
- 涉及页面行为的改动请实测，并在 PR 描述里写明**怎么验的、覆盖了什么、没覆盖什么**。
- 不要提交构建产物（`AzusaAI-WebUI-*.html`）、`vendor/`、轻档载荷与无头浏览器 profile——这些已在 `.gitignore` 里。
- 不要提交密钥、内网地址或任何个人信息；文档与截图同理。
- 想新增外部依赖请先讨论：所有库必须可内联、可离线运行，并登记到 `THIRD_PARTY_NOTICES.md`。
