//! S3 호출 오류. 원본이 출력하던 `AmazonS3Exception`의 `StatusCode`, `ErrorCode`, `Message`를 보존한다.

use std::fmt;

use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use testrust_common::dotnet_http::status_name;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum S3Error {
    /// 서버가 오류 응답을 돌려준 경우(.NET `AmazonS3Exception`과 하위 예외).
    Service {
        status: u16,
        code: String,
        message: Option<String>,
        request_id: Option<String>,
    },
    /// 연결 실패·시간 초과 등 응답을 받지 못한 경우.
    Network(String),
    /// 요청을 만들 수 없는 경우(필수 값 누락 등).
    Request(String),
    /// 호출 인자가 잘못된 경우(.NET `ArgumentException`).
    Argument(String),
    /// 값의 형식이 잘못된 경우(.NET `FormatException`, 예: SSE-C 키가 Base64가 아님).
    Format(String),
    /// 로컬 파일 입출력 실패. 값은 .NET 예외 형식 이름과 메시지(`S3Error::io` 참고).
    Io {
        dotnet_type: &'static str,
        message: String,
    },
    /// 응답은 받았지만 본문을 해석하지 못한 경우(.NET `AmazonUnmarshallingException`).
    Unmarshalling(String),
    /// `DeleteObjects` 응답에 `<Error>` 항목이 있는 경우(.NET `DeleteObjectsException`, `AmazonS3Exception`의 하위 형식).
    /// 값은 `Deleted`·`Error` 항목 수. .NET 예외의 `StatusCode`는 0, `ErrorCode`는 `null`이다.
    DeleteObjects { deleted: usize, errors: usize },
}

impl S3Error {
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Service { status, .. } => Some(*status),
            Self::DeleteObjects { .. } => Some(0),
            _ => None,
        }
    }

    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Service { code, .. } => Some(code),
            _ => None,
        }
    }

    /// .NET에서 던지던 예외 형식 이름. AWSSDK.S3가 별도 형식으로 모델링한 오류 코드만 구분한다.
    pub fn dotnet_type(&self) -> &'static str {
        match self {
            Self::Service { code, .. } => match code.as_str() {
                "NoSuchKey" => "Amazon.S3.Model.NoSuchKeyException",
                "NoSuchBucket" => "Amazon.S3.Model.NoSuchBucketException",
                "NoSuchUpload" => "Amazon.S3.Model.NoSuchUploadException",
                "BucketAlreadyExists" => "Amazon.S3.Model.BucketAlreadyExistsException",
                "BucketAlreadyOwnedByYou" => "Amazon.S3.Model.BucketAlreadyOwnedByYouException",
                "InvalidObjectState" => "Amazon.S3.Model.InvalidObjectStateException",
                "ObjectAlreadyInActiveTierError" => {
                    "Amazon.S3.Model.ObjectAlreadyInActiveTierErrorException"
                }
                "ObjectNotInActiveTierError" => {
                    "Amazon.S3.Model.ObjectNotInActiveTierErrorException"
                }
                _ => "Amazon.S3.AmazonS3Exception",
            },
            Self::Network(_) => "System.Net.Http.HttpRequestException",
            Self::Request(_) => "Amazon.Runtime.AmazonClientException",
            Self::Argument(_) => "System.ArgumentException",
            Self::Format(_) => "System.FormatException",
            Self::Io { dotnet_type, .. } => dotnet_type,
            Self::Unmarshalling(_) => "Amazon.Runtime.AmazonUnmarshallingException",
            Self::DeleteObjects { .. } => "Amazon.S3.DeleteObjectsException",
        }
    }

    /// .NET `catch (AmazonS3Exception)`에 잡히는 오류(서비스 오류와 그 하위 형식인 `DeleteObjectsException`).
    pub fn is_amazon_s3_exception(&self) -> bool {
        matches!(self, Self::Service { .. } | Self::DeleteObjects { .. })
    }
}

impl fmt::Display for S3Error {
    /// `AmazonS3Exception.Message`: 서버 메시지가 있으면 그대로, 없으면 SDK가 만드는 문장.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Service {
                status,
                code,
                message: Some(message),
                ..
            } if !message.is_empty() => {
                let _ = (status, code);
                f.write_str(message)
            }
            Self::Service { status, code, .. } => write!(
                f,
                "Error making request with Error Code {code} and Http Status Code {}. \
                 No further error information was returned by the service.",
                status_name(*status)
            ),
            Self::Network(message)
            | Self::Request(message)
            | Self::Argument(message)
            | Self::Format(message)
            | Self::Io { message, .. }
            | Self::Unmarshalling(message) => f.write_str(message),
            Self::DeleteObjects {
                deleted, errors, ..
            } => write!(
                f,
                "Error deleting objects. Deleted objects: {deleted}. Delete errors: {errors}"
            ),
        }
    }
}

impl std::error::Error for S3Error {}

