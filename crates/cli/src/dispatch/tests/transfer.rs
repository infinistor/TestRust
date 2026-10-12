//! 원본 `CommandDispatcher`의 시나리오 실행: MultiPartTest(ManualUpload), MultiUploadTest, RangeReadTest, FindTagTest, MultiDownloadTest, IoTest.

use std::time::Instant;

use testrust_scenarios::find_tag::FindTagTest;
use testrust_scenarios::io::IoTest;
use testrust_scenarios::multi_download::MultiDownloadTest;
use testrust_scenarios::multi_part::MultiPartTest;
use testrust_scenarios::multi_upload::MultiUploadTest;
use testrust_scenarios::range_read;
use tracing::info;

use super::super::{CommandContext, CommandError, CommandResult, not_ported};
use crate::menu::MenuList;

pub(super) async fn run(ctx: &mut CommandContext, menu: MenuList) -> CommandResult {
    match menu {
        MenuList::ManualUpload => manual_upload(ctx).await,
        MenuList::MultiUploadTest => multi_upload_test(ctx).await,
        MenuList::RangeReadTest => range_read_test(ctx).await,
        MenuList::FindTagTest => find_tag_test(ctx).await,
        MenuList::IoTest => io_test(ctx).await,
        MenuList::DirectoryDownloadTest | MenuList::FileListDownloadTest => {
            download_test(ctx, menu).await
        }
        _ => not_ported(menu),
    }
}

/// 원본 `case MenuList.IoTest`. 인자를 검증하지 않는다(`--source`는 업로드 폴더, `--target`은 다운로드 폴더).
async fn io_test(ctx: &mut CommandContext) -> CommandResult {
    let o = &ctx.options;
    let test = IoTest::new(ctx.config().main_user.clone());
    test.start(
        o.bucket_name.as_deref(),
        o.source.as_deref(),
        o.target.as_deref(),
    )
    .await?;
    Ok(0)
}

/// 원본 `case MenuList.DirectoryDownloadTest`·`FileListDownloadTest`(검증은 `tests/mod.rs`).
/// 목록 파일 경로는 `--test-file-list=`(`path`), 저장할 폴더는 `--file`(`filePath`)이다.
async fn download_test(ctx: &mut CommandContext, menu: MenuList) -> CommandResult {
    let o = &ctx.options;
    let thread = if o.thread < 1 { 10 } else { o.thread };
    let bucket = o.bucket_name.as_deref().unwrap_or_default();
    let download_path = o.file_path.as_deref().unwrap_or_default();

    let mut test = MultiDownloadTest::new(ctx.config().main_user.clone());
    if menu == MenuList::DirectoryDownloadTest {
        test.start(thread, bucket, o.prefix.as_deref(), download_path)
            .await?;
    } else {
        test.file(
            thread,
            bucket,
            o.path.as_deref().unwrap_or_default(),
            download_path,
        )
        .await?;
    }
    Ok(0)
}

/// 원본 `case MenuList.FindTagTest`. `--test-find-tag` 값이 없으면 `NullReferenceException`,
/// 쉼표가 없으면 `IndexOutOfRangeException`이다(원본은 검증하지 않는다).
async fn find_tag_test(ctx: &mut CommandContext) -> CommandResult {
    let o = &ctx.options;
    let Some(tags) = &o.tags else {
        return Err(super::super::input::null_reference());
    };
    let mut parts = tags.split(',');
    let tag_key = parts.next().unwrap_or_default();
    let Some(tag_value) = parts.next() else {
        return Err(CommandError::new(
            "System.IndexOutOfRangeException",
            "Index was outside the bounds of the array.",
        ));
    };

    let test = FindTagTest::new(
        ctx.client().clone(),
        if o.thread < 1 { 10 } else { o.thread },
    );
    test.start(o.bucket_name.as_deref(), tag_key, tag_value)
        .await?;
    Ok(0)
}

/// 원본 `case MenuList.RangeReadTest`(검증은 `tests/mod.rs`).
async fn range_read_test(ctx: &mut CommandContext) -> CommandResult {
    let o = &ctx.options;
    let thread = if o.thread < 1 { 10 } else { o.thread };

    info!("RangeReadTest Start");

    let sw = Instant::now();
    range_read::start(
        thread,
        ctx.client(),
        o.bucket_name.as_deref().unwrap_or_default(),
        o.key.as_deref().unwrap_or_default(),
        &o.range_list,
        o.count,
    )
    .await?;
    info!(
        "RangeReadTest : complete time = {}ms",
        sw.elapsed().as_millis()
    );
    Ok(0)
}

/// 원본 `case MenuList.MultiUploadTest`(검증은 `tests/mod.rs`).
async fn multi_upload_test(ctx: &mut CommandContext) -> CommandResult {
    let o = &ctx.options;
    let thread = if o.thread < 1 { 10 } else { o.thread };
    let bucket = o.bucket_name.as_deref().unwrap_or_default();
    let key = o.key.as_deref().unwrap_or_default();
    let (file_size, part_size) = (o.file_size, o.part_size);

    info!("MultiUploadTest Start");

    let sw = Instant::now();
    let client = ctx.client();
    // 버킷 생성
    client.create_bucket(bucket).await;
    // 스레드 생성
    let mut tasks = Vec::new();
    for i in 0..thread {
        if file_size > 1 {
            let test = MultiUploadTest::new_multipart(
                client.clone(),
                i,
                bucket,
                key,
                file_size,
                part_size as i32,
            );
            tasks.push(tokio::spawn(async move { test.multipart_upload().await }));
        } else {
            let test = MultiUploadTest::new_put(client.clone(), i, bucket, key, file_size);
            tasks.push(tokio::spawn(async move { test.put_object().await }));
        }
    }
    // 모든 스레드가 종료될 때까지 대기
    for task in tasks {
        let _ = task.await;
    }

    info!(
        "MultiUploadTest : complete time = {}ms",
        sw.elapsed().as_millis()
    );
    Ok(0)
}

/// 원본 `case MenuList.ManualUpload`(검증은 `tests/mod.rs`).
async fn manual_upload(ctx: &mut CommandContext) -> CommandResult {
    let o = &ctx.options;
    info!("ManualUpload Start");
    let sw = Instant::now();
    let mut test = MultiPartTest::new(&ctx.config().main_user);
    test.start(
        o.bucket_name.as_deref().unwrap_or_default(),
        o.key.as_deref().unwrap_or_default(),
        o.file_path.as_deref().unwrap_or_default(),
        o.part_size,
        10,
    )
    .await?;
    info!(
        "ManualUpload : complete time = {}ms",
        sw.elapsed().as_millis()
    );
    Ok(0)
}
