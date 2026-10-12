//! `DotnetUri`가 .NET `System.Uri`와 같은 Host·Port·AbsolutePath를 내는지 확인한다.
//! 기준 출력은 `tools/dotnet-oracle`의 `uris` 명령으로 만든다.

use std::path::Path;

use serde_json::{Value, json};
use testrust_s3::dotnet_uri::DotnetUri;

#[test]
fn uri_matches_dotnet() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity");
    let baseline = std::fs::read_to_string(root.join("baseline/uri/urls.json")).unwrap();
    let expected: Vec<Value> = serde_json::from_str(&baseline).unwrap();
    let urls = std::fs::read_to_string(root.join("uri/urls.txt")).unwrap();
    let urls: Vec<&str> = urls.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(urls.len(), expected.len());

    let mut failures = Vec::new();
    for (url, expected) in urls.iter().zip(&expected) {
        let uri = DotnetUri::parse(url).unwrap();
        let actual = json!({
            "url": url,
            "host": uri.host(),
            "port": uri.port(),
            "isDefaultPort": uri.is_default_port(),
            "absolutePath": uri.absolute_path(),
            "pathAndQuery": uri.path_and_query(),
        });
        if &actual != expected {
            failures.push(format!("{url}\n  expected {expected}\n  actual   {actual}"));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
