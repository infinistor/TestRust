//! 명령행 실행 비교 하네스. 같은 사례를 `TestCore.exe`(기준 생성)와 `TestRust`(비교)로 실행한다.
//!
//! 사례마다 캡처 서버를 띄우고 임시 작업 디렉터리에 `config.ini`(서버 주소를 넣은 템플릿)와 입력 파일을 만든 뒤
//! 실행 파일을 자식 프로세스로 돌린다. 표준 출력·표준 오류·종료 코드·서버가 받은 요청을 정규화해 돌려준다.
//!
//! 정규화
//! - 로그 시각(`INFO  2026-10-07 15:04:05 : `) → `<TIME>`, `123ms` → `<N>ms`, 서버 주소 → `<HOST>`, 작업 디렉터리 → `<DIR>`
//! - 예외 스택 추적 줄(`   at `, ` ---> `, `   --- End of`)은 버린다.
//! - `dump` 사례는 SDK 응답 JSON 덤프 블록(0열의 `{`·`[`부터 짝이 되는 `}`·`]`까지)을 버린다.
//! - 요청은 메서드·경로·쿼리(정렬)·`x-`/`range` 헤더(시각·서명·체크섬 등 SDK마다 다른 것 제외)·본문(XML은 의미 비교용
//!   정규형, 그 밖은 `aws-chunked`·HTTP 청크를 푼 내용의 MD5와 길이)으로 바꾼다. 서버가 받은 순서대로 비교하고,
//!   동시 요청 사례(`"unordered": true`)만 정렬해서 비교한다.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::Duration;

use md5::{Digest, Md5};
use regex::Regex;
use serde_json::{Value, json};

#[path = "fake_mysql.rs"]
mod fake_mysql;

use super::http_capture::{CannedResponse, CaptureServer, CapturedRequest};
use super::xml_canon::canonical_xml;

/// 기본 설정 파일. `{URL}`은 캡처 서버 주소로 바뀐다. 사례의 `config`가 있으면 그것을 쓴다.
pub const DEFAULT_CONFIG: &str = "[Default]\r\nObjectPrefix = temp\r\nFileSize = 1K\r\nPartSize = 5M\r\n\r\n[Main User]\r\nURL = {URL}\r\nAccessKey = testcore-access\r\nSecretKey = testcore-secret-key-0123456789abcdefghij\r\n\r\n[Alt User]\r\nURL = {URL}\r\nAccessKey = testcore-alt-access\r\nSecretKey = testcore-alt-secret-0123456789abcdefghij\r\n";

/// 실행 사례.
#[derive(Debug, Clone)]
pub struct CliCase {
    pub name: String,
    pub args: Vec<String>,
    /// 설정 파일 내용(`{URL}` 치환). 없으면 `DEFAULT_CONFIG`.
    pub config: Option<String>,
    /// 요청 줄에 `contains`가 들어 있으면 해당 응답.
    pub routes: Vec<(String, CannedResponse)>,
    /// 맞는 경로가 없을 때의 응답.
    pub default_response: CannedResponse,
    /// 작업 디렉터리에 만들 파일(상대 경로, 내용).
    pub files: Vec<(String, String)>,
    /// SDK 응답 JSON 덤프를 비교에서 뺄지.
    pub dump: bool,
    /// 요청을 동시에 보내는 사례(멀티파트 전송, 버킷 비우기 등). 요청 순서를 비교하지 않는다.
    pub unordered: bool,
    /// 통계 줄(평균·대역폭·시간)의 숫자와 단위를 `<N>`으로 가린다. 건수는 비교한다.
    pub stats: bool,
    /// XML이 아닌 요청 본문은 MD5 없이 길이만 비교한다(무작위 본문).
    pub ignore_body: bool,
    /// 이 정규식에 맞는 출력 줄은 버린다(시간에 따라 횟수가 달라지는 진행 출력 등).
    pub drop_lines: Option<String>,
    /// 이 정규식에 맞는 부분은 `<RAND>`로 바꾼다(출력·요청 경로의 무작위 버킷 이름 등).
    pub mask: Option<String>,
    /// 가짜 MySQL 서버가 사용량 조회마다 돌려줄 행(`[파일 수, 사용량]`, `null`이면 행 없음). 있으면 서버를 띄운다.
    pub db_rows: Option<Vec<Option<(i64, i64)>>>,
    /// 작업 디렉터리에 만들 빈 디렉터리(상대 경로).
    pub dirs: Vec<String>,
    /// 요청 줄에 `contains`가 들어 있으면 응답을 `ms`밀리초 늦춘다(시간 제한 시나리오의 요청 횟수를 정한다).
    pub delays: Vec<(String, u64)>,
    /// 실행 뒤 내용을 비교할 파일·디렉터리(상대 경로, 디렉터리는 아래 파일 전부).
    pub outputs: Vec<String>,
}

