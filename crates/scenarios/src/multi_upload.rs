//! `Test/MultiUploadTest.cs`: 같은 키 이름에 스레드 번호를 붙여 동시에 올리고 내용을 확인하는 업로드 테스트.
//!
//! - `PutObject`: `Key + ThreadId`에 `Key` 문자열을 본문으로 올리고 다시 받아 같은지 비교한다.
//! - `MultipartUpload`: 무작위 문자열을 `partSize` 단위 파트로 올려 완료한 뒤, `HeadObject`로 크기를 확인하고
//!   `partSize` 단위 Range 조회로 내용을 비교한다.
//!
//! 예외는 모두 잡아 `ERROR` 로그로 남기므로(`catch (Exception e) { log.Error(e); }`) 시나리오 밖으로 나가지 않는다.
//!
//! 원본 특이점(그대로 둔다)
//!
//! - `CheckContentUsingRange`는 마지막 구간의 끝을 `Size - 2`로 잡아(`end = Size - 1; end -= 1`) 마지막 바이트를 확인하지
//!   않는다. 비교가 어긋나도 `Body does not match` 로그만 남기고 결과는 `Success`다(예외가 났을 때만 `Fail`).
//! - `(int)Size`로 줄이므로 2GiB 이상이면 넘친다.
//! - 로그에는 `Key + ThreadId`가 아니라 `Key`만 찍는다.
//! - `partSize`가 음수이면 `RandomTextLong`이 `ArgumentOutOfRangeException`을 던진다(업로드 초기화 뒤).

use aws_sdk_s3::primitives::ByteStream;
use testrust_config::util::random_text_long;
use testrust_s3::S3Client;
use testrust_s3::s3_client::{PartETag, PutBody};
use tracing::{error, info};

use crate::ScenarioError;
use crate::files::read_error;
use crate::input::decode_text;

/// 원본 `MultipartUploadData`.
#[derive(Debug, Default)]
pub struct MultipartUploadData {
    pub upload_id: String,
    pub parts: Vec<PartETag>,
    pub body: String,
}

/// 원본 `MultiUploadTest`.
pub struct MultiUploadTest {
    client: S3Client,
    thread_id: i32,
    bucket_name: String,
    key: String,
    size: i64,
    part_size: i32,
}

/// `Utility.GetBody(response)`: 본문을 UTF-8(BOM 감지) 문자열로 읽는다.
async fn get_body(body: ByteStream) -> Result<String, ScenarioError> {
    let bytes = body.collect().await.map_err(read_error)?.into_bytes();
    Ok(decode_text(&bytes))
}

/// `GetObject`의 모델링된 예외(`NoSuchKeyException`, `InvalidObjectStateException`)를 구분해 바꾼다.
fn get_error(e: testrust_s3::S3Error) -> ScenarioError {
    ScenarioError::s3(e, &["NoSuchKey", "InvalidObjectState"])
}

impl MultiUploadTest {
    /// 원본 생성자(단일 `PutObject` 업로드용): `PartSize = 0`.
    pub fn new_put(
        client: S3Client,
        thread_id: i32,
        bucket_name: &str,
        key: &str,
        size: i64,
    ) -> Self {
        Self {
            client,
            thread_id,
            bucket_name: bucket_name.to_string(),
            key: key.to_string(),
            size,
            part_size: 0,
        }
    }

    /// 원본 생성자(멀티파트 업로드용).
    pub fn new_multipart(
        client: S3Client,
        thread_id: i32,
        bucket_name: &str,
        key: &str,
        size: i64,
        part_size: i32,
    ) -> Self {
        Self {
            part_size,
            ..Self::new_put(client, thread_id, bucket_name, key, size)
        }
    }

    fn object_key(&self) -> String {
        format!("{}{}", self.key, self.thread_id)
    }

    /// 원본 `PutObject()`: 올리고 다시 받아 내용이 같은지 확인한다.
    pub async fn put_object(&self) {
        if let Err(e) = self.put_object_inner().await {
            error!("{e}");
        }
    }

    async fn put_object_inner(&self) -> Result<(), ScenarioError> {
        let key = self.object_key();
        self.client
            .put_object(
                &self.bucket_name,
                &key,
                PutBody::Text(self.key.clone()),
                true,
                None,
            )
            .await?;
        info!("PutObject({})", self.key);

        // 업로드 완료 후 내용 확인
        let response = self
            .client
            .get_object(&self.bucket_name, &key, None, None)
            .await
            .map_err(get_error)?;
        let body = get_body(response.output.body).await?;
        if body != self.key {
            error!("Body does not match");
        }
        Ok(())
    }

    /// 원본 `MultipartUpload()`: 멀티파트로 올리고 Range 조회로 내용을 확인한다.
    pub async fn multipart_upload(&self) {
        if self.part_size == 0 {
            error!("PartSize is 0");
            return;
        }
        let key = self.object_key();
        match self
            .setup_multipart_upload(&self.bucket_name, &key, self.size as i32, self.part_size)
            .await
        {
            Ok(data) => {
                // 업로드 완료 후 내용 확인
                if !self
                    .check_content_using_range(&self.bucket_name, &key, &data.body, self.part_size)
                    .await
                {
                    error!("CheckContentUsingRange : {} Fail", self.key);
                } else {
                    info!("CheckContentUsingRange : {} Success", self.key);
                }
            }
            Err(e) => error!("{e}"),
        }
    }

