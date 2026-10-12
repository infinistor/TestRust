//! 원본 `Distributed/WorkerDiagnostics.cs`: Worker `--debug` 출력(자격 증명 가리기).
//!
//! 원본의 `Print*`는 콘솔에 바로 쓴다. 여기서는 출력할 문자열을 만드는 `*_text`와 콘솔에 쓰는 `print_*`를 나눠 둔다.

use std::path::Path;

use serde::Serialize;
use testrust_common::to_dotnet_json;
use testrust_config::UserData;
use testrust_s3::s3_client::error::full_path;
use testrust_scenarios::ScenarioError;

use crate::contracts::{TestRequest, WorkloadSettings};
use crate::settings::DistributedSettings;

#[derive(Serialize)]
struct MaskedUser {
    #[serde(rename = "URL")]
    url: String,
    #[serde(rename = "RegionName")]
    region_name: String,
    #[serde(rename = "AccessKey")]
    access_key: &'static str,
    #[serde(rename = "SecretKey")]
    secret_key: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct SettingsView {
    config_path: String,
    name: String,
    url: String,
    work_path: String,
    result_path: String,
    lease_timeout_seconds: i32,
    bucket_suffix: String,
    user_source: &'static str,
    user: Option<MaskedUser>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct RequestView<'a> {
    run_id: &'a Option<String>,
    worker_id: &'a Option<String>,
    test_type: &'a Option<String>,
    lease_timeout_seconds: i32,
    bucket_suffix: &'a str,
    user_source: &'static str,
    user: Option<MaskedUser>,
    workload: &'a Option<WorkloadSettings>,
    effective_bucket_name: String,
    effective_thread_prefix: String,
}

/// 원본 `PrintSettings`의 출력: 서버 시작 시 확정된 운영 설정. Controller 사용자 정보는 아직 수신 전이다.
pub fn settings_text(settings: &DistributedSettings, config_path: &str) -> String {
    let view = SettingsView {
        config_path: full_path(Path::new(config_path))
            .to_string_lossy()
            .into_owned(),
        name: settings.name.clone(),
        url: settings.url.clone(),
        work_path: settings.work_path.to_string_lossy().into_owned(),
        result_path: settings.result_path.to_string_lossy().into_owned(),
        lease_timeout_seconds: settings.lease_timeout_seconds,
        bucket_suffix: settings.bucket_suffix.clone(),
        user_source: if settings.local_user.is_none() {
            "Controller (pending test request)"
        } else {
            "Worker config.ini"
        },
        user: masked_user(settings.local_user.as_ref()),
    };
    format!("[DEBUG] Worker settings\n{}", to_dotnet_json(&view))
}

/// 원본 `PrintSettings(settings, configPath)`.
pub fn print_settings(settings: &DistributedSettings, config_path: &str) {
    println!("{}", settings_text(settings, config_path));
}

/// 원본 `PrintRequest`의 출력: 로컬 사용자와 버킷 접미어를 적용한 요청에서 실제 버킷 및 스레드 접두어를 확인한다.
pub fn request_text(
    settings: &DistributedSettings,
    request: &TestRequest,
) -> Result<String, ScenarioError> {
    let workload = request
        .workload
        .as_ref()
        .ok_or_else(|| ScenarioError::new("System.NullReferenceException", "Workload"))?;
    let main = workload.to_main(
        request.worker_id.as_deref().unwrap_or_default(),
        &settings.work_path.to_string_lossy(),
    )?;
    let view = RequestView {
        run_id: &request.run_id,
        worker_id: &request.worker_id,
        test_type: &request.test_type,
        lease_timeout_seconds: request.lease_timeout_seconds,
        bucket_suffix: &settings.bucket_suffix,
        user_source: if settings.local_user.is_none() {
            "Controller"
        } else {
            "Worker config.ini"
        },
        user: masked_user(request.user.as_ref()),
        workload: &request.workload,
        effective_bucket_name: main.bucket_name,
        effective_thread_prefix: main.thread_prefix,
    };
    Ok(format!(
        "[DEBUG] Worker effective test settings\n{}",
        to_dotnet_json(&view)
    ))
}

/// 원본 `PrintRequest(settings, request)`.
pub fn print_request(
    settings: &DistributedSettings,
    request: &TestRequest,
) -> Result<(), ScenarioError> {
    println!("{}", request_text(settings, request)?);
    Ok(())
}

fn masked_user(user: Option<&UserData>) -> Option<MaskedUser> {
    user.map(|u| MaskedUser {
        url: endpoint(&u.url),
        region_name: u.region_name.clone(),
        access_key: if u.access_key.is_empty() {
            "(unset)"
        } else {
            "***"
        },
        secret_key: if u.secret_key.is_empty() {
            "(unset)"
        } else {
            "***"
        },
    })
}

/// 원본 `Endpoint`: URL에도 접속 정보가 들어갈 수 있으므로 사용자 정보와 쿼리, 조각은 출력하지 않는다
/// (`new UriBuilder(uri) { UserName = "", Password = "", Query = "", Fragment = "" }.Uri.AbsoluteUri`).
fn endpoint(url: &str) -> String {
    const INVALID: &str = "(invalid URL)";
    let Some((scheme, rest)) = url.split_once("://") else {
        return INVALID.to_string();
    };
    if !scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return INVALID.to_string();
    }
    let scheme = scheme.to_ascii_lowercase();
    let rest = rest.split('#').next().unwrap_or_default();
    let rest = rest.split('?').next().unwrap_or_default();
    let (authority, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let host_port = authority.rsplit('@').next().unwrap_or_default();
    let (host, port) = match host_port.rfind(':') {
        Some(i) if !host_port[i..].contains(']') => (&host_port[..i], Some(&host_port[i + 1..])),
        _ => (host_port, None),
    };
    let port = match port {
        None | Some("") => None,
        Some(text) => match text.parse::<u16>() {
            Ok(port) => Some(port),
            Err(_) => return INVALID.to_string(),
        },
    };
    let is_file = scheme == "file";
    if host.is_empty() && !is_file {
        return INVALID.to_string();
    }
    let default_port = match scheme.as_str() {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        "ftp" => Some(21),
        _ => None,
    };
    let port = match port {
        Some(port) if Some(port) != default_port => format!(":{port}"),
        _ => String::new(),
    };
    let path = if path.is_empty() && !is_file {
        "/"
    } else {
        path
    };
    format!("{scheme}://{}{port}{path}", host.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_strips_credentials_query_and_fragment() {
        assert_eq!(
            endpoint("http://url-user:url-pass@127.0.0.1:9/?token=url-token#url-fragment"),
            "http://127.0.0.1:9/"
        );
        assert_eq!(endpoint("http://127.0.0.1:9001"), "http://127.0.0.1:9001/");
        assert_eq!(endpoint("HTTPS://Host:443/a/b?x"), "https://host/a/b");
        assert_eq!(endpoint("invalid-url"), "(invalid URL)");
        assert_eq!(endpoint(""), "(invalid URL)");
    }
}