impl CliCase {
    /// 인자만 있는 사례(기본 설정, 모든 요청에 빈 200 응답).
    pub fn args(name: impl Into<String>, args: &[&str]) -> Self {
        Self {
            name: name.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
            config: None,
            routes: Vec::new(),
            default_response: CannedResponse {
                status: 200,
                ..CannedResponse::default()
            },
            files: Vec::new(),
            dump: false,
            unordered: false,
            stats: false,
            ignore_body: false,
            drop_lines: None,
            mask: None,
            db_rows: None,
            dirs: Vec::new(),
            outputs: Vec::new(),
            delays: Vec::new(),
        }
    }

    /// 사례 JSON(`tests/parity/cli/run/**/*.json`)을 읽는다.
    ///
    /// ```json
    /// { "args": [...], "config": "...", "dump": true,
    ///   "routes": [{ "contains": "GET /b?acl", "status": 200, "responseBody": "...", "responseHeaders": {} }],
    ///   "default": { "status": 404, "responseBody": "..." },
    ///   "files": { "parts.json": "..." } }
    /// ```
    pub fn from_json(name: impl Into<String>, value: &Value) -> Self {
        let response = |v: &Value| CannedResponse {
            status: v["status"].as_u64().unwrap_or(200) as u16,
            headers: v
                .get("responseHeaders")
                .and_then(Value::as_object)
                .map(|h| {
                    h.iter()
                        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                        .collect()
                })
                .unwrap_or_default(),
            body: v
                .get("responseBody")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        };
        let mut case = Self::args(name, &[]);
        case.args = value["args"]
            .as_array()
            .expect("args")
            .iter()
            .map(|a| a.as_str().unwrap().to_string())
            .collect();
        case.config = value
            .get("config")
            .and_then(Value::as_str)
            .map(str::to_string);
        case.dump = value.get("dump").and_then(Value::as_bool).unwrap_or(false);
        case.unordered = value
            .get("unordered")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let flag = |name: &str| value.get(name).and_then(Value::as_bool).unwrap_or(false);
        case.stats = flag("stats");
        case.ignore_body = flag("ignore_body");
        case.drop_lines = value
            .get("drop_lines")
            .and_then(Value::as_str)
            .map(str::to_string);
        case.mask = value
            .get("mask")
            .and_then(Value::as_str)
            .map(str::to_string);
        case.db_rows = value.get("db_rows").and_then(Value::as_array).map(|rows| {
            rows.iter()
                .map(|row| {
                    row.as_array()
                        .map(|r| (r[0].as_i64().unwrap(), r[1].as_i64().unwrap()))
                })
                .collect()
        });
        let strings = |name: &str| -> Vec<String> {
            value
                .get(name)
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect())
                .unwrap_or_default()
        };
        case.dirs = strings("dirs");
        case.outputs = strings("outputs");
        if let Some(delays) = value.get("delays").and_then(Value::as_array) {
            case.delays = delays
                .iter()
                .map(|d| {
                    (
                        d["contains"].as_str().unwrap().to_string(),
                        d["ms"].as_u64().unwrap(),
                    )
                })
                .collect();
        }
        if let Some(routes) = value.get("routes").and_then(Value::as_array) {
            case.routes = routes
                .iter()
                .map(|r| (r["contains"].as_str().unwrap().to_string(), response(r)))
                .collect();
        }
        if let Some(default) = value.get("default") {
            case.default_response = response(default);
        }
        if let Some(files) = value.get("files").and_then(Value::as_object) {
            case.files = files
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                .collect();
        }
        case
    }
}

