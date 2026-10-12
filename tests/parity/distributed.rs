//! 분산 실행 계약 JSON을 .NET(`tools/dotnet-oracle contracts`, 기준 `baseline/distributed/contracts.json`)과 비교한다.
//!
//! - 통신용(`*.web`): `JsonSerializerDefaults.Web`(camelCase, 압축) — Rust [`to_web_json`]과 글자 단위로 같아야 한다.
//! - 파일용(`*.file`): PascalCase, 들여쓰기 — Rust [`to_dotnet_json`]과 같아야 한다.
//! - .NET이 쓴 JSON을 Rust가 읽어 다시 쓰면 원문과 같아야 한다(왕복).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;
use serde::de::DeserializeOwned;
use testrust_common::dotnet_datetime::DateTimeKind;
use testrust_common::{
    DotnetDateTime, DotnetDateTimeOffset, from_web_json, to_dotnet_json, to_web_json,
};
use testrust_config::UserData;
use testrust_distributed::contracts::{
    RunResult, RunSnapshot, StartRequest, TestRequest, WorkerStatus, WorkloadSettings,
};

fn baseline() -> BTreeMap<String, String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/parity/baseline/distributed/contracts.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn offset(text: &str) -> DotnetDateTimeOffset {
    DotnetDateTimeOffset::parse(text).unwrap()
}

fn request() -> TestRequest {
    TestRequest {
        run_id: Some("0123456789abcdef0123456789abcdef".into()),
        worker_id: Some("driver1".into()),
        test_type: Some("Mix".into()),
        user: Some(UserData::new(
            "http://127.0.0.1:9000",
            "kr-1",
            "access",
            "secret",
        )),
        lease_timeout_seconds: 20,
        workload: Some(WorkloadSettings {
            bucket_name: Some("bucket-a".into()),
            thread_prefix: Some("TH".into()),
            object_prefix: Some("FILE".into()),
            file_size: 1024,
            retry_count: 2,
            is_admin: true,
            thread_count: 4,
            file_count: 100,
            times: 30,
            read_ratio: 7,
            write_ratio: 3,
            delete_ratio: 1,
            bucket_type: 2,
            division_count: 500,
            e_tag_check: true,
            use_chunk_encoding: true,
            check: true,
            start: 5,
            random: true,
            bulk: true,
            max_count: 9,
        }),
    }
}

fn running() -> RunSnapshot {
    let start = offset("2026-10-08T01:02:03.1234567+00:00");
    RunSnapshot {
        run_id: Some("0123456789abcdef0123456789abcdef".into()),
        worker_id: Some("driver1".into()),
        test_type: Some("Mix".into()),
        state: Some("Running".into()),
        error: None,
        sample_at_utc: offset("2026-10-08T01:02:05.5+00:00"),
        scheduled_at_utc: Some(start),
        started_at_utc: Some(offset("2026-10-08T01:02:03.1234577+00:00")),
        issuing_stopped_at_utc: None,
        completed_at_utc: None,
        elapsed_seconds: 1.5,
        result: Some(RunResult {
            read: 10,
            read_failed: 1,
            head: 2,
            head_failed: 0,
            write: 5,
            write_failed: 2,
            delete: 3,
            delete_failed: 1,
            list: 4,
            list_failed: 1,
            total: 30,
            total_failed: 5,
            time: 1,
            start_time: DotnetDateTime::parse_xml("2026-10-08T01:02:03.1234567Z").unwrap(),
            end_time: DotnetDateTime::default(),
            test_type: Some("Mix".into()),
            thread_count: 4,
            file_size: 1024,
            read_ratio: 7,
            write_ratio: 3,
            delete_ratio: 1,
            bucket_type: Some("Thread".into()),
            bucket_name: Some("bucket-a-driver1".into()),
            object_prefix: Some("FILE".into()),
            thread_prefix: Some("TH/driver1".into()),
        }),
    }
}

fn failed() -> RunSnapshot {
    RunSnapshot {
        run_id: Some("0123456789abcdef0123456789abcdef".into()),
        worker_id: Some("driver1".into()),
        test_type: Some("Put".into()),
        state: Some("Failed".into()),
        error: Some("Controller heartbeat 만료 \"x\"".into()),
        sample_at_utc: offset("2026-10-08T01:02:03+00:00"),
        elapsed_seconds: 0.0,
        result: Some(RunResult {
            start_time: DotnetDateTime::default(),
            end_time: DotnetDateTime::default(),
            test_type: Some("Unknown".into()),
            bucket_type: None,
            bucket_name: None,
            object_prefix: None,
            thread_prefix: None,
            ..RunResult::default()
        }),
        ..RunSnapshot::default()
    }
}

/// 통신용: 직렬화가 같고, .NET 원문을 읽어 다시 써도 같다.
fn check_web<T: Serialize + DeserializeOwned>(
    base: &BTreeMap<String, String>,
    name: &str,
    value: &T,
) {
    let expected = &base[name];
    assert_eq!(&to_web_json(value), expected, "{name} 직렬화");
    let parsed: T = from_web_json(expected).unwrap_or_else(|e| panic!("{name} 읽기: {e}"));
    assert_eq!(&to_web_json(&parsed), expected, "{name} 왕복");
}

/// 파일용: PascalCase 들여쓰기.
fn check_file<T: Serialize + DeserializeOwned>(
    base: &BTreeMap<String, String>,
    name: &str,
    value: &T,
) {
    let expected = &base[name];
    assert_eq!(&to_dotnet_json(value), expected, "{name} 직렬화");
    let parsed: T = serde_json::from_str(expected).unwrap_or_else(|e| panic!("{name} 읽기: {e}"));
    assert_eq!(&to_dotnet_json(&parsed), expected, "{name} 왕복");
}

#[test]
fn contracts_match_dotnet() {
    let base = baseline();
    check_web(&base, "request.web", &request());
    check_file(&base, "request.file", &request());
    check_web(
        &base,
        "request-no-user.web",
        &TestRequest {
            run_id: Some("fedcba9876543210fedcba9876543210".into()),
            worker_id: Some("w-2".into()),
            test_type: Some("Get".into()),
            ..TestRequest::default()
        },
    );
    check_web(
        &base,
        "status.web",
        &WorkerStatus {
            name: Some("driver1".into()),
            available: true,
            run_id: None,
            lease_timeout_seconds: 15,
            uses_local_user: true,
        },
    );
    check_web(
        &base,
        "start.web",
        &StartRequest {
            start_at_utc: offset("2026-10-08T01:02:03.1234567+00:00"),
        },
    );
    check_web(
        &base,
        "start-whole.web",
        &StartRequest {
            start_at_utc: offset("2026-10-08T01:02:03+00:00"),
        },
    );
    check_web(&base, "running.web", &running());
    check_file(&base, "running.file", &running());
    check_web(&base, "failed.web", &failed());
    check_file(&base, "failed.file", &failed());
    assert_eq!(
        offset("2026-10-08T01:02:03.1234567+00:00").to_o_text(),
        base["start.o"]
    );
    // 결과의 UTC 시각은 `Z`, 기본값은 오프셋 없이.
    assert_eq!(running().result.unwrap().start_time.kind, DateTimeKind::Utc);
}
