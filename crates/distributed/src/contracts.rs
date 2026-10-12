//! 원본 `Distributed/Contracts.cs`: Controller와 Worker가 주고받는 요청·상태·통계.
//!
//! - serde 이름은 .NET 속성 이름(PascalCase)이다. 통신은 [`to_web_json`](camelCase, `JsonSerializerDefaults.Web`)과
//!   [`from_web_json`], Worker 결과 파일과 해시 입력은 [`to_dotnet_json`]·`serde_json`(PascalCase)을 쓴다.
//! - 열거형(`BucketType`)은 정수, `null`도 그대로 쓴다. `RunSnapshot.IsFinal`은 쓰기만 한다.
//!
//! [`to_web_json`]: testrust_common::to_web_json
//! [`from_web_json`]: testrust_common::from_web_json
//! [`to_dotnet_json`]: testrust_common::to_dotnet_json

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use testrust_common::{DotnetDateTime, DotnetDateTimeOffset};
use testrust_config::{Config, EnumBucketTypes, MainConfig, UpDownConfig, UserData};
use testrust_model::UpDownResult;
use testrust_scenarios::ScenarioError;

use crate::settings::{argument, validate_name};

/// 분산 실행이 지원하는 테스트(원본 `TestRequest.From`의 메뉴 대응).
pub const TEST_TYPES: [&str; 5] = ["Prepare", "Put", "Get", "Delete", "Mix"];

/// 원본 `TestRequest.From`이 `CommandOptions`에서 읽는 값.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOptions {
    /// `Prepare`·`Put`·`Get`·`Delete`·`Mix`. 그 밖의 메뉴는 `None`(ArgumentException).
    pub test_type: Option<&'static str>,
    pub check: bool,
    pub start: i32,
    pub random: bool,
    pub bulk: bool,
    pub count: i32,
}

/// 원본 `TestRequest`: Controller가 Worker 한 개에 전달하는 실행 식별자, 부하 설정, S3 접속 정보.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TestRequest {
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub worker_id: Option<String>,
    #[serde(default)]
    pub test_type: Option<String>,
    #[serde(default = "default_workload")]
    pub workload: Option<WorkloadSettings>,
    #[serde(default)]
    pub user: Option<UserData>,
    #[serde(default = "default_lease")]
    pub lease_timeout_seconds: i32,
}

fn default_workload() -> Option<WorkloadSettings> {
    Some(WorkloadSettings::default())
}

fn default_lease() -> i32 {
    15
}

impl Default for TestRequest {
    fn default() -> Self {
        Self {
            run_id: None,
            worker_id: None,
            test_type: None,
            workload: default_workload(),
            user: None,
            lease_timeout_seconds: default_lease(),
        }
    }
}

/// 원본 `WorkloadSettings`: MainConfig·UpDownConfig와 테스트별 CLI 인자.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct WorkloadSettings {
    pub bucket_name: Option<String>,
    pub thread_prefix: Option<String>,
    pub object_prefix: Option<String>,
    pub file_size: i64,
    pub retry_count: i32,
    pub is_admin: bool,
    /// Worker 하나가 만드는 부하 스레드 수. 전체 스레드 수는 각 Worker 설정의 합이다.
    pub thread_count: i32,
    pub file_count: i32,
    pub times: i32,
    pub read_ratio: i32,
    pub write_ratio: i32,
    pub delete_ratio: i32,
    /// `EnumBucketTypes`(정수).
    pub bucket_type: i32,
    pub division_count: i32,
    #[serde(rename = "ETagCheck")]
    pub e_tag_check: bool,
    pub use_chunk_encoding: bool,
    pub check: bool,
    pub start: i32,
    pub random: bool,
    pub bulk: bool,
    pub max_count: i32,
}

