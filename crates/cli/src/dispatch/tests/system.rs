//! 원본 `CommandDispatcher`의 시나리오 실행: LocalTest, MultiSystemTest, AccessIpsTest, UsedSizeTest.

use testrust_common::to_dotnet_json;
use testrust_scenarios::access_ips::AccessIpsTest;
use testrust_scenarios::local::LocalTest;
use testrust_scenarios::multi_system::MultiSystemTest;
use testrust_scenarios::used_size::UsedSizeTest;

use super::super::{CommandContext, CommandResult, not_ported};
use crate::menu::MenuList;

pub(super) async fn run(ctx: &mut CommandContext, menu: MenuList) -> CommandResult {
    use MenuList::*;
    match menu {
        LocalPrepareTest
        | LocalPutTest
        | LocalGetTest
        | LocalGetTestV2
        | LocalPutGetTest
        | LocalDeleteTest
        | LocalMultipartPrepareTest
        | LocalMultipartPutTest
        | LocalMultipartGetTest
        | LocalMultipartGetTestV2
        | LocalMultipartPutGetTest => local(ctx, menu).await,
        MultiSystemListTest
        | MultiSystemUploadTest
        | MultiSystemUpDownTest
        | MultiSystemAllTest => multi_system(ctx, menu).await,
        AccessIpsTest => access_ips(ctx).await,
        UsedSizeTest => used_size(ctx).await,
        _ => not_ported(menu),
    }
}

/// `UsedSizeTest`: `Start`와 `StartVersions`를 모두 실행하고 하나라도 0이 아니면 -1.
async fn used_size(ctx: &mut CommandContext) -> CommandResult {
    let config = ctx.config();
    let mut test = UsedSizeTest::new(&config.used_size, &config.db, &config.main_user);
    let mut result = 0;
    if test.start().await? != 0 {
        result = -1;
    }
    if test.start_versions().await? != 0 {
        result = -1;
    }
    Ok(result)
}

/// `AccessIpsTest`: 설정을 출력한 뒤 실행한다.
async fn access_ips(ctx: &mut CommandContext) -> CommandResult {
    let config = ctx.config();
    // print config
    println!("Portal : {}", to_dotnet_json(&config.portal));
    println!("AccessIps : {}", to_dotnet_json(&config.access_ips));

    let mut test = AccessIpsTest::new(&config.portal, &config.access_ips)?;
    test.start().await?;
    Ok(0)
}

/// `MultiSystemXxxTest` 메뉴.
async fn multi_system(ctx: &mut CommandContext, menu: MenuList) -> CommandResult {
    use MenuList::*;
    let config = ctx.config();
    let bucket_name = ctx.options.bucket_name.clone().unwrap_or_default();
    let mut test = MultiSystemTest::new(
        &config.main,
        config.multi_system_upload(),
        &config.main_user,
        &bucket_name,
        &ctx.cancel,
    );
    println!("{}", to_dotnet_json(&config.multi_system_view()));
    let multipart = ctx.options.multipart;
    match menu {
        MultiSystemListTest => test.list(ctx.options.flag).await?,
        MultiSystemUploadTest => test.prepare(multipart).await?,
        MultiSystemUpDownTest => test.put_get(multipart).await?,
        _ => test.mix(multipart).await?,
    }
    Ok(0)
}

/// `LocalXxxTest` 메뉴. 대상 경로 확인(`TargetPath`)은 상위(`tests/mod.rs`)에서 한다.
async fn local(ctx: &mut CommandContext, menu: MenuList) -> CommandResult {
    use MenuList::*;
    let config = ctx.config();
    let use_multipart = matches!(
        menu,
        LocalMultipartPrepareTest
            | LocalMultipartPutTest
            | LocalMultipartGetTest
            | LocalMultipartGetTestV2
            | LocalMultipartPutGetTest
    );
    let mut test = LocalTest::new(
        &config.main,
        &config.up_down,
        &config.main.target_path,
        use_multipart,
    );
    let (check, start) = (ctx.options.check, ctx.options.start_count);
    match menu {
        LocalPrepareTest | LocalMultipartPrepareTest => test.prepare(check, start).await?,
        LocalPutTest | LocalMultipartPutTest => test.write().await?,
        LocalGetTest | LocalMultipartGetTest => test.read().await?,
        LocalGetTestV2 | LocalMultipartGetTestV2 => test.read_v2(start).await?,
        LocalPutGetTest | LocalMultipartPutGetTest => test.put_get().await?,
        _ => test.delete().await?,
    }
    Ok(0)
}
