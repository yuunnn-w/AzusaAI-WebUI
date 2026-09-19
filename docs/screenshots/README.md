# 截图目录

本目录存放 README 引用的实机截图：**真机 Thorium 122（原生 CDP）** 打开三档产物、连真实模型服务采集（非示意图），主题为马卡龙配色。旧 9 张（2026-09-14/15 采集，Chrome 152 headless）已于 2026-09-18 全部替换，旧版留档在 `../../../shared/archive/screenshots-old-2026-09-18/`。**2026-09-19（v0.2.1 收口批）重拍 2 张**：`05-office-parse.png`（附件档位文案改为三档新口径「混合模式(文字 + N 张图按序发送)」）与 `08-jupyter.png`（Jupyter 底部日志栏已删，只剩同步状态行）；旧版留档在 `../../../shared/archive/v021-final-suite-2026-09-19/docs-reshoot/old/`。

| 文件名 | 场景 | 主题 | 视口 | 被引用于 |
| --- | --- | --- | --- | --- |
| `01-hero-light.png` | 新会话空态（侧栏含真实对话历史 + 欢迎页 + 输入区），状态行含模型与上下文环 | 浅色 · 樱花粉 | 1600×900 | README hero（主图） |
| `02-hero-dark.png` | 同场景深色对照 | 深色 · 樱花粉 | 1600×900 | README hero（副图） |
| `03-chat-stream.png` | 生成中：思考折叠块 + Markdown 表格 + 代码块 + 用量环同屏（真模型流式） | 浅色 · 薄荷 | 1600×900 | README「对话与生成」 |
| `04-attach-preview.png` | 附件卡片 + PDF 逐页预览弹窗（3 页 · 图片模式 150 DPI） | 浅色 · 薄荷 | 1600×900 | README「知识与文件」 |
| `05-office-parse.png` | 办公解析：xlsx（2 表 + 1 图）→ 预览弹窗内文本 + 抽出的图片（meta 已按三档新口径标「混合模式(文字 + 1 张图按序发送)」） | 浅色 · 薄荷 | 1600×900 | README「知识与文件」 |
| `06-workspace-rail.png` | 工作区右栏：工作区选择器 + 工具条 + 虚拟盘文件树（docs / src / 文件） | 深色 · 樱花粉 | 1600×900 | README「代码执行」 |
| `07-python-card.png` | `ExecutePython` 工具卡片展开（代码 + 运行状态） | 深色 · 珊瑚 | 1600×900 | README「代码执行」 |
| `08-jupyter.png` | JupyterLite 以工作区为根：`shot-demo.ipynb` 已执行出结果 + 左侧文件浏览器（demo notebook / csv / md）+ 底部**仅有**同步状态行（日志栏已删） | JupyterLab 默认浅色 | 1600×900 | README「代码执行」 |
| `09-mcp-panel.png` | 设置 → MCP 工具：内置工具清单（默认全关）+ 权限三态 | 浅色 · 水蓝 | 1600×900 | README「工具」 |
| `10-profiles.png` | 三档产物的「设置 → 环境 → Python 运行时」状态行纵向拼合（full 151 / normal 99 / minimal 33 个预置包） | 浅色 · 樱花粉 | 1120×583 | README「快速开始」 |
| `11-templates.png` | 提示词模板浮窗（18 条 / 6 组，含分组标题） | 浅色 · 水蓝 | 1600×900 | README「对话与生成」 |
| `12-compact.png` | 上下文压缩工具气泡：执行成功 + 丢弃条数 / tok 结算 / 时间 / 摘要区 | 浅色 · 樱花粉 | 1600×900 | README「上下文压缩」 |

## 采集记录（2026-09-18 全量重拍）

- 采集环境：Windows + **Thorium `122.0.6261.171`（`--headless=new`）+ 原生 CDP**（`Emulation.setDeviceMetricsOverride` 固定 1600×900 / DSF 1；`10` 为三档分别裁「Python 运行时」区块后纵向拼合 1120×583）。
- 模型：真实 llama.cpp 端点（`Qwen3.8-27B-UD-Q6_K_M.gguf`，26 万 token 上下文、多模态）——界面里显示的 Base URL 是中性默认 `http://127.0.0.1:8080/v1`（本机代理指向真实端点，故画面不出现任何内网地址）；聊天屏均为真实流式生成，统计行（tok / tok·s⁻¹ / 秒）为服务端与端到端实测。
- 附件屏：`04` 用自制 3 页 PDF（图片模式，150 DPI）+ 插图 PNG；`05` 用自造 xlsx（两张表 + 工作表内嵌图片），解析文本与抽图均来自产品自身解析链路。
- 发布纪律核对：全部 12 张不含 API Key、内网地址、真实用户名与本机绝对路径；无控制台错误覆盖层；单张实测 92 KB–510 KB（PNG，普通 git 对象，不走 Git LFS），合计约 4.0 MB。

> 采集装置（CDP 驱动 / 夹具生成 / PNG 重压缩）为一次性工具，留在工作区 `shared/tmp/shots/`，不随仓库分发。

## 采集记录（2026-09-19 局部重拍 2 张）

- 缘由（v0.2.1 收口批，批次 A）：① `b33` 删除 Jupyter 底部日志栏 ⇒ 旧 `08` 底部仍带日志行的画面对不上现状；② `b31` 附件三档（纯文本／混合／图片）改写了附件档位文案（`attachModeLabel`）⇒ 旧 `05` 的 meta「文本 + 图片都会发给模型」已过期。
- 采集环境：Windows + **Thorium `122.0.6261.171`（`--headless=new`）+ 原生 CDP**（`Emulation.setDeviceMetricsOverride` 固定 1600×900 / DSF 1）；主题与旧图一致（`05` 浅色·薄荷；`08` JupyterLab 默认浅色）。
- 场景与夹具：`05` 复用 2026-09-18 同一份 `windtunnel.xlsx`（2 表 + 1 图，附件真实解析链路），meta 现为「XLSX · 2 表 · 1 图 · 混合模式(文字 + 1 张图按序发送) · 约 1,248 tokens(估算)」；`08` 工作区放 `shot-demo.ipynb` + `测点记录.csv` + `说明.md`，真执行单元格（输出 `sqrt(2) = 1.414214`），底部只有 `#jl-sync-note` 状态行。
- 其余 10 张经对照本轮实机截图（聊天 / 右栏 / 预览 / 设置 / 工具卡）核对，**未发现与本批改动相关的过期点**；`09-mcp-panel.png` 另与现行「MCP 工具」面板逐节比对（分组「文件 (9)」、内置清单与权限三态一致）确认仍有效。
- 旧版备份与取证：`shared/archive/v021-final-suite-2026-09-19/docs-reshoot/old/`（`05` `88704059…`/275,925 B、`08` `2940ae0d…`/92,280 B）；新图读数（预览 meta、子文档探针）在 `shared/archive/v021-final-suite-2026-09-19/readings/r11*.json`。
