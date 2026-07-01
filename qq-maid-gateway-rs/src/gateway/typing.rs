//! QQ C2C 原生“正在输入”状态的请求级生命周期。
//!
//! 这里只发送平台状态，不生成聊天文本，也不参与 Core 的工具成功判定。

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use tokio::{task::JoinHandle, time::sleep};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use super::{
    event::C2cMessage,
    logging::{mask_identifier, mask_openid},
};
use crate::{api::QqApiClient, config::AgentTypingConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TypingStopReason {
    FirstFrame,
    FinalReply,
    RequestFailed,
    Timeout,
    Cancelled,
}

impl TypingStopReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::FirstFrame => "first_frame",
            Self::FinalReply => "final_reply",
            Self::RequestFailed => "request_failed",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug)]
pub(super) struct C2cTypingStatusGuard {
    token: CancellationToken,
    sent: Arc<AtomicBool>,
    stopped: bool,
    task: JoinHandle<()>,
    masked_user: String,
    source_message_id: String,
    delay_ms: u128,
    started_at: Instant,
}

impl C2cTypingStatusGuard {
    pub(super) fn schedule(
        config: &AgentTypingConfig,
        api: QqApiClient,
        message: &C2cMessage,
        transport: &'static str,
    ) -> Option<Self> {
        if !config.enabled {
            return None;
        }
        let token = CancellationToken::new();
        let task_token = token.clone();
        let sent = Arc::new(AtomicBool::new(false));
        let task_sent = sent.clone();
        let delay = config.delay;
        let delay_ms = delay.as_millis();
        let user_openid = message.user_openid.clone();
        let msg_id = message.message_id.clone();
        let masked_user = mask_openid(&user_openid);
        let source_message_id = mask_identifier(&msg_id);
        let task_masked_user = masked_user.clone();
        let task_source_message_id = source_message_id.clone();
        let started_at = Instant::now();

        debug!(
            user = %masked_user,
            source_message_id = %source_message_id,
            transport,
            delay_ms,
            "typing_status_scheduled"
        );

        let task = tokio::spawn(async move {
            tokio::select! {
                _ = task_token.cancelled() => {
                    debug!(
                        user = %task_masked_user,
                        source_message_id = %task_source_message_id,
                        transport,
                        delay_ms,
                        elapsed_ms = started_at.elapsed().as_millis(),
                        "typing_status_cancelled"
                    );
                }
                _ = sleep(delay) => {
                    match api.send_c2c_typing(&user_openid, Some(&msg_id)).await {
                        Ok(_) => {
                            task_sent.store(true, Ordering::SeqCst);
                            debug!(
                                user = %task_masked_user,
                                source_message_id = %task_source_message_id,
                                transport,
                                delay_ms,
                                elapsed_ms = started_at.elapsed().as_millis(),
                                "typing_status_sent"
                            );
                        }
                        Err(error) => {
                            warn!(
                                user = %task_masked_user,
                                source_message_id = %task_source_message_id,
                                transport,
                                delay_ms,
                                elapsed_ms = started_at.elapsed().as_millis(),
                                error = %error.log_summary(),
                                "typing_status_send_failed"
                            );
                        }
                    }
                }
            }
        });

        Some(Self {
            token,
            sent,
            stopped: false,
            task,
            masked_user,
            source_message_id,
            delay_ms,
            started_at,
        })
    }

    pub(super) fn stop(&mut self, reason: TypingStopReason) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.token.cancel();
        debug!(
            user = %self.masked_user,
            source_message_id = %self.source_message_id,
            delay_ms = self.delay_ms,
            elapsed_ms = self.started_at.elapsed().as_millis(),
            sent = self.sent.load(Ordering::SeqCst),
            stop_reason = reason.as_str(),
            "typing_status_stopped"
        );
    }
}

impl Drop for C2cTypingStatusGuard {
    fn drop(&mut self) {
        if !self.stopped {
            self.token.cancel();
        }
        self.task.abort();
    }
}
