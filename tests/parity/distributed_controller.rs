//! 분산 Controller 쪽 비교: ResultWriter(CSV·JSON·경로 규칙)와 ResultConsoleFormatter를 .NET
//! (`tools/dotnet-oracle distributed-writer`, 기준 `baseline/distributed/writer.json`)과 비교하고, Controller 프로토콜을
//! 가짜 Worker로 시험한다.
//!
//! 기준 출력 다시 만들기(TESTCore HEAD 빌드, 경로에 8.3 이름이 섞이지 않게 긴 경로를 쓴다):
//! `dotnet tools/dotnet-oracle/bin/Debug/net10.0/DotnetOracle.dll distributed-writer <긴 경로의 빈 디렉터리>`

#[path = "support/fake_driver.rs"]
mod fake_driver;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fake_driver::{Behavior, FakeDriver};
use serde_json::Value;
use testrust_common::{DotnetDateTime, DotnetDateTimeOffset};
use testrust_config::Config;
use testrust_distributed::console::{self, FormatOptions};
use testrust_distributed::contracts::{RunOptions, RunResult, RunSnapshot, WorkloadSettings};
use testrust_distributed::controller::{run_async, run_with_cleanup_budget};
use testrust_distributed::result_writer::{ResultSettings, ResultWriter, WorkerSample};
use testrust_distributed::settings::DistributedSettings;
use testrust_distributed::{DistributedArgs, run};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::layer::SubscriberExt;

const RUN_ID: &str = "0123456789abcdef0123456789abcdef";

fn baseline() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/parity/baseline/distributed/writer.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// 첫 번째로 다른 줄을 보여 주는 비교.
fn assert_text(name: &str, expected: &str, actual: &str) {
    if expected == actual {
        return;
    }
    let (e, a): (Vec<&str>, Vec<&str>) =
        (expected.split('\n').collect(), actual.split('\n').collect());
    for i in 0..e.len().max(a.len()) {
        if e.get(i) != a.get(i) {
            panic!(
                "{name}: {}번째 줄이 다르다\n.NET: {:?}\nRust: {:?}",
                i + 1,
                e.get(i),
                a.get(i)
            );
        }
    }
}

// ---------------------------------------------------------------- 표본 만들기

fn t(ms: i64) -> DotnetDateTimeOffset {
    let base = DotnetDateTimeOffset::parse("2026-10-08T01:00:00Z").unwrap();
    DotnetDateTimeOffset::new(base.value() + chrono::Duration::milliseconds(ms))
}

fn secs(s: f64) -> DotnetDateTimeOffset {
    t((s * 1000.0) as i64)
}

fn ticks(from: DotnetDateTimeOffset, to: DotnetDateTimeOffset) -> f64 {
    ((to.value() - from.value()).num_nanoseconds().unwrap() / 100) as f64 / 10_000_000.0
}

/// .NET `new UpDownResult { ... }`(문자열 설정은 `null`).
fn result(test_type: &str) -> RunResult {
    RunResult {
        start_time: DotnetDateTime::default(),
        end_time: DotnetDateTime::default(),
        test_type: Some(test_type.to_string()),
        bucket_type: None,
        bucket_name: None,
        object_prefix: None,
        thread_prefix: None,
        ..RunResult::default()
    }
}

#[allow(clippy::too_many_arguments)]
fn snap(
    worker: &str,
    test_type: &str,
    state: &str,
    sample: DotnetDateTimeOffset,
    started: Option<DotnetDateTimeOffset>,
    completed: Option<DotnetDateTimeOffset>,
    mut result: RunResult,
    error: Option<&str>,
    stopped: Option<DotnetDateTimeOffset>,
) -> RunSnapshot {
    result.start_time = DotnetDateTime::utc(started.unwrap_or_else(|| t(0)).value());
    result.test_type = Some(test_type.to_string());
    RunSnapshot {
        run_id: Some(RUN_ID.to_string()),
        worker_id: Some(worker.to_string()),
        test_type: Some(test_type.to_string()),
        state: Some(state.to_string()),
        error: error.map(str::to_string),
        sample_at_utc: sample,
        scheduled_at_utc: None,
        started_at_utc: started,
        issuing_stopped_at_utc: stopped,
        completed_at_utc: completed,
        elapsed_seconds: started.map_or(0.0, |s| ticks(s, completed.unwrap_or(sample)).max(0.0)),
        result: Some(result),
    }
}

fn sample(worker: &str, snapshot: RunSnapshot) -> WorkerSample {
    WorkerSample::new(worker, Some(snapshot))
}

// ---------------------------------------------------------------- 설정·로그 캡처

fn write_settings(dir: &Path, drivers: usize) -> DistributedSettings {
    std::fs::create_dir_all(dir).unwrap();
    let mut ini = format!("[controller]\r\nResultPath = results\r\ndrivers = {drivers}\r\n");
    for i in 1..=drivers {
        ini.push_str(&format!(
            "[driver{i}]\r\nname = w{i}\r\nurl = http://127.0.0.1:{}/driver\r\n",
            18000 + i
        ));
    }
    let path = dir.join("controller.ini");
    std::fs::write(&path, ini).unwrap();
    DistributedSettings::load(path.to_str().unwrap(), false).unwrap()
}

