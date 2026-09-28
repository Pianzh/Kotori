//! **退出后自动上传：为什么没上传**那一族。
//!
//! 起因是用户 2026-09-28 在 Windows 上的实测："推出后没有自动上传，需要手动上传"。
//! 手动能传是因为它走另一个函数、不受单款开关限制（见 `actions::rpc_sync_now`），所以
//! 现象本身是自洽的 —— 但**没有任何一处说得清是哪一道闸门关着**，而四条早退路里有一条
//! 连日志都没有。
//!
//! 这些测试盯的是"**说得清**"，不是"传了没有"。理由在 `exit_upload_gate` 的说明里：
//! 从前那个函数返回 `()`，于是"开关关着"与"引擎没装"在调用方眼里一模一样；单测夹具里
//! 没有引擎二进制，那个函数**必然**在"引擎没装"那里掉头 —— 于是"开关关着时不该上传"
//! 这条测试即使写了，也只会因为**错误的理由**变绿。
//!
//! 所以判据 [`exit_upload_gate`] 是纯函数（只看 [`Config`]，不碰引擎、不碰云端、不读密钥
//! 环），下面每条都直接问它要确切答案。`sync_after_game_exit` 与 `sync.status` 报的是同一
//! 份 [`SkipReason`]，所以这些断言同时也是"界面和日志不会分叉"的看门测试。
//!
//! ⚠ **这里一条都不去碰引擎**，因此也没有一条会打到网络上去。"引擎不可用"与"引擎报错"
//! 那两臂在 `tests/ipc_e2e/sync.rs` 里用假 rclone 验 —— 单测里 `find_kopia` 会回退到
//! `PATH`，那台机器上装没装 kopia 会决定走哪条路，在这里写断言等于把 CI 的步骤顺序
//! 变成测试的前提。

use super::super::{ExitUpload, SkipReason, exit_upload_gate};
use super::{call, daemon_at, demo_config};
use crate::config::Config;
use crate::secrets::testing::FakeTool;

/// 把某一款摆成想要的样子（`Config` 是值，先 clone 一份再改那一条）。
fn with_game(config: &Config, edit: impl FnOnce(&mut crate::config::GameConfig)) -> Config {
    let mut config = config.clone();
    edit(config.games.get_mut("demo").expect("夹具里总有 demo"));
    config
}

/// **单款开关**关着 ⇒ 说的是"这一款的开关关着"，不是"没找到引擎"。
///
/// 这是本文件的核心：判据是纯的，所以"关着的开关"与"别的任何原因"能在这里分得开。
/// 从前只有 `()` 可言，"没传"是"不该传"还是"没传成"谁也分不出来。
#[test]
fn the_per_game_switch_names_itself_instead_of_saying_nothing() {
    let config = with_game(&demo_config(), |game| game.sync_enabled = false);

    let refusal = exit_upload_gate("demo", &config).expect_err("关掉这一款就该被挡下");
    assert_eq!(
        refusal.reason,
        SkipReason::PerGameOff,
        "开关关着必须报成 per_game_off，别的说法都会把用户引到错的地方：{}",
        refusal.detail
    );
    // 话要能直接给用户看：得说清是**哪一款**、是**这一款的开关**。
    assert!(refusal.detail.contains("demo"), "{}", refusal.detail);
    assert!(refusal.detail.contains("开关"), "{}", refusal.detail);
}

/// 总开关关着也要有自己的说法 —— 这条早退路**从前连日志都没有**。
#[test]
fn the_global_switch_is_reported_too() {
    let mut config = demo_config();
    config.sync.enabled = false;

    let refusal = exit_upload_gate("demo", &config).expect_err("总开关关着就该被挡下");
    assert_eq!(refusal.reason, SkipReason::SyncOff, "{}", refusal.detail);
    assert!(
        refusal.detail.contains("总开关"),
        "要说清是总开关，不是这一款：{}",
        refusal.detail
    );
}

/// 没填存档位置：说"没填位置"，而不是含糊的"跳过上传"。
#[test]
fn a_game_without_save_locations_says_what_is_missing() {
    let config = with_game(&demo_config(), |game| game.save_paths.clear());

    let refusal = exit_upload_gate("demo", &config).expect_err("没填位置就该被挡下");
    assert_eq!(
        refusal.reason,
        SkipReason::NoSavePaths,
        "{}",
        refusal.detail
    );
    assert!(refusal.detail.contains("存档位置"), "{}", refusal.detail);
}

