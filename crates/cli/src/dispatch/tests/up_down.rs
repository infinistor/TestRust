//! 원본 `CommandDispatcher`의 UpDownTest 실행: Upload·Download·Prepare·Put·Get·Delete·Mix·All·Multipart·PutTag·AWS·ListObj·FullTest·MultiDelete.
//!
//! 도움말과 실행 전 검증은 `tests/mod.rs`에서 했다. 여기서는 원본 `case`의 실행 부분(`new UpDownTest(...)`와 시나리오 호출,
//! 분산 실행과 같은 진입점인 `BasicTestRunner.Execute`)을 옮긴다. 모든 메뉴의 반환값은 0이다.
//!
//! 원본 `FullTest`는 Prepare → ReadV2 → 버킷 비우기(삭제 포함)를 차례로 실행한다. Ctrl+C는 실행 중인 테스트만
//! 멈추므로(`scenarios::shutdown`) 뒤 단계는 원본처럼 그대로 실행된다.

use testrust_scenarios::clear::ClearTest;
use testrust_scenarios::up_down::UpDownTest;

use super::super::{CommandContext, CommandResult, not_ported};
use crate::menu::MenuList;

/// 설정과 취소 토큰으로 만든 원본 `new UpDownTest(config.Main, config.UpDown, config.MainUser)`.
fn new_test(ctx: &CommandContext) -> UpDownTest {
    let config = ctx.config();
    UpDownTest::new(
        &config.main,
        &config.up_down,
        &config.main_user,
        &ctx.cancel,
    )
}

pub(super) async fn run(ctx: &mut CommandContext, menu: MenuList) -> CommandResult {
    use MenuList::*;
    let o = &ctx.options;
    let (check, start, random, bulk, count) = (o.check, o.start_count, o.random, o.bulk, o.count);
    let (prefix, key) = (o.prefix.clone(), o.key.clone());
    let mut test = new_test(ctx);
    match menu {
        ListObjTest => test.list_object_test().await?,
        UploadTest => test.upload().await?,
        DownloadTest => test.download().await?,
        // `BasicTestRunner.Execute(test, "Prepare", check, startCount, random)`
        Prepare => test.prepare(check, start, random).await?,
        PrepareDir => test.prepare_dir(check, start).await?,
        NewPutTest => test.prepare_new(check, start).await?,
        // `BasicTestRunner.Execute(test, "Put", start: startCount, random: random)`
        PutTest => test.write(start, random).await?,
        HeadTest => test.head().await?,
        // `BasicTestRunner.Execute(test, "Get")`
        GetTest => test.read().await?,
        GetTestV2 => test.read_v2(start).await?,
        NewGetTest => test.read_new(start).await?,
        GetTestV3 => test.read_v3().await?,
        // `BasicTestRunner.Execute(test, "Delete", bulk: bulk, maxCount: count)`
        DeleteTest => test.delete(bulk, count).await?,
        DeleteTestV2 => test.delete_v2(start).await?,
        NewDelTest => test.delete_new(start).await?,
        DeleteTestVersion => test.delete_version(bulk, count, prefix.as_deref()).await?,
        DeleteDirectoryTest => test.delete_directory().await?,
        // `BasicTestRunner.Execute(test, "Mix")`
        MixTest => test.mix().await?,
        MixV2Test => test.mix_v2().await?,
        NewMixTest => test.mix_new().await?,
        PutGetTest => test.put_get().await?,
        AllTest => test.all().await?,
        FullTest => {
            test.prepare(false, 0, false).await?;
            let mut read_test = new_test(ctx);
            read_test.read_v2(0).await?;
            // `clear.BucketClear(config.Main.BucketName, isDelete: true)`: 기본 `maxKeys`는 1000.
            let bucket = ctx.config().main.bucket_name.clone();
            let mut clear = ClearTest::new(ctx.client().clone());
            clear
                .bucket_clear(&bucket, Option::None, Option::None, 1000, true)
                .await;
        }
        MultiDeleteTest => {
            test.delete_one(key.as_deref().unwrap_or_default(), count)
                .await?
        }
        MultipartUploadTest => test.multipart_upload().await?,
        MultipartUploadAndDownloadTest => test.multipart_upload_v2().await?,
        PutTagTest => test.upload_tag().await?,
        AWSTest => test.aws_test().await?,
        _ => return not_ported(menu),
    }
    Ok(0)
}