struct Capture(Arc<Mutex<Vec<String>>>);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Capture {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        struct Message(String);
        impl tracing::field::Visit for Message {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.0 = format!("{value:?}");
                }
            }
        }
        let mut message = Message(String::new());
        event.record(&mut message);
        self.0.lock().unwrap().push(message.0);
    }
}

/// `f`가 남기는 INFO 로그 메시지를 모은다.
fn capture_logs<T>(f: impl FnOnce() -> T) -> (T, Vec<String>) {
    let messages = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry().with(Capture(messages.clone()));
    let value = tracing::subscriber::with_default(subscriber, f);
    let messages = messages.lock().unwrap().clone();
    (value, messages)
}

fn result_settings(settings: &DistributedSettings) -> ResultSettings {
    ResultSettings::new(
        Some(WorkloadSettings {
            bucket_name: Some("bucket-a".into()),
            file_size: 1024,
            thread_count: 2,
            file_count: 3,
            times: 5,
            read_ratio: 1,
            write_ratio: 1,
            ..WorkloadSettings::default()
        }),
        settings,
    )
}

/// 쓰기가 끝난 CSV·JSON(`Total.SampleAtUtc`는 저장 시각이라 가린다)과 로그.
fn output(writer: ResultWriter, messages: Vec<String>) -> Value {
    let (json_path, csv_path) = (
        writer.json_path().to_path_buf(),
        writer.csv_path().to_path_buf(),
    );
    drop(writer);
    let csv = std::fs::read(csv_path).unwrap();
    let mut json = std::fs::read_to_string(json_path).unwrap();
    let total = json.find("\"Total\": {").unwrap();
    let key = "\"SampleAtUtc\": \"";
    let start = total + json[total..].find(key).unwrap() + key.len();
    let end = start + json[start..].find('"').unwrap();
    json.replace_range(start..end, "<NOW>");
    serde_json::json!({
        "csv": String::from_utf8(csv.clone()).unwrap(),
        "csvBom": csv.starts_with(&[0xEF, 0xBB, 0xBF]),
        "json": json,
        "messages": messages,
    })
}

fn writer_get(dir: &Path) -> Value {
    let settings = write_settings(dir, 2);
    let (writer, messages) = capture_logs(|| {
        let mut writer = ResultWriter::new(&settings, None, RUN_ID, "Get").unwrap();
        let at = t(0);
        let s = |worker: &str, count: i64, time: DotnetDateTimeOffset, done: bool| {
            let mut r = result("Get");
            r.read = count;
            r.file_size = 1024;
            sample(
                worker,
                snap(
                    worker,
                    "Get",
                    if done { "Completed" } else { "Running" },
                    time,
                    Some(at),
                    done.then_some(time),
                    r,
                    None,
                    None,
                ),
            )
        };
        let bad = WorkerSample::failed("w1", "bad,\"quoted\"\nerror");
        writer
            .sample(&[s("w1", 0, at, false), s("w2", 0, at, false)], at)
            .unwrap();
        writer
            .sample(
                &[s("w1", 3, secs(2.0), false), s("w2", 5, secs(2.0), false)],
                secs(2.0),
            )
            .unwrap();
        writer
            .sample(&[bad, s("w2", 5, secs(4.0), false)], secs(4.0))
            .unwrap();
        writer
            .sample(
                &[s("w1", 3, secs(6.0), true), s("w2", 5, secs(6.0), true)],
                secs(6.0),
            )
            .unwrap();
        writer
            .save_final("Completed", None, Some(&result_settings(&settings)))
            .unwrap();
        writer
    });
    output(writer, messages)
}

