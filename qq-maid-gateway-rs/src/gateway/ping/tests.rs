use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

use crate::{
    auth::{AccessTokenManager, AccessTokenSnapshot, AccessTokenSnapshotState},
    config::AppConfig,
    gateway::event::C2cMessage,
};

use super::{
    GatewayRuntimeStatus, PingMode, build_c2c_ping_reply,
    healthz::{LlmHealthSnapshot, probe_llm_healthz},
    is_ping_command,
    render::render_c2c_ping_reply_at,
};

fn config() -> AppConfig {
    AppConfig {
        app_id: "appid".to_owned(),
        app_secret: "app-secret-value".to_owned(),
        sandbox: false,
        api_base: "https://api.sgroup.qq.com".to_owned(),
        token_refresh_margin: Duration::from_secs(60),
        respond_url: "http://127.0.0.1:8787/v1/respond?debug=1&token=real-token&timeout=800"
            .to_owned(),
        enable_markdown: false,
        enable_image: true,
        verbose_log: false,
        push_enabled: true,
        push_host: "127.0.0.1".to_owned(),
        push_port: 8788,
        push_token: None,
    }
}

fn message() -> C2cMessage {
    C2cMessage {
        message_id: "msg-sensitive-123456".to_owned(),
        user_openid: "user-openid-123456".to_owned(),
        content: "/ping".to_owned(),
        reply: None,
        timestamp: Some("2026-06-10T12:00:00+08:00".to_owned()),
        attachments: Vec::new(),
    }
}

fn message_with_content(content: &str) -> C2cMessage {
    C2cMessage {
        content: content.to_owned(),
        ..message()
    }
}

fn token_snapshot() -> AccessTokenSnapshot {
    AccessTokenSnapshot {
        state: AccessTokenSnapshotState::Cached,
        expires_in_seconds: Some(120),
        refresh_margin_seconds: 60,
    }
}

fn health(status: &str) -> LlmHealthSnapshot {
    LlmHealthSnapshot {
        healthz_url: "http://127.0.0.1:8787/healthz".to_owned(),
        status: status.to_owned(),
    }
}

fn spawn_one_response_server(response: &'static [u8]) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut buffer = [0; 1024];
        let _ = stream.read(&mut buffer);
        let _ = stream.write_all(response);
    });
    format!("http://{addr}/v1/respond?token=server-token&debug=1")
}

#[test]
fn detects_ping_command_case_insensitively() {
    assert!(is_ping_command(" /PING "));
    assert!(is_ping_command(" /ping all "));
    assert!(is_ping_command(" /PING ALL "));
    assert!(!is_ping_command("/ping now"));
    assert!(!is_ping_command("/ping all extra"));
}

#[test]
fn runtime_records_recent_events_without_full_message_id() {
    let runtime = GatewayRuntimeStatus::new_for_test();
    let message = message();

    runtime.record_gateway_connected();
    runtime.record_ready();
    runtime.record_resumed();
    runtime.record_heartbeat_ack();
    runtime.record_reconnect();
    runtime.record_invalid_session(false);
    runtime.record_c2c_message_received(&message);
    runtime.record_respond_success();
    runtime.record_respond_failure("http status 500\nwith details");
    runtime.record_qq_send_success();
    runtime.record_qq_send_failure("request failed timeout with a long but safe summary");

    let snapshot = runtime.snapshot();

    assert!(snapshot.last_gateway_connected_at.is_some());
    assert!(snapshot.last_ready_at.is_some());
    assert!(snapshot.last_resumed_at.is_some());
    assert!(snapshot.last_heartbeat_ack_at.is_some());
    assert!(snapshot.last_reconnect_at.is_some());
    assert_eq!(
        snapshot
            .last_invalid_session
            .as_ref()
            .map(|item| item.can_resume),
        Some(false)
    );
    assert_eq!(
        snapshot.last_c2c_message_id.as_deref(),
        Some("******123456")
    );
    assert!(snapshot.last_respond_success_at.is_some());
    assert_eq!(
        snapshot.last_respond_failure_summary.as_deref(),
        Some("http status 500 with details")
    );
    assert!(snapshot.last_qq_send_success_at.is_some());
    assert!(snapshot.last_qq_send_failure_at.is_some());
}

#[test]
fn renders_summary_ping_reply_without_debug_noise_or_secrets() {
    let runtime = GatewayRuntimeStatus::new_for_test();
    runtime.update_state(|state| {
        state.last_gateway_connected_at = Some("unix:1000".to_owned());
        state.last_ready_at = Some("unix:1001".to_owned());
        state.last_heartbeat_ack_at = Some("unix:1190".to_owned());
        state.last_c2c_received_at = Some("unix:1200".to_owned());
        state.last_qq_send_success_at = Some("unix:1180".to_owned());
        state.last_respond_success_at = Some("unix:1170".to_owned());
    });

    let reply = render_c2c_ping_reply_at(
        &message(),
        &config(),
        &runtime,
        &token_snapshot(),
        &health("ok(status=200)"),
        PingMode::Summary,
        1200,
    );

    assert!(reply.contains("# 🟢 服务运行正常"));
    assert!(reply.contains("## 核心链路"));
    assert!(reply.contains("| Gateway | 🟢 正常 | 已运行 "));
    assert!(reply.contains("| QQ 连接 | 🟢 已连接 | WebSocket 已连接于 3分钟20秒前 |"));
    assert!(reply.contains("| 心跳 | 🟢 正常 | 10秒前收到 ACK |"));
    assert!(reply.contains("| LLM | 🟢 正常 | healthz 200 |"));
    assert!(reply.contains("| 消息接收 | 🟢 正常 | 刚刚收到当前消息 |"));
    assert!(reply.contains("| 消息发送 | 🟢 正常 | 最近一次发送尝试成功于 20秒前 |"));
    assert!(reply.contains("- 未发现发送、LLM 或 Session 异常"));
    assert!(reply.contains("| 接收时间 | 2026-06-10 12:00:00 +08:00 |"));
    assert!(!reply.contains("## 调试详情"));
    assert!(!reply.contains("pid："));
    assert!(!reply.contains("instance："));
    assert!(!reply.contains("respond_url："));
    assert!(!reply.contains("当前用户："));
    assert!(!reply.contains("scope_key"));
    assert!(!reply.contains("unix:"));
    assert!(!reply.contains("user-openid-123456"));
    assert!(!reply.contains("msg-sensitive-123456"));
    assert!(!reply.contains("real-token"));
    assert!(!reply.contains("app-secret-value"));
    assert!(!reply.contains("Authorization"));
}

