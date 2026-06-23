//! 列车时刻查询运行时模块。
//!
//! 该模块对 12306 数据源做最小封装：
//! - 对外暴露稳定的查询请求、结果和执行器 trait；
//! - 将 HTTP 请求、JSON 解析和错误分类收敛在这里；
//! - 上层 `/火车` flow 只依赖 trait，不直接感知 12306 接口细节。

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::NaiveDate;
use reqwest::StatusCode;
use serde::Deserialize;

use crate::{config::AppConfig, error::LlmError};

/// 12306 列车时刻接口地址。
const TRAIN_QUERY_URL: &str =
    "https://mobile.12306.cn/wxxcx/wechat/main/travelServiceQrcodeTrainInfo";

/// 列车时刻查询请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrainScheduleRequest {
    /// 车次，例如 `G1`、`D1234` 或 `1461`。
    pub train_code: String,
    /// 查询日期，按中国标准时间解释。
    pub travel_date: NaiveDate,
}

/// 单个经停站信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrainStop {
    /// 站序。
    pub station_no: u32,
    /// 车站名。
    pub station_name: String,
    /// 到达时间，格式固定为 `HH:MM`；始发站通常为空。
    pub arrive_time: Option<String>,
    /// 出发时间，格式固定为 `HH:MM`；终到站通常为空。
    pub departure_time: Option<String>,
    /// 停留分钟数。
    pub stopover_minutes: Option<u32>,
    /// 相对发车日的跨日偏移。
    pub day_difference: i32,
    /// 该站对应的站内车次显示值；部分跨线车会与主车次不同。
    pub station_train_code: String,
}

/// 完整列车时刻表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrainSchedule {
    /// 规范化后的车次。
    pub train_code: String,
    /// 查询日期。
    pub travel_date: NaiveDate,
    /// 始发站。
    pub start_station: String,
    /// 终到站。
    pub end_station: String,
    /// 全部经停站。
    pub stops: Vec<TrainStop>,
}

/// 列车查询执行器 trait。
#[async_trait]
pub trait TrainExecutor: Send + Sync {
    /// 查询指定日期的列车时刻表。
    async fn query_train_schedule(
        &self,
        req: TrainScheduleRequest,
    ) -> Result<TrainSchedule, LlmError>;

    /// 返回执行器名称，供 diagnostics 使用。
    fn provider_name(&self) -> &'static str;
}

/// 动态派发的列车查询执行器。
pub type DynTrainExecutor = Arc<dyn TrainExecutor>;

/// 根据配置构建默认 12306 查询执行器。
pub fn build_train_executor(config: &AppConfig) -> Result<DynTrainExecutor, LlmError> {
    Ok(Arc::new(Train12306Executor::new(config)?))
}

/// 12306 列车时刻执行器。
pub struct Train12306Executor {
    client: reqwest::Client,
}

impl Train12306Executor {
    /// 构造执行器，沿用全局请求超时配置，避免单命令长期阻塞。
    pub fn new(config: &AppConfig) -> Result<Self, LlmError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.request_timeout_seconds))
            .build()
            .map_err(|err| {
                LlmError::config(format!("failed to build 12306 train HTTP client: {err}"))
            })?;
        Ok(Self { client })
    }
}

#[async_trait]
impl TrainExecutor for Train12306Executor {
    async fn query_train_schedule(
        &self,
        req: TrainScheduleRequest,
    ) -> Result<TrainSchedule, LlmError> {
        let start_day = req.travel_date.format("%Y%m%d").to_string();
        let response = self
            .client
            .post(TRAIN_QUERY_URL)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(format!("trainCode={}&startDay={start_day}", req.train_code))
            .send()
            .await
            .map_err(map_train_request_error)?;

        let status = response.status();
        if !status.is_success() {
            return Err(train_status_error(status));
        }

        let payload: TrainApiResponse = response.json().await.map_err(|err| {
            LlmError::provider(format!("invalid 12306 train JSON: {err}"), "train_json")
        })?;
        payload.into_schedule(req)
    }

    fn provider_name(&self) -> &'static str {
        "12306"
    }
}

#[derive(Debug, Deserialize)]
struct TrainApiResponse {
    #[serde(default)]
    status: bool,
    #[serde(rename = "errorMsg", default)]
    error_msg: String,
    #[serde(default)]
    data: Option<TrainApiData>,
}

#[derive(Debug, Deserialize)]
struct TrainApiData {
    #[serde(rename = "trainDetail", default)]
    train_detail: Option<TrainApiDetail>,
}

