//! `KHttpClient`·`CurlClient`가 함께 쓰는 HTTP 전송. 실제 전송은 공용 `testrust_s3::http_transport`가 하고,
//! 여기서는 .NET `HttpClient`가 붙이는 `Host`·`Content-Type`·`Content-Length`만 더한다.

use std::sync::Arc;

use bytes::Bytes;
use http::Method;
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::connect::HttpConnector;
use testrust_s3::DotnetUri;
use testrust_s3::http_transport::{self, HttpTransport};

pub(crate) use http_transport::{HTTP_TIMEOUT, HttpResponse};

/// 요청을 보내지 못했을 때의 원인.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TransportError {
    /// 주소가 절대 URI가 아니거나 지원하지 않는 형식이다. 값은 .NET 예외의 (형식, 메시지)
    InvalidUri(&'static str, String),
    /// `HttpClient.Timeout` 초과
    Timeout,
    /// 연결·전송·수신 실패
    Connect(String),
}

impl From<http_transport::TransportError> for TransportError {
    fn from(error: http_transport::TransportError) -> Self {
        match error {
            http_transport::TransportError::Timeout => Self::Timeout,
            http_transport::TransportError::InvalidRequest(message)
            | http_transport::TransportError::Connect(message) => Self::Connect(message),
        }
    }
}

pub(crate) struct Transport {
    inner: HttpTransport,
}

impl Transport {
    /// 주어진 커넥터로 만든다. 헤더 이름은 .NET처럼 `Content-Type` 형식으로 보낸다.
    pub(crate) fn new(connector: HttpsConnector<HttpConnector>) -> Self {
        Self {
            inner: HttpTransport::new(connector, true),
        }
    }

    /// 운영체제 신뢰 저장소로 서버 인증서를 검증한다(.NET 기본 `HttpClient`).
    pub(crate) fn verifying() -> Self {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let builder =
            match HttpsConnectorBuilder::new().with_provider_and_native_roots(provider.clone()) {
                Ok(builder) => builder,
                Err(_) => HttpsConnectorBuilder::new().with_tls_config(
                    rustls::ClientConfig::builder_with_provider(provider)
                        .with_safe_default_protocol_versions()
                        .expect("기본 프로토콜 버전")
                        .with_root_certificates(rustls::RootCertStore::empty())
                        .with_no_client_auth(),
                ),
            };
        Self::new(builder.https_or_http().enable_http1().build())
    }

    /// 요청을 보낸다. `body`가 있으면 `Content-Type`을 붙이고, 빈 본문이면 `Content-Length: 0`도 붙인다
    /// (.NET `StringContent`). `Host`는 기본 포트면 호스트만, 아니면 `호스트:포트`.
    pub(crate) async fn send(
        &self,
        method: Method,
        url: &str,
        headers: &[(&str, String)],
        body: Option<(&str, Vec<u8>)>,
    ) -> Result<HttpResponse, TransportError> {
        let uri = DotnetUri::parse(url).map_err(|_| invalid_uri(url))?;
        let host = if uri.is_default_port() {
            uri.host().to_string()
        } else {
            format!("{}:{}", uri.host(), uri.port())
        };
        let mut all: Vec<(&str, &str)> = vec![("Host", host.as_str())];
        all.extend(headers.iter().map(|(n, v)| (*n, v.as_str())));
        let content = match &body {
            Some((content_type, bytes)) => {
                all.push(("Content-Type", content_type));
                if bytes.is_empty() {
                    all.push(("Content-Length", "0"));
                }
                Bytes::from(bytes.clone())
            }
            None => Bytes::new(),
        };
        Ok(self.inner.send(method, &uri, all, content).await?)
    }
}

fn invalid_uri(url: &str) -> TransportError {
    if url.contains("://") {
        TransportError::InvalidUri(
            "System.UriFormatException",
            "Invalid URI: The format of the URI could not be determined.".into(),
        )
    } else {
        TransportError::InvalidUri(
            "System.InvalidOperationException",
            "An invalid request URI was provided. Either the request URI must be an absolute URI or BaseAddress must be set.".into(),
        )
    }
}