/// 실행 결과(정규화 후).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliOutcome {
    pub stdout: Vec<String>,
    pub stderr: Vec<String>,
    pub exit_code: i32,
    pub requests: Vec<Value>,
    /// 사례의 `outputs`(상대 경로 → 정규화한 줄). 비어 있으면 기준 출력에 쓰지 않는다.
    pub outputs: BTreeMap<String, Vec<String>>,
}

impl CliOutcome {
    pub fn to_json(&self) -> Value {
        let mut value = json!({
            "stdout": self.stdout,
            "stderr": self.stderr,
            "exitCode": self.exit_code,
            "requests": self.requests,
        });
        if !self.outputs.is_empty() {
            value["outputs"] = json!(self.outputs);
        }
        value
    }

    pub fn from_json(value: &Value) -> Self {
        let lines = |v: &Value| -> Vec<String> {
            v.as_array()
                .unwrap()
                .iter()
                .map(|l| l.as_str().unwrap().to_string())
                .collect()
        };
        Self {
            stdout: lines(&value["stdout"]),
            stderr: lines(&value["stderr"]),
            exit_code: value["exitCode"].as_i64().unwrap() as i32,
            requests: value["requests"].as_array().unwrap().clone(),
            outputs: value
                .get("outputs")
                .and_then(Value::as_object)
                .map(|o| o.iter().map(|(k, v)| (k.clone(), lines(v))).collect())
                .unwrap_or_default(),
        }
    }
}

/// 출력 인코딩. TestCore.exe와 TestRust 모두 UTF-8로 쓴다(시스템 코드 페이지로 쓰는 프로그램을 비교할 때만 `Cp949`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum OutputEncoding {
    Utf8,
    Cp949,
}

/// 사례를 실행한다. `exe`는 실행 파일, 작업 디렉터리는 사례마다 새로 만든다.
pub async fn run_case(exe: &Path, case: &CliCase, encoding: OutputEncoding) -> CliOutcome {
    let delays = case
        .delays
        .iter()
        .map(|(contains, ms)| (contains.clone(), Duration::from_millis(*ms)))
        .collect();
    let server = CaptureServer::start_with_delays(
        case.default_response.clone(),
        case.routes.clone(),
        delays,
    )
    .await;
    let host = format!("127.0.0.1:{}", server.port);
    let dir = tempfile::tempdir().unwrap();
    // `db_rows`가 있으면 사용량 조회에 그 행을 돌려주는 가짜 MySQL 서버를 띄운다(설정의 `{DBPORT}`).
    let db = match &case.db_rows {
        Some(rows) => Some(fake_mysql::FakeMysql::start(rows.clone()).await),
        None => None,
    };
    let config = case
        .config
        .as_deref()
        .unwrap_or(DEFAULT_CONFIG)
        .replace("{URL}", &format!("http://{host}"))
        .replace("{DBPORT}", &db.as_ref().map_or(0, |db| db.port).to_string());
    std::fs::write(dir.path().join("config.ini"), config).unwrap();
    for (path, content) in &case.files {
        let path = dir.path().join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }
    for path in &case.dirs {
        std::fs::create_dir_all(dir.path().join(path)).unwrap();
    }

    let child = tokio::process::Command::new(exe)
        .args(&case.args)
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap_or_else(|e| panic!("{}: {e}", exe.display()));
    let output = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())
        .await
        .unwrap_or_else(|_| panic!("{}: 시간 초과", case.name))
        .unwrap();

    let decode = |bytes: &[u8]| match encoding {
        OutputEncoding::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
        OutputEncoding::Cp949 => encoding_rs::EUC_KR.decode(bytes).0.into_owned(),
    };
    let short = dir.path().display().to_string();
    let long = std::fs::canonicalize(dir.path())
        .map(|p| {
            p.display()
                .to_string()
                .trim_start_matches(r"\\?\")
                .to_string()
        })
        .unwrap_or_else(|_| short.clone());
    let mut dirs = Vec::new();
    for d in [long, short] {
        for v in [d.clone(), d.replace('\\', "/")] {
            if !dirs.contains(&v) {
                dirs.push(v);
            }
        }
    }
    // 오브젝트 키에 로컬 경로가 들어가는 시나리오(IoTest)는 요청 줄에 퍼센트 인코딩한 경로가 나온다.
    for d in dirs.clone() {
        for keep_slash in [false, true] {
            let encoded = percent_encode(&d, keep_slash);
            if !dirs.contains(&encoded) {
                dirs.push(encoded);
            }
        }
    }
    // 긴 표기가 짧은 표기를 포함할 수 있어 긴 것부터 바꾼다.
    dirs.sort_by_key(|d| std::cmp::Reverse(d.len()));
    let context = Context {
        host: &host,
        dirs,
        stats: case.stats,
        ignore_body: case.ignore_body,
        drop_lines: case.drop_lines.as_deref().map(|r| Regex::new(r).unwrap()),
        mask: case.mask.as_deref().map(|r| Regex::new(r).unwrap()),
    };
    // 요청은 서버가 받은 순서대로 둔다(동시 요청 사례는 비교할 때 정렬한다).
    let requests: Vec<Value> = server
        .requests()
        .iter()
        .map(|r| normalize_request(r, &context))
        .collect();
    CliOutcome {
        stdout: normalize_output(&decode(&output.stdout), &context, case.dump),
        stderr: normalize_output(&decode(&output.stderr), &context, false),
        exit_code: output.status.code().unwrap_or(i32::MIN),
        requests,
        outputs: {
            let mut outputs = collect_outputs(dir.path(), &case.outputs, &context);
            // 가짜 DB가 받은 사용량 조회(SQL 문자열)
            if let Some(db) = &db {
                if std::env::var_os("FAKE_MYSQL_LOG").is_some() {
                    eprintln!("[{}] 받은 질의: {:#?}", case.name, db.all_queries());
                }
                outputs.insert("<db>".to_string(), db.queries.lock().unwrap().clone());
            }
            outputs
        },
    }
}

