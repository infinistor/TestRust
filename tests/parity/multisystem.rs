//! `MultiSystemClient`가 TESTCore `MultiSystemClient`와 같은 시스템·순서로 요청하고 같은 카운터·로그를 남기는지 확인한다.
//! 기준 출력은 `tools/dotnet-oracle`의 `multisystem` 명령으로 만든다. 게이트웨이·구·신 시스템마다 서버를 하나씩 띄운다.
//! 로그는 메시지 첫 줄과 예외 형식 이름(있으면)을 비교한다.

#[path = "support/route_server.rs"]
mod route_server;

use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use route_server::{RouteServer, routes};
use serde_json::{Value, json};
use testrust_clients::MultiSystemClient;
use testrust_config::{EnumBucketTypes, MultiSystemClientConfig, UserData};
use testrust_s3::S3Client;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};

#[derive(Default)]
struct Logs(Mutex<Vec<String>>);

struct Collector(Arc<Logs>);

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
        let line = match message.0.split_once('\n') {
            Some((first, exception)) => {
                format!(
                    "{first} | {}",
                    exception.split(':').next().unwrap_or_default()
                )
            }
            None => message.0,
        };
        self.0.0.lock().unwrap().push(line);
    }
}

#[tokio::test]
async fn multi_system_client_matches_dotnet() {
    let logs = Arc::new(Logs::default());
    let subscriber = tracing_subscriber::registry().with(Collector(logs.clone()));
    let _guard = tracing::subscriber::set_default(subscriber);

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity");
    let mut names: Vec<String> = std::fs::read_dir(root.join("multisystem"))
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
        let spec = read(format!("multisystem/{name}.json"));
        let baseline = read(format!("baseline/multisystem/{name}.json"));
        let int =
            |key: &str, default: i64| spec.get(key).and_then(Value::as_i64).unwrap_or(default);

        let work = tempfile::tempdir().unwrap();
        let file_path = work.path().join("body.txt");
        std::fs::write(
            &file_path,
            spec.get("fileContent")
                .and_then(Value::as_str)
                .unwrap_or("hello world"),
        )
        .unwrap();

        let lines = Arc::new(Mutex::new(Vec::new()));
        let mut servers = Vec::new();
        let mut clients = Vec::new();
        for system in ["gateway", "old", "new"] {
            // 요청 줄 앞에 시스템 이름을 붙여 공용 목록에 넣는다.
            let shared = lines.clone();
            let own = Arc::new(Mutex::new(Vec::new()));
            let hook: route_server::Hook = Arc::new(move |line: &str| {
                shared.lock().unwrap().push(format!("{system} {line}"));
            });
            let server = RouteServer::start(routes(&spec["systems"][system]), own, hook).await;
            let user = UserData::new(
                format!("http://127.0.0.1:{}", server.port),
                "",
                "AKIAEXAMPLE",
                "secretExample",
            );
            clients.push(S3Client::from_user(&user, false, 0, false));
            servers.push(server);
        }
        let config = MultiSystemClientConfig::new(
            "TH",
            "obj",
            int("fileCount", 2) as i32,
            int("fileSize", 11),
            int("partSize", 4),
            EnumBucketTypes::from_name(
                spec.get("bucketType")
                    .and_then(Value::as_str)
                    .unwrap_or("None"),
            ),
        );
        let client = MultiSystemClient::new(
            config,
            "my-bucket",
            1,
            &file_path,
            clients[0].clone(),
            clients[1].clone(),
            clients[2].clone(),
        );
        logs.0.lock().unwrap().clear();
        let result = match spec["op"].as_str().unwrap() {
            "prepare" => client.prepare().await,
            "prepare-multipart" => client.prepare_multipart().await,
            "get" => client.get().await,
            "put-get" => client.put_get().await,
            "put-get-multipart" => client.put_get_multipart().await,
            "mix" => client.mix().await,
            "mix-multipart" => client.mix_multipart().await,
            other => panic!("알 수 없는 op: {other}"),
        };
        drop(servers);

        let counter = |c: &std::sync::atomic::AtomicI32| c.load(Ordering::Relaxed);
        let actual = json!({
            "quit": client.quit(),
            "requests": lines.lock().unwrap().clone(),
            "counters": [
                counter(&client.write_count), counter(&client.write_error_count),
                counter(&client.read_count), counter(&client.read_error_count),
                counter(&client.delete_count), counter(&client.delete_error_count),
                counter(&client.part_count), counter(&client.object_count),
            ],
            "logs": logs.0.lock().unwrap().clone(),
            "error": result.err().map(|e| e.to_string()),
        });
        let expected = json!({
            "quit": baseline["quit"],
            "requests": baseline["requests"],
            "counters": baseline["counters"],
            "logs": baseline["logs"],
            "error": baseline["error"].as_object().map(|e| format!("{}: {}", e["type"].as_str().unwrap(), e["message"].as_str().unwrap())),
        });
        if actual != expected {
            failures.push(format!(
                "{name}\n  expected {expected}\n  actual   {actual}"
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
