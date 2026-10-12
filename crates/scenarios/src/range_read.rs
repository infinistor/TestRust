//! `Test/RangeReadTest.cs`: 단일 오브젝트를 Range 단위로 읽는다.
//!
//! `HeadObject`로 크기를 확인한 뒤(크기가 0 이하이면 아무것도 하지 않고 끝낸다) 스레드 수만큼 읽기 작업을 동시에 돌린다.
//!
//! - `rangeList`가 비어 있으면 `[0, 크기)` 안에서 무작위 `start`, `end`(`start <= end`)를 `count`회(1 미만이면 1회) 읽는다.
//! - `rangeList`가 있으면 크기 목록을 순환하면서 처음부터 끝까지 순서대로 읽는다. 마지막 구간은 오브젝트 끝으로 줄인다.
//! - 응답 본문은 버리지 않고 메모리로 모두 읽는다(`Utility.GetBodySplit`).
//!
//! 원본은 일반 `Thread`를 쓰므로 스레드 안에서 처리하지 않은 예외(S3 오류 등)가 나면 프로세스가 비정상 종료한다.
//! 여기서도 작업의 오류를 [`testrust_common::dotnet_exit::crash`]로 처리한다(최종 출력 없음).
//! 첫 `HeadObject`의 오류는 호출한 쪽(디스패처)으로 올라간다.
//!
//! 원본 특이점(그대로 둔다)
//!
//! - 크기가 0 이하인 항목만 있는 `rangeList`는 `offset`이 늘지 않아 끝나지 않는다. 음수 크기는 `offset`을 되돌린다.

use rand::Rng;
use testrust_common::dotnet_exit::crash;
use testrust_s3::S3Client;

use crate::ScenarioError;
use crate::files::read_error;

/// 원본 `RangeReadTest.Start(threadCount, client, bucketName, key, rangeList, count = 1)`.
pub async fn start(
    thread_count: i32,
    client: &S3Client,
    bucket_name: &str,
    key: &str,
    range_list: &[i64],
    count: i32,
) -> Result<(), ScenarioError> {
    let head = client.head_object(bucket_name, key, None, None).await?;
    let content_length = head.output.content_length().unwrap_or(0);
    if content_length <= 0 {
        return Ok(());
    }

    let mut threads = Vec::new();
    for _ in 0..thread_count {
        let client = client.clone();
        let (bucket_name, key) = (bucket_name.to_string(), key.to_string());
        let range_list = range_list.to_vec();
        threads.push(tokio::spawn(async move {
            if let Err(e) = get_test(
                &client,
                &bucket_name,
                &key,
                content_length,
                &range_list,
                count,
            )
            .await
            {
                crash(&e.dotnet_type, &e.message);
            }
        }));
    }
    for thread in threads {
        let _ = thread.await;
    }
    Ok(())
}

/// 원본 `GetTest(client, bucketName, key, contentLength, ranges, count = 1)`.
pub async fn get_test(
    client: &S3Client,
    bucket_name: &str,
    key: &str,
    content_length: i64,
    ranges: &[i64],
    count: i32,
) -> Result<(), ScenarioError> {
    if content_length <= 0 {
        return Ok(());
    }

    if ranges.is_empty() {
        let read_count = if count < 1 { 1 } else { count };
        for _ in 0..read_count {
            // [0, contentLength) 안에서만 start/end 선택
            let (start, end) = {
                let mut rng = rand::rng();
                let start = rng.random_range(0..content_length);
                (start, rng.random_range(start..content_length))
            };
            read_range(client, bucket_name, key, start, end).await?;
        }
        return Ok(());
    }

    let mut offset = 0i64;
    while offset < content_length {
        for &size in ranges {
            if offset >= content_length {
                break;
            }
            let mut end = offset + size - 1;
            if end > content_length - 1 {
                end = content_length - 1;
            }
            read_range(client, bucket_name, key, offset, end).await?;
            offset = end + 1;
        }
    }
    Ok(())
}

/// 원본 `ReadRange`: 구간을 받아 본문을 모두 읽는다.
async fn read_range(
    client: &S3Client,
    bucket_name: &str,
    key: &str,
    start: i64,
    end: i64,
) -> Result<(), ScenarioError> {
    let response = client
        .get_object(bucket_name, key, None, Some((start, end)))
        .await
        .map_err(|e| ScenarioError::s3(e, &["NoSuchKey", "InvalidObjectState"]))?;
    response.output.body.collect().await.map_err(read_error)?;
    Ok(())
}
