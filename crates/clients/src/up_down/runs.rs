//! 원본 테스트 루프 메서드(`#region CosBench Like Test` 이하). 메서드 이름과 순서, `Quit` 확인 위치,
//! 통계를 올리는 시점은 원본과 같다.

use aws_sdk_s3::types::Tag;
use chrono::{Datelike, Local, Timelike};
use rand::Rng;
use tracing::error;

use super::ops::{
    delete_object, download_object, get_object, head_object, is_object, list_objects,
    put_directory, put_object, put_object_tag, to_directory_key, upload_object,
};
use super::{TAG_KEY_NAME, UpDownClient, UpDownError};
use crate::file_util::create_random_file;

impl UpDownClient {
    /// 원본 `Prepare(maxCount, check, start)`.
    pub async fn prepare(
        &self,
        max_count: i32,
        check: bool,
        start: i32,
    ) -> Result<(), UpDownError> {
        self.set_object_count(start);
        for _ in start..max_count {
            if self.quit.get() {
                break;
            }
            let object_name = self.next_object_name()?;
            if check
                && is_object(
                    &self.client,
                    &self.bucket_name,
                    &object_name,
                    self.config.file_size,
                )
                .await
            {
                continue;
            }
            if put_object(
                &self.client,
                &self.bucket_name,
                &object_name,
                &self.file_path,
                self.config.use_chunk_encoding,
            )
            .await
            {
                self.stats.write.add_success(1);
            } else {
                if self.config.distributed {
                    self.stats.write.add_error(1);
                    return Err(UpDownError::InvalidOperation(
                        "Prepare 객체 생성 실패".to_string(),
                    ));
                }
                error!("Failed to Create {object_name}");
                self.quit.set(true);
            }
        }
        self.quit.set(true);
        Ok(())
    }

    /// 원본 `PrepareDir`: 이름 끝에 `/`를 붙인 폴더 객체를 만든다.
    pub async fn prepare_dir(
        &self,
        max_count: i32,
        check: bool,
        start: i32,
    ) -> Result<(), UpDownError> {
        self.set_object_count(start);
        for _ in start..max_count {
            if self.quit.get() {
                break;
            }
            let object_name = to_directory_key(self.next_object_name()?);
            if check && is_object(&self.client, &self.bucket_name, &object_name, 0).await {
                continue;
            }
            if put_directory(&self.client, &self.bucket_name, &object_name).await {
                self.stats.write.add_success(1);
            } else {
                error!("Failed to Create {object_name}");
                self.quit.set(true);
            }
        }
        self.quit.set(true);
        Ok(())
    }

    /// 원본 `PrepareNew`: 객체마다 새 S3 클라이언트를 만든다.
    pub async fn prepare_new(
        &self,
        max_count: i32,
        check: bool,
        start: i32,
    ) -> Result<(), UpDownError> {
        self.set_object_count(start);
        for _ in start..max_count {
            if self.quit.get() {
                break;
            }
            let object_name = self.next_object_name()?;
            if check
                && is_object(
                    &self.client,
                    &self.bucket_name,
                    &object_name,
                    self.config.file_size,
                )
                .await
            {
                continue;
            }
            let client = self.new_s3_client();
            if put_object(
                &client,
                &self.bucket_name,
                &object_name,
                &self.file_path,
                self.config.use_chunk_encoding,
            )
            .await
            {
                self.stats.write.add_success(1);
            } else {
                error!("Failed to Create {object_name}");
                self.quit.set(true);
            }
        }
        self.quit.set(true);
        Ok(())
    }

    /// 원본 `PrepareRandom`: 객체마다 내용을 새로 만든다.
    pub async fn prepare_random(&self, count: i32, start: i32) -> Result<(), UpDownError> {
        self.set_object_count(start);
        for _ in start..count {
            if self.quit.get() {
                break;
            }
            let object_name = self.next_object_name()?;
            self.create_random_file().await;
            if put_object(
                &self.client,
                &self.bucket_name,
                &object_name,
                &self.random_file_path(),
                self.config.use_chunk_encoding,
            )
            .await
            {
                self.stats.write.add_success(1);
            } else {
                if self.config.distributed {
                    self.stats.write.add_error(1);
                    return Err(UpDownError::InvalidOperation(
                        "Prepare 객체 생성 실패".to_string(),
                    ));
                }
                error!("Failed to Create {object_name}");
                self.quit.set(true);
            }
        }
        self.quit.set(true);
        Ok(())
    }

