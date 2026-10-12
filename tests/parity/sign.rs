//! AWS4 헤더 서명이 TESTCore `Aws4SignerForAuthorizationHeader`와 같은 결과를 내는지 확인한다.
//! 기준 출력은 `tools/dotnet-oracle`의 `sign` 명령으로 만든다. .NET 서명기는 현재 시각을 쓰므로
//! 기준 출력의 `X-Amz-Date`를 서명 시각으로 사용한다.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{NaiveDateTime, TimeZone, Utc};
use serde_json::Value;
use testrust_s3::signer::{Headers, SignError, X_AMZ_DATE};
use testrust_s3::{AuthorizationHeaderSigner, DotnetUri};

const FIXTURES: &[&str] = &[
    "get-tag-index",
    "delete-tag-index-null-region",
    "list-tag-search",
    "put-tag-index",
    "storage-move-unicode-key",
    "whitespace-and-case",
    "duplicate-query",
    "duplicate-host-header",
    "null-service",
];

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// .NET 예외 형식 이름으로 바꾼다.
fn exception_name(error: &SignError) -> &'static str {
    match error {
        SignError::DuplicateHeader(_) | SignError::DuplicateQueryParameter(_) => {
            "System.ArgumentException"
        }
        SignError::MissingValue(_) => "System.ArgumentNullException",
    }
}

#[test]
fn signature_matches_dotnet() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity");
    for name in FIXTURES {
        let read = |path: String| -> Value {
            serde_json::from_str(&std::fs::read_to_string(root.join(path)).unwrap()).unwrap()
        };
        let request = read(format!("sign/{name}.json"));
        let expected = read(format!("baseline/sign/{name}.json"));

        let expected_headers: BTreeMap<String, String> =
            serde_json::from_value(expected["headers"].clone()).unwrap();
        let date =
            NaiveDateTime::parse_from_str(&expected_headers[X_AMZ_DATE], "%Y%m%dT%H%M%SZ").unwrap();
        let now = Utc.from_utc_datetime(&date);

        let uri = DotnetUri::parse(text(&request, "url").unwrap()).unwrap();
        let signer = AuthorizationHeaderSigner {
            endpoint: &uri,
            http_method: text(&request, "method").unwrap(),
            service: text(&request, "service"),
            region: text(&request, "region"),
        };
        let mut headers: Headers = request["headers"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
            .collect();
        let result = signer.compute_signature(
            &mut headers,
            text(&request, "query").unwrap_or(""),
            text(&request, "bodyHash").unwrap(),
            text(&request, "accessKey").unwrap(),
            text(&request, "secretKey").unwrap(),
            false,
            now,
        );

        let actual_headers: BTreeMap<String, String> = headers.into_iter().collect();
        assert_eq!(actual_headers, expected_headers, "{name} headers");
        match result {
            Ok(authorization) => {
                assert_eq!(
                    Some(authorization.as_str()),
                    text(&expected, "authorization"),
                    "{name}"
                )
            }
            Err(error) => assert_eq!(
                Some(exception_name(&error)),
                text(&expected, "error"),
                "{name}"
            ),
        }
    }
}