fn writer_mix(dir: &Path) -> Value {
    let settings = write_settings(dir, 3);
    let (writer, messages) = capture_logs(|| {
        let mut writer = ResultWriter::new(&settings, None, RUN_ID, "Mix").unwrap();
        // (read, write, head, delete, list, read_failed, write_failed, list_failed, threads)
        let r = |read, write, head, delete, list, rf, wf, lf, threads| {
            let mut r = result("Mix");
            (r.read, r.write, r.head, r.delete, r.list) = (read, write, head, delete, list);
            (r.read_failed, r.write_failed, r.list_failed) = (rf, wf, lf);
            r.thread_count = threads;
            r.file_size = 1_048_576;
            r
        };
        let m = |worker: &str,
                 state: &str,
                 at: DotnetDateTimeOffset,
                 started: Option<DotnetDateTimeOffset>,
                 result: RunResult,
                 completed: Option<DotnetDateTimeOffset>,
                 error: Option<&str>,
                 stopped: Option<DotnetDateTimeOffset>| {
            sample(
                worker,
                snap(
                    worker, "Mix", state, at, started, completed, result, error, stopped,
                ),
            )
        };
        let s1 = 100; // w2는 100ms 늦게 시작한다
        let z = r(0, 0, 0, 0, 0, 0, 0, 0, 0);
        writer
            .sample(
                &[
                    m("w1", "Preparing", t(0), None, z.clone(), None, None, None),
                    m("w2", "Ready", t(0), None, z.clone(), None, None, None),
                    m("w3", "Preparing", t(0), None, z, None, None, None),
                ],
                t(0),
            )
            .unwrap();
        writer
            .sample(
                &[
                    m(
                        "w1",
                        "Running",
                        secs(2.0),
                        Some(secs(1.0)),
                        r(10, 20, 1, 2, 3, 0, 1, 1, 2),
                        None,
                        None,
                        None,
                    ),
                    m(
                        "w2",
                        "Running",
                        secs(2.0),
                        Some(t(s1 + 1000)),
                        r(7, 9, 0, 0, 0, 0, 0, 0, 2),
                        None,
                        None,
                        None,
                    ),
                    WorkerSample::failed("w3", "Worker 조회 실패: HttpRequestException"),
                ],
                secs(2.0),
            )
            .unwrap();
        writer
            .sample(
                &[
                    m(
                        "w1",
                        "Running",
                        secs(4.0),
                        Some(secs(1.0)),
                        r(30, 41, 1, 5, 3, 0, 1, 1, 2),
                        None,
                        None,
                        None,
                    ),
                    m(
                        "w2",
                        "Running",
                        secs(4.0),
                        Some(t(s1 + 1000)),
                        r(5, 19, 0, 0, 0, 0, 0, 0, 2),
                        None,
                        None,
                        None,
                    ),
                    m(
                        "w3",
                        "Running",
                        secs(4.0),
                        Some(secs(3.0)),
                        r(1, 1, 0, 0, 0, 0, 0, 0, 4),
                        None,
                        None,
                        None,
                    ),
                ],
                secs(4.0),
            )
            .unwrap();
        writer
            .sample(
                &[
                    m(
                        "w1",
                        "Cancelled",
                        secs(6.0),
                        Some(secs(1.0)),
                        r(31, 42, 1, 5, 3, 0, 1, 1, 2),
                        Some(secs(6.0)),
                        Some("stopped"),
                        Some(secs(5.0)),
                    ),
                    m(
                        "w2",
                        "Failed",
                        secs(6.0),
                        Some(t(s1 + 1000)),
                        r(5, 19, 0, 0, 0, 0, 0, 0, 2),
                        Some(secs(5.5)),
                        Some("x,y"),
                        None,
                    ),
                    m(
                        "w3",
                        "Completed",
                        secs(6.0),
                        Some(secs(3.0)),
                        r(2, 2, 0, 0, 0, 0, 0, 0, 4),
                        Some(secs(6.0)),
                        None,
                        Some(secs(5.5)),
                    ),
                ],
                secs(6.0),
            )
            .unwrap();
        writer
            .sample(
                &[
                    m(
                        "w1",
                        "Completed",
                        secs(8.0),
                        Some(secs(1.0)),
                        r(31, 42, 1, 5, 3, 0, 1, 1, 2),
                        Some(secs(8.0)),
                        None,
                        None,
                    ),
                    m(
                        "w2",
                        "Completed",
                        secs(8.0),
                        Some(t(s1 + 1000)),
                        r(5, 19, 0, 0, 0, 0, 0, 0, 2),
                        Some(secs(8.0)),
                        None,
                        None,
                    ),
                    WorkerSample::failed("w3", "Worker 조회 실패: TaskCanceledException"),
                ],
                secs(8.0),
            )
            .unwrap();
        writer
            .save_final(
                "Cancelled",
                Some("사용자 중단"),
                Some(&result_settings(&settings)),
            )
            .unwrap();
        writer
    });
    output(writer, messages)
}

#[test]
fn writer_output_matches_dotnet() {
    let base = baseline();
    let dir = tempfile::tempdir().unwrap();
    for (name, actual) in [
        ("get", writer_get(&dir.path().join("get"))),
        ("mix", writer_mix(&dir.path().join("mix"))),
    ] {
        let expected = &base[name];
        assert_eq!(expected["csvBom"], actual["csvBom"], "{name} BOM");
        assert_text(
            &format!("{name} csv"),
            expected["csv"].as_str().unwrap(),
            actual["csv"].as_str().unwrap(),
        );
        assert_text(
            &format!("{name} json"),
            expected["json"].as_str().unwrap(),
            actual["json"].as_str().unwrap(),
        );
        assert_eq!(
            expected["messages"], actual["messages"],
            "{name} console messages"
        );
    }
}

// ---------------------------------------------------------------- 콘솔 블록

fn c(
    test_type: &str,
    state: &str,
    elapsed: f64,
    build: impl FnOnce(&mut RunResult),
) -> RunSnapshot {
    let mut r = result(test_type);
    build(&mut r);
    RunSnapshot {
        test_type: Some(test_type.to_string()),
        state: Some(state.to_string()),
        elapsed_seconds: elapsed,
        result: Some(r),
        ..RunSnapshot::default()
    }
}

