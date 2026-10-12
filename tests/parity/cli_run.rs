//! 명령행 실행 비교: 같은 인자·설정·서버 응답으로 `TestRust`가 `TestCore.exe`와 같은 출력·종료 코드·요청을 내는지 본다.
//!
//! 사례
//! - 자동 사례(`baseline/cli-run/top.json`, `help.json`, `bare.json`): 최상위 흐름, 메뉴마다 `--X --help`,
//!   S3·KSAN 메뉴를 인자 없이 실행(필수 값 검증 문구).
//! - 파일 사례(`cli/run/**/*.json` → `baseline/cli-run/<같은 경로>.json`): 서버 응답을 정한 실제 실행.
//!
//! `dispatch::is_ported`가 `false`인 메뉴(원본 디스패처에 `case`가 없는 메뉴)의 사례는 건너뛴다.
//!
//! 기준 출력 다시 만들기(TESTCore HEAD 빌드 필요, `tests/parity/README.md`):
//! `$env:TESTCORE_BIN = ...; cargo test -p testrust-cli --test parity_cli_run -- --ignored generate`
//! (`CLI_RUN_FILTER`에 이름 일부를 주면 그 사례만 다시 만든다)

#[path = "support/cli_harness.rs"]
mod cli_harness;
#[path = "support/http_capture.rs"]
#[allow(dead_code)]
mod http_capture;
#[path = "support/xml_canon.rs"]
mod xml_canon;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cli_harness::{CliCase, CliOutcome, OutputEncoding, diff, run_case, testcore_exe};
use serde_json::Value;
use testrust_cli::dispatch::is_ported;
use testrust_cli::menu::MenuList;
use testrust_cli::options::{Action, OPTIONS, option_names, parse};

fn parity_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity")
}

/// 자동 사례 묶음 이름과 사례들.
fn auto_cases() -> Vec<(&'static str, Vec<CliCase>)> {
    let top = vec![
        CliCase::args("no-args", &[]),
        CliCase::args("help", &["--help"]),
        CliCase::args("worker-help", &["--worker", "--help"]),
        CliCase::args("invalid-option", &["--foo"]),
        CliCase::args("invalid-extra", &["foo", "--list-buckets", "bar"]),
        CliCase::args("bad-int", &["--part-number=x"]),
        CliCase::args("bad-bool", &["--print=yes"]),
        CliCase::args("missing-value", &["--bucket"]),
        CliCase::args("missing-value-short", &["--list-buckets", "-b"]),
        CliCase::args("bundle-error", &["-vx"]),
        CliCase::args(
            "checksum-type-bad",
            &["--put-object", "--checksum-type=md5"],
        ),
        CliCase::args("size-format", &["--put-object", "--size=abc"]),
        CliCase::args("size-overflow", &["--size=99999999999999999999"]),
        CliCase::args("range-list-null", &["--range-list-"]),
        CliCase::args("config-missing", &["--list-buckets", "-c", "nope.ini"]),
        CliCase::args(
            "config-missing-help",
            &["--list-buckets", "--help", "-c", "nope.ini"],
        ),
        CliCase::args("bool-plus-int", &["--port+"]),
    ];

    let mut help = Vec::new();
    let mut bare = Vec::new();
    let mut in_api = false;
    for def in OPTIONS {
        let name = option_names(def).last().unwrap();
        match def.action {
            Action::Menu(_) => {
                if name == "abort-multipart-upload" {
                    in_api = true;
                }
                help.push(CliCase::args(name, &[&format!("--{name}"), "--help"]));
                if in_api {
                    bare.push(CliCase::args(name, &[&format!("--{name}")]));
                }
                if name == "put-bucket-tagindex" {
                    in_api = false;
                }
            }
            // 값을 받으면서 메뉴를 정하는 옵션(`--pause=`, `--clear=` 등).
            Action::Text(_) | Action::Bool(_) if menu_of(&[format!("--{name}=true")]).is_some() => {
                help.push(CliCase::args(name, &[&format!("--{name}=true"), "--help"]));
            }
            _ => {}
        }
    }
    vec![("top", top), ("help", help), ("bare", bare)]
}

fn menu_of(args: &[String]) -> Option<MenuList> {
    parse(args)
        .ok()
        .map(|r| r.options.menu)
        .filter(|m| *m != MenuList::None)
}

/// `cli/run/**/*.json` 사례. 이름은 `cli/run` 기준 상대 경로(확장자 제외, `/` 구분).
fn file_cases() -> Vec<CliCase> {
    let root = parity_dir().join("cli/run");
    let mut cases = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "json") {
                let name = path
                    .strip_prefix(&root)
                    .unwrap()
                    .with_extension("")
                    .to_string_lossy()
                    .replace('\\', "/");
                let value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap())
                    .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                cases.push(CliCase::from_json(name, &value));
            }
        }
    }
    cases.sort_by(|a, b| a.name.cmp(&b.name));
    cases
}

