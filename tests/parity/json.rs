//! `to_dotnet_json`이 TESTCore `JsonExtensions.ToJsonString()`과 바이트 단위로 같은지 확인한다.
//! 기준 출력은 `tools/dotnet-oracle`의 `json` 명령으로 만든다(Windows에서 수집해 줄바꿈이 `\r\n`).

use std::path::Path;

use serde::Serialize;
use testrust_common::dotnet_json::{NEW_LINE, to_dotnet_json};

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Probe {
    strings: Vec<String>,
    empty: Vec<i32>,
    empty_object: EmptyObject,
    null: Option<String>,
    numbers: Vec<Number>,
    bools: Vec<bool>,
    nested: Nested,
}

#[derive(Serialize)]
struct EmptyObject {}

#[derive(Serialize)]
#[serde(untagged)]
enum Number {
    Int(i64),
    Float(f64),
}

#[derive(Serialize)]
struct Nested {
    #[serde(rename = "A")]
    a: Vec<Inner>,
}

#[derive(Serialize)]
struct Inner {
    #[serde(rename = "B")]
    b: i32,
}

#[test]
fn json_matches_dotnet() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity");
    let strings = std::fs::read_to_string(root.join("json/strings.txt")).unwrap();
    // 오라클과 같은 치환(File.ReadAllLines는 마지막 빈 줄을 만들지 않는다).
    let strings: Vec<String> = strings
        .lines()
        .map(|line| {
            line.replace("\\r", "\r")
                .replace("\\n", "\n")
                .replace("\\t", "\t")
                .replace("\\0", "\0")
                .replace("\\x01", "\u{1}")
                .replace("\\x7f", "\u{7f}")
        })
        .collect();
    use Number::{Float, Int};
    let probe = Probe {
        strings,
        empty: vec![],
        empty_object: EmptyObject {},
        null: None,
        numbers: vec![
            Int(0),
            Int(-1),
            Int(i32::MAX.into()),
            Int(i64::MAX),
            Float(1.5),
            Float(0.1),
            Float(1e20),
            Float(1e-7),
            Float(123456789.125),
            Float(1e14),
            Float(1e15),
            Float(123456789012345.6),
            Float(0.0001),
            Float(0.00001),
            Float(-2.5e-10),
            Float(1.7976931348623157e308),
            Float(5e-324),
            Float(100.0),
            Float(-0.0),
        ],
        bools: vec![true, false],
        nested: Nested {
            a: vec![Inner { b: 1 }],
        },
    };
    let expected = std::fs::read_to_string(root.join("baseline/json/strings.json"))
        .unwrap()
        .replace("\r\n", NEW_LINE);
    assert_eq!(to_dotnet_json(&probe), expected);
}
