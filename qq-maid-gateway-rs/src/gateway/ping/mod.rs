//! Gateway 本地 `/ping` 诊断入口。
//!
//! 该模块只负责识别命令、采集 auth / healthz 快照并编排渲染；
//! 运行事实、健康评估、Markdown 展示和 LLM healthz 探测分别放在子模块中。

mod assess;
mod healthz;
mod render;
mod status;
mod time;

#[cfg(test)]
mod tests;

use crate::{auth::AccessTokenManager, config::AppConfig, gateway::event::C2cMessage};

use self::{healthz::probe_llm_healthz, render::render_c2c_ping_reply};

pub use self::status::{GatewayRuntimeSnapshot, GatewayRuntimeStatus, InvalidSessionSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PingMode {
    Summary,
    All,
}

pub fn is_ping_command(text: &str) -> bool {
    parse_ping_mode(text).is_some()
}

fn parse_ping_mode(text: &str) -> Option<PingMode> {
    let mut parts = text.split_whitespace();
    let command = parts.next()?;
    if !command.eq_ignore_ascii_case("/ping") {
        return None;
    }
    match (parts.next(), parts.next()) {
        (None, None) => Some(PingMode::Summary),
        (Some(arg), None) if arg.eq_ignore_ascii_case("all") => Some(PingMode::All),
        _ => None,
    }
}

pub async fn build_c2c_ping_reply(
    message: &C2cMessage,
    config: &AppConfig,
    runtime: &GatewayRuntimeStatus,
    auth: &AccessTokenManager,
) -> String {
    let token_snapshot = auth.snapshot().await;
    let llm_health = probe_llm_healthz(&config.respond_url).await;
    render_c2c_ping_reply(message, config, runtime, &token_snapshot, &llm_health)
}
