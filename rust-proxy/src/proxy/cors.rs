//! CORS 层（方案 §6.3 FR-9～FR-13）：预检本地应答 + 所有响应 set/replace 注入头部。
//! 全部为纯函数（便于单测 U3/U8）。
//!
//! ⚠ 本文件在「请求路径零 panic」硬规格作用域内（方案 §6.6 / §8.6 1a；
//! 由 `proxy/mod.rs` 的模块级内属性覆盖其子模块）。

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::Response;

use crate::config::CorsConfig;

/// FR-10 固定清单（预检请求未带 `Access-Control-Request-Headers` 时的缺省回显值）：
/// 覆盖应用的三条请求面 —— 聊天 / 压缩 / 外部 MCP。
pub const DEFAULT_ALLOW_HEADERS: &str =
    "Content-Type, Authorization, x-api-key, anthropic-version, \
     anthropic-dangerous-direct-browser-access, Accept, Mcp-Session-Id, MCP-Protocol-Version";

/// FR-12：需要被 JS 读到的响应头（显式清单比 `*` 的浏览器兼容性更稳）。
pub const EXPOSE_HEADERS: &str = "Mcp-Session-Id, MCP-Protocol-Version";

/// FR-11：上游 `Access-Control-*` 同名族（先整体剥离，再写入代理计算值 ⇒ 每族恰好一次）。
const CORS_FAMILY: [&str; 7] = [
    "access-control-allow-origin",
    "access-control-allow-methods",
    "access-control-allow-headers",
    "access-control-expose-headers",
    "access-control-max-age",
    "access-control-allow-credentials",
    "access-control-allow-private-network",
];

/// FR-9 判定式：**含 `Origin`** 且 **含 `Access-Control-Request-Method`** ⇒ 预检；
/// 缺任一条件 ⇒ 按普通请求转发（不吞、不漏 —— U8 / Phase 0 DoD ①b）。
pub fn is_preflight(method: &Method, headers: &HeaderMap) -> bool {
    method == Method::OPTIONS
        && headers.contains_key(header::ORIGIN)
        && headers.contains_key(header::ACCESS_CONTROL_REQUEST_METHOD)
}

/// 预检响应：本地 `204` + 全套 CORS 头；**不转发上游**（FR-9）。
pub fn preflight_response(request_headers: &HeaderMap, config: &CorsConfig) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    let headers = response.headers_mut();

    apply_cors_headers(headers, request_headers, config);
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, OPTIONS"),
    );
    let allow_headers = request_headers
        .get(header::ACCESS_CONTROL_REQUEST_HEADERS)
        .cloned()
        .unwrap_or_else(|| HeaderValue::from_static(DEFAULT_ALLOW_HEADERS));
    headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, allow_headers);
    if let Ok(value) = HeaderValue::from_str(&config.max_age_seconds.to_string()) {
        headers.insert(header::ACCESS_CONTROL_MAX_AGE, value);
    }
    // FR-13：PNA 兜底（Chromium 私有网络预检）—— 仅当请求明确要求时才回。
    let wants_private_network = request_headers
        .get(HeaderName::from_static(
            "access-control-request-private-network",
        ))
        .and_then(|value| value.to_str().ok())
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if wants_private_network {
        headers.insert(
            HeaderName::from_static("access-control-allow-private-network"),
            HeaderValue::from_static("true"),
        );
    }
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
    response
}

