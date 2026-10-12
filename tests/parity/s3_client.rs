//! S3Client가 TESTCore `S3Client`(AWSSDK.S3)와 같은 요청을 보내고 같은 결과를 내는지 확인한다.
//! 기준 출력은 `tools/dotnet-oracle`의 `s3` 명령으로 만든다(op 이름은 `Program.cs`, `S3Ops.cs`와
//! `support/s3_ops.rs`가 같다).
//!
//! SDK가 다르므로 다음은 비교하지 않는다: `User-Agent`, `amz-sdk-*`, `x-amz-user-agent`,
//! `Expect`, `X-Amz-Date`, 서명 값(청크·트레일러 서명 포함), `Host`의 포트, 쿼리 매개변수 순서,
//! .NET만 보내는 `x-amz-api-version`, 서명 대상 목록의 `content-length`(Rust SDK만 서명에 넣는다).
//!
//! 체크섬 없이 `UseChunkEncoding = true`로 올리는 경우는 형식이 다르다. .NET은 트레일러 없는 청크 서명
//! (`STREAMING-AWS4-HMAC-SHA256-PAYLOAD`), Rust SDK는 CRC32 트레일러가 붙은 청크 서명
//! (`STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER`)으로 보낸다. 이 사례는 `CHUNKED_CASES`에서 따로 확인하고,
//! 여러 요청이 섞인 사례는 `STRIP_TRAILER_CASES`로 트레일러 두 줄을 걷어낸 뒤 비교한다.
//!
//! 요청 XML 본문은 SDK마다 요소 순서(.NET은 이름순, Rust SDK는 모델 순서)가 다르다. 먼저 바이트 단위로 비교하고,
//! 다르면 XML 본문에 한해 형제 요소를 이름순(같은 이름은 원래 순서 유지)으로 정렬해 다시 비교한다. 이때 본문에서
//! 계산되는 `Content-Length`, `x-amz-content-sha256`, `x-amz-checksum-crc32`, `Content-MD5`는 각 요청 자신의
//! 본문과 맞는지 검증한 뒤 자리표시자로 바꾼다. 이렇게 비교한 사례는 `eprintln!`으로 알린다.

#[path = "support/http_capture.rs"]
mod http_capture;
#[path = "support/s3_ops.rs"]
mod s3_ops;
#[path = "support/xml_canon.rs"]
mod xml_canon;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use base64::Engine;
use crc::{CRC_32_ISO_HDLC, Crc};
use md5::Md5;
use sha2::{Digest, Sha256};

use http_capture::{CannedResponse, CaptureServer, CapturedRequest};
use serde_json::Value;
use testrust_s3::S3Client;
use xml_canon::canonical_xml;

const CASES: &[&str] = &[
    "list-buckets",
    "put-object-plain",
    "put-object-checksum-plain",
    "put-object-checksum-chunked",
    "put-object-admin",
    "get-object",
    "get-object-range",
    "get-object-leading-slash",
    "head-object",
    "list-objects-v2",
    "delete-objects",
    "put-bucket-versioning",
    "head-bucket-exists",
    "head-bucket-missing",
    "error-500-retry",
    "error-404",
];

const CHUNKED_CASES: &[&str] = &["put-object-chunked", "upload-part-chunked"];