impl Default for WorkloadSettings {
    fn default() -> Self {
        Self {
            bucket_name: None,
            thread_prefix: Some("TH".to_string()),
            object_prefix: Some("FILE".to_string()),
            file_size: 0,
            retry_count: 3,
            is_admin: false,
            thread_count: 0,
            file_count: 0,
            times: 0,
            read_ratio: 0,
            write_ratio: 0,
            delete_ratio: 0,
            bucket_type: EnumBucketTypes::Prefix.0,
            division_count: 1000,
            e_tag_check: false,
            use_chunk_encoding: false,
            check: false,
            start: 0,
            random: false,
            bulk: false,
            max_count: 0,
        }
    }
}

/// 원본 `WorkerStatus`: 작업 수락 가능 여부와 접속 정보 선택에 필요한 사전 조회 응답.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct WorkerStatus {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub lease_timeout_seconds: i32,
    #[serde(default)]
    pub uses_local_user: bool,
}

/// 원본 `StartRequest`: 모든 Worker가 공유하는 UTC 시작 예약(서버 간 시계 동기화가 전제).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StartRequest {
    /// 없으면 `default(DateTimeOffset)`(.NET은 그대로 읽고 `Schedule`이 409로 거절한다).
    #[serde(default = "DotnetDateTimeOffset::min_value")]
    pub start_at_utc: DotnetDateTimeOffset,
}

/// 원본 `RunSnapshot`: 한 Worker의 누적 통계와 실행 상태(중간 조회와 최종 결과에 같은 형식).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct RunSnapshot {
    pub run_id: Option<String>,
    pub worker_id: Option<String>,
    pub test_type: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub sample_at_utc: DotnetDateTimeOffset,
    pub scheduled_at_utc: Option<DotnetDateTimeOffset>,
    pub started_at_utc: Option<DotnetDateTimeOffset>,
    /// 신규 요청 발행 중단 시각(진행 중인 요청 정리 완료와 구분한다).
    pub issuing_stopped_at_utc: Option<DotnetDateTimeOffset>,
    pub completed_at_utc: Option<DotnetDateTimeOffset>,
    pub elapsed_seconds: f64,
    pub result: Option<RunResult>,
}

impl Default for RunSnapshot {
    fn default() -> Self {
        Self {
            run_id: None,
            worker_id: None,
            test_type: None,
            state: Some("Preparing".to_string()),
            error: None,
            sample_at_utc: DotnetDateTimeOffset::min_value(),
            scheduled_at_utc: None,
            started_at_utc: None,
            issuing_stopped_at_utc: None,
            completed_at_utc: None,
            elapsed_seconds: 0.0,
            result: Some(RunResult::default()),
        }
    }
}

impl RunSnapshot {
    /// 원본 `IsFinal`.
    pub fn is_final(&self) -> bool {
        matches!(
            self.state.as_deref(),
            Some("Completed" | "Cancelled" | "Failed")
        )
    }
}

impl Serialize for RunSnapshot {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("RunSnapshot", 14)?;
        s.serialize_field("RunId", &self.run_id)?;
        s.serialize_field("WorkerId", &self.worker_id)?;
        s.serialize_field("TestType", &self.test_type)?;
        s.serialize_field("State", &self.state)?;
        s.serialize_field("Error", &self.error)?;
        s.serialize_field("SampleAtUtc", &self.sample_at_utc)?;
        s.serialize_field("ScheduledAtUtc", &self.scheduled_at_utc)?;
        s.serialize_field("StartedAtUtc", &self.started_at_utc)?;
        s.serialize_field("IssuingStoppedAtUtc", &self.issuing_stopped_at_utc)?;
        s.serialize_field("CompletedAtUtc", &self.completed_at_utc)?;
        s.serialize_field("ElapsedSeconds", &self.elapsed_seconds)?;
        s.serialize_field("Result", &self.result)?;
        s.serialize_field("IsFinal", &self.is_final())?;
        s.end()
    }
}

