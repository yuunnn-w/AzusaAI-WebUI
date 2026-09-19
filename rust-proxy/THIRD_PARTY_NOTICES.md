# 第三方声明（`rust-proxy/`）

本子项目是 **AzusaAI WebUI** 的一部分，整体许可 = 仓库根 [`LICENSE`](../LICENSE)（**GPL-3.0**）。
以下第三方组件被本子项目的**构建链**使用，并（部分）进入发布产物；依其许可证要求保留声明与署名。
**本文件为 Phase 6 收口版**：VC-LTL5 / YY-Thunks 的**许可文本已随附**（`licenses/`），Rust crate 许可清单为**生成版**（§3）。

---

## 1. VC-LTL5 v5.3.1 —— Eclipse Public License 2.0（EPL-2.0）

- **用途**：Windows 7 兼容第 1 层（CRT 替换库：把 UCRT/VCRUNTIME 依赖改为系统 `msvcrt.dll`）。
- **版权/维护**：Chuyu-Team · <https://github.com/Chuyu-Team/VC-LTL5>（release [`v5.3.1`](https://github.com/Chuyu-Team/VC-LTL5/releases/tag/v5.3.1)）
- **许可证**：**EPL-2.0**（Eclipse Public License 2.0；**注意：不是 MIT**）。
- **许可文本（在位）**：`licenses/VC-LTL5-EPL-2.0.txt`（EPL-2.0 全文 13,968 B；取自 v5.3.1 仓库 `LICENSE`；md5 `a4f6f19abdbd25b01280f15872163b2c`）
- **所用文件清单**（release 档，恰 4 个/架构；由 `scripts/fetch-assets.ps1` 从 `VC-LTL-Binary.7z` 解出）：

  | 源（归档内） | 放置（本仓库，不入库） |
  |---|---|
  | `TargetPlatform/6.0.6000.0/lib/x64/libucrt.lib` | `assets/vc-ltl/x64/libucrt.lib` |
  | `TargetPlatform/6.0.6000.0/lib/x64/libvcruntime.lib` | `assets/vc-ltl/x64/libvcruntime.lib` |
  | `TargetPlatform/6.0.6000.0/lib/x64/ucrt.lib` | `assets/vc-ltl/x64/ucrt.lib` |
  | `TargetPlatform/6.0.6000.0/lib/x64/vcruntime.lib` | `assets/vc-ltl/x64/vcruntime.lib` |
  | `TargetPlatform/6.0.6000.0/lib/Win32/libucrt.lib` | `assets/vc-ltl/x86/libucrt.lib` |
  | `TargetPlatform/6.0.6000.0/lib/Win32/libvcruntime.lib` | `assets/vc-ltl/x86/libvcruntime.lib` |
  | `TargetPlatform/6.0.6000.0/lib/Win32/ucrt.lib` | `assets/vc-ltl/x86/ucrt.lib` |
  | `TargetPlatform/6.0.6000.0/lib/Win32/vcruntime.lib` | `assets/vc-ltl/x86/vcruntime.lib` |

- **分发说明**：资产按 **DG14「按需取件」** 不进入本仓库版本历史（只由 `fetch-assets.ps1` 在工作站解出），
  但**发布出去的 exe 内仍替换进了 VC-LTL 提供的 CRT 实现** ⇒ **义务不变**：分发物与 Release 说明中
  保留上述署名与许可证文本（Release 说明模板见 §5）。
- 归档 SHA-256（指南 §8.1 官方值，`fetch-assets.ps1` 逐字节核验；Phase 4W 现取复核一致）：
  `7a18799ed3aa84a225610a5447a56bc534c5c98ccb8dec05caba0e3f633431ad`（18,657,123 B）

## 2. YY-Thunks v1.2.2 —— MIT License

- **用途**：Windows 7 兼容第 2 层（缺失 Win32 导出/`raw-dylib` 导入的 thunk 桩）。
- **版权/维护**：Chuyu-Team · <https://github.com/Chuyu-Team/YY-Thunks>（release [`v1.2.2`](https://github.com/Chuyu-Team/YY-Thunks/releases/tag/v1.2.2)）
- **许可证**：**MIT**。
- **许可文本（在位）**：`licenses/YY-Thunks-MIT.txt`（MIT 全文 1,067 B；与 `YY-Thunks-Objs.zip` 内 `LICENSE` **逐字节一致**（行尾归一后核验）；md5 `74cada428b29d2eed8baa3031e5e4063`）
- **所用文件清单**：

  | 源（归档内） | 放置（本仓库，不入库） |
  |---|---|
  | `objs/x64/YY_Thunks_for_Win7.obj` | `assets/yy-thunks/YY_Thunks_for_Win7.obj` |
  | `objs/x86/YY_Thunks_for_Win7.obj` | `assets/yy-thunks/YY_Thunks_for_Win7_x86.obj`（**放置端改名**；源名无 `_x86` 后缀） |

