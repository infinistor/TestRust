//! TESTCore `Data/UpDownResult.cs` 이식과 `UpDownStats.SaveToJson`(`Converter/JsonSerializer.SaveObjectToJson`).
//!
//! 파일 저장 형식: 속성 이름 camelCase, 들여쓰기, 날짜는 유닉스 초(`DateTimeToUnixTimestampConverter`).

use std::path::Path;

use serde::{Serialize, Serializer};
use testrust_common::{DotnetDateTime, to_dotnet_json};

/// 원본 `UpDownResult`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpDownResult {
    pub read: i64,
    pub read_failed: i64,
    pub head: i64,
    pub head_failed: i64,
    pub write: i64,
    pub write_failed: i64,
    pub delete: i64,
    pub delete_failed: i64,
    pub list: i64,
    pub list_failed: i64,
    pub total: i64,
    pub total_failed: i64,
    pub time: i32,
    #[serde(serialize_with = "unix_seconds")]
    pub start_time: DotnetDateTime,
    #[serde(serialize_with = "unix_seconds")]
    pub end_time: DotnetDateTime,
    pub test_type: String,
    pub thread_count: i32,
    pub file_size: i64,
    pub read_ratio: i32,
    pub write_ratio: i32,
    pub delete_ratio: i32,
    pub bucket_type: String,
    pub bucket_name: String,
    pub object_prefix: String,
    pub thread_prefix: String,
}

impl Default for UpDownResult {
    /// 원본 생성자: `StartTime = DateTime.Now`, `TestType = "Unknown"`, 나머지는 기본값.
    fn default() -> Self {
        Self {
            read: 0,
            read_failed: 0,
            head: 0,
            head_failed: 0,
            write: 0,
            write_failed: 0,
            delete: 0,
            delete_failed: 0,
            list: 0,
            list_failed: 0,
            total: 0,
            total_failed: 0,
            time: 0,
            start_time: DotnetDateTime::now(),
            end_time: DotnetDateTime::default(),
            test_type: "Unknown".to_string(),
            thread_count: 0,
            file_size: 0,
            read_ratio: 0,
            write_ratio: 0,
            delete_ratio: 0,
            bucket_type: String::new(),
            bucket_name: String::new(),
            object_prefix: String::new(),
            thread_prefix: String::new(),
        }
    }
}

fn unix_seconds<S: Serializer>(value: &DotnetDateTime, serializer: S) -> Result<S::Ok, S::Error> {
    let seconds = value.to_unix_seconds().map_err(serde::ser::Error::custom)?;
    serializer.serialize_i64(seconds)
}

impl UpDownResult {
    /// 저장할 JSON 문자열. 날짜를 유닉스 초로 바꿀 수 없으면(.NET 예외) 메시지를 돌려준다.
    pub fn to_json(&self) -> Result<String, String> {
        // 직렬화 오류를 먼저 확인한다(to_dotnet_json은 실패하지 않는다고 가정한다).
        self.start_time.to_unix_seconds()?;
        self.end_time.to_unix_seconds()?;
        Ok(to_dotnet_json(self))
    }

    /// 원본 `UpDownStats.SaveToJson`: 실패하면 `JSON 저장 실패: {메시지}`를 콘솔에 쓰고 `false`.
    pub fn save_to_json(&self, file_path: impl AsRef<Path>) -> bool {
        let file_path = file_path.as_ref();
        let result = self.to_json().and_then(|json| {
            if let Some(dir) = file_path.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(file_path, json).map_err(|e| e.to_string())
        });
        match result {
            Ok(()) => true,
            Err(message) => {
                println!("JSON 저장 실패: {message}");
                false
            }
        }
    }
}
