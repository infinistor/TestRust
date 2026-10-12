//! `LocalClient`가 TESTCore `LocalClient`와 같은 통계·로그·예외·결과 파일을 남기는지 확인한다.
//! 기준 출력은 `tools/dotnet-oracle`의 `local` 명령으로 만든다.
//!
//! 로그는 수준, 메시지 첫 줄, 예외 형식 이름을 비교한다(예외 메시지는 운영체제 문구라 다르다).
//! 메서드 밖으로 나간 예외도 형식 이름만 비교한다.

use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use testrust_clients::LocalClient;
use testrust_config::{EnumBucketTypes, UpDownClientConfig};
use testrust_model::TestClient;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};

#[derive(Default)]
struct Logs(Mutex<Vec<String>>);

struct Collector(Arc<Logs>);

impl<S: Subscriber> Layer<S> for Collector {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let level = *event.metadata().level();
        if level > Level::WARN {
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
        // "메시지\n형식: 예외 메시지" → "메시지 | 형식"
        let line = match message.0.split_once('\n') {
            Some((first, exception)) => {
                let kind = exception.split(':').next().unwrap_or_default();
                format!("{first} | {kind}")
            }
            None => message.0,
        };
        let level = if level == Level::WARN {
            "WARN"
        } else {
            "ERROR"
        };
        self.0.0.lock().unwrap().push(format!("{level} {line}"));
    }
}

fn file_list(target: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push(format!("{relative}:{}", entry.metadata().unwrap().len()));
            }
        }
    }
    let mut files = Vec::new();
    walk(target, target, &mut files);
    files.sort();
    files
}

#[test]
fn local_client_matches_dotnet() {
    let logs = Arc::new(Logs::default());
    let subscriber = tracing_subscriber::registry().with(Collector(logs.clone()));
    let _guard = tracing::subscriber::set_default(subscriber);

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity");
    let mut names: Vec<String> = std::fs::read_dir(root.join("local"))
        .unwrap()
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| n.strip_suffix(".json"))
                .map(str::to_string)
        })
        .collect();
    names.sort();
    let mut failures = Vec::new();
    for name in &names {
        let read = |path: String| -> Value {
            serde_json::from_str(&std::fs::read_to_string(root.join(path)).unwrap()).unwrap()
        };
        let spec = read(format!("local/{name}.json"));
        let baseline = read(format!("baseline/local/{name}.json"));

        let work = tempfile::tempdir().unwrap();
        let target = work.path().join("target");
        let source = work.path().join("source.txt");
        let content = spec
            .get("fileContent")
            .and_then(Value::as_str)
            .unwrap_or("hello world");
        std::fs::write(&source, content).unwrap();
        for existing in spec
            .get("existing")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let path = target.join(existing.as_str().unwrap());
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, content).unwrap();
        }
        if spec
            .get("deleteSource")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            std::fs::remove_file(&source).unwrap();
        }
        let work_text = work.path().display().to_string();

        for (index, step) in spec["steps"].as_array().unwrap().iter().enumerate() {
            let int =
                |key: &str, default: i64| step.get(key).and_then(Value::as_i64).unwrap_or(default);
            let flag = |key: &str| step.get(key).and_then(Value::as_bool).unwrap_or(false);
            logs.0.lock().unwrap().clear();
            let config = UpDownClientConfig::new(
                "TH",
                "obj",
                1,
                1,
                0,
                int("fileSize", 11),
                EnumBucketTypes::from_name(
                    step.get("bucketType")
                        .and_then(Value::as_str)
                        .unwrap_or("None"),
                ),
                flag("etagCheck"),
                1000,
                0,
                false,
                false,
            );
            let client = LocalClient::new(
                &target,
                int("thread", 1) as i32,
                &source,
                config,
                flag("multipart"),
            );
            let max_count = int("maxCount", 0) as i32;
            let start = int("start", 0) as i32;
            let result = match step["op"].as_str().unwrap() {
                "prepare" => client.prepare(max_count, flag("check"), start),
                "read-v2" => client.read_v2(max_count, start),
                "read" => client.read(),
                "delete" => client.delete(),
                other => panic!("알 수 없는 op: {other}"),
            };
            let stats = client.stats();
            let actual = json!({
                "quit": client.quit(),
                "stats": {
                    "write": [stats.write.success(), stats.write.error()],
                    "read": [stats.read.success(), stats.read.error()],
                    "delete": [stats.delete.success(), stats.delete.error()],
                },
                "logs": logs.0.lock().unwrap().iter().map(|l| l.replace(&work_text, "<WORK>")).collect::<Vec<_>>(),
                "error": result.err().map(|e| e.to_string().split(':').next().unwrap_or_default().to_string()),
                "files": file_list(&target),
            });
            let expected_step = &baseline["steps"][index];
            let expected = json!({
                "quit": expected_step["quit"],
                "stats": expected_step["stats"],
                "logs": expected_step["logs"],
                "error": expected_step["error"].get("type"),
                "files": expected_step["files"],
            });
            if actual != expected {
                failures.push(format!(
                    "{name} 단계 {index}\n  expected {expected}\n  actual   {actual}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