    /// 무작위 업로드 파일. 분산 실행에서는 공유 데이터셋 파일(Get의 ETag 기준)을 덮어쓰지 않도록 따로 둔다
    /// (TESTCore `RandomFilePath`, ec427f2).
    fn random_file_path(&self) -> std::path::PathBuf {
        if self.config.distributed {
            let mut path = self.file_path.clone().into_os_string();
            path.push(".random");
            path.into()
        } else {
            self.file_path.clone()
        }
    }

    async fn create_random_file(&self) {
        let path = self.random_file_path();
        let size = self.config.file_size;
        // 파일 쓰기는 블로킹 작업이라 별도 스레드에서 한다.
        let _ = tokio::task::spawn_blocking(move || create_random_file(&path, size, true)).await;
    }

    /// 원본 `Head(maxCount, start)`: 끝나면 처음 번호로 돌아간다.
    pub async fn head(&self, max_count: i32, start: i32) -> Result<(), UpDownError> {
        self.set_object_count(start);
        while !self.quit.get() {
            let object_name = self.next_object_name()?;
            if head_object(&self.client, &self.bucket_name, &object_name)
                .await
                .0
            {
                self.stats.head.add_success(1);
            } else {
                self.stats.head.add_error(1);
            }
            if self.object_count() >= max_count {
                self.set_object_count(0);
            }
        }
        Ok(())
    }

    /// 원본 `Read(maxCount)`: 0..maxCount 사이 임의의 객체를 읽는다.
    pub async fn read(&self, max_count: i32) -> Result<(), UpDownError> {
        self.set_object_count(max_count);
        let etag = if self.config.etag_check {
            let prepared = self
                .prepared_read_etag
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            match prepared {
                Some(etag) => Some(etag),
                None => self.file_etag_if_checked()?,
            }
        } else {
            None
        };
        while !self.quit.get() {
            let object_name = self.random_object_name()?;
            if get_object(
                &self.client,
                &self.bucket_name,
                &object_name,
                self.config.file_size,
                etag.as_deref(),
            )
            .await
            {
                self.stats.read.add_success(1);
            } else {
                self.stats.read.add_error(1);
            }
        }
        Ok(())
    }

    /// 원본 `ReadV2(maxCount, start)`: 번호 순서대로 한 번씩 읽는다.
    pub async fn read_v2(&self, max_count: i32, start: i32) -> Result<(), UpDownError> {
        self.set_object_count(start);
        let etag = self.file_etag_if_checked()?;
        for _ in start..max_count {
            if self.quit.get() {
                break;
            }
            let object_name = self.next_object_name()?;
            if get_object(
                &self.client,
                &self.bucket_name,
                &object_name,
                self.config.file_size,
                etag.as_deref(),
            )
            .await
            {
                self.stats.read.add_success(1);
            } else {
                self.stats.read.add_error(1);
            }
        }
        self.quit.set(true);
        Ok(())
    }

    /// 원본 `ReadNew`: 객체마다 새 S3 클라이언트를 만든다.
    pub async fn read_new(&self, max_count: i32, start: i32) -> Result<(), UpDownError> {
        self.set_object_count(start);
        let etag = self.file_etag_if_checked()?;
        for _ in start..max_count {
            if self.quit.get() {
                break;
            }
            let object_name = self.next_object_name()?;
            let client = self.new_s3_client();
            if get_object(
                &client,
                &self.bucket_name,
                &object_name,
                self.config.file_size,
                etag.as_deref(),
            )
            .await
            {
                self.stats.read.add_success(1);
            } else {
                self.stats.read.add_error(1);
            }
        }
        self.quit.set(true);
        Ok(())
    }