/// RFC 3986 비예약 문자(`A-Za-z0-9-._~`)만 남기고 대문자 16진수로 퍼센트 인코딩한다.
fn percent_encode(text: &str, keep_slash: bool) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) || (keep_slash && byte == b'/') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// 사례의 `outputs`를 읽어 정규화한다. 디렉터리는 아래 파일 전부(이름의 `yyyyMMdd_HHmmss`는 `<TS>`).
fn collect_outputs(
    root: &Path,
    outputs: &[String],
    context: &Context<'_>,
) -> BTreeMap<String, Vec<String>> {
    let mut result = BTreeMap::new();
    let mut add = |name: String, path: &Path| {
        let lines = match std::fs::read(path) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
                text.lines()
                    .map(|line| {
                        let line = ISO_TIME.replace_all(line, "<TIME>");
                        mask_stats(&replace_context(&line, context), context)
                    })
                    .collect()
            }
            Err(_) => vec!["<없음>".to_string()],
        };
        result.insert(TIMESTAMP.replace_all(&name, "<TS>").into_owned(), lines);
    };
    for output in outputs {
        let path = root.join(output);
        if path.is_dir() {
            let mut entries: Vec<_> = walk(&path);
            entries.sort();
            for entry in entries {
                let name = entry
                    .strip_prefix(root)
                    .unwrap()
                    .display()
                    .to_string()
                    .replace('\\', "/");
                add(name, &entry);
            }
        } else {
            add(output.clone(), &path);
        }
    }
    result
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

struct Context<'a> {
    host: &'a str,
    /// 작업 디렉터리의 여러 표기(8.3 짧은 이름, 긴 이름, `/` 구분).
    dirs: Vec<String>,
    stats: bool,
    ignore_body: bool,
    drop_lines: Option<Regex>,
    mask: Option<Regex>,
}

/// JSON 결과의 시각(`2026-10-07T15:04:05.1234567+09:00`).
static ISO_TIME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d+)?(Z|[+-]\d\d:\d\d)?").unwrap()
});
/// 결과 파일 이름의 시각(`_20261007_150405`).
static TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d{8}_\d{6}").unwrap());
/// 통계 줄: 시간·속도에 따라 달라지는 값(평균, 대역폭, 경과 시간)을 담은 줄.
static STATS_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)average|bandwidth|times|elapsed|\bsec\b|/s\b|"time"|"(end|start)time""#)
        .unwrap()
});
/// 숫자(소수, 단위 포함). 자릿수에 따라 달라지는 앞쪽 맞춤 공백도 함께 가린다.
static STATS_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r" *-?\d[\d,]*(\.\d+)?( ?(Byte|[KMGTPEZY]i?B)\b)?").unwrap());

