//! INI 파서가 TESTCore `IniFile`과 같은 결과를 내는지 확인한다.
//! 기준 출력은 `tools/dotnet-oracle`의 `ini` 명령으로 만든다.

use std::path::Path;

use serde_json::{Value, json};
use testrust_config::IniFile;

const FIXTURES: &[&str] = &[
    "edge-cases",
    "bom-crlf",
    "sample",
    "controller.sample",
    "worker.sample",
];

/// 오라클의 `DumpIni`와 같은 모양으로 만든다.
fn dump(ini: &IniFile) -> Value {
    let sections: Vec<Value> = ini
        .iter()
        .map(|(name, section)| {
            let values: Vec<Value> = section
                .iter()
                .map(|(key, value)| {
                    json!({ "key": key, "raw": value.raw().unwrap_or(""), "text": value.text() })
                })
                .collect();
            json!({ "name": name, "values": values })
        })
        .collect();
    let probes: Vec<Value> = ini
        .iter()
        .map(|(name, _)| {
            json!({
                "name": name,
                "upper": ini.contains_section(&name.to_uppercase()),
                "lower": ini.contains_section(&name.to_lowercase()),
            })
        })
        .collect();
    json!({ "sections": sections, "probes": probes })
}

#[test]
fn ini_matches_dotnet() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity");
    for name in FIXTURES {
        let ini = IniFile::load(root.join(format!("ini/{name}.ini"))).unwrap();
        let baseline =
            std::fs::read_to_string(root.join(format!("baseline/ini/{name}.json"))).unwrap();
        let expected: Value = serde_json::from_str(&baseline).unwrap();
        assert_eq!(dump(&ini), expected, "{name}");
    }
}