fn console_cases() -> BTreeMap<String, String> {
    let mut cases = BTreeMap::new();
    let mut f = |name: &str, s: RunSnapshot, reported, expected, final_, options: FormatOptions| {
        cases.insert(
            name.to_string(),
            console::format(&s, reported, expected, final_, &options),
        );
    };
    let none = || FormatOptions::default();
    f(
        "mix-progress",
        c("Mix", "Running", 4.0, |r| (r.read, r.write) = (20, 50)),
        2,
        2,
        false,
        FormatOptions {
            increments: Some([Some(0), Some(12), None, None, None]),
            ..none()
        },
    );
    f(
        "mix-progress-rates",
        c("Mix", "Running", 4.0, |r| {
            (r.read, r.write, r.file_size) = (20, 50, 2048)
        }),
        2,
        2,
        false,
        FormatOptions {
            rates: Some([Some(5.5), Some(12.25), None, None, None]),
            increments: Some([Some(11), Some(49), None, None, None]),
            ..none()
        },
    );
    f(
        "mix-progress-missing-rate",
        c("Mix", "Running", 4.0, |r| {
            (r.read, r.write, r.file_size) = (20, 50, 2048)
        }),
        1,
        2,
        false,
        FormatOptions {
            rates: Some([Some(5.5), None, None, None, None]),
            error: Some("일부 Worker 통계 누락".into()),
            increments: Some([Some(1), None, None, None, None]),
            ..none()
        },
    );
    f(
        "mix-cancelled",
        c("Mix", "Running", 2.5, |r| {
            (r.read, r.write, r.write_failed, r.file_size) = (10, 20, 2, 1024 * 1024)
        }),
        2,
        3,
        true,
        FormatOptions {
            state: Some("Cancelled".into()),
            error: Some("사용자 중단".into()),
            ..none()
        },
    );
    f(
        "get-progress",
        c("Get", "Running", 1.0, |r| {
            (r.read, r.read_failed, r.file_size) = (8, 1, 1024)
        }),
        1,
        2,
        false,
        FormatOptions {
            rates: Some([Some(4.0), None, None, None, None]),
            increments: Some([Some(8), None, None, None, None]),
            ..none()
        },
    );
    f(
        "get-progress-no-input",
        c("Get", "Running", 1.0, |r| r.read = 8),
        2,
        2,
        false,
        none(),
    );
    f(
        "get-final",
        c("Get", "Completed", 6.0, |r| {
            (r.read, r.file_size) = (8, 1024)
        }),
        2,
        2,
        true,
        FormatOptions {
            state: Some("Completed".into()),
            ..none()
        },
    );
    f(
        "delete-progress",
        c("Delete", "Running", 3.0, |r| {
            (r.delete, r.delete_failed) = (100, 3)
        }),
        2,
        2,
        false,
        FormatOptions {
            rates: Some([None, None, None, Some(33.333333333333336), None]),
            increments: Some([None, None, None, Some(100), None]),
            ..none()
        },
    );
    f(
        "put-final-rounding",
        c("Put", "Completed", 3.0, |r| {
            (r.write, r.write_failed, r.file_size) = (1, 2, 5)
        }),
        2,
        2,
        true,
        FormatOptions {
            state: Some("Completed".into()),
            error: Some("  ".into()),
            ..none()
        },
    );
    for test_type in ["Put", "Prepare", "Delete", "Get", "Mix"] {
        let s = RunSnapshot {
            test_type: Some(test_type.to_string()),
            state: Some("Failed".into()),
            result: Some(result(test_type)),
            ..RunSnapshot::default()
        };
        f(&format!("empty-failed-{test_type}"), s, 0, 2, true, none());
    }
    f(
        "prepare-progress",
        c("Prepare", "Preparing", 0.0, |_| {}),
        0,
        2,
        false,
        none(),
    );
    cases
}

#[test]
fn console_blocks_match_dotnet() {
    let base = baseline();
    let expected = base["console"].as_object().unwrap();
    let actual = console_cases();
    assert_eq!(expected.len(), actual.len());
    for (name, text) in &actual {
        assert_text(name, expected[name].as_str().unwrap(), text);
    }
}

// ---------------------------------------------------------------- 경로 규칙