/// `stats` 사례: 통계 줄의 숫자를 가린다.
fn mask_stats(line: &str, context: &Context<'_>) -> String {
    if !context.stats {
        return line.to_string();
    }
    // 저장한 결과 파일 이름의 시각(`_yyyyMMdd_HHmmss`)도 실행마다 다르다.
    let line = TIMESTAMP.replace_all(line, "<TS>");
    if STATS_LINE.is_match(&line) {
        // 앞 공백은 한 칸으로 남긴다(`Average :    12.3` → `Average : <N>`).
        STATS_NUMBER
            .replace_all(&line, |caps: &regex::Captures<'_>| {
                if caps[0].starts_with(' ') {
                    " <N>"
                } else {
                    "<N>"
                }
            })
            .into_owned()
    } else {
        line.into_owned()
    }
}

static LOG_TIME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(INFO |ERROR|WARN |DEBUG|FATAL) \d{4}-\d\d-\d\d \d\d:\d\d:\d\d : ").unwrap()
});
static MILLIS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d+ms\b").unwrap());
/// 벽시계 시각을 찍는 줄머리(`[2026-10-07 15:04:05]Next Key Marker : ...`).
static WALL_CLOCK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\d{4}-\d\d-\d\d \d\d:\d\d:\d\d\]").unwrap());
/// 서명된 URL의 시각·서명(V4 `X-Amz-*`, V2 `Expires`·`Signature`).
static PRESIGN: LazyLock<[(Regex, &'static str); 5]> = LazyLock::new(|| {
    [
        (
            Regex::new(r"(X-Amz-Date=)\d{8}T\d{6}Z").unwrap(),
            "${1}<DATE>",
        ),
        (
            Regex::new(r"(X-Amz-Credential=[^&\s]*?(?:%2F|/))\d{8}").unwrap(),
            "${1}<DATE>",
        ),
        (
            Regex::new(r"(X-Amz-Signature=)[0-9a-f]+").unwrap(),
            "${1}<SIG>",
        ),
        (Regex::new(r"([?&]Expires=)\d+").unwrap(), "${1}<EXPIRES>"),
        (Regex::new(r"([?&]Signature=)[^&\s]+").unwrap(), "${1}<SIG>"),
    ]
});

/// 시각으로 만든 오브젝트 키(`2026/10/07/16/03/`, `2026/10/07/`): 실행 시각마다 달라져 가린다.
static DATE_PATH: LazyLock<[(Regex, &'static str); 2]> = LazyLock::new(|| {
    [
        (
            Regex::new(r"\d{4}/\d\d/\d\d/\d\d/\d\d/").unwrap(),
            "<YYYY/MM/DD/HH/mm>/",
        ),
        (Regex::new(r"\d{4}/\d\d/\d\d/").unwrap(), "<YYYY/MM/DD>/"),
    ]
});

static PRESIGN_EXPIRES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(X-Amz-Expires=)(\d+)").unwrap());

fn replace_context(text: &str, context: &Context<'_>) -> String {
    let mut text = text.replace(context.host, "<HOST>");
    // Windows 임시 경로는 8.3 짧은 이름과 긴 이름으로 모두 나타날 수 있다.
    for dir in &context.dirs {
        text = text.replace(dir.as_str(), "<DIR>");
    }
    text = WALL_CLOCK.replace_all(&text, "[<TIME>]").into_owned();
    for (pattern, replacement) in DATE_PATH.iter() {
        text = pattern.replace_all(&text, *replacement).into_owned();
    }
    for (pattern, replacement) in PRESIGN.iter() {
        text = pattern.replace_all(&text, *replacement).into_owned();
    }
    // 유효 시간은 서명 직전 현재 시각에서 계산해 초 경계를 넘으면 1초 짧아진다(.NET·Rust 모두). 분 단위로 올린다.
    text = PRESIGN_EXPIRES
        .replace_all(&text, |caps: &regex::Captures<'_>| {
            let seconds: i64 = caps[2].parse().unwrap_or_default();
            let seconds = if (seconds + 1) % 60 == 0 {
                seconds + 1
            } else {
                seconds
            };
            format!("{}{seconds}", &caps[1])
        })
        .into_owned();
    if let Some(mask) = &context.mask {
        text = mask.replace_all(&text, "<RAND>").into_owned();
    }
    text
}