    /// 원본 `ReadV3`: 목록(V2)을 받아 그 안의 객체를 읽는다.
    /// 원본은 `NextContinuationToken`을 `StartAfter`로 넘긴다(그대로 따른다).
    pub async fn read_v3(&self) -> Result<(), UpDownError> {
        let prefix = self.listing_prefix();
        let mut next_key_marker = String::new();
        let etag = self.file_etag_if_checked()?;
        while !self.quit.get() {
            let response = self
                .client
                .list_objects_v2(
                    &self.bucket_name,
                    Some(&prefix),
                    Some(&next_key_marker),
                    testrust_s3::s3_client::S3_MAX_KEYS,
                    None,
                    None,
                )
                .await?
                .output;
            let objects = response
                .contents
                .as_ref()
                .ok_or(UpDownError::NullReference)?;
            self.object_count
                .fetch_add(objects.len() as i32, std::sync::atomic::Ordering::Relaxed);
            for item in objects {
                if self.quit.get() {
                    break;
                }
                if get_object(
                    &self.client,
                    &self.bucket_name,
                    item.key().unwrap_or_default(),
                    self.config.file_size,
                    etag.as_deref(),
                )
                .await
                {
                    self.stats.read.add_success(1);
                } else {
                    self.stats.read.add_error(1);
                }
            }
            if response.is_truncated.unwrap_or(false) {
                next_key_marker = response.next_continuation_token.clone().unwrap_or_default();
            } else {
                next_key_marker = String::new();
            }
        }
        Ok(())
    }

    /// 원본 `Write(start)`.
    pub async fn write(&self, start: i32) -> Result<(), UpDownError> {
        self.set_object_count(start);
        while !self.quit.get() {
            let object_name = self.next_object_name()?;
            self.count_put(&object_name).await;
        }
        Ok(())
    }

    async fn count_put(&self, object_name: &str) -> bool {
        self.count_put_file(object_name, &self.file_path).await
    }

    async fn count_put_file(&self, object_name: &str, path: &std::path::Path) -> bool {
        let ok = put_object(
            &self.client,
            &self.bucket_name,
            object_name,
            path,
            self.config.use_chunk_encoding,
        )
        .await;
        if ok {
            self.stats.write.add_success(1);
        } else {
            self.stats.write.add_error(1);
        }
        ok
    }

    async fn count_get(&self, object_name: &str, etag: Option<&str>) {
        if get_object(
            &self.client,
            &self.bucket_name,
            object_name,
            self.config.file_size,
            etag,
        )
        .await
        {
            self.stats.read.add_success(1);
        } else {
            self.stats.read.add_error(1);
        }
    }

    async fn count_delete(&self, object_name: &str, version_id: Option<&str>) -> bool {
        let ok = delete_object(&self.client, &self.bucket_name, object_name, version_id).await;
        if ok {
            self.stats.delete.add_success(1);
        } else {
            self.stats.delete.add_error(1);
        }
        ok
    }

    /// 원본 `WriteRandom(start)`: 객체마다 내용을 새로 만든다.
    pub async fn write_random(&self, start: i32) -> Result<(), UpDownError> {
        self.set_object_count(start);
        while !self.quit.get() {
            let object_name = self.next_object_name()?;
            self.create_random_file().await;
            self.count_put_file(&object_name, &self.random_file_path())
                .await;
        }
        Ok(())
    }

    /// 원본 `WriteV2(maxCount)`: 번호가 maxCount에 닿으면 0으로 돌아간다.
    pub async fn write_v2(&self, max_count: i32) -> Result<(), UpDownError> {
        while !self.quit.get() {
            let object_name = self.next_object_name()?;
            self.count_put(&object_name).await;
            if self.object_count() >= max_count {
                self.set_object_count(0);
            }
        }
        Ok(())
    }

    /// 원본 `Delete(bulk, maxCount)`: 목록을 받아 지운다.
    pub async fn delete(&self, bulk: bool, max_count: i32) -> Result<(), UpDownError> {
        let prefix = self.listing_prefix();
        let distributed = self.config.distributed;
        while !self.quit.get() {
            if max_count > 0 && self.stats.delete.success() >= i64::from(max_count) {
                break;
            }
            let response = self
                .client
                .list_objects_v2(
                    &self.bucket_name,
                    Some(&prefix),
                    None,
                    testrust_s3::s3_client::S3_MAX_KEYS,
                    None,
                    None,
                )
                .await?
                .output;
            if self.quit.get() {
                break;
            }
            let mut objects = match (&response.contents, distributed) {
                (Some(list), _) => list.clone(),
                (None, true) => Vec::new(),
                (None, false) => return Err(UpDownError::NullReference),
            };
            self.object_count
                .fetch_add(objects.len() as i32, std::sync::atomic::Ordering::Relaxed);
            if distributed {
                self.stats.list.add_success(1);
                if self.quit.get() {
                    break;
                }
                if max_count > 0 {
                    let remaining = (i64::from(max_count) - self.stats.delete.success())
                        .clamp(i64::from(i32::MIN), i64::from(i32::MAX));
                    // Take(음수)는 빈 목록.
                    objects.truncate(remaining.max(0) as usize);
                }
                if objects.is_empty() {
                    break;
                }
            }
            if bulk {
                let keys: Vec<(String, Option<String>)> = objects
                    .iter()
                    .map(|o| (o.key().unwrap_or_default().to_string(), None))
                    .collect();
                let delete = self
                    .client
                    .delete_objects(&self.bucket_name, &keys, None, None)
                    .await?
                    .output;
                self.stats
                    .delete
                    .add_success(delete.deleted.as_ref().map_or(0, |d| d.len() as i64));
                self.stats
                    .delete
                    .add_error(delete.errors.as_ref().map_or(0, |e| e.len() as i64));
            } else {
                for item in &objects {
                    if self.quit.get() {
                        break;
                    }
                    self.count_delete(item.key().unwrap_or_default(), None)
                        .await;
                }
            }
            if !response.is_truncated.unwrap_or(false) {
                self.quit.set(true);
            }
        }
        Ok(())
    }