/// 원본 `UpDownResult`의 통신·파일 형식(문자열은 `null`일 수 있고, 시각은 System.Text.Json `DateTime` 형식).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct RunResult {
    pub read: i64,
    pub read_failed: i64,
    pub head: i64,
    pub head_failed: i64,
    pub write: i64,
    pub write_failed: i64,
    pub delete: i64,
    pub delete_failed: i64,
    pub list: i64,
    pub list_failed: i64,
    pub total: i64,
    pub total_failed: i64,
    pub time: i32,
    #[serde(
        serialize_with = "date_time_json",
        deserialize_with = "parse_date_time"
    )]
    pub start_time: DotnetDateTime,
    #[serde(
        serialize_with = "date_time_json",
        deserialize_with = "parse_date_time"
    )]
    pub end_time: DotnetDateTime,
    pub test_type: Option<String>,
    pub thread_count: i32,
    pub file_size: i64,
    pub read_ratio: i32,
    pub write_ratio: i32,
    pub delete_ratio: i32,
    pub bucket_type: Option<String>,
    pub bucket_name: Option<String>,
    pub object_prefix: Option<String>,
    pub thread_prefix: Option<String>,
}

impl Default for RunResult {
    /// 원본 생성자: `StartTime = DateTime.Now`, `TestType = "Unknown"`, 나머지 문자열은 `null`.
    fn default() -> Self {
        Self {
            bucket_type: None,
            bucket_name: None,
            object_prefix: None,
            thread_prefix: None,
            ..Self::from(&UpDownResult::default())
        }
    }
}

impl From<&UpDownResult> for RunResult {
    fn from(r: &UpDownResult) -> Self {
        Self {
            read: r.read,
            read_failed: r.read_failed,
            head: r.head,
            head_failed: r.head_failed,
            write: r.write,
            write_failed: r.write_failed,
            delete: r.delete,
            delete_failed: r.delete_failed,
            list: r.list,
            list_failed: r.list_failed,
            total: r.total,
            total_failed: r.total_failed,
            time: r.time,
            start_time: r.start_time,
            end_time: r.end_time,
            test_type: Some(r.test_type.clone()),
            thread_count: r.thread_count,
            file_size: r.file_size,
            read_ratio: r.read_ratio,
            write_ratio: r.write_ratio,
            delete_ratio: r.delete_ratio,
            bucket_type: Some(r.bucket_type.clone()),
            bucket_name: Some(r.bucket_name.clone()),
            object_prefix: Some(r.object_prefix.clone()),
            thread_prefix: Some(r.thread_prefix.clone()),
        }
    }
}

fn date_time_json<S: Serializer>(value: &DotnetDateTime, serializer: S) -> Result<S::Ok, S::Error> {
    testrust_common::dotnet_json::raw_string(&value.to_json_text(), serializer)
}

fn parse_date_time<'de, D: Deserializer<'de>>(deserializer: D) -> Result<DotnetDateTime, D::Error> {
    let text = String::deserialize(deserializer)?;
    DotnetDateTime::parse_xml(&text).map_err(serde::de::Error::custom)
}

static BUCKET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("^[a-z0-9][a-z0-9.-]*[a-z0-9]$").unwrap());

impl TestRequest {
    /// 원본 `TestRequest.From(config, options, runId, workerId, lease)`: CLI 덮어쓰기가 반영된 설정에서 원격 실행에
    /// 필요한 값만 뽑는다.
    pub fn from_config(
        config: &Config,
        options: &RunOptions,
        run_id: &str,
        worker_id: &str,
        lease: i32,
    ) -> Result<Self, ScenarioError> {
        let test_type = options
            .test_type
            .ok_or_else(|| argument("분산 실행은 test-prepare/put/get/delete/mix만 지원합니다."))?;
        let (m, u) = (&config.main, &config.up_down);
        Ok(Self {
            run_id: Some(run_id.to_string()),
            worker_id: Some(worker_id.to_string()),
            test_type: Some(test_type.to_string()),
            user: Some(config.main_user.clone()),
            lease_timeout_seconds: lease,
            workload: Some(WorkloadSettings {
                bucket_name: Some(m.bucket_name.clone()),
                thread_prefix: Some(m.thread_prefix.clone()),
                object_prefix: Some(m.object_prefix.clone()),
                file_size: m.file_size,
                retry_count: m.retry_count,
                is_admin: m.is_admin,
                thread_count: u.thread_count,
                file_count: u.file_count,
                times: u.times,
                read_ratio: u.read_ratio,
                write_ratio: u.write_ratio,
                delete_ratio: u.delete_ratio,
                bucket_type: u.bucket_type.0,
                division_count: u.division_count,
                e_tag_check: u.etag_check,
                use_chunk_encoding: u.use_chunk_encoding,
                check: options.check,
                start: options.start,
                random: options.random,
                bulk: options.bulk,
                max_count: options.count,
            }),
        })
    }