fn relative(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

fn list_files(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            out.push(relative(root, &path));
            if path.is_dir() {
                walk(&path, root, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.retain(|p| p != "controller.ini");
    out.sort();
    out
}

#[test]
fn save_path_rules_match_dotnet() {
    let base = baseline();
    let expected = base["paths"].as_object().unwrap();
    let root = tempfile::tempdir().unwrap();
    // (이름, save, 준비)
    type Prepare = Box<dyn Fn(&Path)>;
    let file = |rel: &'static str| -> Prepare {
        Box::new(move |d| {
            let p = d.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "old").unwrap();
        })
    };
    let cases: Vec<(&str, Option<&str>, Option<Prepare>)> = vec![
        ("default", None, None),
        ("empty", Some(""), None),
        ("blank", Some("  "), None),
        ("dir", Some("out"), None),
        ("dir-dotted", Some("dir.d/name"), None),
        ("json", Some("out/x.json"), None),
        ("json-upper", Some("out/x.JSON"), None),
        ("csv-extension", Some("out/y.csv"), None),
        ("trailing-dot", Some("out/name."), None),
        ("hidden", Some("out/.json"), None),
        ("double-extension", Some("out/a.b.json"), None),
        ("absolute", Some("{ABS}/abs/z.json"), None),
        ("json-exists", Some("out/x.json"), Some(file("out/x.json"))),
        ("csv-exists", Some("out/x.json"), Some(file("out/x.csv"))),
        (
            "default-json-exists",
            None,
            Some(file("results/0123456789abcdef0123456789abcdef.json")),
        ),
        (
            "csv-is-directory",
            Some("out/x.json"),
            Some(Box::new(|d| {
                std::fs::create_dir_all(d.join("out/x.csv")).unwrap()
            })),
        ),
    ];
    assert_eq!(cases.len(), expected.len());
    for (index, (name, save, prepare)) in cases.iter().enumerate() {
        let case_dir = root.path().join(format!("c{index}"));
        let settings = write_settings(&case_dir, 2);
        if let Some(prepare) = prepare {
            prepare(&case_dir);
        }
        let save = save.map(|s| s.replace("{ABS}", &case_dir.to_string_lossy().replace('\\', "/")));
        let actual = match ResultWriter::new(&settings, save.as_deref(), RUN_ID, "Get") {
            Ok(writer) => {
                let value = serde_json::json!({
                    "json": relative(&case_dir, writer.json_path()),
                    "csv": relative(&case_dir, writer.csv_path()),
                    "files": list_files(&case_dir),
                });
                drop(writer);
                value
            }
            Err(e) => {
                let kind = e.dotnet_type.rsplit('.').next().unwrap().to_string();
                let message = if matches!(kind.as_str(), "IOException" | "ArgumentException") {
                    e.message
                        .replace(case_dir.to_string_lossy().as_ref(), "<DIR>")
                } else {
                    String::new()
                };
                serde_json::json!({ "error": kind, "message": message, "files": list_files(&case_dir) })
            }
        };
        assert_eq!(expected[*name], actual, "{name}");
    }
}

// ---------------------------------------------------------------- DistributedChecks 항목

/// `DistributedChecks`의 CSV 읽기: 따옴표 안의 쉼표·줄바꿈을 지킨다.
fn read_csv(text: &str) -> Vec<Vec<String>> {
    let (mut rows, mut row, mut field, mut quoted) = (Vec::new(), Vec::new(), String::new(), false);
    let mut chars = text.trim_start_matches('\u{FEFF}').chars().peekable();
    while let Some(ch) = chars.next() {
        match (quoted, ch) {
            (true, '"') if chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            (true, '"') => quoted = false,
            (false, '"') => quoted = true,
            (false, ',') => row.push(std::mem::take(&mut field)),
            (false, '\r') => {}
            (false, '\n') => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            (_, ch) => field.push(ch),
        }
    }
    rows
}

#[test]
fn distributed_checks_result_writer_items() {
    let dir = tempfile::tempdir().unwrap();
    let value = writer_get(dir.path());
    let csv = value["csv"].as_str().unwrap();
    assert_eq!(value["csvBom"], true, "CSV UTF-8 BOM");
    let rows = read_csv(csv);
    let header = rows[0].clone();
    let col = |name: &str| header.iter().position(|h| h == name).unwrap();
    assert!(
        rows.iter().all(|r| r.len() == header.len()),
        "따옴표·여러 줄 오류 문자열이 열 수를 유지한다"
    );
    assert_eq!(
        rows[4][col("ReadOpsPerSecond")],
        "1.5",
        "인베리언트 숫자와 구간 처리량"
    );
    assert_eq!(rows[7][col("ReadSuccess")], "");
    assert_eq!(
        rows[7][col("Available")],
        "false",
        "누락 Worker는 0이 아니라 빈 칸"
    );
    assert_eq!(
        rows[9][col("ReadOpsPerSecond")],
        "",
        "누락이면 전체 구간 처리량을 비운다"
    );
    assert_eq!(
        rows[12][col("ReadOpsPerSecond")],
        "",
        "복구 첫 표본은 전체 증분 기준을 다시 잡는다"
    );
    assert_eq!(rows[7][col("Error")], "bad,\"quoted\"\nerror");

    let messages: Vec<String> = value["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap().to_string())
        .collect();
    let re = |pattern: &str, text: &str| regex::Regex::new(pattern).unwrap().is_match(text);
    assert!(
        messages.len() == 4
            && messages
                .iter()
                .filter(|m| m.contains("[READ TEST FINAL RESULTS]"))
                .count()
                == 1,
        "진행 블록은 조회마다 한 번, 최종은 한 번"
    );
    assert!(
        messages
            .iter()
            .all(|m| !m.contains("w1") && !m.contains("Read="))
    );
    assert!(re(r"Worker Count\s+:\s+2/2", &messages[0]));
    assert!(re(r"Read Average\s+:\s+4\.000 file/sec", &messages[1]));
    assert!(re(r"Total Average\s+:\s+1\.333 file/sec", &messages[3]));
    assert!(re(r"Total Time\s+:\s+6\.000 sec", &messages[3]));
    assert!(re(r"Read Count\s+:\s+8 \(\+ 8\)", &messages[1]));
    assert!(messages[0].contains("(+ N/A)") && messages[2].contains("(+ N/A)"));
    assert!(!messages[3].contains("(+"));
    assert!(messages[2].contains("Partial") && messages[2].contains("N/A"));
    assert!(re(r"Worker Count\s+:\s+1/2", &messages[2]));

    let cases = console_cases();
    let mixed = &cases["mix-progress"];
    assert!(re(r"Read Count\s+:\s+20 \(\+ 0\)", mixed));
    assert!(re(r"Write Count\s+:\s+50 \(\+ 12\)", mixed));
    let cancelled = &cases["mix-cancelled"];
    for needle in [
        "[MIX TEST FINAL RESULTS]",
        "Cancelled",
        "사용자 중단",
        "12.000 file/sec",
        "12.000 MiB/s",
        "93.750 %",
    ] {
        assert!(cancelled.contains(needle), "{needle}");
    }
    assert!(re(r"Read Average\s+:\s+4\.000 file/sec", cancelled));
    assert!(re(r"Write Average\s+:\s+8\.000 file/sec", cancelled));
    for (kind, title) in [
        ("Put", "WRITE"),
        ("Prepare", "PREPARE"),
        ("Delete", "DELETE"),
    ] {
        let text = &cases[&format!("empty-failed-{kind}")];
        assert!(
            text.contains(&format!("[{title} TEST FINAL RESULTS]"))
                && text.contains("0.000 file/sec")
        );
        assert!(!text.contains("NaN") && !text.contains("Infinity"));
    }
    // 같은 경로로 다시 만들면 기존 결과를 보존한 채 IOException.
    let settings = write_settings(&dir.path().join("collision"), 2);
    let first = ResultWriter::new(&settings, Some("a.json"), RUN_ID, "Get").unwrap();
    let json = first.json_path().to_path_buf();
    drop(first);
    let e = ResultWriter::new(&settings, Some("a.json"), RUN_ID, "Get")
        .err()
        .unwrap();
    assert_eq!(e.dotnet_type, "System.IO.IOException");
    assert!(json.exists());
}

// ---------------------------------------------------------------- Controller 프로토콜

const CONFIG: &str = "[Default]\nBucketName=test-bucket\nThreadPrefix=TH\nObjectPrefix=FILE\nFileSize=128\n[UpDown]\nThreadCount=2\nTimes=1\nBucketType=4\nReadRatio=1\nWriteRatio=1\n[Main User]\nURL=http://127.0.0.1:9000\nAccessKey=test-access\nSecretKey=test-secret\n";

fn load_config(dir: &Path, text: &str) -> Config {
    let path = dir.join("config.ini");
    std::fs::write(&path, text).unwrap();
    Config::load(path.to_str().unwrap(), None).unwrap()
}

struct Env {
    dir: tempfile::TempDir,
    settings: DistributedSettings,
}

/// 가짜 Worker마다 `driverN` 구역을 만든 Controller 설정. `extra`는 `[controller]` 구역에 더한다.
fn env(drivers: &[(&str, &FakeDriver)], extra: &str) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let mut ini = format!(
        "[controller]\nResultPath = results\ndrivers = {}\nPollIntervalSeconds = 1\nRequestTimeoutSeconds = 1\nStartDelaySeconds = 1\nPrepareTimeoutSeconds = 30\n{extra}\n",
        drivers.len()
    );
    for (i, (name, driver)) in drivers.iter().enumerate() {
        ini.push_str(&format!(
            "[driver{}]\nname = {name}\nurl = {}\n",
            i + 1,
            driver.url()
        ));
    }
    let path = dir.path().join("controller.ini");
    std::fs::write(&path, ini).unwrap();
    let settings = DistributedSettings::load(path.to_str().unwrap(), false).unwrap();
    Env { dir, settings }
}

fn options() -> RunOptions {
    RunOptions {
        test_type: Some("Put"),
        ..RunOptions::default()
    }
}

fn report(env: &Env) -> Value {
    let found: Vec<PathBuf> = std::fs::read_dir(env.dir.path().join("results"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    assert_eq!(found.len(), 1, "{found:?}");
    serde_json::from_str(&std::fs::read_to_string(&found[0]).unwrap()).unwrap()
}

async fn run_controller(env: &Env, config: &Config, token: &CancellationToken) -> i32 {
    tokio::time::timeout(
        Duration::from_secs(90),
        run_with_cleanup_budget(
            &env.settings,
            config,
            &options(),
            None,
            token,
            Duration::from_secs(5),
        ),
    )
    .await
    .expect("Controller가 제한 시간 안에 끝나야 한다")
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn scheduled_start_with_two_workers() {
    let w1 = FakeDriver::start(Behavior::new("w1")).await;
    let mut local = Behavior::new("w2");
    local.uses_local_user = true;
    let w2 = FakeDriver::start(local).await;
    let env = env(&[("w1", &w1), ("w2", &w2)], "");
    let config = load_config(env.dir.path(), CONFIG);

    let before = DotnetDateTimeOffset::now();
    let save = env.dir.path().join("out/run.json");
    let exit = tokio::time::timeout(
        Duration::from_secs(60),
        run_async(
            &env.settings,
            &config,
            &options(),
            Some(save.to_str().unwrap()),
            &CancellationToken::new(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(exit, 0);
    assert!(save.exists() && save.with_extension("csv").exists());
    let csv = std::fs::read(save.with_extension("csv")).unwrap();
    assert!(csv.starts_with(&[0xEF, 0xBB, 0xBF]));
    let report: Value = serde_json::from_str(&std::fs::read_to_string(&save).unwrap()).unwrap();
    assert_eq!(report["State"], "Completed");
    assert_eq!(report["Complete"], true);
    assert_eq!(report["ReportedWorkers"], 2);
    assert_eq!(report["Error"], Value::Null);
    assert_eq!(report["Settings"]["Workload"]["ThreadCount"], 2);
    assert_eq!(report["Settings"]["Drivers"][1]["Name"], "w2");
    let run_id = report["RunId"].as_str().unwrap();
    assert_eq!(run_id.len(), 32);

    for (driver, name) in [(&w1, "w1"), (&w2, "w2")] {
        let requests = driver.requests();
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].path, "/driver/status");
        let submit = &driver.find("POST", "/runs")[0];
        assert_eq!(
            submit.content_type.as_deref(),
            Some("application/json; charset=utf-8")
        );
        let body: Value = serde_json::from_str(&submit.body).unwrap();
        assert_eq!(body["runId"], run_id);
        assert_eq!(body["workerId"], name);
        assert_eq!(body["testType"], "Put");
        assert_eq!(body["leaseTimeoutSeconds"], 15);
        assert_eq!(body["workload"]["threadCount"], 2);
        assert_eq!(body["workload"]["bucketName"], "test-bucket");
        if name == "w1" {
            assert_eq!(body["user"]["url"], "http://127.0.0.1:9000");
            assert_eq!(body["user"]["accessKey"], "test-access");
        } else {
            assert_eq!(
                body["user"],
                Value::Null,
                "Worker 설정의 사용자를 쓰면 접속 정보를 보내지 않는다"
            );
        }
        assert!(
            driver
                .requests()
                .iter()
                .any(|r| r.path == format!("/driver/runs/{run_id}"))
        );
        assert!(
            !driver
                .find("POST", &format!("/runs/{run_id}/heartbeat"))
                .is_empty(),
            "하트비트"
        );
        assert!(
            driver.find("POST", "/stop").is_empty(),
            "성공하면 중단하지 않는다"
        );
    }
    let starts: Vec<Value> = [&w1, &w2]
        .iter()
        .map(|d| serde_json::from_str(&d.find("POST", "/start")[0].body).unwrap())
        .collect();
    assert_eq!(
        starts[0], starts[1],
        "모든 Worker가 같은 시작 시각을 받는다"
    );
    let at = DotnetDateTimeOffset::parse(starts[0]["startAtUtc"].as_str().unwrap()).unwrap();
    assert!(ticks(before, at) >= 0.9, "StartDelaySeconds 뒤로 예약한다");
}

#[tokio::test(flavor = "multi_thread")]
async fn status_check_errors_stop_before_submit() {
    let cases: Vec<(&str, Behavior, &str)> = vec![
        (
            "name mismatch",
            Behavior::new("other"),
            "w1: 이름 불일치 또는 실행 중",
        ),
        (
            "unavailable",
            Behavior {
                available: false,
                ..Behavior::new("w1")
            },
            "w1: 이름 불일치 또는 실행 중",
        ),
        (
            "lease",
            Behavior {
                lease_timeout_seconds: 5,
                ..Behavior::new("w1")
            },
            "w1: Worker의 lease 상한을 확인하세요.",
        ),
        (
            "http error",
            Behavior {
                status_code: 500,
                ..Behavior::new("w1")
            },
            "Controller 오류: HttpRequestException",
        ),
    ];
    for (name, behavior, message) in cases {
        let w1 = FakeDriver::start(behavior).await;
        let w2 = FakeDriver::start(Behavior::new("w2")).await;
        let env = env(&[("w1", &w1), ("w2", &w2)], "");
        let config = load_config(env.dir.path(), CONFIG);
        let exit = run_controller(&env, &config, &CancellationToken::new()).await;
        assert_eq!(exit, 1, "{name}");
        let report = report(&env);
        assert_eq!(report["State"], "Failed", "{name}");
        assert_eq!(report["Error"], message, "{name}");
        assert_eq!(report["Complete"], false);
        assert!(
            w1.find("POST", "/runs").is_empty() && w2.find("POST", "/runs").is_empty(),
            "{name}: 제출 전에 멈춘다"
        );
        assert!(
            w1.find("POST", "/stop").is_empty(),
            "{name}: 제출하지 않았으면 중단 요청도 없다"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_controller_user_is_reported_per_worker() {
    let w1 = FakeDriver::start(Behavior::new("w1")).await;
    let env = env(&[("w1", &w1)], "");
    let config = load_config(
        env.dir.path(),
        &CONFIG.replace("AccessKey=test-access", "AccessKey="),
    );
    assert_eq!(
        run_controller(&env, &config, &CancellationToken::new()).await,
        1
    );
    assert_eq!(
        report(&env)["Error"],
        "w1: Controller의 [Main User] 설정이 필요합니다. S3 접속 설정 오류: URL, AccessKey, SecretKey를 확인하세요."
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn prepare_timeout_stops_submitted_workers() {
    let never_ready = Behavior {
        ready_after_polls: None,
        ..Behavior::new("w1")
    };
    let w1 = FakeDriver::start(never_ready.clone()).await;
    let w2 = FakeDriver::start(Behavior {
        name: "w2".into(),
        ..never_ready
    })
    .await;
    let env = env(&[("w1", &w1), ("w2", &w2)], "PrepareTimeoutSeconds = 1");
    let config = load_config(env.dir.path(), CONFIG);
    assert_eq!(
        run_controller(&env, &config, &CancellationToken::new()).await,
        1
    );
    let report = report(&env);
    assert_eq!(report["State"], "Failed");
    assert_eq!(report["Error"], "Worker 준비 제한 시간 초과");
    for driver in [&w1, &w2] {
        assert_eq!(
            driver.find("POST", "/stop").len(),
            1,
            "제출한 Worker에 중단을 요청한다"
        );
        assert!(driver.find("POST", "/start").is_empty());
    }
    // 중단 뒤 최종 상태(Cancelled)까지 수집한다.
    assert_eq!(report["Workers"][0]["State"], "Cancelled");
    assert_eq!(report["Total"]["State"], "Cancelled");
}

#[tokio::test(flavor = "multi_thread")]
async fn user_cancellation_stops_workers() {
    let running = Behavior {
        running_polls: 1000,
        ..Behavior::new("w1")
    };
    let w1 = FakeDriver::start(running.clone()).await;
    let w2 = FakeDriver::start(Behavior {
        name: "w2".into(),
        ..running
    })
    .await;
    let env = env(&[("w1", &w1), ("w2", &w2)], "");
    let config = load_config(env.dir.path(), CONFIG);
    let token = CancellationToken::new();
    let cancel = token.clone();
    let (w1_ref, w2_ref) = (&w1, &w2);
    let canceller = async {
        loop {
            if !w1_ref.find("POST", "/start").is_empty()
                && !w2_ref.find("POST", "/start").is_empty()
            {
                tokio::time::sleep(Duration::from_millis(400)).await;
                cancel.cancel();
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    let (exit, ()) = tokio::join!(run_controller(&env, &config, &token), canceller);
    assert_eq!(exit, 1);
    let report = report(&env);
    assert_eq!(report["State"], "Cancelled");
    assert_eq!(report["Error"], "사용자 중단");
    for driver in [&w1, &w2] {
        assert_eq!(driver.find("POST", "/stop").len(), 1);
    }
    assert_eq!(report["Workers"][1]["State"], "Cancelled");
}

#[tokio::test(flavor = "multi_thread")]
async fn worker_failure_and_ack_failure() {
    // Worker가 실행 중에 실패한다.
    let failing = Behavior {
        ready_after_polls: Some(1),
        fail_after_polls: Some((1, "boom".into())),
        ..Behavior::new("w1")
    };
    let w1 = FakeDriver::start(failing).await;
    let w2 = FakeDriver::start(Behavior::new("w2")).await;
    let env1 = env(&[("w1", &w1), ("w2", &w2)], "");
    let config = load_config(env1.dir.path(), CONFIG);
    assert_eq!(
        run_controller(&env1, &config, &CancellationToken::new()).await,
        1
    );
    let report1 = report(&env1);
    assert_eq!(report1["Error"], "w1: boom");
    assert_eq!(report1["State"], "Failed");
    assert_eq!(w2.find("POST", "/stop").len(), 1);

    // 예약 시작 수락이 실패한다.
    let w3 = FakeDriver::start(Behavior {
        start_status: 500,
        ..Behavior::new("w1")
    })
    .await;
    let w4 = FakeDriver::start(Behavior::new("w2")).await;
    let env2 = env(&[("w1", &w3), ("w2", &w4)], "");
    assert_eq!(
        run_controller(&env2, &config, &CancellationToken::new()).await,
        1
    );
    assert_eq!(
        report(&env2)["Error"],
        "Controller 오류: HttpRequestException"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn identity_errors_end_with_connection_timeout() {
    let wrong = Behavior {
        wrong_identity: true,
        ..Behavior::new("w1")
    };
    let w1 = FakeDriver::start(wrong).await;
    let env = env(&[("w1", &w1)], "LeaseTimeoutSeconds = 3");
    let config = load_config(env.dir.path(), CONFIG);
    assert_eq!(
        run_controller(&env, &config, &CancellationToken::new()).await,
        1
    );
    let report = report(&env);
    assert_eq!(report["Error"], "w1: 연결 제한 시간 초과");
    assert_eq!(report["UnavailableWorkers"][0], "w1");
    let csv = std::fs::read_dir(env.dir.path().join("results"))
        .unwrap()
        .flatten()
        .find(|e| e.path().extension().is_some_and(|x| x == "csv"))
        .unwrap();
    let text = std::fs::read_to_string(csv.path()).unwrap();
    assert!(text.contains("Worker 조회 실패: HttpRequestException"));
}

// ---------------------------------------------------------------- DistributedApplication

#[tokio::test]
async fn application_argument_errors() {
    let args = DistributedArgs {
        worker: true,
        controller: true,
        ..DistributedArgs::default()
    };
    let e = run(args, || None).await.unwrap_err();
    assert_eq!(e.dotnet_type, "System.ArgumentException");
    assert_eq!(
        e.message,
        "--worker와 --controller는 함께 지정할 수 없습니다."
    );

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("worker.ini");
    std::fs::write(
        &path,
        "[worker]\nname = w1\nurl = http://127.0.0.1:18011/driver\n",
    )
    .unwrap();
    let args = DistributedArgs {
        worker: true,
        menu_selected: true,
        config_path: path.to_string_lossy().into_owned(),
        ..DistributedArgs::default()
    };
    let e = run(args, || None).await.unwrap_err();
    assert_eq!(
        e.message,
        "Worker 서버 모드에 테스트 명령을 함께 지정할 수 없습니다."
    );

    let controller = dir.path().join("controller.ini");
    std::fs::write(
        &controller,
        "[controller]\n[driver1]\nname = w1\nurl = http://127.0.0.1:18011/driver\n",
    )
    .unwrap();
    let args = DistributedArgs {
        controller: true,
        config_path: controller.to_string_lossy().into_owned(),
        ..DistributedArgs::default()
    };
    let e = run(args, || None).await.unwrap_err();
    assert_eq!(e.message, "테스트 설정 파일을 읽지 못했습니다.");

    // 메뉴가 분산 실행 대상이 아니면 TestRequest 생성이 거절한다.
    let config = load_config(dir.path(), CONFIG);
    let args = DistributedArgs {
        controller: true,
        config_path: controller.to_string_lossy().into_owned(),
        ..DistributedArgs::default()
    };
    let e = run(args, move || Some(config)).await.unwrap_err();
    assert_eq!(
        e.message,
        "분산 실행은 test-prepare/put/get/delete/mix만 지원합니다."
    );
}