/// FR-11：对**所有**实际响应（含 4xx/5xx 与代理自身的 502）注入 CORS 头。
/// 三条写死：① 先剥离上游 `Access-Control-*` 同名族；② 写入代理计算值（每族恰好一份，
/// "追加"会产出两个 `Access-Control-Allow-Origin` ⇒ 浏览器一律拒绝）；
/// ③ `echo` 模式下回显请求 `Origin` 并保留 `Vary: Origin`。
pub fn apply_cors_headers(
    response_headers: &mut HeaderMap,
    request_headers: &HeaderMap,
    config: &CorsConfig,
) {
    for name in CORS_FAMILY {
        response_headers.remove(name);
    }

    if config.mode == "echo" {
        if let Some(origin) = request_headers.get(header::ORIGIN) {
            response_headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin.clone());
            response_headers.append(header::VARY, HeaderValue::from_static("Origin"));
        } else {
            response_headers.insert(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("*"),
            );
        }
    } else {
        response_headers.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
    }

    response_headers.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static(EXPOSE_HEADERS),
    );
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            let name = HeaderName::from_bytes(name.as_bytes()).unwrap();
            map.append(name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    /// U8：OPTIONS 接管判定式四分支（方案 §10.1 U8；对应 Phase 0 DoD ①/①b 的正负配对）。
    #[test]
    fn u8_preflight_decision_matrix() {
        let origin = ("Origin", "null");
        let acrm = ("access-control-request-method", "POST");
        assert!(is_preflight(&Method::OPTIONS, &headers(&[origin, acrm])));
        assert!(!is_preflight(&Method::OPTIONS, &headers(&[origin])));
        assert!(!is_preflight(&Method::OPTIONS, &headers(&[acrm])));
        assert!(!is_preflight(&Method::OPTIONS, &headers(&[])));
        // 非 OPTIONS 方法即使带全头也不判预检（普通请求照常转发）
        assert!(!is_preflight(&Method::POST, &headers(&[origin, acrm])));
        // 重复 Origin 不得误判
        assert!(is_preflight(
            &Method::OPTIONS,
            &headers(&[origin, origin, acrm])
        ));
    }

    /// U3（预检面）：204 + 全套头；缺省 allow-headers 用固定清单；PNA 兜底。
    #[test]
    fn u3_preflight_response_headers() {
        let config = CorsConfig {
            mode: "*".to_string(),
            max_age_seconds: 600,
        };
        let request = headers(&[
            ("Origin", "null"),
            ("Access-Control-Request-Method", "POST"),
            (
                "Access-Control-Request-Headers",
                "content-type, authorization",
            ),
        ]);
        let response = preflight_response(&request, &config);
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let out = response.headers();
        assert_eq!(out.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(), "*");
        assert_eq!(
            out.get(header::ACCESS_CONTROL_ALLOW_METHODS).unwrap(),
            "GET, POST, OPTIONS"
        );
        assert_eq!(
            out.get(header::ACCESS_CONTROL_ALLOW_HEADERS).unwrap(),
            "content-type, authorization"
        );
        assert_eq!(out.get(header::ACCESS_CONTROL_MAX_AGE).unwrap(), "600");
        assert_eq!(out.get(header::CONTENT_LENGTH).unwrap(), "0");
        assert_eq!(out.get(header::VARY), None);

        let bare = headers(&[
            ("Origin", "null"),
            ("Access-Control-Request-Method", "POST"),
        ]);
        let response = preflight_response(&bare, &config);
        assert_eq!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
                .unwrap(),
            DEFAULT_ALLOW_HEADERS
        );

        let pna = headers(&[
            ("Origin", "null"),
            ("Access-Control-Request-Method", "POST"),
            ("Access-Control-Request-Private-Network", "true"),
        ]);
        let response = preflight_response(&pna, &config);
        assert_eq!(
            response
                .headers()
                .get("access-control-allow-private-network")
                .unwrap(),
            "true"
        );
        // 未请求 PNA 时不得回该头（避免多余头）
        let response = preflight_response(&bare, &config);
        assert_eq!(
            response
                .headers()
                .get("access-control-allow-private-network"),
            None
        );
    }

    /// U3（实际响应面，P1-5 的三条断言）：上游已带不匹配值 + 同名族 ⇒ 被整体剥离、
    /// 代理值恰好一份；错误状态码同样成立（函数与状态码无关）。
    #[test]
    fn u3_actual_response_set_replace_semantics() {
        let config = CorsConfig {
            mode: "*".to_string(),
            max_age_seconds: 600,
        };
        let request = headers(&[("Origin", "null")]);

        let mut upstream = HeaderMap::new();
        upstream.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("http://example.com"),
        );
        upstream.append(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("http://other.example"),
        );
        upstream.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("DELETE"),
        );
        upstream.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("1"),
        );
        upstream.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));

        apply_cors_headers(&mut upstream, &request, &config);

        let acao: Vec<_> = upstream
            .get_all(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .iter()
            .collect();
        assert_eq!(
            acao.len(),
            1,
            "最终必须恰好一个 Access-Control-Allow-Origin"
        );
        assert_eq!(acao[0], "*", "不得保留上游的不匹配值");
        assert_eq!(
            upstream.get(header::ACCESS_CONTROL_ALLOW_METHODS),
            None,
            "上游同名族必须被剥离"
        );
        assert_eq!(upstream.get(header::ACCESS_CONTROL_MAX_AGE), None);
        assert_eq!(
            upstream.get(header::ACCESS_CONTROL_EXPOSE_HEADERS).unwrap(),
            EXPOSE_HEADERS
        );
        assert_eq!(
            upstream.get(header::VARY).unwrap(),
            "Accept-Encoding",
            "通配模式不得改动上游 Vary"
        );
    }

    /// U3（echo 面）：回显 Origin + `Vary: Origin`；无 Origin 时回退 `*` 且不写 Vary。
    #[test]
    fn u3_echo_mode_varies_on_origin() {
        let config = CorsConfig {
            mode: "echo".to_string(),
            max_age_seconds: 600,
        };
        let request = headers(&[("Origin", "null")]);
        let mut out = HeaderMap::new();
        apply_cors_headers(&mut out, &request, &config);
        assert_eq!(
            out.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
            "null"
        );
        assert_eq!(out.get(header::VARY).unwrap(), "Origin");

        let mut out = HeaderMap::new();
        apply_cors_headers(&mut out, &HeaderMap::new(), &config);
        assert_eq!(out.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(), "*");
        assert_eq!(out.get(header::VARY), None);
    }
}
