//! TESTCore `Client/CurlClient.cs` 이식: JSON을 주고받는 단순 HTTP 클라이언트(Mover가 사용).
//!
//! 원본과 같게 맞춘 동작:
//!
//! - `POST`는 `Content-Type: application/json; charset=utf-8`로 본문을 보내고, 요청에는 그 밖의 기본 헤더가 없다.
//! - 응답 코드가 `200 OK`가 아니면 `HttpRequestException`이며 메시지는
//!   `Post Failed({URL}, {StatusCode}) : {본문}`(`Get`은 `Get Failed`). `StatusCode`는 .NET 열거형 이름이다.
//! - 본문은 `AllowTrailingCommas`, 숫자는 문자열도 허용(`AllowReadingFromString`), 이름 변환 없음(대소문자 구분)
//!   옵션으로 읽는다. 본문이 비어 있으면 `JsonException`, `null`이면 `None`.
//! - `.Result`를 쓰므로 네트워크 오류는 `AggregateException`으로 감싸인다.
//! - 인증서는 운영체제 신뢰 저장소로 검증한다(`new HttpClient()` 기본 동작).
//!
//! 원본은 `HttpClient` 하나를 모든 호출이 공유한다. 여기서는 [`CurlClient`] 값 하나가 연결 풀을 소유하며,
//! 호출 쪽(`MoverClient`)이 하나를 만들어 재사용한다.
//!
//! 다른 점: 3xx 응답을 따라가지 않는다(.NET 기본값은 자동 리디렉션). 응답 본문의 `charset`은 무시하고
//! UTF-8로 읽는다. 네트워크 오류 메시지는 운영체제·런타임마다 다르다.

use http::Method;
use testrust_common::dotnet_http::status_name;

use crate::http::{HTTP_TIMEOUT, Transport, TransportError};
use crate::json::{FromJson, JsonError, ReadOptions, deserialize};

/// `CurlClientConfig.DefaultOptions`
const OPTIONS: ReadOptions = ReadOptions {
    allow_trailing_commas: true,
    number_from_string: true,
};

/// `StringContent(Data, Encoding.UTF8, "application/json")`가 붙이는 `Content-Type`
const JSON_CONTENT_TYPE: &str = "application/json; charset=utf-8";

/// CurlClient 호출이 실패할 때의 오류. 원본에서 던지던 .NET 예외 형식을 그대로 구분한다.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CurlError {
    /// `System.Net.Http.HttpRequestException`: 응답 코드가 `200 OK`가 아님
    #[error("{0}")]
    Status(String),
    /// `System.Text.Json.JsonException`: 응답 본문을 읽지 못함
    #[error(transparent)]
    Json(#[from] JsonError),
    /// 요청을 보내지 못한 경우(`AggregateException`으로 감싸임)
    #[error("{message}")]
    Transport {
        dotnet_type: &'static str,
        message: String,
    },
}

impl CurlError {
    /// 원본에서 던지던 예외의 전체 형식 이름.
    pub fn dotnet_type(&self) -> &'static str {
        match self {
            Self::Status(_) => "System.Net.Http.HttpRequestException",
            Self::Json(_) => "System.Text.Json.JsonException",
            Self::Transport { dotnet_type, .. } => dotnet_type,
        }
    }

    fn from_transport(error: TransportError) -> Self {
        let message = match error {
            TransportError::InvalidUri(_, message) | TransportError::Connect(message) => message,
            TransportError::Timeout => format!(
                "The request was canceled due to the configured HttpClient.Timeout of {} seconds elapsing.",
                HTTP_TIMEOUT.as_secs()
            ),
        };
        Self::Transport {
            dotnet_type: "System.AggregateException",
            message: format!("One or more errors occurred. ({message})"),
        }
    }
}

/// 원본 `CurlClient<T>`. 응답 형식 `T`는 호출마다 지정한다.
pub struct CurlClient {
    transport: Transport,
}

impl Default for CurlClient {
    fn default() -> Self {
        Self::new()
    }
}

impl CurlClient {
    pub fn new() -> Self {
        Self {
            transport: Transport::verifying(),
        }
    }

    /// 원본 `Post(URL, Data)`
    pub async fn post<T: FromJson>(&self, url: &str, data: &str) -> Result<Option<T>, CurlError> {
        let body = Some((JSON_CONTENT_TYPE, data.as_bytes().to_vec()));
        self.request::<T>(Method::POST, "Post", url, body).await
    }

    /// 원본 `Get(URL)`
    pub async fn get<T: FromJson>(&self, url: &str) -> Result<Option<T>, CurlError> {
        self.request::<T>(Method::GET, "Get", url, None).await
    }

    async fn request<T: FromJson>(
        &self,
        method: Method,
        name: &str,
        url: &str,
        body: Option<(&str, Vec<u8>)>,
    ) -> Result<Option<T>, CurlError> {
        let response = self
            .transport
            .send(method, url, &[], body)
            .await
            .map_err(CurlError::from_transport)?;
        if response.status == 200 {
            return Ok(deserialize::<T>(&response.body, OPTIONS)?);
        }
        Err(CurlError::Status(format!(
            "{name} Failed({url}, {}) : {}",
            status_name(response.status),
            response.body
        )))
    }
}