#[derive(Debug, Deserialize)]
struct TrainApiDetail {
    #[serde(rename = "trainCode", default)]
    train_code: Option<String>,
    #[serde(rename = "stopTime", default)]
    stop_time: Vec<TrainApiStop>,
}

#[derive(Debug, Deserialize)]
struct TrainApiStop {
    #[serde(rename = "stationNo", default)]
    station_no: Option<String>,
    #[serde(rename = "stationName", default)]
    station_name: Option<String>,
    #[serde(rename = "arriveTime", default)]
    arrive_time: Option<String>,
    #[serde(rename = "startTime", default)]
    start_time: Option<String>,
    #[serde(rename = "stopover_time", default)]
    stopover_time: Option<String>,
    #[serde(rename = "dayDifference", default)]
    day_difference: Option<String>,
    #[serde(rename = "stationTrainCode", default)]
    station_train_code: Option<String>,
}

impl TrainApiResponse {
    fn into_schedule(self, req: TrainScheduleRequest) -> Result<TrainSchedule, LlmError> {
        if !self.status {
            let message = if self.error_msg.trim().is_empty() {
                "12306 train service returned unsuccessful status".to_owned()
            } else {
                format!("12306 train service error: {}", self.error_msg.trim())
            };
            return Err(LlmError::provider(message, "train_status"));
        }

        let Some(detail) = self.data.and_then(|data| data.train_detail) else {
            return Err(no_schedule_error());
        };
        if detail.stop_time.is_empty() {
            return Err(no_schedule_error());
        }

        let mut stops = Vec::with_capacity(detail.stop_time.len());
        for (index, stop) in detail.stop_time.into_iter().enumerate() {
            stops.push(TrainStop {
                station_no: parse_u32_field(stop.station_no.as_deref()).unwrap_or(index as u32 + 1),
                station_name: required_train_field(stop.station_name, "stationName")?,
                arrive_time: normalize_train_time(stop.arrive_time.as_deref()),
                departure_time: normalize_train_time(stop.start_time.as_deref()),
                stopover_minutes: parse_u32_field(stop.stopover_time.as_deref()),
                day_difference: parse_i32_field(stop.day_difference.as_deref()).unwrap_or(0),
                station_train_code: stop
                    .station_train_code
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| req.train_code.clone()),
            });
        }

        let start_station = stops
            .first()
            .map(|stop| stop.station_name.clone())
            .ok_or_else(no_schedule_error)?;
        let end_station = stops
            .last()
            .map(|stop| stop.station_name.clone())
            .ok_or_else(no_schedule_error)?;
        let train_code = detail
            .train_code
            .and_then(|value| {
                let trimmed = value.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_ascii_uppercase())
            })
            .unwrap_or_else(|| req.train_code.clone());

        Ok(TrainSchedule {
            train_code,
            travel_date: req.travel_date,
            start_station,
            end_station,
            stops,
        })
    }
}

fn required_train_field(value: Option<String>, field_name: &str) -> Result<String, LlmError> {
    value
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
        .ok_or_else(|| {
            LlmError::provider(
                format!("12306 train response missing required field: {field_name}"),
                "train_json",
            )
        })
}

fn normalize_train_time(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() || value == "----" || value == "--:--" {
        return None;
    }
    if value.len() == 4 && value.chars().all(|ch| ch.is_ascii_digit()) {
        return Some(format!("{}:{}", &value[..2], &value[2..]));
    }
    Some(value.to_owned())
}

fn parse_u32_field(value: Option<&str>) -> Option<u32> {
    let value = value?.trim();
    (!value.is_empty()).then_some(())?;
    value.parse::<u32>().ok()
}

fn parse_i32_field(value: Option<&str>) -> Option<i32> {
    let value = value?.trim();
    (!value.is_empty()).then_some(())?;
    value.parse::<i32>().ok()
}

fn map_train_request_error(err: reqwest::Error) -> LlmError {
    if err.is_timeout() {
        return LlmError::timeout("train");
    }
    LlmError::http(format!("12306 train request failed: {err}"))
}

fn train_status_error(status: StatusCode) -> LlmError {
    let message = if status == StatusCode::NOT_FOUND {
        "12306 train service returned 404".to_owned()
    } else {
        format!("12306 train service returned HTTP {status}")
    };
    LlmError::http(message)
}

fn no_schedule_error() -> LlmError {
    LlmError::new(
        "no_schedule",
        "no train schedule found for the requested date",
        "train",
    )
}
