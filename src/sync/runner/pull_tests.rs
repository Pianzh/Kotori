//! 启动前取回那一族（`super::pull`）。
//!
//! 从 `pull.rs` 里搬出来（AGENTS.md：单个源码文件尽量 500 行内）—— 与 `index_tests`、
//! `digest_tests` 同一个习惯，用 `#[path]` 指过来。

use crate::sync::archive;
use crate::sync::cloud::PackIdentity;
use crate::sync::runner::PullResult;
use crate::sync::runner::testing::{FakeRclone, target};

/// 本机这一款的云端身份。云端那些包由 [`publish`] 带着它上传。
const CLOUD_ID: &str = "cloud-demo";

fn identity() -> PackIdentity {
    PackIdentity {
        cloud_id: CLOUD_ID.to_string(),
        machine_id: Some("machine-a".to_string()),
        fingerprint: None,
        locations: vec!["rel-savedata".to_string()],
    }
}

/// 把一个包放到云端：先在本地打一个，再让假 rclone 搬过去。
///
/// `mtime_ms` 显式给定，不靠"文件刚写完"——两次写入之间只差几毫秒，而 ms
/// 精度下它们可能落在同一刻度上，那这条测试就会时绿时红。
fn publish(fake: &FakeRclone, saves: &std::path::Path, stamp: &str, body: &str, mtime_ms: i64) {
    publish_as(Some(&identity()), fake, saves, stamp, body, mtime_ms);
}

/// 同上，但可以指定包里的身份（`None` = 更早的 kotori 传的那种没有身份的包）。
fn publish_as(
    identity: Option<&PackIdentity>,
    fake: &FakeRclone,
    saves: &std::path::Path,
    stamp: &str,
    body: &str,
    mtime_ms: i64,
) {
    std::fs::create_dir_all(saves).unwrap();
    let path = saves.join("save.sav");
    std::fs::write(&path, body).unwrap();
    set_mtime_ms(&path, mtime_ms);
    let target = target(saves, "savedata", "rel-savedata");
    let zip = fake.dir.join("publish.zip");
    archive::pack(&zip, &[target], chrono::Utc::now(), identity).unwrap();
    fake.put_package("demo", stamp, &zip);
}

fn set_mtime_ms(path: &std::path::Path, ms: i64) {
    let time = std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms as u64);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

#[tokio::test]
async fn a_pull_takes_the_newest_package_and_leaves_newer_local_files_alone() {
    let fake = FakeRclone::new("pull");
    let cloud = fake.dir.join("cloud-saves");
    // 云端那一版很旧（1970 年的第 1 秒），本机这一份是刚写的。
    publish(&fake, &cloud, "20260901T000000Z", "from the cloud", 1_000);

    // 本机版本更新：上一次上传失败了，用户的进度只在本机。
    let saves = fake.dir.join("saves");
    std::fs::create_dir_all(&saves).unwrap();
    std::fs::write(saves.join("save.sav"), "local and newer").unwrap();
    let target = target(&saves, "savedata", "rel-savedata");

    let outcome = fake
        .runner(0)
        .pull("demo", "Demo", "demo", &[target], Some(CLOUD_ID), None)
        .await
        .into_outcome();
    assert!(outcome.ok, "{outcome:?}");
    assert_eq!(outcome.locations[0].action, "kept");
    assert!(
        outcome.locations[0].detail.contains("保持不动"),
        "{:?}",
        outcome.locations[0]
    );
    assert_eq!(
        std::fs::read_to_string(saves.join("save.sav")).unwrap(),
        "local and newer",
        "a pull must never eat the progress the user just made"
    );
}

#[tokio::test]
async fn a_pull_brings_back_files_the_cloud_has_newer_versions_of() {
    let fake = FakeRclone::new("pull-newer");
    let cloud = fake.dir.join("cloud-saves");
    publish(&fake, &cloud, "20260901T000000Z", "from the cloud", 2_000);

    // 本机是旧的（时间戳被推回更早）。
    let saves = fake.dir.join("saves");
    std::fs::create_dir_all(&saves).unwrap();
    let local = saves.join("save.sav");
    std::fs::write(&local, "old local").unwrap();
    set_mtime_ms(&local, 1_000);

    let outcome = fake
        .runner(0)
        .pull(
            "demo",
            "Demo",
            "demo",
            &[target(&saves, "savedata", "rel-savedata")],
            Some(CLOUD_ID),
            None,
        )
        .await
        .into_outcome();
    assert!(outcome.ok, "{outcome:?}");
    assert_eq!(outcome.locations[0].action, "pulled");
    assert_eq!(
        std::fs::read_to_string(saves.join("save.sav")).unwrap(),
        "from the cloud"
    );
}

