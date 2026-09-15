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

本项目整体以 **GPL-3.0** 发布（见 `LICENSE`）；除上表标注的 LGPL/GPL 三项外，第三方库均为宽松许可，与 GPL-3.0 兼容。
