use qq_maid_common::time_context::{
    diagnostic_time_unix_seconds, format_diagnostic_time_ago_for_display_at,
    format_diagnostic_time_for_display, format_diagnostic_time_without_unix_for_display,
};

pub(super) fn time_ago(value: &str, now_seconds: i64) -> String {
    format_diagnostic_time_ago_for_display_at(value, now_seconds)
        .unwrap_or_else(|| format_diagnostic_time_without_unix_for_display(value))
}

pub(super) fn time_or_placeholder(value: Option<&str>) -> String {
    value
        .filter(|text| !text.trim().is_empty())
        .map(format_diagnostic_time_without_unix_for_display)
        .unwrap_or_else(|| "未提供".to_owned())
}

pub(super) fn age_seconds(value: &str, now_seconds: i64) -> Option<i64> {
    diagnostic_time_unix_seconds(value).map(|seconds| now_seconds.saturating_sub(seconds))
}

pub(super) fn diagnostic_time_option_text(value: Option<&str>) -> String {
    value
        .filter(|text| !text.trim().is_empty())
        .map(format_diagnostic_time_for_display)
        .unwrap_or_else(|| "无".to_owned())
}