#[tokio::test]
async fn a_game_the_cloud_has_never_seen_is_not_an_error() {
    let fake = FakeRclone::new("pull-empty");
    let saves = fake.dir.join("saves");
    std::fs::create_dir_all(&saves).unwrap();

    // ⚠ "没什么可做"不是错误（`ok == true`），但它**不是"铺过了"**：调用方要据此
    // 决定不写基线、也不立"本次已对上账"的牌子（§6.5 第 5 步）。
    let result = fake
        .runner(0)
        .pull(
            "demo",
            "Demo",
            "demo",
            &[target(&saves, "savedata", "rel-savedata")],
            Some(CLOUD_ID),
            None,
        )
        .await;
    assert!(
        matches!(result, PullResult::Refused(_)),
        "云端没有这一款 = 一个字节都没铺：{result:?}"
    );
    let outcome = result.into_outcome();
    assert!(outcome.ok, "{outcome:?}");
    assert_eq!(outcome.locations[0].action, "skipped");
    assert!(outcome.locations[0].detail.contains("云端还没有"));
    assert_eq!(fake.calls().len(), 1, "only the listing happened");
}

#[tokio::test]
async fn a_package_that_cannot_be_fetched_says_so_instead_of_pretending_there_is_none() {
    let fake = FakeRclone::new("pull-broken");
    let saves = fake.dir.join("saves");
    std::fs::create_dir_all(&saves).unwrap();
    fake.put(
        "kotori:bkt/prefix/games/demo/20260901T000000Z.zip",
        "garbage",
    );
    // 假 rclone 把对象原样搬下来，所以这里下来的是一个坏包。
    std::fs::write(saves.join("save.sav"), "local").unwrap();

    let outcome = fake
        .runner(0)
        .pull(
            "demo",
            "Demo",
            "demo",
            &[target(&saves, "savedata", "rel-savedata")],
            Some(CLOUD_ID),
            None,
        )
        .await
        .into_outcome();

    assert!(!outcome.ok);
    let error = outcome.error.unwrap();
    assert!(error.contains("读不出来"), "{error}");
    assert!(
        !error.contains("云端还没有"),
        "a broken package is not 'nothing to do': {error}"
    );
    assert_eq!(
        std::fs::read_to_string(saves.join("save.sav")).unwrap(),
        "local"
    );
}

#[tokio::test]
async fn a_location_the_package_does_not_cover_is_reported_separately() {
    let fake = FakeRclone::new("pull-missing-location");
    let cloud = fake.dir.join("cloud-saves");
    publish(&fake, &cloud, "20260901T000000Z", "cloud", 1_000);

    let saves = fake.dir.join("saves");
    std::fs::create_dir_all(&saves).unwrap();
    let outcome = fake
        .runner(0)
        .pull(
            "demo",
            "Demo",
            "demo",
            &[
                target(&saves, "savedata", "rel-savedata"),
                // 这一台机器上还有另一个位置，云端这一版里没有它。
                target(&saves, "extra", "rel-extra"),
            ],
            Some(CLOUD_ID),
            None,
        )
        .await
        .into_outcome();

    assert!(outcome.ok, "{outcome:?}");
    assert_eq!(outcome.locations[1].action, "skipped");
    assert!(outcome.locations[1].detail.contains("这个位置"));
}

/// 云端那一版是**别的一款**（身份对不上）：一个文件都不许铺。
///
/// 这条盯的是整件事里唯一不可逆的错误：把别人的存档铺进本机这一款，
/// 用户下一次上传再把它推回云端 —— **静默损坏存档**。
#[tokio::test]
async fn a_pull_from_another_identity_touches_nothing() {
    let fake = FakeRclone::new("pull-other-identity");
    let cloud = fake.dir.join("cloud");
    publish(
        &fake,
        &cloud,
        "20260901T000000Z",
        "someone else's save",
        2_000,
    );

    let saves = fake.dir.join("saves");
    std::fs::create_dir_all(&saves).unwrap();
    let local = saves.join("save.sav");
    std::fs::write(&local, "my own progress").unwrap();
    set_mtime_ms(&local, 1_000);

    let outcome = fake
        .runner(0)
        .pull(
            "demo",
            "Demo",
            "demo",
            &[target(&saves, "savedata", "rel-savedata")],
            Some("another-identity"),
            None,
        )
        .await
        .into_outcome();

    assert!(!outcome.ok, "{outcome:?}");
    let error = outcome.error.unwrap();
    assert!(error.contains("另一个身份"), "{error}");
    assert!(error.contains("本机存档一个都没动"), "{error}");
    assert_eq!(
        std::fs::read_to_string(&local).unwrap(),
        "my own progress",
        "闸门必须在铺文件之前拦下来"
    );
}

/// 本机这一款还没认领过身份：不猜，直接跳过（取回那条路**不认领**身份）。
#[tokio::test]
async fn a_pull_before_this_machine_claimed_an_identity_is_skipped() {
    let fake = FakeRclone::new("pull-unpaired");
    let cloud = fake.dir.join("cloud");
    publish(&fake, &cloud, "20260901T000000Z", "from the cloud", 2_000);

    let saves = fake.dir.join("saves");
    std::fs::create_dir_all(&saves).unwrap();

    let outcome = fake
        .runner(0)
        .pull(
            "demo",
            "Demo",
            "demo",
            &[target(&saves, "savedata", "rel-savedata")],
            None,
            None,
        )
        .await
        .into_outcome();

    assert!(!outcome.ok, "{outcome:?}");
    let error = outcome.error.unwrap();
    assert!(error.contains("还没有云端身份"), "{error}");
    assert!(
        !saves.join("save.sav").exists(),
        "跳过就是跳过：一个文件都不该落下来"
    );
}