- **验证工具（不随发布物分发）**：`YY.Depends.Analyzer.exe` + `Config/` + `objs/` 仅解出到
  `tools/depends/`（不入库），用于本仓库的 Win7 静态导入验证（见 `scripts/verify-win7.ps1`）。
- 归档 SHA-256（指南 §8.1 官方值；Phase 4W 现取复核一致）：`518ed7ef4825e8a41997fbccfa2c8090cf31a6038fd51520a2e49886f947f9fc`（14,108,378 B）

## 3. Rust 依赖许可清单（生成版 · Phase 6）

**crate 总数（不含本子项目自身）：134**（直接 + 传递依赖；本子项目自身 = `azusa-local-proxy` 0.1.0，GPL-3.0，见仓库根 `LICENSE`）。
**生成方式**（等价 `cargo-license` 的现取清单）：
`cargo metadata --format-version 1 --offline --locked` ⇒ 取各包 `license` 字段（SPDX 字符串）；
机器可读同版清单：`licenses/rust-crates-licenses-20260919.txt`（**134 条 crate；全文 142 行**，含 7 行注释头与 1 行分隔空行）。

> **计数变化（140 → 134）**：Phase 6 删除配置文件支持后，`toml 0.8.x` 家族及其传递依赖随之下线
> （`toml 0.8.23` / `toml_edit 0.22.27` / `toml_datetime 0.6.11` / `toml_write 0.1.2` / `serde_spanned 0.6.9` / `winnow 0.7.15`，共 6 个）。
> `toml 1.1.x` 家族**仍在**（构建期 `winresource` 需要），故不是"toml 全部消失"。

许可分布（SPDX 归组）：

| 许可（SPDX 字符串） | 数量 |
|---|---|
| MIT OR Apache-2.0 | 77 |
| MIT | 24 |
| Unicode-3.0 | 18 |
| Apache-2.0 OR MIT | 7 |
| MIT/Apache-2.0 | 2 |
| (MIT OR Apache-2.0) AND Unicode-3.0 | 1 |
| Apache-2.0 | 1 |
| Apache-2.0 OR BSL-1.0 | 1 |
| Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | 1 |
| MIT AND BSD-3-Clause | 1 |
| Unlicense OR MIT | 1 |

> 备注：`MIT/Apache-2.0`（斜杠形，serde_urlencoded / version_check）为**旧式双许可写法**（= MIT 或 Apache-2.0）；
> `Unicode-3.0` 一族（icu4x 数据表）为 Unicode License v3；各 crate 的完整许可文本位于其发布包内 LICENSE 文件。

完整清单（134 条 crate）：

