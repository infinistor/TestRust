//! `UpDownClient`가 TESTCore `UpDownClient`와 같은 순서로 요청하고, 같은 통계·ERROR 로그·예외를 남기는지 확인한다.
//! 기준 출력은 `tools/dotnet-oracle/gen-updown-cases.ps1`(오라클 `updown` 명령)로 만든다.
//!
//! 끝없이 도는 메서드는 서버가 `quitAfter`번째 요청을 받으면 응답하기 전에 `Quit`을 켠다(오라클과 같은 방식).
//! 요청은 메서드·경로·쿼리 매개변수 집합으로 비교한다. `aws-test`는 객체 이름에 현재 시각이 들어가므로
//! 날짜 경로를 자리표시자로 바꾼다.

use std::collections::BTreeSet;
use std::path::Path;
#[path = "support/route_server.rs"]
mod route_server;

use std::sync::{Arc, Mutex, OnceLock};

use route_server::{Hook, RouteServer, routes};
use serde_json::Value;
use testrust_clients::UpDownClient;
use testrust_config::{EnumBucketTypes, UpDownClientConfig, UserData};
use testrust_model::TestClient;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};

/// ERROR 로그 메시지를 모은다.
#[derive(Default)]
struct ErrorLogs(Mutex<Vec<String>>);

struct Collector(Arc<ErrorLogs>);

impl<S: Subscriber> Layer<S> for Collector {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if *event.metadata().level() != Level::ERROR {
            return;
        }
        struct Message(String);
        impl Visit for Message {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.0 = format!("{value:?}");
                }
            }
        }
        let mut message = Message(String::new());
        event.record(&mut message);
        self.0.0.lock().unwrap().push(message.0);
    }
}

fn normalize_line(line: &str) -> (String, String, BTreeSet<String>) {
    let mut parts = line.split(' ');
    let method = parts.next().unwrap().to_string();
    let target = parts.next().unwrap();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    // aws-test: /{버킷}/{년}/{월}/{일}/{시}/{분}/FILE_... 의 날짜 부분
    let segments: Vec<&str> = path.split('/').collect();
    let path = if segments.len() == 8 && segments[2].len() == 4 && segments[7].starts_with("FILE_")
    {
        format!("/{}/<DATE>/{}", segments[1], segments[7])
    } else {
        path.to_string()
    };
    let query = query
        .split('&')
        .filter(|q| !q.is_empty())
        .map(|q| q.trim_end_matches('=').to_string())
        .collect();
    (method, path, query)
}

fn i64s(value: &Value) -> Vec<i64> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect()
}

