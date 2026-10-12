//! 원본 `UpDownTest.SaveResultToJson`의 저장 경로 규칙과 로그.
//!
//! `Save`에 확장자가 있으면(`Path.HasExtension`) 그 파일에, 없으면 디렉터리로 보고
//! `{SanitizeFileName(이름)}_{yyyyMMdd_HHmmss}.json`을 만들어 `Path.Combine(Save, 파일 이름)` 경로에 쓴다.

use testrust_model::UpDownResult;
use tracing::{error, info};

use crate::util::{has_extension, path_combine, sanitize_file_name};

/// 결과를 `save` 경로에 저장하고 원본과 같은 로그를 남긴다.
pub(super) fn save_result(result: &UpDownResult, save: &str, test_type: &str) {
    let file_path = if has_extension(save) {
        save.to_string()
    } else {
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let file_name = format!("{}_{timestamp}.json", sanitize_file_name(test_type));
        path_combine(save, &file_name)
    };
    if result.save_to_json(&file_path) {
        info!("테스트 결과가 JSON 파일로 저장되었습니다: {file_path}");
    } else {
        error!("JSON 파일 저장에 실패했습니다.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_rules() {
        assert!(has_extension("result.json"));
        assert!(has_extension("a/b.c/d.json"));
        assert!(has_extension(".json"));
        assert!(!has_extension("a.b/c"));
        assert!(!has_extension("save"));
        assert!(!has_extension("dir."));
        assert!(!has_extension(""));
    }

    #[test]
    fn combine_adds_separator_once() {
        assert_eq!(path_combine("", "f.json"), "f.json");
        assert_eq!(path_combine("out/", "f.json"), "out/f.json");
        assert_eq!(
            path_combine("out", "f.json"),
            format!("out{}f.json", std::path::MAIN_SEPARATOR)
        );
    }
}