    /// 원본 `DeleteV2(maxCount, start)`.
    pub async fn delete_v2(&self, max_count: i32, start: i32) -> Result<(), UpDownError> {
        self.set_object_count(start);
        for _ in start..max_count {
            if self.quit.get() {
                break;
            }
            let object_name = self.next_object_name()?;
            self.count_delete(&object_name, None).await;
        }
        self.quit.set(true);
        Ok(())
    }

    /// 원본 `DeleteNew`: 객체마다 새 S3 클라이언트를 만든다.
    pub async fn delete_new(&self, max_count: i32, start: i32) -> Result<(), UpDownError> {
        self.set_object_count(start);
        for _ in start..max_count {
            if self.quit.get() {
                break;
            }
            let object_name = self.next_object_name()?;
            let client = self.new_s3_client();
            if delete_object(&client, &self.bucket_name, &object_name, None).await {
                self.stats.delete.add_success(1);
            } else {
                self.stats.delete.add_error(1);
            }
        }
        self.quit.set(true);
        Ok(())
    }

    /// 원본 `DeleteOne(key, maxCount)`: 같은 키를 maxCount번 지운다.
    pub async fn delete_one(&self, key: &str, max_count: i32) -> Result<(), UpDownError> {
        self.set_object_count(0);
        while self.object_count() < max_count {
            if self.quit.get() {
                break;
            }
            self.count_delete(key, None).await;
            self.object_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(())
    }

    /// 원본 `DeleteDirectory()`: 폴더(공통 접두어)를 재귀로 지운다.
    pub async fn delete_directory(&self) -> Result<(), UpDownError> {
        while !self.quit.get() {
            self.delete_directory_prefix(String::new()).await?;
        }
        Ok(())
    }

    fn delete_directory_prefix(
        &self,
        prefix: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), UpDownError>> + Send + '_>>
    {
        Box::pin(async move {
            if self.quit.get() {
                return Ok(());
            }
            let Some(response) = list_objects(
                &self.client,
                &self.bucket_name,
                Some(&prefix),
                None,
                Some("/"),
            )
            .await
            else {
                self.stats.list.add_error(1);
                return Ok(());
            };
            self.stats.list.add_success(1);
            let keys: Vec<(String, Option<String>)> = match &response.common_prefixes {
                Some(prefixes) if !prefixes.is_empty() => prefixes
                    .iter()
                    .map(|p| (p.prefix().unwrap_or_default().to_string(), None))
                    .collect(),
                _ => return Ok(()),
            };
            if self.quit.get() {
                return Ok(());
            }
            self.client
                .delete_objects(&self.bucket_name, &keys, None, None)
                .await?;
            self.stats.delete.add_success(keys.len() as i64);
            for (key, _) in keys {
                if self.quit.get() {
                    break;
                }
                self.delete_directory_prefix(key).await?;
            }
            Ok(())
        })
    }

    /// 원본 `DeleteVersion(bulk, maxCount, prefix)`.
    pub async fn delete_version(
        &self,
        bulk: bool,
        max_count: i32,
        prefix: Option<&str>,
    ) -> Result<(), UpDownError> {
        let prefix = prefix.map_or_else(|| self.listing_prefix(), str::to_string);
        while !self.quit.get() {
            if max_count > 0 && self.stats.delete.success() >= i64::from(max_count) {
                break;
            }
            let response = self
                .client
                .list_versions(
                    &self.bucket_name,
                    Some(&prefix),
                    None,
                    None,
                    testrust_s3::s3_client::S3_MAX_KEYS,
                    None,
                )
                .await?
                .output;
            if self.quit.get() {
                break;
            }
            // 원본 `Versions`는 버전과 삭제 마커를 문서 순서로 담는다(`null`이면 `NullReferenceException`).
            let versions = response.entries().ok_or(UpDownError::NullReference)?;
            if bulk {
                let keys: Vec<(String, Option<String>)> = versions
                    .iter()
                    .map(|v| {
                        (
                            v.key().unwrap_or_default().to_string(),
                            v.version_id().map(str::to_string),
                        )
                    })
                    .collect();
                let delete = self
                    .client
                    .delete_objects(&self.bucket_name, &keys, None, None)
                    .await?
                    .output;
                // 원본은 null 확인 없이 `.Count`를 읽는다.
                let deleted = delete.deleted.as_ref().ok_or(UpDownError::NullReference)?;
                self.stats.delete.add_success(deleted.len() as i64);
                let errors = delete.errors.as_ref().ok_or(UpDownError::NullReference)?;
                self.stats.delete.add_error(errors.len() as i64);
            } else {
                for item in versions {
                    if self.quit.get() {
                        break;
                    }
                    self.count_delete(item.key().unwrap_or_default(), item.version_id())
                        .await;
                }
            }
            if !response.is_truncated.unwrap_or(false) {
                self.quit.set(true);
            }
        }
        Ok(())
    }

    /// 원본 `Mix()`: 쓰기 `WriteRatio`번, 읽기 `ReadRatio`번을 번갈아 한다.
    pub async fn mix(&self) -> Result<(), UpDownError> {
        let mut my_read = 0;
        let mut my_write = 0;
        let etag = self.file_etag_if_checked()?;
        while !self.quit.get() {
            if my_read <= 0 && my_write <= 0 {
                my_read = self.config.read_ratio;
                my_write = self.config.write_ratio;
            }
            if my_write > 0 {
                let object_name = self.next_object_name()?;
                self.count_put(&object_name).await;
                my_write -= 1;
            }
            if self.quit.get() {
                break;
            }
            if my_read > 0 {
                let object_name = self.random_object_name()?;
                self.count_get(&object_name, etag.as_deref()).await;
                my_read -= 1;
            }
        }
        Ok(())
    }

    /// 원본 `MixV2()`: 쓴 객체 중에서 읽고 지운다.
    pub async fn mix_v2(&self) -> Result<(), UpDownError> {
        let mut object_names: Vec<String> = Vec::new();
        let etag = self.file_etag_if_checked()?;
        while !self.quit.get() {
            for _ in 0..self.config.write_ratio {
                if self.quit.get() {
                    break;
                }
                let object_name = self.next_object_name()?;
                if self.count_put(&object_name).await {
                    object_names.push(object_name);
                }
            }
            let mut i = 0;
            while i < self.config.read_ratio && !object_names.is_empty() {
                if self.quit.get() {
                    break;
                }
                let index = rand::rng().random_range(0..object_names.len());
                let object_name = object_names[index].clone();
                self.count_get(&object_name, etag.as_deref()).await;
                i += 1;
            }
            let mut i = 0;
            while i < self.config.delete_ratio && !object_names.is_empty() {
                if self.quit.get() {
                    break;
                }
                let object_name = object_names[0].clone();
                if self.count_delete(&object_name, None).await {
                    object_names.remove(0);
                }
                i += 1;
            }
            object_names.clear();
        }
        Ok(())
    }

    /// 원본 `MixNew()`: 연산마다 새 S3 클라이언트를 만든다.
    pub async fn mix_new(&self) -> Result<(), UpDownError> {
        let etag = self.file_etag_if_checked()?;
        while !self.quit.get() {
            let object_name = self.next_object_name()?;
            let put_client = self.new_s3_client();
            if put_object(
                &put_client,
                &self.bucket_name,
                &object_name,
                &self.file_path,
                self.config.use_chunk_encoding,
            )
            .await
            {
                self.stats.write.add_success(1);
            } else {
                self.stats.write.add_error(1);
            }
            if self.quit.get() {
                break;
            }
            let get_client = self.new_s3_client();
            if get_object(
                &get_client,
                &self.bucket_name,
                &object_name,
                self.config.file_size,
                etag.as_deref(),
            )
            .await
            {
                self.stats.read.add_success(1);
            } else {
                self.stats.read.add_error(1);
            }
            if self.quit.get() {
                break;
            }
            let delete_client = self.new_s3_client();
            if delete_object(&delete_client, &self.bucket_name, &object_name, None).await {
                self.stats.delete.add_success(1);
            } else {
                self.stats.delete.add_error(1);
            }
        }
        Ok(())
    }

    /// 원본 `PutGet()`.
    pub async fn put_get(&self) -> Result<(), UpDownError> {
        let etag = self.file_etag_if_checked()?;
        while !self.quit.get() {
            let object_name = self.next_object_name()?;
            self.count_put(&object_name).await;
            if self.quit.get() {
                break;
            }
            self.count_get(&object_name, etag.as_deref()).await;
        }
        Ok(())
    }

    /// 원본 `All()`: 쓰고 읽고 지운다.
    pub async fn all(&self) -> Result<(), UpDownError> {
        let etag = self.file_etag_if_checked()?;
        while !self.quit.get() {
            let object_name = self.next_object_name()?;
            self.count_put(&object_name).await;
            if self.quit.get() {
                break;
            }
            self.count_get(&object_name, etag.as_deref()).await;
            if self.quit.get() {
                break;
            }
            self.count_delete(&object_name, None).await;
        }
        Ok(())
    }

    /// 원본 `MultiUpload(loopCount, partSize)`.
    pub async fn multi_upload(&self, loop_count: i32, part_size: i64) -> Result<(), UpDownError> {
        for _ in 0..loop_count {
            if self.quit.get() {
                break;
            }
            let object_name = self.next_object_name()?;
            if self.multipart_object(&object_name, part_size).await {
                self.stats.write.add_success(1);
            } else if !self.quit.get() {
                self.stats.write.add_error(1);
            }
        }
        self.quit.set(true);
        Ok(())
    }

    async fn multipart_object(&self, object_name: &str, part_size: i64) -> bool {
        self.multipart_upload(
            &self.client,
            &self.bucket_name,
            object_name,
            &self.file_path,
            self.config.file_size,
            part_size,
            self.config.use_chunk_encoding,
        )
        .await
    }

    /// 원본 `MultiUploadV2(loopCount, partSize)`: 올린 뒤 읽어서 확인한다.
    pub async fn multi_upload_v2(
        &self,
        loop_count: i32,
        part_size: i64,
    ) -> Result<(), UpDownError> {
        let etag = self.file_etag_if_checked()?;
        for _ in 0..loop_count {
            if self.quit.get() {
                break;
            }
            let object_name = self.next_object_name()?;
            if self.multipart_object(&object_name, part_size).await {
                self.stats.write.add_success(1);
            } else if !self.quit.get() {
                self.stats.write.add_error(1);
            }
            if self.quit.get() {
                break;
            }
            self.count_get(&object_name, etag.as_deref()).await;
        }
        self.quit.set(true);
        Ok(())
    }

    /// 원본 `Upload(count, partSize)`: TransferUtility 업로드.
    pub async fn upload(&self, count: i32, part_size: i64) -> Result<(), UpDownError> {
        for _ in 0..count {
            if self.quit.get() {
                break;
            }
            let object_name = self.next_object_name()?;
            if upload_object(
                &self.client,
                &self.bucket_name,
                &object_name,
                &self.file_path,
                part_size,
            )
            .await
            {
                self.stats.write.add_success(1);
            } else {
                self.stats.write.add_error(1);
            }
        }
        self.quit.set(true);
        Ok(())
    }

    /// 원본 `Download(count)`: TransferUtility 다운로드.
    pub async fn download(&self, count: i32) -> Result<(), UpDownError> {
        for _ in 0..count {
            if self.quit.get() {
                break;
            }
            let object_name = self.next_object_name()?;
            if download_object(
                &self.client,
                &self.bucket_name,
                &object_name,
                &self.file_path,
            )
            .await
            {
                self.stats.read.add_success(1);
            } else {
                self.stats.read.add_error(1);
            }
        }
        self.quit.set(true);
        Ok(())
    }

    /// 원본 `UploadTag(count)`: 진행 구간(10분의 1)마다 다른 태그를 붙인다.
    /// `count < 10`이면 원본처럼 0으로 나누기 오류가 난다.
    pub async fn upload_tag(&self, count: i32) -> Result<(), UpDownError> {
        let tags: Vec<Tag> = (0..10)
            .map(|i| {
                let value = self.thread_number * 10 + i + 1;
                Tag::builder()
                    .key(TAG_KEY_NAME)
                    .value(format!("tag{value}"))
                    .build()
                    .expect("키와 값이 있다")
            })
            .collect();
        let check_point = count / 10;
        for i in 0..count {
            if self.quit.get() {
                break;
            }
            let object_name = self.next_object_name()?;
            if check_point == 0 {
                return Err(UpDownError::DivideByZero);
            }
            let tag_index = (i / check_point) as usize;
            // 원본 `uploadTagList[tagIndex]`: 범위를 넘으면 예외(count가 10의 배수가 아닐 때 마지막 구간).
            let tag = tags
                .get(tag_index)
                .cloned()
                .ok_or(UpDownError::IndexOutOfRange)?;
            if put_object_tag(
                &self.client,
                &self.bucket_name,
                &object_name,
                &self.file_path,
                tag,
            )
            .await
            {
                self.stats.write.add_success(1);
            } else {
                self.stats.write.add_error(1);
            }
        }
        self.quit.set(true);
        Ok(())
    }

    /// 원본 `AWSTest(count)`: Put → Head → Get → Delete(버전) → ListObjects를 반복한다.
    pub async fn aws_test(&self, count: i32) -> Result<(), UpDownError> {
        let etag = self.file_etag_if_checked()?;
        for _ in 0..count {
            if self.quit.get() {
                break;
            }
            let now = Local::now();
            let index = self
                .object_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let object_name = format!(
                "{}/{:02}/{:02}/{:02}/{:02}/FILE_{}_{:05}",
                now.year(),
                now.month(),
                now.day(),
                now.hour(),
                now.minute(),
                self.thread_number,
                index
            );
            self.count_put(&object_name).await;
            if self.quit.get() {
                break;
            }
            let (ok, version_id) = head_object(&self.client, &self.bucket_name, &object_name).await;
            if ok {
                self.stats.head.add_success(1);
            } else {
                self.stats.head.add_error(1);
            }
            if self.quit.get() {
                break;
            }
            self.count_get(&object_name, etag.as_deref()).await;
            if self.quit.get() {
                break;
            }
            // 원본은 Head 결과의 버전 ID(실패하면 빈 문자열, 헤더가 없으면 null)를 그대로 넘긴다.
            self.count_delete(&object_name, version_id.as_deref()).await;
            if self.quit.get() {
                break;
            }
            if list_objects(&self.client, &self.bucket_name, None, None, None)
                .await
                .is_some()
            {
                self.stats.list.add_success(1);
            } else {
                self.stats.list.add_error(1);
            }
        }
        self.quit.set(true);
        Ok(())
    }

    /// 원본 `SampleUpload(prefix, fileCount)`: 내용이 "1"인 객체를 올린다. 실패는 그대로 오류로 낸다.
    pub async fn sample_upload(&self, prefix: &str, file_count: i32) -> Result<(), UpDownError> {
        let begin = self.thread_number * file_count;
        let end = (self.thread_number + 1) * file_count;
        for i in begin..end {
            if self.quit.get() {
                break;
            }
            self.client
                .put_object(
                    &self.bucket_name,
                    &format!("{prefix}{i:07}"),
                    testrust_s3::s3_client::PutBody::Text("1".to_string()),
                    false,
                    None,
                )
                .await?;
        }
        Ok(())
    }

    /// 원본 `ListObject()`: 스레드 접두어로 목록을 끝까지 반복해서 받는다.
    pub async fn list_object(&self) -> Result<(), UpDownError> {
        let prefix = self.thread_prefix();
        let mut next_marker: Option<String> = None;
        while !self.quit.get() {
            let Some(response) = list_objects(
                &self.client,
                &self.bucket_name,
                Some(&prefix),
                next_marker.as_deref(),
                None,
            )
            .await
            else {
                self.stats.list.add_error(1);
                continue;
            };
            if response.is_truncated.unwrap_or(false) {
                next_marker = response.next_marker.clone();
            } else {
                next_marker = None;
                self.stats
                    .loop_end_count
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            self.stats.list.add_success(1);
        }
        Ok(())
    }
}