impl<E: ProvideErrorMetadata + std::error::Error + Send + Sync + 'static> From<SdkError<E>>
    for S3Error
{
    fn from(error: SdkError<E>) -> Self {
        match &error {
            SdkError::ServiceError(service) => {
                let status = service.raw().status().as_u16();
                let inner = service.err();
                Self::Service {
                    status,
                    // 본문 없는 오류 응답이면 .NET은 상태 코드 이름을 오류 코드로 쓴다.
                    code: inner
                        .code()
                        .map(str::to_string)
                        .unwrap_or_else(|| status_name(status)),
                    message: inner.message().map(str::to_string),
                    request_id: inner.meta().extra("aws_request_id").map(str::to_string),
                }
            }
            // 응답을 받았지만 해석하지 못했다(예: 200 응답의 깨진 XML).
            SdkError::ResponseError(_) => Self::Unmarshalling(format!(
                "Error unmarshalling response back from AWS. {}",
                aws_sdk_s3::error::DisplayErrorContext(&error)
            )),
            SdkError::ConstructionFailure(_) => {
                Self::Request(aws_sdk_s3::error::DisplayErrorContext(&error).to_string())
            }
            _ => Self::Network(aws_sdk_s3::error::DisplayErrorContext(&error).to_string()),
        }
    }
}

impl S3Error {
    /// 로컬 파일 오류를 .NET이 던지는 예외 형식과 메시지로 바꾼다.
    /// 파일이 없으면 `FileNotFoundException`(상위 디렉터리도 없으면 `DirectoryNotFoundException`),
    /// 권한이 없으면 `UnauthorizedAccessException`, 그 밖은 `IOException`.
    pub fn io(path: &std::path::Path, error: &std::io::Error) -> Self {
        // .NET 메시지는 `Path.GetFullPath`한 전체 경로를 쓴다.
        let full = full_path(path);
        let path = full.as_path();
        let shown = path.display();
        let (dotnet_type, message) = match error.kind() {
            std::io::ErrorKind::NotFound => {
                let parent_exists = path
                    .parent()
                    .is_none_or(|p| p.as_os_str().is_empty() || p.is_dir());
                if parent_exists {
                    (
                        "System.IO.FileNotFoundException",
                        format!("Could not find file '{shown}'."),
                    )
                } else {
                    (
                        "System.IO.DirectoryNotFoundException",
                        format!("Could not find a part of the path '{shown}'."),
                    )
                }
            }
            std::io::ErrorKind::PermissionDenied => (
                "System.UnauthorizedAccessException",
                format!("Access to the path '{shown}' is denied."),
            ),
            _ => ("System.IO.IOException", error.to_string()),
        };
        Self::Io {
            dotnet_type,
            message,
        }
    }
}

/// .NET `Path.GetFullPath`: 현재 디렉터리 기준 절대 경로로 바꾸고 `.`·`..`를 글자 그대로 정리한다.
pub fn full_path(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut out = std::path::PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // 루트 위로는 올라가지 않는다.
                if out.parent().is_some() {
                    out.pop();
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// .NET SDK가 요청을 만들기 전에 필수 문자열 속성(`null`·빈 문자열)을 확인하며 던지는 `ArgumentException`.
/// 요청을 보내지 않는다(빈 키로 `DELETE /버킷/`처럼 다른 연산이 나가는 것을 막는다).
pub fn required(value: &str, property: &str, request: &str) -> Result<(), S3Error> {
    if value.is_empty() {
        return Err(S3Error::Argument(format!(
            "{property} is a required property and must be set before making this call. (Parameter '{request}.{property}')"
        )));
    }
    Ok(())
}

/// AWSSDK v4의 생성된 마샬러가 필수 쿼리 값(`null`·빈 문자열)을 확인하며 던지는 `AmazonS3Exception`
/// (`StatusCode` 0, `ErrorCode` 없음). 요청을 보내지 않는다.
pub fn required_field(value: &str, field: &str) -> Result<(), S3Error> {
    if value.is_empty() {
        return Err(S3Error::Service {
            status: 0,
            code: String::new(),
            message: Some(format!(
                "Request object does not have required field {field} set"
            )),
            request_id: None,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_path_collapses_dots() {
        let base = std::env::current_dir().unwrap();
        assert_eq!(
            full_path(std::path::Path::new("a/./b/../c.txt")),
            base.join("a").join("c.txt")
        );
    }

    #[test]
    fn io_error_types() {
        let missing = std::env::temp_dir().join("testrust-missing-file.bin");
        let error = std::io::Error::from(std::io::ErrorKind::NotFound);
        let mapped = S3Error::io(&missing, &error);
        assert_eq!(mapped.dotnet_type(), "System.IO.FileNotFoundException");
        assert_eq!(
            mapped.to_string(),
            format!("Could not find file '{}'.", missing.display())
        );
        let deep = std::env::temp_dir().join("testrust-missing-dir/x.bin");
        assert_eq!(
            S3Error::io(&deep, &error).dotnet_type(),
            "System.IO.DirectoryNotFoundException"
        );
    }
}
