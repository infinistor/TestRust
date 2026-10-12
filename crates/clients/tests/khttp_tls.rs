//! HTTPS: `KHttpClient`는 서버 인증서를 검증하지 않고(원본의 `ServerCertificateCustomValidationCallback = true`),
//! `CurlClient`는 운영체제 신뢰 저장소로 검증한다(`new HttpClient()` 기본 동작).
//! 자체 서명 인증서로 TLS 서버를 띄워 두 동작을 확인한다.

use std::sync::{Arc, Mutex};

use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use testrust_clients::curl::{CurlClient, CurlError};
use testrust_clients::khttp::KHttpClient;
use testrust_clients::portal::PortalResponse;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

/// 자체 서명 인증서(`127.0.0.1`)로 TLS 서버를 띄운다. 받은 요청 머리글을 모아 둔다.
async fn start_tls_server() -> (u16, Arc<Mutex<Vec<String>>>) {
    let certified = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()]).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![certified.cert.der().clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                certified.signing_key.serialize_der(),
            )),
        )
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let shared = requests.clone();
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                break;
            };
            let acceptor = acceptor.clone();
            let requests = shared.clone();
            tokio::spawn(async move {
                // 인증서 검증에 실패하는 클라이언트는 핸드셰이크에서 끊긴다.
                let Ok(mut stream) = acceptor.accept(socket).await else {
                    return;
                };
                let mut buffer = Vec::new();
                let mut byte = [0u8; 1];
                while !buffer.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut byte).await.unwrap_or(0) == 0 {
                        return;
                    }
                    buffer.push(byte[0]);
                }
                requests
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buffer).into_owned());
                let body = r#"{"Result":1}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (port, requests)
}

#[tokio::test]
async fn khttp_accepts_self_signed_certificate() {
    let (port, requests) = start_tls_server().await;
    let client = KHttpClient::new(Some("test-key")).unwrap();
    let response = client
        .get::<PortalResponse>(&format!("https://127.0.0.1:{port}//api/v1/Health"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.result.0, 1);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET //api/v1/Health HTTP/1.1\r\n"));
    assert!(requests[0].contains(&format!("Host: 127.0.0.1:{port}\r\n")));
    assert!(requests[0].contains("Authorization: test-key\r\n"));
}

#[tokio::test]
async fn curl_client_rejects_self_signed_certificate() {
    let (port, requests) = start_tls_server().await;
    let error = CurlClient::new()
        .get::<PortalResponse>(&format!("https://127.0.0.1:{port}/api/Status/u"))
        .await
        .expect_err("자체 서명 인증서는 거부해야 한다");
    assert!(
        matches!(&error, CurlError::Transport { dotnet_type, .. } if *dotnet_type == "System.AggregateException"),
        "{error:?}"
    );
    assert!(requests.lock().unwrap().is_empty());
}
