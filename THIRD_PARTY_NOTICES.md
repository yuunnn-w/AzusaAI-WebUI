# 第三方软件声明（THIRD PARTY NOTICES）

本项目分发产物（`AzusaAI-WebUI-{full,normal,minimal}.html`）是单文件应用，所有第三方库均以「内嵌源码 / 载荷」形式随包分发（零外部请求）。
以下清单核对自随包源码注释与上游发行版（`vendor/`）的包元数据（2026-09-12；pyodide 段为 2026-09-13 补）；各库的完整许可文本保存在其随包源码的注释头或上游发行版中。

| 库 | 版本 | 许可证 | 内嵌位置（源码） |
|----|------|--------|------------------|
| [marked](https://github.com/markedjs/marked) | 12.0.2 | MIT | `src/libs.part`（页面内注释含版权行） |
| [DOMPurify](https://github.com/cure53/DOMPurify) | 3.1.6 | Apache-2.0 或 MPL-2.0（双许可，任选其一） | `src/libs.part` |
| [highlight.js](https://github.com/highlightjs/highlight.js) | 11.9.0 | BSD-3-Clause | `src/libs.part` |
| [KaTeX](https://github.com/KaTeX/KaTeX)（含 20 个 woff2 字体） | 0.16.11 | MIT | `src/katex.min.js` / `src/katex-embedded.css` / `src/fonts/` |
| [pdf.js](https://github.com/mozilla/pdf.js)（pdfjs-dist legacy） | 6.3.289 | Apache-2.0 | `src/pdfjs.part` |
| [Tesseract.js](https://github.com/naptha/tesseract.js) | 7.0.0 | Apache-2.0 | `src/tesseract.part` |
| [tesseract.js-core](https://github.com/naptha/tesseract.js-core) | 7.0.0 | Apache-2.0 | `src/tesseract.part` |
| [tessdata_fast](https://github.com/tesseract-ocr/tessdata_fast)（eng + chi_sim 训练数据） | 随 Tesseract 7 分发 | Apache-2.0 | `src/tesseract.part` |
| [tiny-inflate](https://github.com/foliojs/tiny-inflate) | 1.0.3 | MIT | `src/tesseract.part`（仅无 `DecompressionStream` 的内核启用）；另在 `src/appE.part`（pyodide 加载器的兜底解压，仅无 `DecompressionStream` 时启用）内联同一实现**并含一处修正** |
| [pyodide](https://github.com/pyodide/pyodide)（核心集：`pyodide.asm.mjs` / `pyodide.asm.wasm` / `pyodide.js` / `pyodide-lock.json`） | 314.0.6 | MPL-2.0 | `src/pyodide.part`（`type="text/plain"` 载荷，全内嵌、运行时零网络） |
| [CPython](https://github.com/python/cpython) 标准库（`python_stdlib.zip`） | 3.14.2 | PSF-2.0 | `src/pyodide.part` |
| 预置 Python 包（**151 个 wheel**：pyodide 官方构建 115 + 纯 Python 补充 36） | 见 `src/pyodide.part` 内 `pyodide-lock.min.json` | 以宽松许可为主：MIT 63 / BSD 系列 58 / Apache-2.0 24 / PSF-2.0 4 / ISC 1 / MIT-CMU 1 / MPL-2.0 1（certifi）/ 0BSD·Zlib·CC0-1.0·CNRI-Python 各 1；**LGPL-2.1-only（docxtpl）/ LGPL-3.0-only（fpdf2）/ GPL-3.0（pingouin）各 1**；2 个（`google-crc32c`、`matplotlib-inline`）的 METADATA 未给 License 字段 | `src/pyodide.part` |

上述 151 个预置包均为**上游 wheel 原样未修改**内嵌（许可文本随各 wheel 的 `*.dist-info/` 一并分发）；许可归属实测自各 wheel 的 `*.dist-info/METADATA` 的 `License-Expression` / `License` / `Classifier: License ::` 字段（2026-09-13，151/151 抽取成功）。

办公文档解析所用四个库（第十四轮新增，随 `src/office.part` 分发，均来自上游发行版未修改；四份文件的字节数与 SHA-256 断言写在 `scripts/make-office-part.js` 里）：

| 库 | 版本 | 许可证 | 内嵌位置（源码） |
|----|------|--------|------------------|
| [mammoth](https://github.com/mwilliamson/mammoth.js) | 1.12.3 | BSD-2-Clause | `src/office.part`（**含打包依赖**：jszip、@xmldom/xmldom、bluebird、underscore、xmlbuilder 等，随 min 产物一并分发；CLI 用的 argparse(Python-2.0) **不在**浏览器构建里，实测 `mammoth.browser.min.js` 内 0 处出现，故不随附） |
| [SheetJS 社区版](https://git.sheetjs.com/sheetjs/sheetjs) | 0.20.3 | Apache-2.0 | `src/office.part`（官方 CDN 版 `xlsx.full.min.js`；**非** npm 上的 0.18.5 —— 后者含 CVE-2023-30533 / CVE-2024-22363 且无修复版） |
| [@jose.espana/docstream](https://github.com/jose-espana/docstream)（`harshankur/officeParser` 的 fork） | 0.1.3 | MIT | `src/office.part`（**含打包上游**：pdfjs-dist 5.4.530 = Apache-2.0、**Tesseract.js 6.0.1 = Apache-2.0**（其 `workerPath` 是构建期被中和的两条 URL 之一，见 `scripts/make-office-part.js`）、@xmldom/xmldom = MIT、buffer/ieee754/safe-buffer/@jspm-core 垫片 = MIT 或 BSD-3-Clause；file-type 在该浏览器构建里实测 0 处出现、不随附；本项目**不调用**其 PDF / OCR 路径） |
| [fflate](https://github.com/101arrowz/fflate) | 0.8.3 | MIT | `src/office.part`（仅用于 ZIP 结构预扫描 = 压缩炸弹防线） |

**对 tiny-inflate 的一处修改（如实声明）**：`src/appE.part` 里内联的那份（pyodide 加载器兜底解压）与上游 1.0.3 只差一个比较运算符 —— 存储块（stored block）位对齐回退循环由 `while (d.bitcount > 8)` 改为 `while (d.bitcount >= 8)`。上游写法在未消费位数恰为 8 的整数倍时会少退一个字节，导致 `LEN/INVLEN` 读错并抛 `Data error`；实测本项目 153 个内嵌 gzip 段中**上游写法失败 38 段**，改正后 **153/153** 与 zlib 参照逐字节一致（细节见 `docs/PYODIDE-NOTES.md`）。`src/tesseract.part` 里那份**未修改**（上游原样）。

本项目整体以 **GPL-3.0** 发布（见 `LICENSE`）；除上表标注的 LGPL/GPL 三项及站点打包依赖中的 `elkjs`（EPL-2.0）外，第三方库均为宽松许可，与 GPL-3.0 兼容。

JupyterLite 集成（v0.2.0 轮新增，随 `src/jupyterlite.part` 分发；载荷由 `scripts/make-jupyterlite-part.js` 从 `vendor/jupyterlite-src/` 生成）：

| 库 | 版本 | 许可证 | 内嵌位置（源码） |
|----|------|--------|------------------|
| [JupyterLite](https://github.com/jupyterlite/jupyterlite)（jupyterlite-core + lab 站点构建） | 0.8.3 | BSD-3-Clause（wheel 内附 `licenses/LICENSE` 全文） | `src/jupyterlite.part`（站点载荷段；站点文件 **`site/**` 469 件**——另有 `vendor/jupyterlite-src/MANIFEST.json` 的 `entryCount` **477** = 该 469 件 + 8 件非站点件（6 个 wheel + `pyodide.mjs` + `official-all.json`）；两数不同源，勿混用） |
| [jupyterlite-pyodide-kernel](https://github.com/jupyterlite/pyodide-kernel)（含内置 `piplite` 与 `ipykernel` / `widgetsnbextension` 兼容桩轮） | 0.8.6（桩轮：`ipykernel` 6.9.2 / `piplite` 0.8.6 / `widgetsnbextension` 3.6.999·4.0.999） | BSD-3-Clause（各 wheel 内附 `licenses/LICENSE` 全文） | `src/jupyterlite.part`（站点段 `extensions/@jupyterlite/pyodide-kernel-extension/static/pypi/*.whl`） |
| [JupyterLab](https://github.com/jupyterlab/jupyterlab)（lab 站点构建；含 lumino / CodeMirror 等运行期依赖） | 4.6.3 | BSD-3-Clause | `src/jupyterlite.part`（站点段 `site/build/**`、`site/lab/**`） |
| [comm](https://github.com/ipython/comm) | 0.2.3 | BSD-3-Clause（wheel 内附 `licenses/LICENSE` 全文） | `src/jupyterlite.part`（`pyodide/comm-0.2.3-py3-none-any.whl`） |
| [micropip](https://github.com/pyodide/micropip)（pyodide 发行版件） | 0.11.1 | MPL-2.0（wheel 内附 `licenses/LICENSE` 全文） | `src/jupyterlite.part`（`pyodide/micropip-0.11.1-py3-none-any.whl`） |
| [pyodide](https://github.com/pyodide/pyodide)（`pyodide.mjs` 模块加载器；与上表 pyodide 行同源） | 314.0.6 | MPL-2.0 | `src/jupyterlite.part`（`pyodide/pyodide.mjs`） |
| JupyterLite 闭包另 12 件 wheel（`ipython` / `asttokens` / `decorator` / `executing` / `matplotlib_inline` / `prompt_toolkit` / `pure_eval` / `pygments` / `six` / `stack_data` / `traitlets` / `wcwidth`） | 见 `src/jupyterlite.part` 内 `jupyterlite-lock.json` | 属上表 151 件 wheel 之内（同 sha256；许可随各 wheel `*.dist-info/`） | `src/jupyterlite.part`（`pyodide/*.whl`，Jupyter 内核装载用副本） |
| [jedi](https://github.com/davidhalter/jedi)（Jupyter 内核属性补全；**取自 pyodide 发行版，不在上表 151 件名册内**） | 0.19.2 | MIT（wheel 内附 `jedi-0.19.2.dist-info/LICENSE.txt` 全文） | `src/jupyterlite.part`（`pyodide/jedi-0.19.2-py2.py3-none-any.whl`；来源 `https://cdn.jsdelivr.net/pyodide/v314.0.6/full/jedi-0.19.2-py2.py3-none-any.whl`，与 `vendor/pyodide-src/pyodide-lock.json` 同 sha256） |
| [parso](https://github.com/davidhalter/parso)（jedi 的依赖；**取自 pyodide 发行版，不在上表 151 件名册内**） | 0.8.6 | MIT（wheel 内附 `parso-0.8.6.dist-info/licenses/LICENSE.txt` 全文） | `src/jupyterlite.part`（`pyodide/parso-0.8.6-py2.py3-none-any.whl`；来源 `https://cdn.jsdelivr.net/pyodide/v314.0.6/full/parso-0.8.6-py2.py3-none-any.whl`，与 `vendor/pyodide-src/pyodide-lock.json` 同 sha256） |

**「含打包依赖」段（JupyterLite lab 站点）**：lab 站点构建自带一份许可报告，逐项列出 **300 个包**（JupyterLite / JupyterLab 自身组件与运行期 npm 依赖）的许可标识，分布：BSD-3-Clause 146 / MIT 124 / ISC 15 / Apache-2.0 8 / BSD-2-Clause 4 / MPL-2.0 或 Apache-2.0 1（`dompurify`，构建期依赖）/ EPL-2.0 1（`elkjs`，弱 copyleft，如实登记）/ CC-BY-4.0 AND OFL-1.1 AND MIT 1（`@fortawesome/fontawesome-free`：图标 CC-BY-4.0 / 字体 OFL-1.1 / 代码 MIT）；该报告随包内嵌于 `src/jupyterlite.part` 的 `build/third-party-licenses.json`（300 项中 173 项另含许可文本）。内核扩展闭包另有随包内嵌的 `extensions/@jupyterlite/pyodide-kernel-extension/static/third-party-licenses.json`（5 包：`@jupyterlite/apputils` 0.8.3、`@jupyterlite/pyodide-kernel` 0.8.6、`@jupyterlite/services` 0.8.3、`coincident` 1.2.3（ISC）、`comlink` 4.4.2（Apache-2.0）；其中 3 项含许可文本）。站点内嵌字体：[MathJax](https://github.com/mathjax/MathJax) 22 个 `.woff`（随 `mathjax-full` 3.2.2，站点许可报告标 Apache-2.0）与 [FontAwesome](https://github.com/FortAwesome/Font-Awesome) 5.15.4 字体（`fa-*` 的 woff2 / woff / ttf / eot / svg 五类格式）；两者均已内联为 `data:` URL 随包分发。站点图标（`icon-120x120.png` / `icon-512x512.png` / `lab/favicon.ico`）随站点构建产物内嵌。

**对站点文件的程序化改写（如实声明）**：生成期共 5 处改动（唯一权威表 = `scripts/jupyterlite-patches.json`）：`393.*.js` 去掉动态 import 的 `{type:"module"}` 选项（1 处）· `comlink.worker.*.js` / `coincident.worker.*.js` 尾部 `export{…}` 改为 `self.<导出名>=<本地名>;` 赋值（2 件）· `jupyter-lite.json` 注入内核配置（9 键集合，含 `pyodideUrl` / `pipliteWheelUrl` / `pipliteUrls` / `disablePyPIFallback` / `loadPyodideOptions` 等）· `remoteEntry.*.js` 自定位变量钉为离线伪源 `https://azusa-jupyter.invalid/`（1 处）· `static/pypi/all.json` 以合并索引覆盖（5 键 → 18 键）；`build/schemas/all_federated.json` 另列改点但仅做形状断言、不改字节。改写点均落在加载接线与索引文件，不涉及上列各组件的许可与版权文本。

**取证方式（2026-09-17 实取）**：① 各 wheel 的 `*.dist-info/METADATA`（`License-Expression` / `License` / `Classifier: License ::` 字段）与 `*.dist-info/licenses/LICENSE` 全文（`vendor/jupyterlite-src/` 及其 `site/extensions/…/static/pypi/`）；② 站点与内核扩展自带许可报告（`build/third-party-licenses.json` 逐包 `licenseId` + 配套文本）；③ `pyodide.mjs` / `micropip` 来源 = pyodide 发行版 `v314.0.6/full`（`vendor/jupyterlite-src/MANIFEST.json` 的 `url` 字段；sha256 已与上游逐位对拍）。