#[test]
fn renders_ping_all_with_debug_details_without_secrets() {
    let runtime = GatewayRuntimeStatus::new_for_test();
    runtime.update_state(|state| {
        state.last_gateway_connected_at = Some("unix:1000".to_owned());
        state.last_ready_at = Some("unix:1001".to_owned());
        state.last_resumed_at = Some("unix:1105".to_owned());
        state.last_heartbeat_ack_at = Some("unix:1190".to_owned());
        state.last_reconnect_at = Some("unix:1100".to_owned());
        state.last_c2c_received_at = Some("unix:1200".to_owned());
        state.last_c2c_message_id = Some("******123456".to_owned());
        state.last_respond_failure_at = Some("unix:1140".to_owned());
        state.last_respond_failure_summary = Some("connect failed".to_owned());
        state.last_respond_success_at = Some("unix:1170".to_owned());
        state.last_qq_send_failure_at = Some("unix:1130".to_owned());
        state.last_qq_send_failure_summary = Some("http status 429".to_owned());
        state.last_qq_send_success_at = Some("unix:1180".to_owned());
    });

    let reply = render_c2c_ping_reply_at(
        &message_with_content("/ping all"),
        &config(),
        &runtime,
        &token_snapshot(),
        &health("ok(status=200)"),
        PingMode::All,
        1200,
    );

    assert!(reply.contains("# 🟡 服务可用，但存在警告"));
    assert!(reply.contains("5秒后恢复"));
    assert!(reply.contains("QQ 发送曾失败，最近一次尝试成功于 20秒前"));
    assert!(reply.contains("LLM respond曾失败，最近一次尝试成功于 30秒前"));
    assert!(reply.contains("## 调试详情"));
    assert!(reply.contains("### 概览"));
    assert!(reply.contains("### Gateway"));
    assert!(reply.contains("### 消息"));
    assert!(reply.contains("### 发送"));
    assert!(reply.contains("### LLM"));
    assert!(reply.contains("### 配置"));
    assert!(reply.contains("LLM healthz：ok(status=200)"));
    assert!(reply.contains("当前时间："));
    assert!(reply.contains("+08:00 (unix:"));
    assert!(reply.contains("started_at：1970-01-01 08:00:01 +08:00 (unix:1)"));
    assert!(reply.contains("当前消息时间：2026-06-10 12:00:00 +08:00"));
    assert!(!reply.contains("当前消息时间：2026-06-10T12:00:00+08:00"));
    assert!(!reply.contains("最近收到：unix:"));
    assert!(!reply.contains("最近 respond 失败：unix:"));
    assert!(!reply.contains("最近 QQ 发送失败：unix:"));
    assert!(reply.contains("respond query：debug=1&token=***&timeout=800"));
    assert!(reply.contains("当前用户：******123456"));
    assert!(reply.contains("当前 scope_key：private:******123456"));
    assert!(reply.contains("访问令牌缓存：cached, expires_in=120s"));
    assert!(!reply.contains("记忆模块"));
    assert!(!reply.contains("存储"));
    assert!(!reply.contains("user-openid-123456"));
    assert!(!reply.contains("msg-sensitive-123456"));
    assert!(!reply.contains("real-token"));
    assert!(!reply.contains("app-secret-value"));
    assert!(!reply.contains("Authorization"));
}

#[tokio::test]
async fn build_ping_degrades_invalid_llm_url_to_field_summary() {
    let mut config = config();
    config.respond_url = "http://".to_owned();
    let runtime = GatewayRuntimeStatus::new_for_test();
    let auth = AccessTokenManager::new(
        reqwest::Client::new(),
        "appid",
        "app-secret-value",
        Duration::from_secs(60),
    );

    let reply = build_c2c_ping_reply(&message(), &config, &runtime, &auth).await;

    assert!(reply.contains("# 🔴 服务异常"));
    assert!(reply.contains("| LLM | 🔴 异常 | healthz invalid url |"));
    assert!(reply.contains("LLM healthz 异常：invalid url"));
    assert!(!reply.contains("respond_url：invalid url"));
    assert!(!reply.contains("访问令牌缓存：empty"));
    assert!(!reply.contains("app-secret-value"));
}

#[tokio::test]
async fn llm_healthz_probe_reports_http_status_and_masks_url() {
    let respond_url =
        spawn_one_response_server(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n");

    let result = probe_llm_healthz(&respond_url).await;

    assert_eq!(result.status, "http status 503");
    assert!(result.healthz_url.ends_with("/healthz"));
    assert!(!result.healthz_url.contains("server-token"));
}
