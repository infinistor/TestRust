//! `Test/FindTagTest.cs`: 버킷의 모든 오브젝트에서 특정 태그(키·값)를 가진 오브젝트 수를 센다.
//!
//! 1. `ListObjects`를 마커로 이어 가며 모든 키를 모은다(응답이 잘리면 `nextMarker : X`를 콘솔에 찍는다).
//! 2. `Parallel.ForEach`(`MaxDegreeOfParallelism = maxConcurrency`)로 `GetObjectTagging`을 부르고 일치하는 수를 센다.
//!    오브젝트 하나의 오류는 `Failed to get tags for {key}: {메시지}`로 남기고 다음으로 넘어간다.
//!
//! 마커는 TESTCore HEAD(`c83e35f`)에서 고친 대로 `ListObjects(bucket, marker: nextMarker)`로 넘긴다.
//!
//! 원본 특이점(그대로 둔다)
//!
//! - AWSSDK v4는 빈 목록을 `null`로 돌려준다. 그래서 오브젝트가 하나도 없는 응답(`S3Objects`)은 `Select`의
//!   `ArgumentNullException`(`Parameter 'source'`)으로 시나리오가 끝나고(호출한 쪽으로 올라간다), 태그가 없는
//!   오브젝트(`Tagging`)는 `Failed to get tags for ...: Object reference not set to an instance of an object.`
//!   줄을 남긴다.
//! - 버킷 이름을 검사하지 않는다. 없으면 요청을 만들 때 `ArgumentException`이 난다.
//! - 오류 줄과 `GetObjectTagging` 요청 순서는 `maxConcurrency`가 1보다 크면 실행마다 다르다.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use testrust_s3::S3Client;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::{error, info};

use crate::ScenarioError;
use crate::input::null_reference;

/// `Enumerable.Select(null, ...)`: `S3Objects`가 `null`이면 `NullReferenceException`이 아니라 이 예외가 난다.
fn source_null() -> ScenarioError {
    ScenarioError::new(
        "System.ArgumentNullException",
        "Value cannot be null. (Parameter 'source')",
    )
}

/// 원본 `FindTagTest`.
pub struct FindTagTest {
    client: S3Client,
    max_concurrency: i32,
}

impl FindTagTest {
    /// 원본 `FindTagTest(client, maxConcurrency)`.
    pub fn new(client: S3Client, max_concurrency: i32) -> Self {
        Self {
            client,
            max_concurrency,
        }
    }

    /// 원본 `Start(bucketName, tagKey, tagValue)`.
    pub async fn start(
        &self,
        bucket_name: Option<&str>,
        tag_key: &str,
        tag_value: &str,
    ) -> Result<(), ScenarioError> {
        info!(
            "FindTagTest Start (Max Concurrency: {})",
            self.max_concurrency
        );
        let sw = Instant::now();

        // 원본은 버킷 이름을 검사하지 않아 요청을 만들 때(`null`·빈 문자열) `ArgumentException`이 난다.
        let Some(bucket_name) = bucket_name.filter(|b| !b.is_empty()) else {
            return Err(ScenarioError::new(
                "System.ArgumentException",
                "BucketName is a required property and must be set before making this call. (Parameter 'ListObjectsRequest.BucketName')",
            ));
        };
        let mut next_marker = String::new();
        let mut all_keys: Vec<String> = Vec::new();

        // 모든 오브젝트 키를 수집
        loop {
            let response = self
                .client
                .list_objects(bucket_name, None, Some(&next_marker), 1000, None)
                .await?;
            let contents = response.output.contents();
            if contents.is_empty() {
                return Err(source_null());
            }
            all_keys.extend(
                contents
                    .iter()
                    .map(|o| o.key().unwrap_or_default().to_string()),
            );

            if response.output.is_truncated() == Some(true) {
                // .NET SDK는 `NextMarker`가 없으면 마지막 키를 쓴다.
                next_marker = response
                    .output
                    .next_marker()
                    .or_else(|| contents.last().and_then(|o| o.key()))
                    .unwrap_or_default()
                    .to_string();
                println!("nextMarker : {next_marker}");
            } else {
                break;
            }
        }

        // 한 번에 병렬 처리
        let total_count = Arc::new(AtomicUsize::new(0));
        let semaphore = Arc::new(Semaphore::new(self.max_concurrency.max(1) as usize));
        let mut tasks = JoinSet::new();
        for key in all_keys {
            let permit = semaphore
                .clone()
                .acquire_owned()
                .await
                .expect("세마포어는 닫지 않는다");
            let client = self.client.clone();
            let total_count = total_count.clone();
            let (bucket_name, tag_key, tag_value) = (
                bucket_name.to_string(),
                tag_key.to_string(),
                tag_value.to_string(),
            );
            tasks.spawn(async move {
                match has_tag(&client, &bucket_name, &key, &tag_key, &tag_value).await {
                    Ok(true) => {
                        total_count.fetch_add(1, Ordering::SeqCst);
                    }
                    Ok(false) => {}
                    Err(e) => error!("Failed to get tags for {key}: {}", e.message),
                }
                drop(permit);
            });
        }
        while tasks.join_next().await.is_some() {}

        info!(
            "FindTagTest({bucket_name}, {tag_key}, {tag_value}) End : {} time = {}ms",
            total_count.load(Ordering::SeqCst),
            sw.elapsed().as_millis()
        );
        Ok(())
    }
}

/// `response.Tagging.Exists(tag => tag.Key == tagKey && tag.Value == tagValue)`. 태그가 없으면 `Tagging`이 `null`이다.
async fn has_tag(
    client: &S3Client,
    bucket_name: &str,
    key: &str,
    tag_key: &str,
    tag_value: &str,
) -> Result<bool, ScenarioError> {
    let response = client.get_object_tagging(bucket_name, key, None).await?;
    let tags = response.output.tag_set();
    if tags.is_empty() {
        return Err(null_reference());
    }
    Ok(tags
        .iter()
        .any(|tag| tag.key() == tag_key && tag.value() == tag_value))
}