/// 이 파일 아래 `NEW_CASES`는 `CASES`와 같은 방식으로 비교한다(분리한 것은 목록 관리를 위해서다).
const NEW_CASES: &[&str] = &[
    // 버킷 ACL, 소유권, 위치, 로깅, 알림, 버전 관리
    "put-bucket-acl-canned",
    "put-bucket-acl-policy",
    "get-bucket-acl",
    "list-directory-buckets",
    "delete-bucket",
    "get-bucket-ownership-controls",
    "put-bucket-ownership-controls",
    "delete-bucket-ownership-controls",
    "get-bucket-location",
    "put-bucket-logging",
    "get-bucket-logging",
    "put-bucket-notification",
    "get-bucket-notification",
    "put-bucket-versioning-suspended",
    "get-bucket-versioning",
    // CORS, 태그, 수명 주기, 정책
    "put-cors",
    "get-cors",
    "delete-cors",
    "get-bucket-tagging",
    "put-bucket-tagging",
    "delete-bucket-tagging",
    "put-lifecycle",
    "get-lifecycle",
    "delete-lifecycle",
    "put-bucket-policy",
    "get-bucket-policy",
    "delete-bucket-policy",
    "get-bucket-policy-status",
    // 객체 잠금, 퍼블릭 액세스 차단, 암호화, 웹사이트
    "put-object-lock-configuration",
    "get-object-lock-configuration",
    "put-public-access-block",
    "get-public-access-block",
    "delete-public-access-block",
    "get-bucket-encryption",
    "put-bucket-encryption",
    "delete-bucket-encryption",
    "get-bucket-website",
    "put-bucket-website",
    "delete-bucket-website",
    // 인벤토리, 메트릭, 분석
    "get-bucket-inventory",
    "list-bucket-inventory",
    "put-bucket-inventory",
    "delete-bucket-inventory",
    "get-bucket-metrics",
    "list-bucket-metrics",
    "put-bucket-metrics",
    "delete-bucket-metrics",
    "get-bucket-analytics",
    "list-bucket-analytics",
    "put-bucket-analytics",
    "delete-bucket-analytics",
    // 복제
    "get-bucket-replication",
    "put-bucket-replication",
    "delete-bucket-replication",
    // 객체
    "put-object-acl-canned",
    "put-object-acl-policy",
    "get-object-acl",
    "copy-object",
    "copy-object-special",
    "list-versions",
    "list-versions-empty-body",
    "head-bucket-empty-name",
    "put-bucket-empty-name",
    "delete-objects-errors",
    "get-object-tagging",
    "put-object-tagging",
    "delete-object-tagging",
    "get-object-retention",
    "put-object-retention",
    "put-object-retention-nobypass",
    "put-object-legal-hold",
    "get-object-legal-hold",
    "restore-object",
    "restore-object-nodays",
    "put-object-content-type-noext",
    "put-object-content-type-hidden",
    "put-object-file-plain",
    "put-object-file-xyz",
    // 멀티파트
    "initiate-multipart-upload",
    "initiate-multipart-upload-noext",
    "initiate-multipart-upload-xyz",
    "copy-part",
    "complete-multipart-upload",
    "abort-multipart-upload",
    "list-multipart-uploads",
    "list-parts",
    "list-parts-nomarker",
    // TransferUtility
    "download",
    "download-404",
    "download-existing",
    "download-newdir",
    "download-error-existing",
    "download-badetag",
    "upload-conflict",
];

/// 트레일러 없는 청크 서명(.NET)과 CRC32 트레일러가 붙은 청크 서명(Rust)의 차이를 걷어내고 비교하는 사례.
const STRIP_TRAILER_CASES: &[&str] = &[
    "put-object-file-chunked",
    "put-object-tagging-header",
    "put-object-content-type-xyz",
    "upload-part-file-chunked",
    "upload-small",
    "upload-multi",
    "upload-multi-parallel",
    "upload-exact",
    "upload-content-type",
    "upload-content-type-multi",
    "upload-noext-small",
    "upload-noext-multi",
    "upload-bytes-small",
    "upload-bytes-multi",
    "upload-stream-small",
    "upload-stream-multi",
    "upload-stream-multi-noct",
    "upload-fail",
];

/// 파일 업로드 사례 중 트레일러가 없는 것(청크 서명을 쓰지 않는 사례).
const PLAIN_UPLOAD_CASES: &[&str] = &["upload-empty", "upload-part-file-plain"];

/// 동시 업로드라 요청 순서가 다를 수 있는 사례(요청 줄 순으로 정렬해 비교한다).
const UNORDERED_CASES: &[&str] = &["upload-multi-parallel"];

/// .NET이 본문 없는 요청에 의미 없는 `Content-Type`을 붙이는 사례(복사 파트: `application/x-amz-json-1.0`).
const IGNORE_CONTENT_TYPE_CASES: &[&str] = &["copy-part"];