async fn run_case(name: &str, logs: &Arc<ErrorLogs>) -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity");
    let read = |path: String| -> Value {
        serde_json::from_str(&std::fs::read_to_string(root.join(path)).unwrap()).unwrap()
    };
    let spec = read(format!("updown/{name}.json"));
    let baseline = read(format!("baseline/updown/{name}.json"));
    let text = |key: &str| {
        spec.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let int = |key: &str, default: i64| spec.get(key).and_then(Value::as_i64).unwrap_or(default);
    let flag = |key: &str| spec.get(key).and_then(Value::as_bool).unwrap_or(false);

    let work = tempfile::tempdir().unwrap();
    let file_path = work.path().join("body.txt");
    let content = spec
        .get("fileContent")
        .and_then(Value::as_str)
        .unwrap_or("hello world");
    std::fs::write(&file_path, content).unwrap();

    let lines = Arc::new(Mutex::new(Vec::new()));
    let client_slot: Arc<OnceLock<Arc<UpDownClient>>> = Arc::new(OnceLock::new());
    let quit_after = int("quitAfter", 0) as usize;
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hook: Hook = {
        let slot = client_slot.clone();
        let counter = counter.clone();
        Arc::new(move |_: &str| {
            let n = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            if n == quit_after
                && let Some(client) = slot.get()
            {
                client.set_quit(true);
            }
        })
    };
    let server = RouteServer::start(routes(&spec["routes"]), lines.clone(), hook).await;
    let port = server.port;

    let bucket_type = EnumBucketTypes::from_name(
        spec.get("bucketType")
            .and_then(Value::as_str)
            .unwrap_or("None"),
    );
    let mut config = UpDownClientConfig::new(
        "TH",
        "obj",
        int("readRatio", 1) as i32,
        int("writeRatio", 1) as i32,
        int("deleteRatio", 0) as i32,
        int("fileSize", 11),
        bucket_type,
        flag("etagCheck"),
        1000,
        int("retry", 0) as i32,
        false,
        flag("useChunkEncoding"),
    );
    config.distributed = flag("distributed");
    let user = UserData::new(
        format!("http://127.0.0.1:{port}"),
        "",
        "AKIAEXAMPLE",
        "secretExample",
    );
    let client = Arc::new(UpDownClient::new("my-bucket", 1, file_path, config, user));
    let _ = client_slot.set(client.clone());

    logs.0.lock().unwrap().clear();
    let max_count = int("maxCount", 0) as i32;
    let start = int("start", 0) as i32;
    let part_size = int("partSize", 0);
    let result = match text("op").as_str() {
        "prepare" => client.prepare(max_count, flag("check"), start).await,
        "prepare-dir" => client.prepare_dir(max_count, flag("check"), start).await,
        "prepare-new" => client.prepare_new(max_count, flag("check"), start).await,
        "prepare-random" => client.prepare_random(max_count, start).await,
        "read-new" => client.read_new(max_count, start).await,
        "write-random" => client.write_random(start).await,
        "delete-new" => client.delete_new(max_count, start).await,
        "delete-directory" => client.delete_directory().await,
        "mix-new" => client.mix_new().await,
        "mix-v2" => client.mix_v2().await,
        "head" => client.head(max_count, start).await,
        "read-v2" => client.read_v2(max_count, start).await,
        "read-v3" => client.read_v3().await,
        "write" => client.write(start).await,
        "write-v2" => client.write_v2(max_count).await,
        "delete" => client.delete(flag("bulk"), max_count).await,
        "delete-v2" => client.delete_v2(max_count, start).await,
        "delete-one" => client.delete_one(&text("key"), max_count).await,
        "delete-version" => {
            let prefix = text("prefix");
            client
                .delete_version(
                    flag("bulk"),
                    max_count,
                    (!prefix.is_empty()).then_some(prefix.as_str()),
                )
                .await
        }
        "mix" => client.mix().await,
        "put-get" => client.put_get().await,
        "all" => client.all().await,
        "multi-upload" => client.multi_upload(max_count, part_size).await,
        "multi-upload-v2" => client.multi_upload_v2(max_count, part_size).await,
        "upload" => client.upload(max_count, part_size).await,
        "download" => client.download(max_count).await,
        "upload-tag" => client.upload_tag(max_count).await,
        "aws-test" => client.aws_test(max_count).await,
        "sample-upload" => client.sample_upload(&text("prefix"), max_count).await,
        "list-object" => client.list_object().await,
        other => panic!("알 수 없는 op: {other}"),
    };
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    drop(server);

    let mut failures = Vec::new();
    // `mix`·`mix-v2`는 읽을 객체를 무작위로 고르므로 GET 대상 이름은 비교하지 않는다.
    let random_read = name == "mix" || name == "mix-v2";
    let normalize = |line: &str| {
        let (method, path, query) = normalize_line(line);
        if random_read && method == "GET" {
            (method, "/my-bucket/<RANDOM>".to_string(), query)
        } else {
            (method, path, query)
        }
    };
    let actual_lines: Vec<_> = lines.lock().unwrap().iter().map(|l| normalize(l)).collect();
    let expected_lines: Vec<_> = baseline["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| normalize(l.as_str().unwrap()))
        .collect();
    if actual_lines != expected_lines {
        failures.push(format!(
            "{name} 요청\n  expected {expected_lines:?}\n  actual   {actual_lines:?}"
        ));
    }

    let stats = client.stats();
    let actual_stats = vec![
        vec![
            stats.write.success(),
            stats.write.error(),
            stats.write.part(),
        ],
        vec![stats.read.success(), stats.read.error()],
        vec![stats.head.success(), stats.head.error()],
        vec![stats.delete.success(), stats.delete.error()],
        vec![stats.list.success(), stats.list.error()],
        vec![
            stats
                .loop_end_count
                .load(std::sync::atomic::Ordering::Relaxed),
        ],
    ];
    let s = &baseline["stats"];
    let expected_stats = vec![
        i64s(&s["write"]),
        i64s(&s["read"]),
        i64s(&s["head"]),
        i64s(&s["delete"]),
        i64s(&s["list"]),
        vec![s["loopEnd"].as_i64().unwrap()],
    ];
    if actual_stats != expected_stats {
        failures.push(format!(
            "{name} 통계\n  expected {expected_stats:?}\n  actual   {actual_stats:?}"
        ));
    }
    if client.quit() != baseline["quit"].as_bool().unwrap() {
        failures.push(format!(
            "{name} Quit\n  expected {}\n  actual   {}",
            baseline["quit"],
            client.quit()
        ));
    }

    let actual_logs = logs.0.lock().unwrap().clone();
    let expected_logs: Vec<String> = baseline["logs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap().to_string())
        .collect();
    if actual_logs != expected_logs {
        failures.push(format!(
            "{name} 로그\n  expected {expected_logs:?}\n  actual   {actual_logs:?}"
        ));
    }

    let actual_error = result.err().map(|e| e.to_string());
    let expected_error = baseline["error"].as_object().map(|e| {
        format!(
            "{}: {}",
            e["type"].as_str().unwrap(),
            e["message"].as_str().unwrap()
        )
    });
    if actual_error != expected_error {
        failures.push(format!(
            "{name} 예외\n  expected {expected_error:?}\n  actual   {actual_error:?}"
        ));
    }
    failures
}

#[tokio::test]
async fn up_down_client_matches_dotnet() {
    let logs = Arc::new(ErrorLogs::default());
    let subscriber = tracing_subscriber::registry().with(Collector(logs.clone()));
    let _guard = tracing::subscriber::set_default(subscriber);

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity/updown");
    let mut names: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| n.strip_suffix(".json"))
                .map(str::to_string)
        })
        .collect();
    names.sort();
    assert!(names.len() >= 40, "사례 수 {}", names.len());

    let mut failures = Vec::new();
    for name in &names {
        failures.extend(run_case(name, &logs).await);
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