/// 配置里根本没有这一款 —— **不许报成"开关关着"**。
///
/// 从前那句判断是 `config.games.get(id).is_some_and(|g| g.sync_enabled)`：找不到游戏与
/// 开关关着在这里是**同一个 false**，于是"这一款不在配置里"被静默说成了"你关掉了它" ——
/// 而用户能做的事完全相反（该去建这条档案，不是去打开开关）。
#[test]
fn a_game_missing_from_the_config_is_not_reported_as_a_closed_switch() {
    let config = demo_config();

    let refusal = exit_upload_gate("ghost", &config).expect_err("配置里没有就该说没有");
    assert_eq!(
        refusal.reason,
        SkipReason::UnknownGame,
        "\"配置里没有这一款\"和\"你把开关关了\"要分得开：{}",
        refusal.detail
    );
    assert!(refusal.detail.contains("ghost"), "{}", refusal.detail);
}

/// 两件事都不成立时，**先说开关**。
///
/// 顺序刻意与 `actions::sync_after_game_exit` 从前的实现一致：开关在前、位置在后。反过来
/// 的话，"报了存档位置没填"而"开关其实关着"会把用户引到一件他不需要做的事上。
#[test]
fn the_switch_wins_over_the_missing_locations_so_the_message_stays_useful() {
    let config = with_game(&demo_config(), |game| {
        game.sync_enabled = false;
        game.save_paths.clear();
    });

    let refusal = exit_upload_gate("demo", &config).expect_err("两道都关着");
    assert_eq!(refusal.reason, SkipReason::PerGameOff, "{}", refusal.detail);
}

/// 一切就绪时交出该用的设置（总开关开着、这一款开着、位置填了）。
#[test]
fn a_ready_game_gets_its_settings_back() {
    let config = demo_config();

    let settings = exit_upload_gate("demo", &config).expect("默认就该是通的");
    assert_eq!(settings.bucket, "bkt");
    assert!(settings.enabled);
}

