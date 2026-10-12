//! "값 없음" 표식. .NET SDK 모델은 값을 안 준 속성을 요청 XML에서 그냥 뺀다. `aws-sdk-s3` 모델은 필수 값이
//! 비어 있으면 `build()`가 실패하므로, 호출 쪽이 [`UNSET`]을 넣어 만든 뒤 서명 전에 그 요소(또는 속성)를 지운다.
//!
//! - 요소: `<Tag>__testrust_unset__</Tag>` 전체를 지운다. 비어 있던 상위 요소는 `<Parent></Parent>`로 남는다.
//! - 속성: ` xsi:type="__testrust_unset__"`를 지운다.
//!
//! 본문을 바꾸면 SDK가 이미 넣은 `Content-Length`와 체크섬 헤더(`Content-MD5`, `x-amz-checksum-crc32`)도 다시 계산한다.

use aws_sdk_s3::config::http::HttpRequest;
use aws_sdk_s3::primitives::SdkBody;

use super::{add_content_md5, add_crc32};

/// 값이 없음을 나타내는 표식 문자열(실제 설정 값으로는 나올 수 없다).
pub const UNSET: &str = "__testrust_unset__";

/// 표식이 든 요소·속성을 지운 본문.
fn strip_text(text: &str) -> String {
    let mut text = text.to_string();
    while let Some(at) = text.find(UNSET) {
        let end = at + UNSET.len();
        let before = &text[..at];
        let range = if before.ends_with('"') {
            // 속성: ` name="표식"`
            before
                .rfind(' ')
                .filter(|_| text[end..].starts_with('"'))
                .map(|start| (start, end + 1))
        } else if before.ends_with('>') {
            // 요소: `<name>표식</name>`
            before.rfind('<').and_then(|open| {
                // 여는 태그에 속성이 있으면 이름은 첫 공백 앞까지다.
                let tag = before[open + 1..before.len() - 1]
                    .split_whitespace()
                    .next()
                    .unwrap_or_default();
                let close = format!("</{tag}>");
                text[end..]
                    .starts_with(&close)
                    .then_some((open, end + close.len()))
            })
        } else {
            None
        };
        match range {
            Some((start, stop)) => text.replace_range(start..stop, ""),
            // 예상한 모양이 아니면 표식만 지워 반복을 끝낸다.
            None => text.replace_range(at..end, ""),
        }
    }
    text
}

/// 표식이 든 요소·속성을 요청 본문에서 지운다.
pub(crate) fn strip_unset(request: &mut HttpRequest) {
    let Some(stripped) = request
        .body()
        .bytes()
        .and_then(|body| std::str::from_utf8(body).ok())
        .filter(|text| text.contains(UNSET))
        .map(strip_text)
    else {
        return;
    };
    let length = stripped.len().to_string();
    *request.body_mut() = SdkBody::from(stripped);
    let headers = request.headers_mut();
    headers.insert("content-length", length);
    // SDK가 이미 넣은 체크섬은 바뀐 본문에 맞게 다시 계산한다.
    let had_md5 = headers.get("content-md5").is_some();
    let had_crc32 = headers.get("x-amz-checksum-crc32").is_some();
    if had_md5 {
        add_content_md5(request);
    }
    if had_crc32 {
        add_crc32(request);
    }
}

/// 값이 [`UNSET`]인 쿼리 매개변수(`name=표식`)를 주소에서 지운다(.NET은 `null` 값을 쿼리에 넣지 않는다).
pub(crate) fn strip_unset_query(request: &mut HttpRequest) {
    let Some(uri) = strip_unset_query_text(request.uri()) else {
        return;
    };
    // 주소를 바꿀 수 없으면 그대로 보낸다(표식이 서버로 간다).
    let _ = request.set_uri(uri);
}

fn strip_unset_query_text(uri: &str) -> Option<String> {
    let (base, query) = uri.split_once('?')?;
    let marked = |p: &&str| p.split_once('=').is_some_and(|(_, v)| v == UNSET);
    if !query.split('&').any(|p| marked(&p)) {
        return None;
    }
    let kept: Vec<&str> = query.split('&').filter(|p| !marked(p)).collect();
    Some(if kept.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", kept.join("&"))
    })
}

/// [`strip_unset`] 뒤 `Content-MD5`를 넣는다(인벤토리·메트릭·분석).
pub(crate) fn strip_unset_md5(request: &mut HttpRequest) {
    strip_unset(request);
    add_content_md5(request);
}

/// [`strip_unset`] 뒤 CRC32 체크섬을 넣는다(알림).
pub(crate) fn strip_unset_crc32(request: &mut HttpRequest) {
    strip_unset(request);
    add_crc32(request);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_elements_and_attributes() {
        let text = format!(
            "<A><B>{UNSET}</B><C>x</C><Grantee xsi:type=\"{UNSET}\"><ID>1</ID></Grantee><D><E>{UNSET}</E></D></A>"
        );
        assert_eq!(
            strip_text(&text),
            "<A><C>x</C><Grantee><ID>1</ID></Grantee><D></D></A>"
        );
        let with_attr = format!("<A><G xsi:type=\"x\">{UNSET}</G><C>y</C></A>");
        assert_eq!(strip_text(&with_attr), "<A><C>y</C></A>");
    }

    #[test]
    fn refreshes_length_and_checksums() {
        use base64::Engine;
        use md5::{Digest, Md5};
        let mut request = HttpRequest::new(SdkBody::from(format!("<A><B>{UNSET}</B><C>x</C></A>")));
        request.headers_mut().insert("content-length", "99");
        request.headers_mut().insert("content-md5", "stale");
        strip_unset_md5(&mut request);
        let expected = "<A><C>x</C></A>";
        assert_eq!(request.body().bytes(), Some(expected.as_bytes()));
        assert_eq!(request.headers().get("content-length"), Some("15"));
        let md5 = base64::engine::general_purpose::STANDARD.encode(Md5::digest(expected));
        assert_eq!(request.headers().get("content-md5"), Some(md5.as_str()));
    }

    #[test]
    fn strips_query_parameters() {
        assert_eq!(
            strip_unset_query_text(&format!("http://h/b/k?partNumber=1&uploadId={UNSET}"))
                .as_deref(),
            Some("http://h/b/k?partNumber=1")
        );
        assert_eq!(
            strip_unset_query_text(&format!("http://h/b/k?uploadId={UNSET}")).as_deref(),
            Some("http://h/b/k")
        );
        assert_eq!(strip_unset_query_text("http://h/b/k?uploadId=u1"), None);
    }

    #[test]
    fn leaves_requests_without_marker_alone() {
        let mut request = HttpRequest::new(SdkBody::from("<A><C>x</C></A>"));
        request.headers_mut().insert("content-length", "15");
        strip_unset(&mut request);
        assert_eq!(request.body().bytes(), Some("<A><C>x</C></A>".as_bytes()));
        assert_eq!(request.headers().get("content-length"), Some("15"));
    }
}