    /// 원본 `Validate(requireUser)`: 실행 가능한 범위를 검증한다. Worker의 로컬 사용자 선택 전에는 접속 정보 검증을
    /// 미룰 수 있다.
    pub fn validate(&self, require_user: bool) -> Result<(), ScenarioError> {
        if !self.run_id.as_deref().is_some_and(is_guid_n) {
            return Err(argument("RunId 형식 오류"));
        }
        validate_name(self.worker_id.as_deref().unwrap_or_default())?;
        let test_type = self.test_type.as_deref().unwrap_or_default();
        if !TEST_TYPES.contains(&test_type) {
            return Err(argument("지원하지 않는 테스트"));
        }
        if require_user {
            validate_user(self.user.as_ref())?;
        }
        if !(1..=86400).contains(&self.lease_timeout_seconds) {
            return Err(argument("Lease 설정 오류"));
        }
        let w = self
            .workload
            .as_ref()
            .ok_or_else(|| argument("Workload 설정 필요"))?;
        let mut errors = Vec::new();
        fn range(errors: &mut Vec<String>, name: &str, value: i64, min: i64, max: Option<i64>) {
            if value < min || max.is_some_and(|max| value > max) {
                let allowed = match max {
                    Some(max) => format!("{min}~{max}"),
                    None => format!("{min} 이상"),
                };
                errors.push(format!("{name}={value} (허용: {allowed})"));
            }
        }
        range(
            &mut errors,
            "ThreadCount",
            w.thread_count.into(),
            1,
            Some(10000),
        );
        range(&mut errors, "FileSize", w.file_size, 0, None);
        // 기존 INI 로더는 FileCount 생략을 -1로 표현한다. Prepare/Get만 파일 개수를 사용한다.
        let file_min = if matches!(test_type, "Prepare" | "Get") {
            1
        } else {
            -1
        };
        range(
            &mut errors,
            "FileCount",
            w.file_count.into(),
            file_min,
            None,
        );
        range(&mut errors, "Start", w.start.into(), 0, None);
        range(&mut errors, "RetryCount", w.retry_count.into(), 0, None);
        range(
            &mut errors,
            "DivisionCount",
            w.division_count.into(),
            1,
            None,
        );
        let ratio_min = if test_type == "Mix" { 1 } else { 0 };
        range(
            &mut errors,
            "ReadRatio",
            w.read_ratio.into(),
            ratio_min,
            None,
        );
        range(
            &mut errors,
            "WriteRatio",
            w.write_ratio.into(),
            ratio_min,
            None,
        );
        range(&mut errors, "DeleteRatio", w.delete_ratio.into(), 0, None);
        if !(0..=5).contains(&w.bucket_type) {
            errors.push(format!(
                "BucketType={} (허용: 0=None, 1=One, 2=Thread, 3=Time, 4=Prefix, 5=Now)",
                w.bucket_type
            ));
        }
        if matches!(test_type, "Put" | "Get" | "Mix") {
            range(&mut errors, "Times", w.times.into(), 1, Some(86400));
        }
        if !errors.is_empty() {
            return Err(argument(format!(
                "부하 설정 범위 오류: {}",
                errors.join("; ")
            )));
        }
        if test_type == "Prepare" && w.start >= w.file_count {
            return Err(argument("Start는 FileCount보다 작아야 합니다."));
        }
        let blank = |v: &Option<String>| v.as_deref().is_none_or(|s| s.trim().is_empty());
        if blank(&w.thread_prefix) || blank(&w.object_prefix) {
            return Err(argument("Prefix 설정 필요"));
        }
        let bucket_name = w.bucket_name.as_deref();
        let bucket = if w.bucket_type == EnumBucketTypes::Thread.0 {
            Some(format!(
                "{}-{}-{:04}",
                bucket_name.unwrap_or_default(),
                self.worker_id.as_deref().unwrap_or_default(),
                w.thread_count - 1
            ))
        } else {
            bucket_name.map(str::to_string)
        };
        let valid = bucket.is_some_and(|b| {
            (3..=63).contains(&b.len())
                && BUCKET.is_match(&b)
                && !b.contains("..")
                && !b.contains(".-")
                && !b.contains("-.")
                && !is_ip_address(&b)
        });
        if !valid {
            return Err(argument("분산 실행 버킷 이름이 S3 규칙에 맞지 않습니다."));
        }
        Ok(())
    }
}

