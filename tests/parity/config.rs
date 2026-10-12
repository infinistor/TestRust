//! `Config::load`가 TESTCore `Config.GetConfig` + `ToString()`과 같은 결과를 내는지 확인한다.
//! 기준 출력은 `tools/dotnet-oracle`의 `config` 명령으로 만든다(`baseline/config/*.json`).
//! 로드에 실패하는 입력의 기준 출력은 JSON `null`이다.

use std::path::Path;

use serde_json::Value;
use testrust_config::{Config, ConfigError};

/// (픽스처 이름, 메인 사용자 섹션 이름, 기준 출력 이름)
const CASES: &[(&str, Option<&str>, &str)] = &[
    ("sample", None, "sample"),
    ("sample", Some("Alt User"), "sample.user-Alt_User"),
    ("full", None, "full"),
    ("full", Some("Alt User"), "full.user-Alt_User"),
    ("bom-crlf", None, "bom-crlf"),
    ("controller.sample", None, "controller.sample"),
    ("worker.sample", None, "worker.sample"),
    ("empty", None, "empty"),
    ("missing-sections", None, "missing-sections"),
    ("sizes-units", None, "sizes-units"),
    ("sizes-binary", None, "sizes-binary"),
    ("sizes-overflow", None, "sizes-overflow"),
    ("invalid-size-default", None, "invalid-size-default"),
    ("invalid-size-part", None, "invalid-size-part"),
    ("invalid-size-mover", None, "invalid-size-mover"),
    ("invalid-size-usedsize", None, "invalid-size-usedsize"),
    ("invalid-ints", None, "invalid-ints"),
    ("invalid-bools", None, "invalid-bools"),
    ("bucket-types", None, "bucket-types"),
    ("bucket-types-zero", None, "bucket-types-zero"),
    ("custom-user", None, "custom-user"),
    ("custom-user", Some("Custom"), "custom-user.user-Custom"),
    ("custom-user", Some("custom"), "custom-user.user-lowercase"),
    (
        "custom-user",
        Some("Nonexistent"),
        "custom-user.user-Nonexistent",
    ),
    ("custom-user", Some(""), "custom-user.user-empty"),
    // 파일이 없으면 로드 실패.
    ("nonexistent", None, "nonexistent"),
];

/// `Default.FilePath`가 비어 있으면 원본은 `현재 디렉터리/test`를 쓴다. 현재 디렉터리는 실행 위치마다 달라서
/// 기준 출력에는 `<CWD>`로 바꿔 저장했고, 여기서도 같은 방식으로 바꿔 비교한다.
fn normalize_cwd(value: &mut Value) {
    let cwd = std::env::current_dir().unwrap().display().to_string();
    if let Some(path) = value.pointer_mut("/Main/FilePath")
        && let Some(text) = path.as_str()
        && let Some(rest) = text.strip_prefix(&cwd)
    {
        *path = Value::String(format!("<CWD>{rest}"));
    }
}

#[test]
fn config_matches_dotnet() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity");
    for (fixture, user, baseline_name) in CASES {
        let baseline =
            std::fs::read_to_string(root.join(format!("baseline/config/{baseline_name}.json")))
                .unwrap();
        let expected: Value = serde_json::from_str(&baseline).unwrap();
        let label = format!("{fixture} (user: {user:?})");

        let path = root.join(format!("config/{fixture}.ini"));
        match Config::load(&path, *user) {
            Ok(config) => {
                let mut actual = serde_json::to_value(&config).unwrap();
                normalize_cwd(&mut actual);
                assert_eq!(actual, expected, "{label}");
                // ToString()용 문자열도 같은 JSON이어야 한다.
                let mut again: Value = serde_json::from_str(&config.to_json_string()).unwrap();
                normalize_cwd(&mut again);
                assert_eq!(again, expected, "{label} (to_json_string)");
            }
            Err(e) => {
                assert_eq!(expected, Value::Null, "{label}: 로드 실패 {e}");
                if *fixture == "nonexistent" {
                    assert!(matches!(e, ConfigError::NotFound(_)), "{label}");
                    assert!(e.to_string().starts_with("Config file not found : "), "{e}");
                }
            }
        }
    }
}

#[test]
fn main_user_changes_are_seen_by_multi_system() {
    // 원본에서는 MultiSystemConfig가 MainUser와 같은 객체를 쓰므로 이후 변경이 반영된다.
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity/config/custom-user.ini");
    let mut config = Config::load(path, None).unwrap();
    config.main_user.set_url("http://changed:1");
    config.main_user.set_access_key("changed-ak");
    let json = serde_json::to_value(&config).unwrap();
    assert_eq!(json["MainUser"]["AccessKey"], "changed-ak");
    assert_eq!(
        json["MultiSystemUpload"]["OldSystem"]["AccessKey"],
        "changed-ak"
    );
    assert_eq!(
        json["MultiSystemUpload"]["OldSystem"]["URL"],
        "http://old:2"
    );
}

/// `ToString()` 문자열이 .NET 출력과 바이트 단위로 같은지 확인한다(이스케이프·줄바꿈·숫자 형식 포함).
/// 기준 출력은 오라클이 `Console.WriteLine`으로 찍은 원문이라 끝에 줄바꿈이 하나 붙어 있다.
#[test]
fn json_text_matches_dotnet() {
    use testrust_common::dotnet_json::NEW_LINE;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity");
    let config = Config::load(root.join("config/full.ini"), None).unwrap();
    let expected = std::fs::read_to_string(root.join("baseline/config-raw/full.txt"))
        .unwrap()
        .replace("\r\n", NEW_LINE);
    assert_eq!(format!("{}{NEW_LINE}", config.to_json_string()), expected);
}