fn normalize_output(text: &str, context: &Context<'_>, dump: bool) -> Vec<String> {
    let text = text.replace("\r\n", "\n");
    let mut lines = Vec::new();
    let mut depth: Option<char> = None;
    for line in text.split('\n') {
        if let Some(close) = depth {
            if line.starts_with(close) {
                depth = None;
            }
            continue;
        }
        if dump {
            if matches!(line, "{}" | "[]" | "null") {
                continue;
            }
            if line == "{" || line == "[" {
                depth = Some(if line == "{" { '}' } else { ']' });
                continue;
            }
        }
        if line.starts_with("   at ")
            || line.starts_with(" ---> ")
            || line.starts_with("   --- End of")
        {
            continue;
        }
        let line = LOG_TIME.replace(line, "$1 <TIME> : ");
        let line = MILLIS.replace_all(&line, "<N>ms");
        let line = mask_stats(&replace_context(&line, context), context);
        if context
            .drop_lines
            .as_ref()
            .is_some_and(|r| r.is_match(&line))
        {
            continue;
        }
        lines.push(line);
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// SDK마다 다른 헤더(시각, 서명, 본문 해시·체크섬, 청크 길이).
const VOLATILE_HEADERS: &[&str] = &[
    "x-amz-date",
    "x-amz-content-sha256",
    "x-amz-user-agent",
    "x-amz-sdk-checksum-algorithm",
    "x-amz-trailer",
    "x-amz-decoded-content-length",
    "x-amz-security-token",
    "x-amz-api-version",
];

fn normalize_request(request: &CapturedRequest, context: &Context<'_>) -> Value {
    let (method, target) = request.line.split_once(' ').unwrap_or((&request.line, ""));
    let target = target.rsplit_once(' ').map_or(target, |(t, _)| t);
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut query: Vec<&str> = query
        .split('&')
        .filter(|q| !q.is_empty() && *q != "x-id=")
        .collect();
    query.retain(|q| !q.starts_with("x-id="));
    query.sort_unstable();
    let mut headers: Vec<String> = request
        .headers
        .iter()
        .filter_map(|(name, value)| {
            let name = name.to_ascii_lowercase();
            let keep = (name.starts_with("x-") || name == "range")
                && !VOLATILE_HEADERS.contains(&name.as_str())
                && !name.starts_with("x-amz-checksum-");
            keep.then(|| format!("{name}: {}", replace_context(value, context)))
        })
        .collect();
    headers.sort();
    let payload = decode_payload(request);
    let body = if payload.is_empty() {
        Value::Null
    } else if let Some(xml) = std::str::from_utf8(&payload).ok().and_then(canonical_xml) {
        Value::String(xml)
    } else if context.ignore_body {
        Value::String(format!("{} bytes", payload.len()))
    } else {
        Value::String(format!(
            "{} bytes md5={}",
            payload.len(),
            hex::encode(Md5::digest(&payload))
        ))
    };
    json!({
        "method": method,
        "path": replace_context(path, context),
        "query": query,
        "headers": headers,
        "body": body,
    })
}

/// HTTP 청크와 `aws-chunked`(청크 서명·트레일러)를 푼 본문.
fn decode_payload(request: &CapturedRequest) -> Vec<u8> {
    let mut body = request.body.as_bytes().to_vec();
    if request
        .header("Transfer-Encoding")
        .is_some_and(|v| v.eq_ignore_ascii_case("chunked"))
    {
        body = dechunk(&body);
    }
    let aws_chunked = request
        .header("Content-Encoding")
        .is_some_and(|v| v.contains("aws-chunked"))
        || request
            .header("x-amz-content-sha256")
            .is_some_and(|v| v.starts_with("STREAMING-"));
    if aws_chunked {
        body = dechunk(&body);
    }
    body
}

/// `<16진수 크기>[;확장]\r\n<데이터>\r\n` 반복을 푼다. 크기 0 뒤(트레일러)는 버린다.
fn dechunk(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut rest = data;
    while let Some(end) = rest.windows(2).position(|w| w == b"\r\n") {
        let header = String::from_utf8_lossy(&rest[..end]);
        let Ok(size) = usize::from_str_radix(header.split(';').next().unwrap_or("").trim(), 16)
        else {
            return data.to_vec();
        };
        if size == 0 {
            break;
        }
        let start = end + 2;
        if rest.len() < start + size {
            return data.to_vec();
        }
        out.extend_from_slice(&rest[start..start + size]);
        rest = rest.get(start + size + 2..).unwrap_or_default();
    }
    out
}

/// `TESTCORE_BIN` 환경 변수의 `TestCore.exe`.
pub fn testcore_exe() -> Option<PathBuf> {
    let bin = std::env::var_os("TESTCORE_BIN")?;
    let exe = Path::new(&bin).join(if cfg!(windows) {
        "TestCore.exe"
    } else {
        "TestCore"
    });
    exe.exists().then_some(exe)
}

/// 기준 출력과 실제 출력의 차이를 사람이 읽기 좋게 보여 준다.
pub fn diff(expected: &CliOutcome, actual: &CliOutcome) -> String {
    let mut out = String::new();
    let lines = |label: &str, e: &[String], a: &[String], out: &mut String| {
        if e != a {
            out.push_str(&format!("  [{label}]\n"));
            for i in 0..e.len().max(a.len()) {
                let (el, al) = (e.get(i), a.get(i));
                if el != al {
                    out.push_str(&format!(
                        "    {i:>3} - {}\n",
                        el.map_or("<없음>", String::as_str)
                    ));
                    out.push_str(&format!(
                        "    {i:>3} + {}\n",
                        al.map_or("<없음>", String::as_str)
                    ));
                }
            }
        }
    };
    lines("stdout", &expected.stdout, &actual.stdout, &mut out);
    lines("stderr", &expected.stderr, &actual.stderr, &mut out);
    for (name, e) in &expected.outputs {
        let a = actual.outputs.get(name).map_or(&[][..], Vec::as_slice);
        lines(&format!("output {name}"), e, a, &mut out);
    }
    for (name, a) in &actual.outputs {
        if !expected.outputs.contains_key(name) {
            lines(&format!("output {name}"), &[], a, &mut out);
        }
    }
    if expected.exit_code != actual.exit_code {
        out.push_str(&format!(
            "  [exit] {} != {}\n",
            expected.exit_code, actual.exit_code
        ));
    }
    if expected.requests != actual.requests {
        out.push_str("  [requests]\n");
        let e: Vec<String> = expected.requests.iter().map(Value::to_string).collect();
        let a: Vec<String> = actual.requests.iter().map(Value::to_string).collect();
        for r in e.iter().filter(|r| !a.contains(r)) {
            out.push_str(&format!("    - {r}\n"));
        }
        for r in a.iter().filter(|r| !e.contains(r)) {
            out.push_str(&format!("    + {r}\n"));
        }
    }
    out
}

impl CliOutcome {
    /// `stats` 사례 비교용: 가린 숫자(`<N>`)가 든 줄은 값 길이에 따라 맞춤 공백이 달라지므로 연속 공백을 하나로 본다.
    pub fn squash_stats(&self) -> Self {
        static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" {2,}").unwrap());
        let squash = |lines: &[String]| -> Vec<String> {
            lines
                .iter()
                .map(|line| {
                    if line.contains("<N>") {
                        SPACES.replace_all(line, " ").into_owned()
                    } else {
                        line.clone()
                    }
                })
                .collect()
        };
        Self {
            stdout: squash(&self.stdout),
            stderr: squash(&self.stderr),
            outputs: self
                .outputs
                .iter()
                .map(|(name, lines)| (name.clone(), squash(lines)))
                .collect(),
            ..self.clone()
        }
    }

    /// 요청 순서를 무시하고 비교할 수 있게 정렬한 사본.
    pub fn sorted(&self) -> Self {
        let mut requests = self.requests.clone();
        requests.sort_by_key(Value::to_string);
        Self {
            requests,
            ..self.clone()
        }
    }
}
