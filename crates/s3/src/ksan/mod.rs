//! TESTCore `Client/KsanClient.cs` 이식: KSAN 확장 API(tag-index, list-tag-search, storagemove) 호출.
//!
//! 원본과 같게 맞춘 동작:
//!
//! - 주소는 `http://{Host}:{Port}/...`로 만들고 .NET `Uri` 규칙으로 정규화한 경로·쿼리로 요청한다.
//!   서명에는 호출자가 만든 쿼리 문자열을 그대로 쓴다(`WebUtility.UrlEncode`가 만든 `%7E`가 요청 줄에서는
//!   `~`로 풀려 서명과 달라지는 점까지 같다).
//! - 보내는 헤더는 원본과 같은 것만 보낸다(`X-Amz-Content-SHA256`, `X-Amz-Date`, `Host`, `Authorization`,
//!   `Content-Type: text/plain`, `Content-Length`와 storagemove의 백엔드 헤더). GET·DELETE에도
//!   `Content-Length: 0`을 보낸다. 그래서 기본 헤더를 붙이는 reqwest 대신 hyper를 직접 쓴다.
//! - `DeleteBucketTagIndex`만 리전을 지정하지 않아 서명 단계에서 `ArgumentNullException`으로 실패한다.
//! - 200(DELETE는 204)이 아닌 응답은 오류 XML 내용과 관계없이 `Stream was not readable.`로 실패한다.
//!   본문이 `Error` XML이 아니면 그 전에 XML 오류(`InvalidOperationException`)로 실패한다.

pub mod error;
pub mod model;

use bytes::Bytes;
use chrono::Utc;
use http::Method;
use sha2::{Digest, Sha256};
use testrust_config::UserData;

pub use error::KsanError;
pub use model::{
    Content, KsanErrorResponse, ListBucketTagSearchResult, Owner, TagIndexingConfiguration,
};

use crate::dotnet_uri::DotnetUri;
use crate::http_transport::{HttpResponse as Response, HttpTransport, TransportError};
use crate::signer::{
    AuthorizationHeaderSigner, EMPTY_BODY_SHA256, Headers, X_AMZ_CONTENT_SHA256, add_header,
    to_hex_string,
};
use error::STREAM_NOT_READABLE;

const HEADER_DATA: &str = "NONE";
const HEADER_BACKEND: &str = "x-ifs-backend";
const HEADER_KSAN_BACKEND: &str = "x-ksan-backend";
const HEADER_AUTHORIZATION: &str = "Authorization";
const HEADER_CONTENT_LENGTH: &str = "content-length";
const HEADER_CONTENT_TYPE: &str = "content-type";
const DEFAULT_CONTENT_TYPE: &str = "text/plain";
const SERVICE: &str = "s3";

/// 원본 `KsanClient`.
pub struct KsanClient {
    host: String,
    port: i32,
    access_key: String,
    secret_key: String,
    debug: bool,
    http: HttpTransport,
}

/// 서명에 넘길 리전. 원본은 `DeleteBucketTagIndex`만 지정하지 않았다(`null`).
#[derive(Clone, Copy)]
enum Region {
    Empty,
    Null,
}

impl KsanClient {
    pub fn new(
        host: impl Into<String>,
        port: i32,
        access_key: impl Into<String>,
        secret_key: impl Into<String>,
        debug: bool,
    ) -> Self {
        Self {
            host: host.into(),
            port,
            access_key: access_key.into(),
            secret_key: secret_key.into(),
            debug,
            http: HttpTransport::http_only(false),
        }
    }

    /// 원본 `KsanClient(UserData, bool)`: URL에서 호스트와 포트를 꺼낸다.
    pub fn from_user(user: &UserData, debug: bool) -> Result<Self, KsanError> {
        let port = user.port().map_err(|_| {
            let text = user
                .url
                .rsplit_once(':')
                .map(|(_, p)| p.replace('/', ""))
                .unwrap_or_default();
            KsanError::Format(text)
        })?;
        Ok(Self::new(
            user.host(),
            port,
            user.access_key.clone(),
            user.secret_key.clone(),
            debug,
        ))
    }