/// 원본 `ValidateUser`.
pub fn validate_user(user: Option<&UserData>) -> Result<(), ScenarioError> {
    let ok = user.is_some_and(|u| {
        !u.is_empty()
            && u.url
                .parse::<http::Uri>()
                .ok()
                .and_then(|uri| uri.scheme_str().map(str::to_ascii_lowercase))
                .is_some_and(|s| s == "http" || s == "https")
    });
    if ok {
        Ok(())
    } else {
        Err(argument(
            "S3 접속 설정 오류: URL, AccessKey, SecretKey를 확인하세요.",
        ))
    }
}

/// `Guid.TryParseExact(text, "N")`: 16진수 32자.
fn is_guid_n(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `IPAddress.TryParse`(IPv4 부분): 점으로 나눈 1~4개 숫자(10진수·`0x` 16진수)이면 주소로 본다.
fn is_ip_address(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    !parts.is_empty()
        && parts.len() <= 4
        && parts.iter().all(|p| {
            let (digits, radix) = match p.strip_prefix("0x") {
                Some(hex) => (hex, 16),
                None => (*p, 10),
            };
            !digits.is_empty() && u32::from_str_radix(digits, radix).is_ok()
        })
}

impl WorkloadSettings {
    /// 원본 `ToMain(workerId, workPath)`: WorkerId를 버킷 또는 스레드 접두어에 넣어 여러 Worker가 같은 대상에 쓰지
    /// 않게 한다.
    pub fn to_main(&self, worker_id: &str, work_path: &str) -> Result<MainConfig, ScenarioError> {
        let bucket = self.bucket_name.clone().unwrap_or_default();
        let bucket = if self.bucket_type == EnumBucketTypes::Thread.0 {
            format!("{bucket}-{worker_id}")
        } else {
            bucket
        };
        let thread_prefix = format!(
            "{}/{worker_id}",
            self.thread_prefix
                .as_deref()
                .unwrap_or_default()
                .trim_end_matches('/')
        );
        Ok(MainConfig::new(
            &bucket,
            &thread_prefix,
            self.object_prefix.as_deref().unwrap_or_default(),
            work_path,
            &self.file_size.to_string(),
            "5M",
            self.retry_count,
            "",
            self.is_admin,
        )?)
    }

    /// 원본 `ToUpDown()`.
    pub fn to_up_down(&self) -> UpDownConfig {
        UpDownConfig::new(
            self.read_ratio,
            self.write_ratio,
            self.delete_ratio,
            self.thread_count,
            self.file_count,
            self.division_count,
            self.times,
            EnumBucketTypes(self.bucket_type),
            self.e_tag_check,
            self.use_chunk_encoding,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ip_like_bucket_names() {
        assert!(is_ip_address("1.2.3.4"));
        assert!(is_ip_address("123"));
        assert!(!is_ip_address("bucket-a"));
        assert!(!is_ip_address("1.2.3.4.5"));
    }
}