fn baseline_path(group: Option<&str>, name: &str) -> PathBuf {
    let dir = parity_dir().join("baseline/cli-run");
    match group {
        Some(group) => dir.join(format!("{group}.json")),
        None => dir.join(format!("{name}.json")),
    }
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
}

fn write_json(path: &Path, value: &Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut text = serde_json::to_string_pretty(value).unwrap();
    text.push('\n');
    std::fs::write(path, text).unwrap();
}

/// 사례를 동시에 몇 개씩 실행한다.
async fn run_all(
    exe: &Path,
    cases: Vec<CliCase>,
    encoding: OutputEncoding,
) -> Vec<(CliCase, CliOutcome)> {
    let semaphore = Arc::new(tokio::sync::Semaphore::new(8));
    let mut tasks = Vec::new();
    for case in cases {
        let semaphore = semaphore.clone();
        let exe = exe.to_path_buf();
        tasks.push(tokio::spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();
            let outcome = run_case(&exe, &case, encoding).await;
            (case, outcome)
        }));
    }
    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await.unwrap());
    }
    results
}

/// 옮긴 메뉴의 사례인지(파싱 오류·메뉴 없음은 최상위 흐름이라 항상 비교한다).
fn enabled(case: &CliCase) -> bool {
    menu_of(&case.args).is_none_or(is_ported)
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_run_matches_dotnet() {
    let exe = Path::new(env!("CARGO_BIN_EXE_TestRust"));
    let mut expected = BTreeMap::new();
    let mut cases = Vec::new();
    for (group, group_cases) in auto_cases() {
        let baseline = read_json(&baseline_path(Some(group), "")).unwrap_or(Value::Null);
        for case in group_cases {
            if let Some(outcome) = baseline.get(&case.name) {
                expected.insert(
                    format!("{group}/{}", case.name),
                    CliOutcome::from_json(outcome),
                );
                cases.push(CliCase {
                    name: format!("{group}/{}", case.name),
                    ..case
                });
            }
        }
    }
    for case in file_cases() {
        let path = baseline_path(None, &case.name);
        let baseline =
            read_json(&path).unwrap_or_else(|| panic!("기준 출력이 없다: {}", path.display()));
        expected.insert(case.name.clone(), CliOutcome::from_json(&baseline));
        cases.push(case);
    }
    let cases: Vec<CliCase> = cases.into_iter().filter(enabled).collect();
    assert!(!cases.is_empty());
    let total = cases.len();
    let mut failures = Vec::new();
    for (case, actual) in run_all(exe, cases, OutputEncoding::Utf8).await {
        let (expected, actual) = if case.unordered {
            (expected[&case.name].sorted(), actual.sorted())
        } else {
            (expected[&case.name].clone(), actual)
        };
        let (expected, actual) = if case.stats {
            (expected.squash_stats(), actual.squash_stats())
        } else {
            (expected, actual)
        };
        if expected != actual {
            failures.push(format!(
                "{} {:?}\n{}",
                case.name,
                case.args,
                diff(&expected, &actual)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{}/{total} 건 불일치:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "TESTCORE_BIN이 필요한 기준 출력 생성"]
async fn generate() {
    let exe = testcore_exe().expect("TESTCORE_BIN에 TestCore.exe가 있어야 한다");
    let filter = std::env::var("CLI_RUN_FILTER").unwrap_or_default();
    for (group, group_cases) in auto_cases() {
        let path = baseline_path(Some(group), "");
        let cases: Vec<CliCase> = group_cases
            .into_iter()
            .filter(|c| format!("{group}/{}", c.name).contains(&filter))
            .collect();
        if cases.is_empty() {
            continue;
        }
        let mut baseline = match read_json(&path) {
            Some(Value::Object(map)) => map,
            _ => serde_json::Map::new(),
        };
        for (case, outcome) in run_all(&exe, cases, OutputEncoding::Utf8).await {
            baseline.insert(case.name, outcome.to_json());
        }
        write_json(&path, &Value::Object(baseline));
    }
    let cases: Vec<CliCase> = file_cases()
        .into_iter()
        .filter(|c| c.name.contains(&filter))
        .collect();
    for (case, outcome) in run_all(&exe, cases, OutputEncoding::Utf8).await {
        // 동시 요청 사례는 받은 순서가 실행마다 달라 정렬해 저장한다(비교도 정렬해서 한다).
        let outcome = if case.unordered {
            outcome.sorted()
        } else {
            outcome
        };
        write_json(&baseline_path(None, &case.name), &outcome.to_json());
    }
}