/// ⚠ **本文件存在的理由**（用户 2026-09-28）：启动前那一问里选「关掉这一款的同步」会把
/// `sync_enabled = false` **持久化**进 `config.toml`（见 `sync_rpc/selfcheck.rs`）。从那
/// 以后这一款**永远不再自动上传**，而界面上看不出任何因果关系 —— 用户能看到的只有
/// "手动能传、自动不传"。
///
/// 而从前的看门测试 `tests/resolve.rs` 里那条
/// `switching_one_game_off_stops_its_automatic_sync_only` **只测了启动前的取回**
/// （`sync_pull_before_launch`），退出后的上传那半条链一个字都没验过。这条把它补上：同一个
/// 弹窗回答，既要落到配置里，也要真的挡住退出后的上传。
#[tokio::test]
async fn answering_off_in_the_self_check_permanently_stops_exit_uploads() {
    let fake = FakeTool::new("exit-upload-selfcheck-off");
    let (daemon, path) = daemon_at(fake.keyring());
    // 起点：这一款开着，也真的会走自动上传那条路。
    // ⚠ 配置先 clone 成快照再问：那个 `&guard` 推不出 `&Config`（那是 E0308，
    //   不是 deref 能不能coerce 的问题），而判据本来就不要锁。
    let starting = daemon.config.read().await.clone();
    assert_eq!(
        exit_upload_gate("demo", &starting)
            .expect("起点就该是通的")
            .bucket,
        "bkt"
    );

    // 用户在启动前那一问里选了「关掉这一款的同步」。
    let value = call(&daemon, "sync.resolve", r#"{"id":"demo","choice":"off"}"#).await;
    assert_eq!(value["result"]["ok"], true, "{value}");

    // ① 落到了配置里（不只是内存）。
    assert!(
        !crate::config::load_from(&path).unwrap().games["demo"].sync_enabled,
        "那个回答要真的落盘，否则重开 daemon 就当没答过"
    );

    // ② 退出后的上传因此被挡下，而且**说的是这件事**。
    let outcome = daemon.sync_after_game_exit("demo").await;
    let ExitUpload::Skipped(refusal) = outcome else {
        panic!("关掉这一款之后，退出后不该还去上传：{outcome:?}");
    };
    assert_eq!(refusal.reason, SkipReason::PerGameOff, "{}", refusal.detail);

    // ③ 界面也报同一个原因（`sync.status` 走的是同一份判据）。
    let status = call(&daemon, "sync.status", "").await;
    let blocked = &status["result"]["games"][0]["auto_upload_blocked"];
    assert_eq!(
        blocked["reason"], "per_game_off",
        "界面必须说得出是哪一道闸门关着：{status}"
    );
    assert!(blocked["detail"].as_str().is_some_and(|d| !d.is_empty()));

    // ④ 用户自己把开关打开 ⇒ 恢复自动上传（这个回答不是一次性的）。
    let value = call(
        &daemon,
        "game.update",
        r#"{"id":"demo","sync_enabled":true}"#,
    )
    .await;
    assert_eq!(value["result"]["success"], true, "{value}");
    let reopened = daemon.config.read().await.clone();
    assert!(
        exit_upload_gate("demo", &reopened).is_ok(),
        "重新打开之后就该恢复自动上传"
    );

    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

/// 跳过**也要留痕** —— 否则界面上留着的还是上一次那条"上传成功"。
///
/// ⚠ 一局玩完什么也没传，而设置页上"上次同步"还写着成功、时间戳还是上一次那一次：用户
/// 没有任何线索知道这次没传（用户 2026-09-28 报的正是这个查不出来的现象）。修复前
/// `remember` 只在真的传了之后才被调用，所以跳过时 `last` 是 `null` 或者**过期的旧值**。
#[tokio::test]
async fn a_skipped_exit_upload_is_recorded_instead_of_leaving_the_old_result() {
    let fake = FakeTool::new("exit-upload-stale-record");
    let (daemon, path) = daemon_at(fake.keyring());
    let value = call(&daemon, "sync.resolve", r#"{"id":"demo","choice":"off"}"#).await;
    assert_eq!(value["result"]["ok"], true, "{value}");

    // 先假装上一次传成过 —— 用户界面上此刻显示的就是这条。
    daemon
        .sync
        .remember_note("demo", "上传", true, "1 个位置已上传");

    // 这一局玩完，退出后什么也没传。
    daemon.sync_after_game_exit("demo").await;

    let status = call(&daemon, "sync.status", "").await;
    let last = &status["result"]["games"][0]["last"];
    assert_eq!(
        last["detail"], "《demo》这一款的云同步开关关着",
        "跳过之后界面上留着的必须是**这一次**的实话，不是上一次的\"上传成功\"：{status}"
    );
    // 跳过不是失败：它是用户自己选的，不该标成错误。
    assert_eq!(last["ok"], true, "{status}");

    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

/// 界面报的原因与**日志/返回值**说的必须是同一份。
///
/// 这三条各自问同一个判据的一处。分叉了就意味着"界面上写的"与"日志里说的"不是一回事 ——
/// 用户拿着界面那句话去翻日志会找不到，而那正是这次要根治的东西。
#[tokio::test]
async fn the_status_reason_and_the_returned_reason_cannot_diverge() {
    let fake = FakeTool::new("exit-upload-no-divergence");
    let (daemon, path) = daemon_at(fake.keyring());
    let value = call(&daemon, "sync.resolve", r#"{"id":"demo","choice":"off"}"#).await;
    assert_eq!(value["result"]["ok"], true, "{value}");

    let returned = match daemon.sync_after_game_exit("demo").await {
        ExitUpload::Skipped(refusal) => refusal.reason,
        other => panic!("该是跳过：{other:?}"),
    };
    let status = call(&daemon, "sync.status", "").await;
    let reported = status["result"]["games"][0]["auto_upload_blocked"]["reason"]
        .as_str()
        .expect("界面必须报出原因")
        .to_string();

    assert_eq!(
        reported,
        returned.code(),
        "界面报「{reported}」而实际是「{}」——两处说法分叉了",
        returned.code()
    );

    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}
