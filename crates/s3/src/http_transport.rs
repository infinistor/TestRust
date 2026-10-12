//! .NET `HttpClient`처럼 동작하는 공용 HTTP 전송. KsanClient와 `testrust-clients`의 KHttpClient·CurlClient가
//! 함께 쓴다. reqwest처럼 기본 헤더(`accept` 등)를 붙이지 않으려고 hyper를 직접 쓰며, 보내는 헤더는 호출자가
//! 지정한 것뿐이다.
//!
//! - 요청 줄의 경로·쿼리는 .NET `Uri.PathAndQuery`(`DotnetUri`)를 그대로 쓴다.
//! - `HttpClient.Timeout` 기본값(100초) 안에 응답 본문까지 받지 못하면 `Timeout`.
//! - 응답 본문은 `StreamReader` 기본 동작처럼 UTF-8 BOM을 건너뛰고 UTF-8로 읽는다.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::{Method, Request};
use http_body_util::{BodyExt, Full};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;

use crate::dotnet_uri::DotnetUri;

/// `HttpClient.Timeout` 기본값(100초).
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(100);

/// 응답 상태 코드와 본문 문자열.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// 요청을 보내지 못했을 때의 원인.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// 요청을 만들 수 없다(잘못된 헤더 값 등).
    InvalidRequest(String),
    /// `HttpClient.Timeout` 초과.
    Timeout,
    /// 연결·전송·수신 실패. 오류 원인을 `: `로 이어 붙인 문구.
    Connect(String),
}

/// hyper 클라이언트 하나. 연결은 재사용한다(.NET `HttpClient` 한 개와 같다).
pub struct HttpTransport {
    client: Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
}

impl HttpTransport {
    /// 주어진 커넥터로 만든다. `title_case`면 헤더 이름을 `Content-Type`처럼 바꿔 보낸다(.NET이 보내는 형식).
    pub fn new(connector: HttpsConnector<HttpConnector>, title_case: bool) -> Self {
        Self {
            client: Client::builder(TokioExecutor::new())
                .http1_title_case_headers(title_case)
                .build(connector),
        }
    }

    /// HTTP 전용(인증서 저장소 없음). HTTPS 주소는 인증서 검증에 실패한다. KsanClient는 항상 `http://`로 접속한다.
    pub fn http_only(title_case: bool) -> Self {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("기본 프로토콜 버전")
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();
        let connector = HttpsConnectorBuilder::new()
            .with_tls_config(config)
            .https_or_http()
            .enable_http1()
            .build();
        Self::new(connector, title_case)
    }

    /// 요청을 보내고 응답 본문까지 받는다. `headers`는 이름·값 그대로 보낸다(`Host` 포함, 자동으로 붙이지 않는다).
    pub async fn send<'a>(
        &self,
        method: Method,
        uri: &DotnetUri,
        headers: impl IntoIterator<Item = (&'a str, &'a str)>,
        body: Bytes,
    ) -> Result<HttpResponse, TransportError> {
        let target = format!(
            "{}://{}:{}{}",
            uri.scheme(),
            uri.host(),
            uri.port(),
            uri.path_and_query()
        );
        let mut request = Request::builder().method(method).uri(target);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let request = request
            .body(Full::new(body))
            .map_err(|e| TransportError::InvalidRequest(e.to_string()))?;

        let work = async {
            let response = self
                .client
                .request(request)
                .await
                .map_err(|e| TransportError::Connect(error_chain(&e)))?;
            let status = response.status().as_u16();
            let bytes = response
                .into_body()
                .collect()
                .await
                .map_err(|e| TransportError::Connect(error_chain(&e)))?
                .to_bytes();
            Ok(HttpResponse {
                status,
                body: decode_body(&bytes),
            })
        };
        match tokio::time::timeout(HTTP_TIMEOUT, work).await {
            Ok(result) => result,
            Err(_) => Err(TransportError::Timeout),
        }
    }
}

/// 오류와 그 원인들을 `: `로 이어 붙인다.
pub fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(inner) = source {
        text.push_str(": ");
        text.push_str(&inner.to_string());
        source = inner.source();
    }
    text
}

/// `StreamReader` 기본 동작: UTF-8 BOM은 건너뛰고 UTF-8로 읽는다(잘못된 바이트는 U+FFFD).
pub fn decode_body(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}
