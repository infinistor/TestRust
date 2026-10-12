//! 원본 `Distributed/DistributedSettings.cs`: Controller·Worker 운영 설정을 읽고 상대 경로를 INI 위치 기준으로 해석한다.
//!
//! - 섹션·키 이름은 대소문자를 구분한다(원본 `IniFile`). 값은 앞뒤 공백을 지운다.
//! - 키가 없으면 기본값(없으면 `[섹션] 키 설정이 필요합니다.`), 있는데 비어 있으면 `값이 비어 있습니다.`
//!   (`BucketSuffix`만 빈 값 허용).

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use testrust_config::{IniFile, UserData};
use testrust_s3::S3Error;
use testrust_s3::s3_client::error::full_path;
use testrust_scenarios::ScenarioError;

use crate::contracts::validate_user;

/// `System.ArgumentException`.
pub(crate) fn argument(message: impl Into<String>) -> ScenarioError {
    ScenarioError::new("System.ArgumentException", message)
}

/// 원본 `DriverSettings(Name, Url)`: Controller가 접근할 Worker 이름과 `/driver` 기준 주소.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverSettings {
    pub name: String,
    pub url: String,
}

/// 원본 `DistributedSettings`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistributedSettings {
    pub drivers: Vec<DriverSettings>,
    pub name: String,
    pub url: String,
    pub work_path: PathBuf,
    pub result_path: PathBuf,
    pub base_path: PathBuf,
    pub bucket_suffix: String,
    /// Worker `[Main User]`(있으면 Controller가 보낸 접속 정보 대신 쓴다). 진단·결과에는 넣지 않는다.
    pub local_user: Option<UserData>,
    pub prepare_timeout_seconds: i32,
    pub start_delay_seconds: i32,
    pub request_timeout_seconds: i32,
    pub poll_interval_seconds: i32,
    pub lease_timeout_seconds: i32,
}

static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new("^[a-z0-9][a-z0-9-]{0,39}$").unwrap());

impl DistributedSettings {
    /// 원본 `Load(path, worker)`.
    pub fn load(path: &str, worker: bool) -> Result<Self, ScenarioError> {
        let full = full_path(Path::new(path));
        // 원본 `new FileStream(path, FileMode.Open)`: 파일이 없으면 `FileNotFoundException`.
        let bytes =
            std::fs::read(&full).map_err(|e| ScenarioError::from(S3Error::io(&full, &e)))?;
        let ini = IniFile::from_bytes(&bytes);
        let base_path = full.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut result = Self {
            drivers: Vec::new(),
            name: String::new(),
            url: String::new(),
            work_path: PathBuf::new(),
            result_path: PathBuf::new(),
            base_path,
            bucket_suffix: String::new(),
            local_user: None,
            prepare_timeout_seconds: 300,
            start_delay_seconds: 5,
            request_timeout_seconds: 5,
            poll_interval_seconds: 2,
            lease_timeout_seconds: 15,
        };
        let mut section = if worker { "worker" } else { "controller" }.to_string();
        let read = |section: &str, name: &str, fallback: Option<&str>, allow_empty: bool| {
            let Some(value) = ini.section(section).and_then(|s| s.get(name)) else {
                return fallback
                    .map(str::to_string)
                    .ok_or_else(|| argument(format!("[{section}] {name} 설정이 필요합니다.")));
            };
            let text = value.text().trim().to_string();
            if !text.is_empty() || allow_empty {
                Ok(text)
            } else {
                Err(argument(format!("[{section}] {name} 값이 비어 있습니다.")))
            }
        };
        let positive = |section: &str, key: &str, fallback: i32| -> Result<i32, ScenarioError> {
            let text = read(section, key, Some(&fallback.to_string()), false)?;
            match dotnet_int(&text) {
                Some(value) if (1..=86400).contains(&value) => Ok(value),
                _ => Err(argument(format!("{key}는 1~86400 범위여야 합니다."))),
            }
        };
        let default_results = if worker {
            "./worker-results"
        } else {
            "./results"
        };
        result.result_path =
            result.resolve_path(&read(&section, "ResultPath", Some(default_results), false)?);
        result.lease_timeout_seconds = positive(&section, "LeaseTimeoutSeconds", 15)?;
        if worker {
            result.name = read(&section, "name", None, false)?;
            validate_name(&result.name)?;
            result.url = validate_url(&read(&section, "url", None, false)?)?;
            result.work_path =
                result.resolve_path(&read(&section, "WorkPath", Some("./worker-data"), false)?);
            result.bucket_suffix = read(&section, "BucketSuffix", Some(""), true)?;
            // 섹션이 있으면 접속 정보 전체를 우선한다. 누락 필드를 Controller 값과 섞지 않는다.
            if let Some(user_section) = ini.section("Main User") {
                let field = |key: &str, required: bool| -> Result<String, ScenarioError> {
                    let value = user_section
                        .get(key)
                        .map(|v| v.text().trim().to_string())
                        .unwrap_or_default();
                    if required && value.trim().is_empty() {
                        return Err(argument(format!(
                            "Worker [Main User] {key} 설정이 필요합니다. Controller 설정을 사용하려면 [Main User] 섹션을 제거하세요."
                        )));
                    }
                    Ok(value)
                };
                let user = UserData::new(
                    field("URL", true)?,
                    field("RegionName", false)?,
                    field("AccessKey", true)?,
                    field("SecretKey", true)?,
                );
                validate_user(Some(&user))?;
                result.local_user = Some(user);
            }
        } else {
            result.prepare_timeout_seconds = positive(&section, "PrepareTimeoutSeconds", 300)?;
            result.start_delay_seconds = positive(&section, "StartDelaySeconds", 5)?;
            result.request_timeout_seconds = positive(&section, "RequestTimeoutSeconds", 5)?;
            result.poll_interval_seconds = positive(&section, "PollIntervalSeconds", 2)?;
            // 정상적인 조회 대기만으로 연결 제한 시간에 도달하지 않도록 여유를 확보한다.
            if result.lease_timeout_seconds
                <= result.poll_interval_seconds + result.request_timeout_seconds
            {
                return Err(argument(
                    "LeaseTimeoutSeconds는 조회 간격 + 요청 제한 시간보다 커야 합니다.",
                ));
            }
            let count = positive(&section, "drivers", 1)?;
            for i in 1..=count {
                section = format!("driver{i}");
                let name = read(&section, "name", None, false)?;
                validate_name(&name)?;
                let url = validate_url(&read(&section, "url", None, false)?)?;
                result.drivers.push(DriverSettings { name, url });
            }
            let distinct = |values: Vec<String>| {
                let mut lower: Vec<String> = values.iter().map(|v| v.to_lowercase()).collect();
                lower.sort();
                lower.dedup();
                lower.len()
            };
            let count = count as usize;
            if distinct(result.drivers.iter().map(|d| d.name.clone()).collect()) != count
                || distinct(result.drivers.iter().map(|d| d.url.clone()).collect()) != count
            {
                return Err(argument("Worker 이름과 주소는 중복될 수 없습니다."));
            }
        }
        Ok(result)
    }

