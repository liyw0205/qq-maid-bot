//! QQ 引用消息短时索引。
//!
//! 这里只保存平台归一化后的消息摘要，解决 QQ `REFIDX_*` 无法回查原文的问题。
//! 当前实现为进程内缓存，重启后历史引用会失效；业务上下文组装仍由 Core 完成。

pub(crate) mod qq;

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};

use qq_maid_common::input_part::{MessageInputPart, QuotedMediaSummary};

use super::platform::{ConversationTarget, InboundMessage};

pub(crate) type SharedRefIndex = Arc<Mutex<RefIndex>>;

const MAX_REF_ENTRIES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RefIndexKey {
    platform: String,
    app_id: String,
    peer_kind: String,
    peer_id: String,
    ref_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefIndexEntry {
    pub(crate) text_summary: Option<String>,
    pub(crate) media_summaries: Vec<QuotedMediaSummary>,
    pub(crate) input_parts: Vec<MessageInputPart>,
    pub(crate) from_bot: bool,
    pub(crate) timestamp: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct RefIndex {
    entries: HashMap<RefIndexKey, RefIndexEntry>,
    order: VecDeque<RefIndexKey>,
}

impl RefIndex {
    pub(crate) fn insert_inbound(&mut self, inbound: &InboundMessage) {
        let entry = entry_from_inbound(inbound);
        for ref_id in ref_ids_for_current_message(inbound) {
            self.insert(inbound, ref_id, entry.clone());
        }
    }

    pub(crate) fn insert_bot_outbound(
        &mut self,
        platform: super::platform::Platform,
        account_id: Option<&str>,
        conversation: &ConversationTarget,
        message_id: Option<String>,
        text: &str,
    ) {
        let Some(message_id) = clean_optional(message_id) else {
            return;
        };
        let entry = RefIndexEntry {
            text_summary: clean_optional(Some(text.to_owned())),
            media_summaries: Vec::new(),
            input_parts: if text.trim().is_empty() {
                Vec::new()
            } else {
                vec![MessageInputPart::text(text.to_owned())]
            },
            from_bot: true,
            timestamp: None,
        };
        let key = key_for(platform, account_id, conversation, &message_id);
        self.insert_key(key, entry);
    }

    pub(crate) fn enrich_inbound(&self, inbound: &mut InboundMessage) {
        let Some(quoted) = inbound.quoted.as_mut() else {
            return;
        };
        let Some(ref_id) = quoted
            .ref_msg_idx
            .as_deref()
            .or(quoted.reference_id.as_deref())
            .map(str::to_owned)
        else {
            quoted.lookup_found = false;
            quoted.fallback_reason = Some("missing_reference_id".to_owned());
            return;
        };
        let key = key_for(
            inbound.platform,
            inbound.account_id.as_deref(),
            &inbound.conversation,
            &ref_id,
        );
        if let Some(entry) = self.entries.get(&key) {
            quoted.lookup_found = true;
            quoted.text_summary = entry.text_summary.clone();
            quoted.media_summaries = entry.media_summaries.clone();
            quoted.input_parts = entry.input_parts.clone();
            quoted.from_bot = Some(entry.from_bot);
            quoted.fallback_reason = None;
        } else {
            quoted.lookup_found = false;
            quoted.fallback_reason = Some("ref_index_miss".to_owned());
        }
    }

    fn insert(&mut self, inbound: &InboundMessage, ref_id: String, entry: RefIndexEntry) {
        let key = key_for(
            inbound.platform,
            inbound.account_id.as_deref(),
            &inbound.conversation,
            &ref_id,
        );
        self.insert_key(key, entry);
    }

    fn insert_key(&mut self, key: RefIndexKey, entry: RefIndexEntry) {
        if !self.entries.contains_key(&key) {
            self.order.push_back(key.clone());
        }
        self.entries.insert(key, entry);
        while self.entries.len() > MAX_REF_ENTRIES {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }
    }
}

pub(crate) fn ref_index() -> SharedRefIndex {
    Arc::new(Mutex::new(RefIndex::default()))
}

fn ref_ids_for_current_message(inbound: &InboundMessage) -> Vec<String> {
    [
        Some(inbound.message_id.as_str()),
        inbound.current_msg_idx.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter_map(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
    .collect()
}

fn entry_from_inbound(inbound: &InboundMessage) -> RefIndexEntry {
    let text_summary = clean_optional(Some(inbound.text.clone()));
    let input_parts = effective_index_parts(inbound);
    let media_summaries = input_parts
        .iter()
        .filter_map(QuotedMediaSummary::from_input_part)
        .collect::<Vec<_>>();
    RefIndexEntry {
        text_summary,
        media_summaries,
        input_parts,
        from_bot: inbound.actor.is_bot,
        timestamp: inbound.timestamp.clone(),
    }
}

fn effective_index_parts(inbound: &InboundMessage) -> Vec<MessageInputPart> {
    if !inbound.input_parts.is_empty() {
        return inbound.input_parts.clone();
    }
    let mut parts = Vec::new();
    if !inbound.text.trim().is_empty() {
        parts.push(MessageInputPart::text(inbound.text.clone()));
    }
    parts.extend(
        inbound
            .attachments
            .iter()
            .map(|attachment| attachment.to_input_part(inbound.platform)),
    );
    parts
}

fn key_for(
    platform: super::platform::Platform,
    account_id: Option<&str>,
    conversation: &ConversationTarget,
    ref_id: &str,
) -> RefIndexKey {
    RefIndexKey {
        platform: platform.as_str().to_owned(),
        app_id: account_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("-")
            .to_owned(),
        peer_kind: conversation.kind().to_owned(),
        peer_id: conversation.target_id().to_owned(),
        ref_id: ref_id.to_owned(),
    }
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use qq_maid_common::input_part::{MessageMedia, QuotedMessageContext};

    fn inbound(message_id: &str, msg_idx: Option<&str>, text: &str) -> InboundMessage {
        InboundMessage {
            platform: super::super::platform::Platform::QqOfficial,
            account_id: Some("app".to_owned()),
            conversation: ConversationTarget::Private {
                target_id: "user-1".to_owned(),
            },
            actor: super::super::platform::Actor {
                sender_id: Some("user-1".to_owned()),
                display_name: None,
                group_member_role: None,
                is_bot: false,
            },
            message_id: message_id.to_owned(),
            current_msg_idx: msg_idx.map(str::to_owned),
            timestamp: None,
            text: text.to_owned(),
            input_parts: vec![MessageInputPart::text(text.to_owned())],
            attachments: Vec::new(),
            quoted: None,
            mentioned_bot: false,
        }
    }

    #[test]
    fn index_isolated_by_peer_and_fills_quote_context() {
        let mut store = RefIndex::default();
        store.insert_inbound(&inbound("m1", Some("REFIDX_1"), "上一条"));
        let mut current = inbound("m2", Some("REFIDX_2"), "继续");
        current.quoted = Some(QuotedMessageContext {
            current_message_id: Some("m2".to_owned()),
            current_msg_idx: Some("REFIDX_2".to_owned()),
            ref_msg_idx: Some("REFIDX_1".to_owned()),
            ..Default::default()
        });

        store.enrich_inbound(&mut current);

        let quoted = current.quoted.unwrap();
        assert!(quoted.lookup_found);
        assert_eq!(quoted.text_summary.as_deref(), Some("上一条"));
    }

    #[test]
    fn image_quote_keeps_media_summary_and_part() {
        let mut message = inbound("m1", Some("REFIDX_1"), "看图");
        message
            .input_parts
            .push(MessageInputPart::image(MessageMedia {
                mime_type: Some("image/png".to_owned()),
                filename: Some("a.png".to_owned()),
                url: Some("https://example.test/a.png".to_owned()),
                ..Default::default()
            }));
        let mut store = RefIndex::default();
        store.insert_inbound(&message);
        let mut current = inbound("m2", None, "这张怎么处理");
        current.quoted = Some(QuotedMessageContext {
            ref_msg_idx: Some("REFIDX_1".to_owned()),
            ..Default::default()
        });

        store.enrich_inbound(&mut current);

        let quoted = current.quoted.unwrap();
        assert!(quoted.lookup_found);
        assert_eq!(quoted.media_summaries.len(), 1);
        assert!(matches!(
            quoted.input_parts[1],
            MessageInputPart::Image { .. }
        ));
    }

    #[test]
    fn missing_quote_records_fallback_reason() {
        let store = RefIndex::default();
        let mut current = inbound("m2", None, "继续");
        current.quoted = Some(QuotedMessageContext {
            ref_msg_idx: Some("REFIDX_missing".to_owned()),
            ..Default::default()
        });

        store.enrich_inbound(&mut current);

        let quoted = current.quoted.unwrap();
        assert!(!quoted.lookup_found);
        assert_eq!(quoted.fallback_reason.as_deref(), Some("ref_index_miss"));
    }
}