    fn uri(&self, path_and_query: &str) -> Result<DotnetUri, KsanError> {
        DotnetUri::parse(&format!(
            "http://{}:{}/{path_and_query}",
            self.host, self.port
        ))
        .map_err(|_| KsanError::UriFormat("The format of the URI could not be determined.".into()))
    }

    fn base_headers(body_hash: &str, content_length: usize) -> Headers {
        vec![
            (X_AMZ_CONTENT_SHA256.into(), body_hash.into()),
            (HEADER_CONTENT_LENGTH.into(), content_length.to_string()),
            (HEADER_CONTENT_TYPE.into(), DEFAULT_CONTENT_TYPE.into()),
        ]
    }

    fn sign(
        &self,
        uri: &DotnetUri,
        method: &str,
        region: Region,
        headers: &mut Headers,
        query: &str,
        body_hash: &str,
    ) -> Result<(), KsanError> {
        let signer = AuthorizationHeaderSigner {
            endpoint: uri,
            http_method: method,
            service: Some(SERVICE),
            region: match region {
                Region::Empty => Some(""),
                Region::Null => None,
            },
        };
        let authorization = signer.compute_signature(
            headers,
            query,
            body_hash,
            &self.access_key,
            &self.secret_key,
            self.debug,
            Utc::now(),
        )?;
        add_header(headers, HEADER_AUTHORIZATION, authorization)?;
        Ok(())
    }

    /// 원본 `DeleteBucketTagIndex`. 리전이 없어 항상 `ArgumentNullException`으로 실패한다.
    pub async fn delete_bucket_tag_index(&self, bucket_name: &str) -> Result<(), KsanError> {
        let query = "tag-index";
        let uri = self.uri(&format!("{bucket_name}/?{query}"))?;
        let mut headers = Self::base_headers(EMPTY_BODY_SHA256, 0);
        self.sign(
            &uri,
            "DELETE",
            Region::Null,
            &mut headers,
            query,
            EMPTY_BODY_SHA256,
        )?;
        let response = self.send(&uri, Method::DELETE, &headers, "").await?;
        expect_status(&response, 204)
    }

    /// 원본 `GetBucketTagIndex`.
    pub async fn get_bucket_tag_index(
        &self,
        bucket_name: &str,
    ) -> Result<TagIndexingConfiguration, KsanError> {
        let query = "tag-index";
        let uri = self.uri(&format!("{bucket_name}/?{query}"))?;
        let mut headers = Self::base_headers(EMPTY_BODY_SHA256, 0);
        self.sign(
            &uri,
            "GET",
            Region::Empty,
            &mut headers,
            query,
            EMPTY_BODY_SHA256,
        )?;
        let response = self.send(&uri, Method::GET, &headers, "").await?;
        expect_status(&response, 200)?;
        Ok(TagIndexingConfiguration::from_xml(&response.body)?)
    }

    /// 원본 `ListBucketTagSearch`.
    pub async fn list_bucket_tag_search(
        &self,
        bucket_name: &str,
        tag: &str,
        max_keys: i32,
    ) -> Result<ListBucketTagSearchResult, KsanError> {
        if bucket_name.is_empty() {
            return Err(KsanError::Argument("버킷 이름이 비어있습니다.".into()));
        }
        if tag.is_empty() {
            return Err(KsanError::Argument(
                "검색할 태그가 존재하지 않습니다.".into(),
            ));
        }
        if max_keys < 1 {
            return Err(KsanError::Argument("MaxKeys 의 최소값은 1 입니다.".into()));
        }
        let query = format!(
            "list-tag-search&encoding-type=url&max-keys={max_keys}&tag={}",
            web_utility_url_encode(tag)
        );
        let uri = self.uri(&format!("{bucket_name}/?{query}"))?;
        let mut headers = Self::base_headers(EMPTY_BODY_SHA256, 0);
        self.sign(
            &uri,
            "GET",
            Region::Empty,
            &mut headers,
            &query,
            EMPTY_BODY_SHA256,
        )?;
        let response = self.send(&uri, Method::GET, &headers, "").await?;
        expect_status(&response, 200)?;
        Ok(ListBucketTagSearchResult::from_xml(&response.body)?)
    }

