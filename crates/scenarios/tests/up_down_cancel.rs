//! UpDownTest의 종료 처리: 프로세스 토큰이 취소되면(Ctrl+C) 시간 제한이 없는 `HeadTest`도 끝나고
//! 최종 결과 JSON을 저장한다. 원본에서는 `CancelKeyPress` 처리기가 하던 일이다.

use std::time::Duration;

use testrust_config::{EnumBucketTypes, MainConfig, UpDownConfig, UserData};
use testrust_scenarios::up_down::UpDownTest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

/// 모든 요청에 본문 없는 200으로 답하는 서버.
async fn start_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buffer = vec![0u8; 8192];
                let mut pending = Vec::new();
                loop {
                    let Ok(read) = socket.read(&mut buffer).await else {
                        return;
                    };
                    if read == 0 {
                        return;
                    }
                    pending.extend_from_slice(&buffer[..read]);
                    while let Some(end) = pending.windows(4).position(|w| w == b"\r\n\r\n") {
                        pending.drain(..end + 4);
                        let reply = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
                        if socket.write_all(reply).await.is_err() {
                            return;
                        }
                    }
                }
            });
        }
    });
    port
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_ends_head_test_and_saves_result_once() {
    let port = start_server().await;
    let dir = tempfile::tempdir().unwrap();
    let save = dir.path().join("result.json");

    let user = UserData::new(
        format!("http://127.0.0.1:{port}"),
        "",
        "access",
        "secret-key-0123456789",
    );
    let main_config = MainConfig::new(
        "bkt",
        "TH",
        "temp",
        &dir.path().display().to_string(),
        "1K",
        "5M",
        0,
        "",
        false,
    )
    .unwrap();
    let mut config = UpDownConfig::new(1, 1, 0, 2, 3, 1000, 1, EnumBucketTypes::None, false, false);
    config.save = Some(save.display().to_string());

    let cancel = CancellationToken::new();
    let mut test = UpDownTest::new(&main_config, &config, &user, &cancel);
    let canceller = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        canceller.cancel();
    });
    tokio::time::timeout(Duration::from_secs(20), test.head())
        .await
        .expect("취소 뒤에도 HeadTest가 끝나지 않는다")
        .unwrap();

    let text = std::fs::read_to_string(&save).expect("최종 결과 JSON이 저장되지 않았다");
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["testType"], "Head");
    assert_eq!(json["threadCount"], 2);
    assert!(json["head"].as_i64().unwrap() > 0, "{text}");
    assert_eq!(json["headFailed"], 0);
}