/// 云端那一版是更早的 kotori 传的（清单里没有身份段）：同样不猜。
#[tokio::test]
async fn a_pull_refuses_a_package_without_any_identity() {
    let fake = FakeRclone::new("pull-no-identity");
    let cloud = fake.dir.join("cloud");
    publish_as(
        None,
        &fake,
        &cloud,
        "20260901T000000Z",
        "old package",
        2_000,
    );

    let saves = fake.dir.join("saves");
    std::fs::create_dir_all(&saves).unwrap();

    let outcome = fake
        .runner(0)
        .pull(
            "demo",
            "Demo",
            "demo",
            &[target(&saves, "savedata", "rel-savedata")],
            Some(CLOUD_ID),
            None,
        )
        .await
        .into_outcome();

    assert!(!outcome.ok, "{outcome:?}");
    let error = outcome.error.unwrap();
    assert!(error.contains("没有身份信息"), "{error}");
    assert!(!saves.join("save.sav").exists());
}

/// 云端那一版的内容值**就是**索引说的那个 ⇒ 照常铺（正对照）。
///
/// 与下面那条配对：核对这道闸门既不能漏（对不上还铺），也不能**误伤**（对得上却
/// 不铺 —— 那会让每一次自动取回都失败，比不做还坏）。
#[tokio::test]
async fn a_package_matching_the_index_digest_is_laid_down() {
    let fake = FakeRclone::new("pull-digest-ok");
    let cloud = fake.dir.join("cloud-saves");
    publish(&fake, &cloud, "20260901T000000Z", "from the cloud", 2_000);

    let saves = fake.dir.join("saves");
    std::fs::create_dir_all(&saves).unwrap();
    let local = saves.join("save.sav");
    std::fs::write(&local, "old local").unwrap();
    set_mtime_ms(&local, 1_000);
    // 索引里那个值就是**云端这一版的内容值**（上传时顺手算的同一个值）——
    // 拿发布出去的那份内容算：`cloud` 目录就是包里的那一版。
    let digest = archive::local_digest(&[target(&cloud, "savedata", "rel-savedata")]).unwrap();
    let target = target(&saves, "savedata", "rel-savedata");

    let result = fake
        .runner(0)
        .pull(
            "demo",
            "Demo",
            "demo",
            std::slice::from_ref(&target),
            Some(CLOUD_ID),
            Some(&digest),
        )
        .await;

    assert!(matches!(result, PullResult::Laid(_)), "{result:?}");
    assert_eq!(
        std::fs::read_to_string(&local).unwrap(),
        "from the cloud",
        "内容值对得上就该铺"
    );
}

/// ⚠ §6.5 第 3 步：包的 `manifest.digest` 与索引里的 `latest_digest` 对不上
/// （包被人换了，或者索引在骗人）⇒ **丢弃 staging + 报错，绝不铺**。
///
/// 这一条盯的是整件事里最后一不可逆的动作：铺进去之后本机存档就变了，而它下一次
/// 上传还会把这个错的版本推上云。所以闸门必须在 `lay_down` **之前**。
#[tokio::test]
async fn a_package_whose_digest_disagrees_with_the_index_is_not_laid_down() {
    let fake = FakeRclone::new("pull-digest-mismatch");
    let cloud = fake.dir.join("cloud-saves");
    publish(&fake, &cloud, "20260901T000000Z", "from the cloud", 2_000);

    let saves = fake.dir.join("saves");
    std::fs::create_dir_all(&saves).unwrap();
    let local = saves.join("save.sav");
    std::fs::write(&local, "old local").unwrap();
    set_mtime_ms(&local, 1_000);

    // 索引说这一版的内容值是别的什么东西（被人改过、或者它自己就是错的）。
    let expected = "ab".repeat(32);
    let result = fake
        .runner(0)
        .pull(
            "demo",
            "Demo",
            "demo",
            &[target(&saves, "savedata", "rel-savedata")],
            Some(CLOUD_ID),
            Some(&expected),
        )
        .await;

    assert!(
        matches!(result, PullResult::Refused(_)),
        "对不上就不许铺：{result:?}"
    );
    let outcome = result.into_outcome();
    assert!(!outcome.ok, "{outcome:?}");
    let error = outcome.error.unwrap();
    assert!(error.contains("对不上"), "{error}");
    assert!(error.contains("本机存档一个都没动"), "{error}");
    assert_eq!(
        std::fs::read_to_string(&local).unwrap(),
        "old local",
        "核对必须在铺文件之前"
    );
    assert_eq!(
        std::fs::read_to_string(saves.join("save.sav")).unwrap(),
        "old local"
    );
}