const IGNORED_HEADERS: &[&str] = &[
    "user-agent",
    "x-amz-user-agent",
    "amz-sdk-invocation-id",
    "amz-sdk-request",
    "expect",
    "x-amz-date",
    "x-amz-api-version",
];

/// 본문에서 계산되는 헤더. XML 본문을 의미 비교할 때 검증 후 자리표시자로 바꾼다.
const BODY_HASH_HEADERS: &[&str] = &[
    "content-length",
    "x-amz-content-sha256",
    "x-amz-checksum-crc32",
    "content-md5",
];

/// 서명 대상 목록에서 빼고 비교하는 헤더(`x-amz-date`는 둘 다 서명하므로 남긴다).
fn ignored_in_signature(header: &str) -> bool {
    header != "x-amz-date" && (IGNORED_HEADERS.contains(&header) || header == "content-length")
}

/// 청크 서명과 트레일러 서명 값을 자리표시자로 바꾼다.
fn normalize_body(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(index) = rest.find("signature") {
        let (head, tail) = rest.split_at(index + "signature".len());
        out.push_str(head);
        // `chunk-signature=` 또는 `x-amz-trailer-signature:` 뒤 64자리 16진수
        let separator = &tail[..1];
        let value: String = tail[1..]
            .chars()
            .take_while(char::is_ascii_hexdigit)
            .collect();
        if value.len() == 64 {
            out.push_str(separator);
            out.push_str("<SIG>");
            rest = &tail[1 + 64..];
        } else {
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------------------------
// 요청 정규화
// ---------------------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
struct Normalized {
    method: String,
    path: String,
    query: BTreeSet<String>,
    headers: BTreeSet<(String, String)>,
    body: String,
}

/// 비교 방식.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// 본문을 바이트 단위로 비교한다.
    Exact,
    /// XML 본문을 요소 순서와 무관하게 비교한다(본문 해시 헤더는 자기 본문과 맞는지 검증).
    Semantic,
}

#[derive(Debug, Clone, Copy)]
struct Options {
    ignore_content_type: bool,
}

fn normalize(
    request: &CapturedRequest,
    mode: Mode,
    options: Options,
    problems: &mut Vec<String>,
) -> Normalized {
    let mut parts = request.line.split(' ');
    let method = parts.next().unwrap().to_string();
    let target = parts.next().unwrap();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let query = query
        .split('&')
        .filter(|q| !q.is_empty())
        // `?acl`과 `?acl=`은 같은 뜻이다.
        .map(|q| q.trim_end_matches('=').to_string())
        .collect();

    let xml = (mode == Mode::Semantic)
        .then(|| {
            request
                .header("Content-Type")
                .filter(|v| v.starts_with("application/xml"))
                .and_then(|_| canonical_xml(&request.body))
        })
        .flatten();
    let body_hashes_checked = mode == Mode::Semantic && xml.is_some();

    let headers = request
        .headers
        .iter()
        .filter_map(|(name, value)| {
            let name = name.to_ascii_lowercase();
            if IGNORED_HEADERS.contains(&name.as_str()) {
                return None;
            }
            if options.ignore_content_type && name == "content-type" && request.body.is_empty() {
                return None;
            }
            let value = match name.as_str() {
                "host" => value.split(':').next().unwrap().to_string(),
                "authorization" => {
                    let signed = value
                        .split("SignedHeaders=")
                        .nth(1)
                        .and_then(|s| s.split(',').next())
                        .unwrap_or("");
                    let signed: Vec<&str> = signed
                        .split(';')
                        .filter(|h| !ignored_in_signature(h))
                        .filter(|h| {
                            !(options.ignore_content_type
                                && *h == "content-type"
                                && request.body.is_empty())
                        })
                        .collect();
                    format!("SignedHeaders={}", signed.join(";"))
                }
                n if body_hashes_checked && BODY_HASH_HEADERS.contains(&n) => {
                    check_body_hash(n, value, &request.body, &request.line, problems);
                    "<BODY-HASH>".to_string()
                }
                _ => value.clone(),
            };
            Some((name, value))
        })
        .collect();
    Normalized {
        method,
        path: path.to_string(),
        query,
        headers,
        body: xml.unwrap_or_else(|| normalize_body(&request.body)),
    }
}

/// 본문에서 계산한 헤더 값이 맞는지 확인한다(맞지 않으면 `problems`에 기록).
fn check_body_hash(name: &str, value: &str, body: &str, line: &str, problems: &mut Vec<String>) {
    let expected = match name {
        "content-length" => body.len().to_string(),
        "x-amz-content-sha256"
            if value.starts_with("STREAMING-") || value == "UNSIGNED-PAYLOAD" =>
        {
            return;
        }
        "x-amz-content-sha256" => hex::encode(Sha256::digest(body.as_bytes())),
        "x-amz-checksum-crc32" => base64::engine::general_purpose::STANDARD.encode(
            Crc::<u32>::new(&CRC_32_ISO_HDLC)
                .checksum(body.as_bytes())
                .to_be_bytes(),
        ),
        "content-md5" => {
            base64::engine::general_purpose::STANDARD.encode(Md5::digest(body.as_bytes()))
        }
        _ => return,
    };
    if value != expected {
        problems.push(format!(
            "{line}: {name} = {value} (본문에서 계산한 값 {expected})"
        ));
    }
}

fn expected_requests(baseline: &Value) -> Vec<CapturedRequest> {
    baseline["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| CapturedRequest {
            line: r["line"].as_str().unwrap().to_string(),
            headers: r["headers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|h| (h[0].as_str().unwrap().into(), h[1].as_str().unwrap().into()))
                .collect(),
            body: r["body"].as_str().unwrap().to_string(),
        })
        .collect()
}

/// 청크 서명 본문에서 CRC32 트레일러 두 줄을 걷어내 트레일러 없는 형식(.NET)으로 바꾼다.
fn strip_trailer(request: &CapturedRequest) -> CapturedRequest {
    if request.header("X-Amz-Content-SHA256") != Some("STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER")
    {
        return request.clone();
    }
    let body = match request.body.find("x-amz-checksum-crc32:") {
        Some(index) => format!("{}\r\n", &request.body[..index]),
        None => request.body.clone(),
    };
    let headers = request
        .headers
        .iter()
        .filter(|(n, _)| {
            !n.eq_ignore_ascii_case("x-amz-trailer")
                && !n.eq_ignore_ascii_case("x-amz-sdk-checksum-algorithm")
        })
        .map(|(n, v)| {
            let value = if n.eq_ignore_ascii_case("X-Amz-Content-SHA256") {
                "STREAMING-AWS4-HMAC-SHA256-PAYLOAD".to_string()
            } else if n.eq_ignore_ascii_case("Content-Length") {
                body.len().to_string()
            } else if n.eq_ignore_ascii_case("Authorization") {
                v.replace(";x-amz-trailer", "")
                    .replace(";x-amz-sdk-checksum-algorithm", "")
            } else {
                v.clone()
            };
            (n.clone(), value)
        })
        .collect();
    CapturedRequest {
        line: request.line.clone(),
        headers,
        body,
    }
}

/// (상태 코드, 오류, 상세) — 오류는 (.NET 형식 이름, 메시지, 상태 코드, 오류 코드).
type Outcome = (
    Option<u16>,
    Option<(String, String, Option<u16>, Option<String>)>,
    Option<String>,
);

async fn run(spec: &Value, port: u16) -> Outcome {
    let flag = |key: &str, default: bool| spec.get(key).and_then(Value::as_bool).unwrap_or(default);
    let retry = spec.get("retry").and_then(Value::as_i64).unwrap_or(3) as i32;
    let user = testrust_config::UserData::new(
        format!("http://127.0.0.1:{port}"),
        "",
        "AKIAEXAMPLE",
        "secretExample",
    );
    let client = S3Client::from_user(&user, flag("admin", false), retry, flag("checksum", false));
    let input = s3_ops::Spec::new(spec);
    let op = spec.get("op").and_then(Value::as_str).unwrap_or("");
    match s3_ops::run_op(op, &client, &input).await {
        Ok(ok) => (ok.status, None, ok.detail),
        Err(e) => (
            None,
            Some((
                e.dotnet_type().to_string(),
                e.to_string(),
                e.status(),
                e.code().map(str::to_string),
            )),
            None,
        ),
    }
}

fn response_of(spec: &Value, default_status: u64) -> CannedResponse {
    CannedResponse {
        status: spec
            .get("status")
            .and_then(Value::as_u64)
            .unwrap_or(default_status) as u16,
        headers: spec
            .get("responseHeaders")
            .and_then(Value::as_object)
            .map(|h| {
                h.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                    .collect()
            })
            .unwrap_or_default(),
        body: spec
            .get("responseBody")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    }
}

fn canned(spec: &Value) -> CannedResponse {
    response_of(spec, 200)
}

fn routes(spec: &Value) -> Vec<(String, CannedResponse)> {
    spec.get("routes")
        .and_then(Value::as_array)
        .map(|routes| {
            routes
                .iter()
                .map(|r| (r["contains"].as_str().unwrap().to_string(), canned(r)))
                .collect()
        })
        .unwrap_or_default()
}

fn read(path: String) -> Value {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity");
    serde_json::from_str(&std::fs::read_to_string(root.join(path)).unwrap()).unwrap()
}

/// .NET 결과를 같은 모양으로 바꾼다. `DoesS3BucketExist`는 bool을 0/1로, 다운로드 사례는 내려받은 내용(또는
/// 오류 형식·메시지·남은 파일 내용)을 상세로 쓴다.
fn expected_outcome(baseline: &Value) -> Outcome {
    let result = &baseline["result"];
    let status = match result {
        Value::Bool(b) => Some(u16::from(*b)),
        Value::Object(o) => o.get("status").and_then(Value::as_u64).map(|s| s as u16),
        _ => None,
    };
    let detail = match result {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) if o.contains_key("downloaded") => {
            o["downloaded"].as_str().map(str::to_string)
        }
        Value::Object(o) if o.contains_key("error") => Some(format!(
            "{}|{}|{}",
            o["error"].as_str().unwrap(),
            o["message"].as_str().unwrap(),
            o["content"].as_str().unwrap()
        )),
        _ => None,
    };
    let error = baseline["error"].as_object().map(|e| {
        (
            e["type"].as_str().unwrap().to_string(),
            e["message"].as_str().unwrap().to_string(),
            e.get("status").and_then(Value::as_u64).map(|s| s as u16),
            e.get("ErrorCode")
                .and_then(Value::as_str)
                .map(str::to_string),
        )
    });
    (status, error, detail)
}

/// 두 요청 목록의 차이를 줄여서 보여준다.
fn diff(expected: &[Normalized], actual: &[Normalized]) -> String {
    let mut out = format!(
        "  요청 수 expected {} actual {}\n",
        expected.len(),
        actual.len()
    );
    for (i, (e, a)) in expected.iter().zip(actual).enumerate() {
        if e == a {
            continue;
        }
        out.push_str(&format!(
            "  [{i}] {} {} / {} {}\n",
            e.method, e.path, a.method, a.path
        ));
        if e.query != a.query {
            out.push_str(&format!(
                "    query expected {:?} actual {:?}\n",
                e.query, a.query
            ));
        }
        for h in e.headers.difference(&a.headers) {
            out.push_str(&format!("    -header {h:?}\n"));
        }
        for h in a.headers.difference(&e.headers) {
            out.push_str(&format!("    +header {h:?}\n"));
        }
        if e.body != a.body {
            out.push_str(&format!(
                "    body expected {:?}\n    body actual   {:?}\n",
                e.body, a.body
            ));
        }
    }
    out
}

/// 사례 하나를 실행해 .NET 기준 출력과 비교하고 어긋난 내용을 돌려준다.
async fn check_case(name: &str) -> Vec<String> {
    let spec = read(format!("s3/{name}.json"));
    let baseline = read(format!("baseline/s3/{name}.json"));
    let server = CaptureServer::start_with_routes(canned(&spec), routes(&spec)).await;
    let outcome = run(&spec, server.port).await;
    let mut failures = Vec::new();

    let options = Options {
        ignore_content_type: IGNORE_CONTENT_TYPE_CASES.contains(&name),
    };
    let strip = STRIP_TRAILER_CASES.contains(&name);
    let unordered = UNORDERED_CASES.contains(&name);
    let expected_raw = expected_requests(&baseline);
    let actual_raw: Vec<CapturedRequest> = server
        .requests()
        .iter()
        .map(|r| if strip { strip_trailer(r) } else { r.clone() })
        .collect();

    let build = |requests: &[CapturedRequest], mode: Mode, problems: &mut Vec<String>| {
        let mut list: Vec<Normalized> = requests
            .iter()
            .map(|r| normalize(r, mode, options, problems))
            .collect();
        if unordered {
            list.sort_by(|a, b| {
                (&a.method, &a.path, &a.query).cmp(&(&b.method, &b.path, &b.query))
            });
        }
        list
    };
    let mut ignored = Vec::new();
    let expected = build(&expected_raw, Mode::Exact, &mut ignored);
    let actual = build(&actual_raw, Mode::Exact, &mut ignored);
    if actual != expected {
        let mut problems = Vec::new();
        let expected_semantic = build(&expected_raw, Mode::Semantic, &mut problems);
        let actual_semantic = build(&actual_raw, Mode::Semantic, &mut problems);
        if actual_semantic == expected_semantic && problems.is_empty() {
            eprintln!("{name}: XML 본문을 요소 순서와 무관하게 비교했다");
        } else {
            failures.push(format!(
                "{name} 요청 {problems:?}\n{}",
                diff(&expected_semantic, &actual_semantic)
            ));
        }
    }
    let expected_outcome = expected_outcome(&baseline);
    if outcome != expected_outcome {
        failures.push(format!(
            "{name} 결과\n  expected {expected_outcome:?}\n  actual   {outcome:?}"
        ));
    }
    failures
}

#[tokio::test]
async fn s3_client_matches_dotnet() {
    let mut failures = Vec::new();
    for name in CASES
        .iter()
        .chain(NEW_CASES)
        .chain(STRIP_TRAILER_CASES)
        .chain(PLAIN_UPLOAD_CASES)
    {
        failures.extend(check_case(name).await);
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// 체크섬 없는 청크 업로드: .NET은 트레일러 없는 청크 서명, Rust는 CRC32 트레일러가 붙은 청크 서명.
/// 청크 본문(서명 자리 제외)과 디코딩 길이는 같고, Rust 쪽에만 트레일러가 더 붙는지 확인한다.
#[tokio::test]
async fn chunked_uploads_add_crc32_trailer() {
    for name in CHUNKED_CASES {
        let spec = read(format!("s3/{name}.json"));
        let baseline = read(format!("baseline/s3/{name}.json"));
        let expected = &expected_requests(&baseline)[0];
        assert_eq!(
            expected.header("X-Amz-Content-SHA256"),
            Some("STREAMING-AWS4-HMAC-SHA256-PAYLOAD"),
            "{name}"
        );

        let server = CaptureServer::start(canned(&spec)).await;
        let (status, error, _) = run(&spec, server.port).await;
        assert_eq!((status, error), (Some(200), None), "{name}");
        let requests = server.requests();
        assert_eq!(requests.len(), 1, "{name}");
        let actual = &requests[0];
        assert_eq!(
            actual.line.split('?').next(),
            expected.line.split('?').next(),
            "{name}"
        );
        assert_eq!(
            actual.header("Content-Encoding"),
            Some("aws-chunked"),
            "{name}"
        );
        assert_eq!(
            actual.header("X-Amz-Content-SHA256"),
            Some("STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER"),
            "{name}"
        );
        assert_eq!(
            actual.header("X-Amz-Trailer"),
            Some("x-amz-checksum-crc32"),
            "{name}"
        );
        assert_eq!(
            actual.header("X-Amz-Decoded-Content-Length"),
            expected.header("X-Amz-Decoded-Content-Length"),
            "{name}"
        );
        // .NET 본문의 마지막 빈 줄(\r\n) 앞에 트레일러 두 줄이 더 붙는다.
        let expected_body = normalize_body(&expected.body);
        let actual_body = normalize_body(&actual.body);
        let prefix = expected_body.strip_suffix("\r\n").unwrap();
        assert!(
            actual_body.starts_with(prefix),
            "{name}\n{expected_body:?}\n{actual_body:?}"
        );
        assert_eq!(
            &actual_body[prefix.len()..],
            "x-amz-checksum-crc32:DUoRhQ==\r\nx-amz-trailer-signature:<SIG>\r\n\r\n",
            "{name}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 서명된 URL
// ---------------------------------------------------------------------------------------------

use aws_sdk_s3::types::ServerSideEncryption;
use chrono::{DateTime, Duration, TimeZone, Utc};
use testrust_s3::s3_client::HttpVerb;

const PRESIGN_CASES: &[&str] = &[
    "presign-get",
    "presign-put",
    "presign-delete",
    "presign-head",
    "presign-get-sse",
    "presign-admin",
    "presign-special-key",
    "presign-long",
    "presign-v2-get",
    "presign-v2-put",
    "presign-v2-special-key",
    // 키가 `/`로 시작하면 .NET은 경로에서 `/` 하나를 뺀다(사례의 `key`가 op 기본 키를 대신한다).
    "presign-leading-slash",
];

struct PresignParams {
    key: &'static str,
    expires: DateTime<Utc>,
    verb: HttpVerb,
    sse: Option<ServerSideEncryption>,
    content_type: Option<&'static str>,
}

/// 오라클 `S3Ops.cs`의 `presign-*` op와 같은 인자. 고정 시각은 2030-01-02T03:04:05Z.
fn presign_params(op: &str) -> PresignParams {
    let fixed = Utc.with_ymd_and_hms(2030, 1, 2, 3, 4, 5).unwrap();
    let in_seconds = |s: i64| Utc::now() + Duration::seconds(s);
    let get = |expires| PresignParams {
        key: "dir/key.txt",
        expires,
        verb: HttpVerb::Get,
        sse: None,
        content_type: None,
    };
    match op {
        "presign-get" => get(in_seconds(3600)),
        "presign-admin" => get(in_seconds(3600)),
        "presign-long" => get(Utc::now() + Duration::days(8)),
        "presign-put" => PresignParams {
            verb: HttpVerb::Put,
            sse: Some(ServerSideEncryption::Aes256),
            content_type: Some("text/plain"),
            ..get(in_seconds(900))
        },
        "presign-delete" => PresignParams {
            verb: HttpVerb::Delete,
            ..get(in_seconds(60))
        },
        "presign-head" => PresignParams {
            verb: HttpVerb::Head,
            ..get(in_seconds(3600))
        },
        "presign-get-sse" => PresignParams {
            sse: Some(ServerSideEncryption::Aes256),
            ..get(in_seconds(3600))
        },
        "presign-special-key" => PresignParams {
            key: "dir/한글 a+b.txt",
            ..get(in_seconds(3600))
        },
        "presign-v2-get" => get(fixed),
        "presign-v2-put" => PresignParams {
            verb: HttpVerb::Put,
            sse: Some(ServerSideEncryption::Aes256),
            content_type: Some("text/plain"),
            ..get(fixed)
        },
        "presign-v2-special-key" => PresignParams {
            key: "dir/한글 a+b.txt",
            verb: HttpVerb::Delete,
            ..get(fixed)
        },
        op => panic!("알 수 없는 op: {op}"),
    }
}

/// 서명된 URL을 (경로, 쿼리 이름 -> 값)으로 쪼갠다. 호스트·포트는 버린다.
fn split_url(url: &str) -> (String, BTreeMap<String, String>) {
    let rest = url.strip_prefix("http://").expect("프로토콜은 HTTP");
    let (_, target) = rest.split_once('/').unwrap();
    let (path, query) = target.split_once('?').unwrap();
    let query = query
        .split('&')
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap();
            (k.to_string(), v.to_string())
        })
        .collect();
    (format!("/{path}"), query)
}

#[tokio::test]
async fn presigned_urls_match_dotnet() {
    let mut failures = Vec::new();
    for name in PRESIGN_CASES {
        let spec = read(format!("s3/{name}.json"));
        let baseline = read(format!("baseline/s3/{name}.json"));
        let expected_url = baseline["result"].as_str().unwrap();
        let params = presign_params(spec["op"].as_str().unwrap());

        let user = testrust_config::UserData::new(
            "http://127.0.0.1:1".to_string(),
            "",
            "AKIAEXAMPLE",
            "secretExample",
        );
        let admin = spec.get("admin").and_then(Value::as_bool).unwrap_or(false);
        let client = S3Client::from_user(&user, admin, 3, false);
        let actual_url = client
            .generate_presigned_url(
                "my-bucket",
                spec.get("key")
                    .and_then(Value::as_str)
                    .unwrap_or(params.key),
                params.expires,
                params.verb,
                params.sse,
                params.content_type,
            )
            .await
            .unwrap();

        let (expected_path, expected_query) = split_url(expected_url);
        let (actual_path, actual_query) = split_url(&actual_url);
        if expected_path != actual_path {
            failures.push(format!("{name} 경로 {expected_path} != {actual_path}"));
        }
        let names = |q: &BTreeMap<String, String>| q.keys().cloned().collect::<Vec<_>>();
        if names(&expected_query) != names(&actual_query) {
            failures.push(format!(
                "{name} 쿼리 이름 {:?} != {:?}",
                names(&expected_query),
                names(&actual_query)
            ));
            continue;
        }
        let v2 = expected_query.contains_key("AWSAccessKeyId");
        let fixed = name.starts_with("presign-v2-");
        for (key, expected) in &expected_query {
            let actual = &actual_query[key];
            let ok = match key.as_str() {
                // 서명 V4는 현재 시각을 서명하므로 값 자체는 비교하지 않는다.
                "X-Amz-Signature" | "X-Amz-Date" => !actual.is_empty(),
                "X-Amz-Credential" => {
                    let scope = |v: &str| v.split("%2F").skip(2).collect::<Vec<_>>().join("%2F");
                    scope(expected) == scope(actual)
                }
                // 유효 시간은 .NET이 `Expires - 현재 시각`을 초 단위로 버림해 만들므로 실행 시점에 따라 1초 어긋날 수 있다.
                "X-Amz-Expires" | "Expires" => {
                    let a: i64 = actual.parse().unwrap();
                    let e: i64 = expected.parse().unwrap();
                    if v2 && fixed {
                        a == e
                    } else if v2 {
                        // 서명 V2의 `Expires`는 절대 시각(에폭 초)이라 기준 출력을 만든 시각과 다르다.
                        a > 0
                    } else {
                        (a - e).abs() <= 1
                    }
                }
                "Signature" => !(v2 && fixed) || expected == actual,
                _ => expected == actual,
            };
            if !ok {
                failures.push(format!("{name} {key}: {expected} != {actual}"));
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// .NET은 이미 지난 시각으로도 음수 `X-Amz-Expires` URL을 만들지만, Rust는 오류를 돌려준다(문서화된 차이).
#[tokio::test]
async fn presign_rejects_past_expiry() {
    let user = testrust_config::UserData::new("http://127.0.0.1:1".to_string(), "", "a", "s");
    let client = S3Client::from_user(&user, false, 3, false);
    let error = client
        .generate_presigned_url(
            "b",
            "k",
            Utc::now() - Duration::seconds(30),
            HttpVerb::Get,
            None,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(error.dotnet_type(), "Amazon.Runtime.AmazonClientException");
}