| Crate | Version | License (SPDX) |
|---|---|---|
| `android_system_properties` | 0.1.6 | MIT OR Apache-2.0 |
| `atomic-waker` | 1.1.2 | Apache-2.0 OR MIT |
| `autocfg` | 1.5.1 | Apache-2.0 OR MIT |
| `axum` | 0.8.9 | MIT |
| `axum-core` | 0.5.6 | MIT |
| `base64` | 0.22.1 | MIT OR Apache-2.0 |
| `base64` | 0.23.1 | MIT OR Apache-2.0 |
| `bitflags` | 2.13.2 | MIT OR Apache-2.0 |
| `bumpalo` | 3.20.3 | MIT OR Apache-2.0 |
| `bytes` | 1.12.1 | MIT |
| `cc` | 1.4.7 | MIT OR Apache-2.0 |
| `cfg-if` | 1.0.5 | MIT OR Apache-2.0 |
| `chrono` | 0.4.45 | MIT OR Apache-2.0 |
| `core-foundation-sys` | 0.8.7 | MIT OR Apache-2.0 |
| `displaydoc` | 0.2.7 | MIT OR Apache-2.0 |
| `equivalent` | 1.0.2 | Apache-2.0 OR MIT |
| `errno` | 0.3.14 | MIT OR Apache-2.0 |
| `find-msvc-tools` | 0.1.13 | MIT OR Apache-2.0 |
| `form_urlencoded` | 1.2.2 | MIT OR Apache-2.0 |
| `futures-channel` | 0.3.34 | MIT OR Apache-2.0 |
| `futures-core` | 0.3.34 | MIT OR Apache-2.0 |
| `futures-io` | 0.3.34 | MIT OR Apache-2.0 |
| `futures-macro` | 0.3.34 | MIT OR Apache-2.0 |
| `futures-sink` | 0.3.34 | MIT OR Apache-2.0 |
| `futures-task` | 0.3.34 | MIT OR Apache-2.0 |
| `futures-util` | 0.3.34 | MIT OR Apache-2.0 |
| `hashbrown` | 0.17.1 | MIT OR Apache-2.0 |
| `http` | 1.5.0 | MIT OR Apache-2.0 |
| `http-body` | 1.1.0 | MIT |
| `http-body-util` | 0.1.5 | MIT |
| `httparse` | 1.10.1 | MIT OR Apache-2.0 |
| `httpdate` | 1.0.3 | MIT OR Apache-2.0 |
| `hyper` | 1.11.1 | MIT |
| `hyper-util` | 0.1.20 | MIT |
| `iana-time-zone` | 0.1.65 | MIT OR Apache-2.0 |
| `iana-time-zone-haiku` | 0.1.2 | MIT OR Apache-2.0 |
| `icu_collections` | 2.3.0 | Unicode-3.0 |
| `icu_locale_core` | 2.3.0 | Unicode-3.0 |
| `icu_normalizer` | 2.3.0 | Unicode-3.0 |
| `icu_normalizer_data` | 2.3.0 | Unicode-3.0 |
| `icu_properties` | 2.3.0 | Unicode-3.0 |
| `icu_properties_data` | 2.3.0 | Unicode-3.0 |
| `icu_provider` | 2.3.1 | Unicode-3.0 |
| `idna` | 1.1.0 | MIT OR Apache-2.0 |
| `idna_adapter` | 1.2.2 | Apache-2.0 OR MIT |
| `indexmap` | 2.14.2 | Apache-2.0 OR MIT |
| `ipnet` | 2.12.2 | MIT OR Apache-2.0 |
| `itoa` | 1.0.18 | MIT OR Apache-2.0 |
| `js-sys` | 0.3.105 | MIT OR Apache-2.0 |
| `libc` | 0.2.189 | MIT OR Apache-2.0 |
| `litemap` | 0.8.3 | Unicode-3.0 |
| `log` | 0.4.34 | MIT OR Apache-2.0 |
| `matchit` | 0.8.4 | MIT AND BSD-3-Clause |
| `memchr` | 2.8.3 | Unlicense OR MIT |
| `mime` | 0.3.17 | MIT OR Apache-2.0 |
| `mio` | 1.2.3 | MIT |
| `num-traits` | 0.2.19 | MIT OR Apache-2.0 |
| `once_cell` | 1.21.4 | MIT OR Apache-2.0 |
| `percent-encoding` | 2.3.2 | MIT OR Apache-2.0 |
| `pin-project-lite` | 0.2.17 | Apache-2.0 OR MIT |
| `potential_utf` | 0.1.6 | Unicode-3.0 |
| `proc-macro2` | 1.0.107 | MIT OR Apache-2.0 |
| `quote` | 1.0.47 | MIT OR Apache-2.0 |
| `reqwest` | 0.13.5 | MIT OR Apache-2.0 |
| `rustversion` | 1.0.23 | MIT OR Apache-2.0 |
| `ryu` | 1.0.23 | Apache-2.0 OR BSL-1.0 |
| `serde` | 1.0.229 | MIT OR Apache-2.0 |
| `serde_core` | 1.0.229 | MIT OR Apache-2.0 |
| `serde_derive` | 1.0.229 | MIT OR Apache-2.0 |
| `serde_json` | 1.0.151 | MIT OR Apache-2.0 |
| `serde_path_to_error` | 0.1.20 | MIT OR Apache-2.0 |
| `serde_spanned` | 1.1.1 | MIT OR Apache-2.0 |
| `serde_urlencoded` | 0.7.1 | MIT/Apache-2.0 |
| `shlex` | 2.0.1 | MIT OR Apache-2.0 |
| `signal-hook-registry` | 1.4.8 | MIT OR Apache-2.0 |
| `slab` | 0.4.12 | MIT |
| `smallvec` | 1.16.1 | MIT OR Apache-2.0 |
| `socket2` | 0.6.5 | MIT OR Apache-2.0 |
| `stable_deref_trait` | 1.2.1 | MIT OR Apache-2.0 |
| `syn` | 2.0.119 | MIT OR Apache-2.0 |
| `syn` | 3.0.6 | MIT OR Apache-2.0 |
| `sync_wrapper` | 1.0.2 | Apache-2.0 |
| `synstructure` | 0.14.0 | MIT |
| `tinystr` | 0.8.4 | Unicode-3.0 |
| `tokio` | 1.53.1 | MIT |
| `tokio-macros` | 2.7.2 | MIT |
| `tokio-util` | 0.7.19 | MIT |
| `toml` | 1.1.6+spec-1.1.0 | MIT OR Apache-2.0 |
| `toml_datetime` | 1.1.1+spec-1.1.0 | MIT OR Apache-2.0 |
| `toml_parser` | 1.1.3+spec-1.1.0 | MIT OR Apache-2.0 |
| `toml_writer` | 1.1.2+spec-1.1.0 | MIT OR Apache-2.0 |
| `tower` | 0.5.3 | MIT |
| `tower-http` | 0.6.11 | MIT |
| `tower-layer` | 0.3.3 | MIT |
| `tower-service` | 0.3.3 | MIT |
| `tracing` | 0.1.44 | MIT |
| `tracing-core` | 0.1.36 | MIT |
| `try-lock` | 0.2.5 | MIT |
| `unicode-ident` | 1.0.26 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| `url` | 2.5.8 | MIT OR Apache-2.0 |
| `utf8_iter` | 1.0.4 | Apache-2.0 OR MIT |
| `version_check` | 0.9.5 | MIT/Apache-2.0 |
| `want` | 0.3.1 | MIT |
| `wasi` | 0.11.1+wasi-snapshot-preview1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `wasm-bindgen` | 0.2.128 | MIT OR Apache-2.0 |
| `wasm-bindgen-futures` | 0.4.78 | MIT OR Apache-2.0 |
| `wasm-bindgen-macro` | 0.2.128 | MIT OR Apache-2.0 |
| `wasm-bindgen-macro-support` | 0.2.128 | MIT OR Apache-2.0 |
| `wasm-bindgen-shared` | 0.2.128 | MIT OR Apache-2.0 |
| `wasm-streams` | 0.5.0 | MIT OR Apache-2.0 |
| `web-sys` | 0.3.105 | MIT OR Apache-2.0 |
| `windows` | 0.62.2 | MIT OR Apache-2.0 |
| `windows-collections` | 0.3.2 | MIT OR Apache-2.0 |
| `windows-core` | 0.62.2 | MIT OR Apache-2.0 |
| `windows-future` | 0.3.2 | MIT OR Apache-2.0 |
| `windows-implement` | 0.60.2 | MIT OR Apache-2.0 |
| `windows-interface` | 0.59.3 | MIT OR Apache-2.0 |
| `windows-link` | 0.2.1 | MIT OR Apache-2.0 |
| `windows-numerics` | 0.3.1 | MIT OR Apache-2.0 |
| `windows-result` | 0.4.1 | MIT OR Apache-2.0 |
| `windows-strings` | 0.5.1 | MIT OR Apache-2.0 |
| `windows-sys` | 0.61.2 | MIT OR Apache-2.0 |
| `windows-threading` | 0.2.1 | MIT OR Apache-2.0 |
| `winnow` | 1.0.4 | MIT |
| `winresource` | 0.1.31 | MIT |
| `writeable` | 0.6.4 | Unicode-3.0 |
| `yoke` | 0.8.3 | Unicode-3.0 |
| `yoke-derive` | 0.8.3 | Unicode-3.0 |
| `zerofrom` | 0.1.8 | Unicode-3.0 |
| `zerofrom-derive` | 0.1.8 | Unicode-3.0 |
| `zerotrie` | 0.2.5 | Unicode-3.0 |
| `zerovec` | 0.11.8 | Unicode-3.0 |
| `zerovec-derive` | 0.11.6 | Unicode-3.0 |
| `zmij` | 1.0.23 | MIT |

