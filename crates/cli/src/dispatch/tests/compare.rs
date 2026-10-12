//! 원본 `CommandDispatcher`의 시나리오 실행: CompareTest, CopyTest(RangeReadCopy), DuplicateTest, LifecycleTest, MoverTest.

use std::time::Instant;

use testrust_config::{CompareConfig, CopyConfig};
use testrust_scenarios::clear::ClearTest;
use testrust_scenarios::compare::CompareTest;
use testrust_scenarios::copy::CopyTest;
use testrust_scenarios::duplicate::DuplicateTest;
use testrust_scenarios::lifecycle::LifecycleTest;
use testrust_scenarios::mover::MoverTest;
use tracing::{error, info};

use super::super::{CommandContext, CommandResult};
use crate::menu::MenuList;

pub(super) async fn run(ctx: &mut CommandContext, menu: MenuList) -> CommandResult {
    match menu {
        MenuList::CompareTest => {
            let config = ctx.config();
            let test = if ctx.options.another {
                CompareTest::with_alt(config.compare.clone(), &config.main_user, &config.alt_user)
            } else {
                CompareTest::new(config.compare.clone(), &config.main_user)
            };
            test.start().await
        }
        MenuList::RangeReadCopy => {
            info!("RangeReadCopy Start");
            let o = &ctx.options;
            let copy = CopyConfig {
                source_bucket: o.source.clone().unwrap_or_default(),
                source_object: o.source_key.clone().unwrap_or_default(),
                target_bucket: o.bucket_name.clone().unwrap_or_default(),
                target_object: o.key.clone().unwrap_or_default(),
            };
            let started = Instant::now();
            let test = CopyTest::new(copy, ctx.config().main_user.clone());
            test.start().await?;
            info!(
                "RangeReadCopy : complete time = {}ms",
                started.elapsed().as_millis()
            );
            Ok(0)
        }
        MenuList::DuplicateTest => {
            let config = ctx.config();
            let test = DuplicateTest::new(
                config.main.clone(),
                config.duplicate.clone(),
                &config.main_user,
            );
            test.start().await?;
            Ok(0)
        }
        MenuList::LifecycleTest => {
            let test = LifecycleTest::new(ctx.client().clone());
            test.start(ctx.options.bucket_name.as_deref().unwrap_or_default())
                .await?;
            Ok(0)
        }
        MenuList::MoverTest => {
            let config = ctx.config();
            let mut test = MoverTest::new(
                config.main.clone(),
                config.mover.clone(),
                config.main_user.clone(),
            );
            let (source, target) = (
                config.mover.source_bucket.clone(),
                config.mover.target_bucket.clone(),
            );
            if !test.start().await {
                error!("MoverTest failed");
            } else {
                let compare_option = CompareConfig::new(
                    &source, &target, false, true, true, false, true, false, true,
                );
                let compare = CompareTest::new(compare_option, &ctx.config().main_user);
                compare.start().await?;
            }
            let mut bucket_clear = ClearTest::new(ctx.client().clone());
            bucket_clear
                .bucket_clear(&source, None, None, 1000, false)
                .await;
            bucket_clear
                .bucket_clear(&target, None, None, 1000, false)
                .await;
            Ok(0)
        }
        _ => unreachable!("compare 묶음이 아닌 메뉴"),
    }
}
