use std::time::Duration;

use crate::gateway::logging::mask_url;

const LLM_HEALTHZ_TIMEOUT: Duration = Duration::from_millis(800);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LlmHealthSnapshot {
    pub(super) healthz_url: String,
    pub(super) status: String,
}

pub(super) async fn probe_llm_healthz(respond_url: &str) -> LlmHealthSnapshot {
    let Ok(healthz_url) = healthz_url_from_respond_url(respond_url) else {
        return LlmHealthSnapshot {
            healthz_url: "invalid url".to_owned(),
            status: "invalid url".to_owned(),
        };
    };
    let healthz_url_text = mask_url(healthz_url.as_str());

    let client = match reqwest::Client::builder()
        .timeout(LLM_HEALTHZ_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(_) => {
            return LlmHealthSnapshot {
                healthz_url: healthz_url_text,
                status: "client build failed".to_owned(),
            };
        }
    };

    match client.get(healthz_url.clone()).send().await {
        Ok(response) => {
            let status = response.status();
            let summary = if status.is_success() {
                format!("ok(status={})", status.as_u16())
            } else {
                format!("http status {}", status.as_u16())
            };
            LlmHealthSnapshot {
                healthz_url: healthz_url_text,
                status: summary,
            }
        }
        Err(error) => LlmHealthSnapshot {
            healthz_url: healthz_url_text,
            status: healthz_error_summary(&error),
        },
    }
}

pub(super) fn llm_health_ok(llm_health: &LlmHealthSnapshot) -> bool {
    llm_health.status.starts_with("ok(status=")
}

pub(super) fn healthz_status_detail(llm_health: &LlmHealthSnapshot) -> String {
    llm_health
        .status
        .strip_prefix("ok(status=")
        .and_then(|rest| rest.strip_suffix(')'))
        .map(|status| format!("healthz {status}"))
        .unwrap_or_else(|| format!("healthz {}", llm_health.status))
}

fn healthz_url_from_respond_url(respond_url: &str) -> Result<reqwest::Url, ()> {
    let mut url = reqwest::Url::parse(respond_url.trim()).map_err(|_| ())?;
    url.set_path("/healthz");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn healthz_error_summary(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "timeout".to_owned()
    } else if error.is_connect() {
        "connect failed".to_owned()
    } else if error.is_request() {
        "request failed".to_owned()
    } else {
        "healthz failed".to_owned()
    }
}
