//! 옵션 표·도움말·`Usage` 상수 비교: 오라클 `cli-options`, `cli-help`, `cli-usage` 출력과 같아야 한다.

use std::path::Path;

use serde_json::Value;
use testrust_cli::options::{OPTIONS, option_names, write_option_descriptions};
use testrust_cli::usage;
use testrust_common::dotnet_json::NEW_LINE;

fn baseline(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/parity/baseline/cli")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn option_table_matches_dotnet() {
    let expected: Vec<Value> = serde_json::from_str(&baseline("options.json")).unwrap();
    assert_eq!(OPTIONS.len(), expected.len());
    for (def, expected) in OPTIONS.iter().zip(&expected) {
        assert_eq!(def.prototype, expected["prototype"], "prototype");
        assert_eq!(
            def.description, expected["description"],
            "{}",
            def.prototype
        );
        let names: Vec<&str> = option_names(def).collect();
        let expected_names: Vec<&str> = expected["names"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n.as_str().unwrap())
            .collect();
        assert_eq!(names, expected_names, "{}", def.prototype);
        let kind = if def.action.takes_value() {
            "Required"
        } else {
            "None"
        };
        assert_eq!(kind, expected["valueKind"], "{}", def.prototype);
    }
}

#[test]
fn help_matches_dotnet() {
    let expected = baseline("help.txt").replace("\r\n", NEW_LINE);
    let actual = write_option_descriptions();
    for (i, (a, e)) in actual.lines().zip(expected.lines()).enumerate() {
        assert_eq!(a, e, "{}번째 줄", i + 1);
    }
    assert_eq!(actual, expected);
}

#[test]
fn usage_constants_match_dotnet() {
    let expected: Vec<Value> = serde_json::from_str(&baseline("usage.json")).unwrap();
    assert_eq!(usage::CONSTANTS.len(), expected.len());
    for ((name, value), expected) in usage::CONSTANTS.iter().zip(&expected) {
        assert_eq!(*name, expected["name"]);
        assert_eq!(*value, expected["value"], "{name}");
    }
    assert_eq!(
        usage::sub_flag(usage::BUCKET, " : 버킷명", ""),
        usage::USAGE_BUCKET_NAME
    );
}
