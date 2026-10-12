//! 명령·시나리오 실행 중 난 .NET 예외(형식 이름과 메시지).

use std::fmt;

use testrust_s3::S3Error;

/// 원본에서 던져진 .NET 예외. 최상위(`app.rs`)의 `catch (Exception e) { _log.Error(e); return ERROR_NORMAL; }`로 가거나,
/// 스레드 안에서 나면 [`crate::crash`]로 프로세스를 끝낸다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioError {
    /// .NET 예외 형식 이름(예: `Amazon.S3.AmazonS3Exception`).
    pub dotnet_type: String,
    pub message: String,
}

impl ScenarioError {
    pub fn new(dotnet_type: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            dotnet_type: dotnet_type.into(),
            message: message.into(),
        }
    }

    /// S3 오류를 .NET 예외로 바꾼다. .NET SDK는 연산마다 오류 응답 해석기가 아는 코드(`modeled`, 예: GetObject의
    /// `NoSuchKey`)만 전용 예외(`NoSuchKeyException`)로 던지고, 그 밖의 서비스 오류는 `AmazonS3Exception`이다.
    /// `S3Error::dotnet_type`은 연산을 구분하지 않으므로 여기서 연산별로 좁힌다.
    pub fn s3(error: S3Error, modeled: &[&str]) -> Self {
        let dotnet_type = match &error {
            S3Error::Service { code, .. } if !modeled.contains(&code.as_str()) => {
                "Amazon.S3.AmazonS3Exception"
            }
            other => other.dotnet_type(),
        };
        Self::new(dotnet_type, error.to_string())
    }
}

impl fmt::Display for ScenarioError {
    /// `Exception.ToString()`의 첫 줄(스택 추적은 재현하지 않는다).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.dotnet_type, self.message)
    }
}

impl std::error::Error for ScenarioError {}

/// 전용 예외로 모델링한 오류 코드가 없는 연산(대부분)의 변환. `?`도 이 규칙을 따른다.
impl From<S3Error> for ScenarioError {
    fn from(error: S3Error) -> Self {
        Self::s3(error, &[])
    }
}

/// KSAN 확장 API 오류: `catch (Exception e)`가 남기는 `형식: 메시지`.
impl From<testrust_s3::ksan::KsanError> for ScenarioError {
    fn from(error: testrust_s3::ksan::KsanError) -> Self {
        Self::new(error.dotnet_type(), error.to_string())
    }
}

/// 이름 생성 등 도우미 오류를 원본 .NET 예외로.
impl From<testrust_config::UtilError> for ScenarioError {
    fn from(error: testrust_config::UtilError) -> Self {
        use testrust_config::UtilError::*;
        match error {
            InvalidNumber(value) => Self::new(
                "System.FormatException",
                format!("The input string '{value}' was not in a correct format."),
            ),
            DivideByZero => Self::new(
                "System.DivideByZeroException",
                "Attempted to divide by zero.",
            ),
            OutOfRange(parameter) => Self::new(
                "System.ArgumentOutOfRangeException",
                format!(
                    "Specified argument was out of the range of valid values. (Parameter '{parameter}')"
                ),
            ),
        }
    }
}

/// 경로를 모르는 파일 오류(경로를 알면 [`crate::input::io_error`]).
impl From<std::io::Error> for ScenarioError {
    fn from(error: std::io::Error) -> Self {
        let dotnet_type = match error.kind() {
            std::io::ErrorKind::NotFound => "System.IO.FileNotFoundException",
            std::io::ErrorKind::PermissionDenied => "System.UnauthorizedAccessException",
            _ => "System.IO.IOException",
        };
        Self::new(dotnet_type, error.to_string())
    }
}

/// UpDownClient 메서드 밖으로 나가던 예외.
impl From<testrust_clients::UpDownError> for ScenarioError {
    fn from(error: testrust_clients::UpDownError) -> Self {
        use testrust_clients::UpDownError::*;
        let message = error.to_string();
        match error {
            S3(e) => e.into(),
            Util(e) => e.into(),
            Io(e) => e.into(),
            NullReference => Self::new(
                "System.NullReferenceException",
                "Object reference not set to an instance of an object.",
            ),
            IndexOutOfRange | DivideByZero | InvalidOperation(_) => {
                // 표시 문자열이 `형식: 메시지`다.
                let (dotnet_type, message) = message.split_once(": ").unwrap_or(("", &message));
                Self::new(dotnet_type, message)
            }
        }
    }
}

/// Portal 호출 실패(`PortalXxxException`, HTTP·JSON 오류).
impl From<testrust_clients::portal::PortalError> for ScenarioError {
    fn from(error: testrust_clients::portal::PortalError) -> Self {
        Self::new(error.dotnet_type(), error.to_string())
    }
}

impl From<testrust_clients::LocalError> for ScenarioError {
    fn from(error: testrust_clients::LocalError) -> Self {
        match error {
            testrust_clients::LocalError::Util(e) => e.into(),
            testrust_clients::LocalError::Io(e) => e.into(),
        }
    }
}

impl From<testrust_clients::MultiSystemError> for ScenarioError {
    fn from(error: testrust_clients::MultiSystemError) -> Self {
        match error {
            testrust_clients::MultiSystemError::Util(e) => e.into(),
            testrust_clients::MultiSystemError::Io(e) => e.into(),
        }
    }
}