    /// 원본 `PutBucketTagIndex`.
    pub async fn put_bucket_tag_index(&self, bucket_name: &str) -> Result<(), KsanError> {
        if bucket_name.is_empty() {
            return Err(KsanError::Argument("버킷 이름이 비어있습니다.".into()));
        }
        let query = "tag-index";
        let uri = self.uri(&format!("{bucket_name}?{query}"))?;
        let content = TagIndexingConfiguration::default().to_xml();
        let content_hash = to_hex_string(&Sha256::digest(content.as_bytes()), true);
        // 원본은 string.Length(UTF-16 길이)를 쓴다. 내용이 ASCII라 바이트 수와 같다.
        let mut headers = Self::base_headers(&content_hash, content.encode_utf16().count());
        self.sign(
            &uri,
            "PUT",
            Region::Empty,
            &mut headers,
            query,
            &content_hash,
        )?;
        let response = self.send(&uri, Method::PUT, &headers, &content).await?;
        expect_status(&response, 200)
    }

    /// 원본 `StorageMove`. `storage_class`가 없으면(원본 `null`) 빈 값으로 들어간다.
    pub async fn storage_move(
        &self,
        bucket_name: &str,
        key: &str,
        storage_class: &str,
        version_id: Option<&str>,
    ) -> Result<(), KsanError> {
        if bucket_name.is_empty() {
            return Err(KsanError::Argument("버킷 이름이 비어있습니다.".into()));
        }
        let mut query = format!("storagemove&StorageClass={storage_class}&encoding-type=url");
        if let Some(version_id) = version_id {
            query.push_str(&format!("&VersionId={version_id}"));
        }
        let uri = self.uri(&format!("{bucket_name}/{key}?{query}"))?;
        let mut headers = Self::base_headers(EMPTY_BODY_SHA256, 0);
        headers.push((HEADER_BACKEND.into(), HEADER_DATA.into()));
        headers.push((HEADER_KSAN_BACKEND.into(), HEADER_DATA.into()));
        self.sign(
            &uri,
            "POST",
            Region::Empty,
            &mut headers,
            &query,
            EMPTY_BODY_SHA256,
        )?;
        let response = self.send(&uri, Method::POST, &headers, "").await?;
        expect_status(&response, 200)
    }

    /// 원본 `ConstructWebRequest` + `Client.Send`.
    async fn send(
        &self,
        uri: &DotnetUri,
        method: Method,
        headers: &Headers,
        body: &str,
    ) -> Result<Response, KsanError> {
        let headers = headers.iter().map(|(n, v)| (n.as_str(), v.as_str()));
        self.http
            .send(method, uri, headers, Bytes::from(body.to_string()))
            .await
            .map_err(|error| {
                KsanError::Http(match error {
                    // .NET은 `TaskCanceledException`을 던진다(메시지는 같다).
                    TransportError::Timeout => "The request was canceled due to the configured HttpClient.Timeout of 100 seconds elapsing.".to_string(),
                    TransportError::InvalidRequest(message) | TransportError::Connect(message) => message,
                })
            })
    }
}

/// 기대한 상태 코드가 아니면 원본 `new KsanException(response)`와 같은 오류를 만든다.
fn expect_status(response: &Response, expected: u16) -> Result<(), KsanError> {
    if response.status == expected {
        return Ok(());
    }
    // 첫 번째 `GetErrorResponse`: 공백 본문이면 통과, 아니면 `Error` XML로 읽는다(실패하면 그 예외).
    if !response.body.trim().is_empty() {
        KsanErrorResponse::from_xml(&response.body)?;
    }
    // 두 번째 `GetErrorResponse`: 이미 닫힌 스트림을 다시 읽다가 실패한다.
    Err(KsanError::Argument(STREAM_NOT_READABLE.into()))
}

/// .NET `WebUtility.UrlEncode`: 영숫자와 `-_.!*()`는 그대로, 공백은 `+`, 나머지는 UTF-8 `%XX`(대문자).
pub fn web_utility_url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for b in value.bytes() {
        match b {
            b'a'..=b'z'
            | b'A'..=b'Z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'*'
            | b'('
            | b')' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_utility_encoding() {
        assert_eq!(web_utility_url_encode("a b~é"), "a+b%7E%C3%A9");
        assert_eq!(web_utility_url_encode("!()*-._09AZaz"), "!()*-._09AZaz");
    }
}