## 4. 备注（许可取舍台账）

- 主项目 GPL-3.0 × VC-LTL5 EPL-2.0 的组合问题属 **DG15**（用户拍板项）；本文件按方案既定口径
  先落地"**如实声明**"，不代替用户做法律判断。
- 若最终不接受该组合，Win7 免安装路线只能退到"要求目标机安装 UCRT"（牺牲"拷来即用"）——
  该取舍记录在方案 §8.3 / DG15。

## 5. 发布说明模板（GitHub Release 描述用 · 发布时原文粘贴）

> 本节即方案 §5.6.5 第 10 条所指的 "Release 说明" 内容（两库许可文本 + 署名）；发布时原文粘贴即可。

```text
本发布包含 AzusaAI 本地反向代理（rust-proxy/，GPL-3.0，见仓库根 LICENSE）。

其中 Windows 7 兼容构建（azusa-local-proxy-win7-x64.exe）链接了以下第三方组件：

- VC-LTL5 v5.3.1（Chuyu-Team）—— Eclipse Public License 2.0（EPL-2.0），Windows 7 兼容第 1 层（CRT 替换库）。
  许可证全文见 licenses/VC-LTL5-EPL-2.0.txt；项目主页 https://github.com/Chuyu-Team/VC-LTL5
- YY-Thunks v1.2.2（Chuyu-Team）—— MIT License，Windows 7 兼容第 2 层（API thunk 桩）。
  许可证全文见 licenses/YY-Thunks-MIT.txt；项目主页 https://github.com/Chuyu-Team/YY-Thunks

完整第三方声明（含 134 个 Rust 依赖 crate 的许可清单与所用文件清单）见 THIRD_PARTY_NOTICES.md。
```