    /// 원본 `CheckContentUsingRange(bucketName, key, data, step)`: 예외가 나면 `false`.
    async fn check_content_using_range(
        &self,
        bucket_name: &str,
        key: &str,
        data: &str,
        step: i32,
    ) -> bool {
        match self
            .check_range(bucket_name, key, data, i64::from(step))
            .await
        {
            Ok(()) => true,
            Err(e) => {
                error!("{e}");
                false
            }
        }
    }

    async fn check_range(
        &self,
        bucket_name: &str,
        key: &str,
        data: &str,
        step: i64,
    ) -> Result<(), ScenarioError> {
        let size = self.size;
        let mut start = 0i64;

        // 원본 데이터 길이 비교
        let head = self
            .client
            .head_object(bucket_name, key, None, None)
            .await?;
        let content_length = head.output.content_length().unwrap_or(0);
        if size != content_length {
            error!("Size : {size}, ContentLength : {content_length}");
        }

        while start < size {
            let mut end = start + step;
            if end > size {
                end = size - 1;
            }
            end -= 1;

            let response = self
                .client
                .get_object(bucket_name, key, None, Some((start, end)))
                .await
                .map_err(get_error)?;
            let get_length = response.output.content_length().unwrap_or(0);
            let body = get_body(response.output.body).await?;
            let length = end - start + 1;

            if length != get_length {
                error!("Length : {length}, ContentLength : {get_length}");
            }
            if body != substring(data, start, length)? {
                error!("Body does not match");
            }
            start += step;
        }
        Ok(())
    }

    /// 원본 `SetupMultipartUpload(bucketName, key, size, partSize)`: 무작위 데이터를 파트로 올려 완료한다.
    pub async fn setup_multipart_upload(
        &self,
        bucket_name: &str,
        key: &str,
        size: i32,
        part_size: i32,
    ) -> Result<MultipartUploadData, ScenarioError> {
        let mut upload_data = MultipartUploadData::default();
        let init = self
            .client
            .initiate_multipart_upload(bucket_name, key)
            .await?;
        upload_data.upload_id = init.output.upload_id().unwrap_or_default().to_string();

        let parts = make_part_data(size, part_size)?;
        for part in parts {
            upload_data.body.push_str(&part);
            let part_number = upload_data.parts.len() as i32 + 1;
            let response = self
                .client
                .upload_part(
                    bucket_name,
                    key,
                    &upload_data.upload_id,
                    part_number,
                    PutBody::Bytes(part.into_bytes()),
                    0,
                    -1,
                    true,
                )
                .await?;
            upload_data.parts.push(PartETag {
                part_number: Some(part_number),
                e_tag: response.output.e_tag().map(str::to_string),
                ..PartETag::default()
            });
        }
        self.client
            .complete_multipart_upload(bucket_name, key, &upload_data.upload_id, &upload_data.parts)
            .await?;
        Ok(upload_data)
    }
}

/// `data.Substring(start, length)`(데이터는 ASCII). 범위를 벗어나면 `ArgumentOutOfRangeException`.
fn substring(data: &str, start: i64, length: i64) -> Result<&str, ScenarioError> {
    let out_of_range = |message: &str, parameter: &str| {
        ScenarioError::new(
            "System.ArgumentOutOfRangeException",
            format!("{message} (Parameter '{parameter}')"),
        )
    };
    if start < 0 {
        return Err(out_of_range(
            "StartIndex cannot be less than zero.",
            "startIndex",
        ));
    }
    if start > data.len() as i64 {
        return Err(out_of_range(
            "startIndex cannot be larger than length of string.",
            "startIndex",
        ));
    }
    if length < 0 {
        return Err(out_of_range("Length cannot be less than zero.", "length"));
    }
    if start + length > data.len() as i64 {
        return Err(out_of_range(
            "Index and length must refer to a location within the string.",
            "length",
        ));
    }
    Ok(&data[start as usize..(start + length) as usize])
}

/// 원본 `MakePartData(size, partSize)`: 전체 크기를 `partSize` 이하 조각으로 나눈 무작위 문자열 목록.
/// `partSize`가 음수이면 `RandomTextLong`(`Enumerable.Range(0, 음수)`)이 예외를 던진다.
fn make_part_data(size: i32, part_size: i32) -> Result<Vec<String>, ScenarioError> {
    let mut list = Vec::new();
    let mut remain = size;
    while remain > 0 {
        let now = if remain > part_size {
            part_size
        } else {
            remain
        };
        if now < 0 {
            return Err(ScenarioError::new(
                "System.ArgumentOutOfRangeException",
                "Specified argument was out of the range of valid values. (Parameter 'count')",
            ));
        }
        list.push(random_text_long(now as usize));
        remain -= now;
    }
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_data_splits_by_part_size() {
        let parts = make_part_data(25, 10).unwrap();
        assert_eq!(
            parts.iter().map(String::len).collect::<Vec<_>>(),
            [10, 10, 5]
        );
        assert_eq!(make_part_data(10, 10).unwrap().len(), 1);
        assert!(make_part_data(0, 10).unwrap().is_empty());
        assert!(make_part_data(5, -1).is_err());
    }

    #[test]
    fn substring_follows_dotnet_bounds() {
        assert_eq!(substring("abcdef", 2, 3).unwrap(), "cde");
        assert_eq!(substring("abcdef", 6, 0).unwrap(), "");
        assert!(substring("abcdef", 4, 3).is_err());
        assert!(substring("abcdef", 7, 0).is_err());
    }
}
