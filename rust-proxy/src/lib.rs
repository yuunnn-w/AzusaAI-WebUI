//! AzusaAI 本地反向代理 —— **库面**（`main.rs` 与 `tests/` 共用）。
//!
//! ⚠ 为什么有这个 lib（方案 §8.2 的结构上**新增** `lib.rs` / `server.rs`，已在 Phase 1 报告登记）：
//! `tests/proxy_it.rs`（方案 §10.2 的集成套件）需要**在进程内**起反代、注入停止信号与 mock 上游
//! （尤其 §10.2 I3「流式总时长不受任何总超时约束」与 FR-34 优雅停机需要可控的触发点）。
//! Rust 的集成测试**无法** `use` 二进制 crate 的任何模块（`tests/` 只能链接 lib target）⇒ 把可测面
//! 抽到 lib；`main.rs` 只留 CLI 解析、单实例、信号接线与错误出口（进程语义那一层）。
//!
//! 请求处理路径的「零 panic」硬规格（方案 §6.6 / §8.6 1a）作用域不变：`proxy/**` 与 `stats.rs`
//! 各自用**模块级内属性**收口，与 lib/bin 拆分无关。

pub mod cli;
pub mod config;
pub mod logging;
pub mod osver;
pub mod output;
pub mod proxy;
pub mod server;
pub mod service;
pub mod stats;