    /// 원본 `ResolvePath(path)`: `Path.GetFullPath(path, BasePath)`.
    pub fn resolve_path(&self, path: &str) -> PathBuf {
        full_path(&self.base_path.join(path))
    }
}

/// `int.TryParse`(앞뒤 공백·부호 허용).
fn dotnet_int(text: &str) -> Option<i32> {
    text.trim().parse().ok()
}

/// 원본 `ValidateName`.
pub fn validate_name(name: &str) -> Result<(), ScenarioError> {
    if NAME.is_match(name) {
        Ok(())
    } else {
        Err(argument(
            "Worker 이름은 1~40자의 소문자 영숫자·하이픈이어야 합니다.",
        ))
    }
}

/// 원본 `ValidateUrl`: `http(s)://호스트:포트/driver`만 받고 `Uri.AbsoluteUri`(끝 `/` 제거)로 정규화한다.
fn validate_url(text: &str) -> Result<String, ScenarioError> {
    let error = || argument("Worker url은 http(s)://호스트:포트/driver 형식이어야 합니다.");
    let uri: http::Uri = text.parse().map_err(|_| error())?;
    let scheme = uri.scheme_str().ok_or_else(error)?.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(error());
    }
    let authority = uri.authority().ok_or_else(error)?;
    if authority.as_str().contains('@') || text.contains('#') || uri.query().is_some() {
        return Err(error());
    }
    if uri.path().trim_end_matches('/') != "/driver" {
        return Err(error());
    }
    let host = authority.host().to_ascii_lowercase();
    let default_port = if scheme == "http" { 80 } else { 443 };
    let port = match authority.port_u16() {
        Some(port) if port != default_port => format!(":{port}"),
        _ => String::new(),
    };
    Ok(format!("{scheme}://{host}{port}{}", uri.path())
        .trim_end_matches('/')
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_normalization() {
        assert_eq!(
            validate_url("http://Host:18011/driver/").unwrap(),
            "http://host:18011/driver"
        );
        assert_eq!(
            validate_url("http://h:80/driver").unwrap(),
            "http://h/driver"
        );
        assert!(validate_url("http://h/other").is_err());
        assert!(validate_url("ftp://h/driver").is_err());
        assert!(validate_url("http://u:p@h/driver").is_err());
        assert!(validate_url("http://h/driver?x=1").is_err());
    }

    #[test]
    fn names() {
        assert!(validate_name("driver-1").is_ok());
        assert!(validate_name("Driver").is_err());
        assert!(validate_name("-a").is_err());
        assert!(validate_name(&"a".repeat(41)).is_err());
    }
}
