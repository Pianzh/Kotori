//! 自检与配对结论那一族（启动前那一问 + `sync.resolve`）。
//!
//! 夹具在 `super`（`tests.rs`）—— 那边已经顶过 500 行软线，所以这一族单开一个文件。

use super::{call, daemon_at};
use crate::secrets::Keyring;
use crate::secrets::testing::FakeTool;

/// 每款一个的云同步开关：关掉这一款 ⇒ **不自动**取回 / 上传；手动那条路不受它限制。
#[tokio::test]
async fn switching_one_game_off_stops_its_automatic_sync_only() {
    let fake = FakeTool::new("per-game-switch");
    let (daemon, _) = daemon_at(fake.keyring());

    // 开着（默认）：启动前那条路会去干活（这里凭据是齐的，所以给得出回话）。
    assert!(
        daemon.sync_pull_before_launch("demo").await.is_some(),
        "开关开着时，启动前该走同步那条路"
    );

    // 用户把这一款关掉。
    let value = call(
        &daemon,
        "game.update",
        r#"{"id":"demo","sync_enabled":false}"#,
    )
    .await;
    assert_eq!(value["result"]["success"], true, "{value}");
    assert!(!daemon.config.read().await.games["demo"].sync_enabled);

    // 自动取回：一个字都不做（回话是 `None` = 没什么可报的）。
    assert!(
        daemon.sync_pull_before_launch("demo").await.is_none(),
        "关掉之后不许自动取回"
    );

    // 手动「立即同步」是用户自己按的：不受这个开关限制。
    let value = call(&daemon, "sync.now", r#"{"id":"demo"}"#).await;
    assert!(
        value["result"].is_object() || value["error"].is_object(),
        "手动同步这一条路不该被开关拦掉: {value}"
    );

    // 再打开：开关就是个开关，不是一次性的。
    let value = call(
        &daemon,
        "game.update",
        r#"{"id":"demo","sync_enabled":true}"#,
    )
    .await;
    assert_eq!(value["result"]["success"], true, "{value}");
    assert!(daemon.config.read().await.games["demo"].sync_enabled);
}

/// 启动前的自检与"用户答了什么"：三种回答各自的效果，以及**答过就不许再问**。
#[tokio::test]
async fn the_pre_launch_self_check_asks_once_and_remembers_the_answer() {
    use crate::sync::selfcheck::Decision;

    let fake = FakeTool::new("selfcheck");
    let (daemon, _) = daemon_at(fake.keyring());
    let signature = crate::sync::signature::of(&daemon.config.read().await.sync).unwrap();

    // 新档案、没有指纹：认不出云端那一条 ⇒ **问一次**（不带"疑似找到的那一条"）。
    assert_eq!(
        daemon.sync_selfcheck("demo").await,
        Decision::Ask {
            found: None,
            trouble: None
        }
    );

    // "自己挑一条绑上"：绑上云端那一条 ⇒ 结论是"已确认"，而且**真的绑着**。
    let value = call(
        &daemon,
        "sync.resolve",
        r#"{"id":"demo","choice":"pair","cloud_id":"cloud-1","cloud_key":"demo"}"#,
    )
    .await;
    assert_eq!(value["result"]["ok"], true, "{value}");
    {
        let config = daemon.config.read().await;
        assert_eq!(config.games["demo"].cloud_id.as_deref(), Some("cloud-1"));
        assert_eq!(
            config.games["demo"].cloud_conclusion.as_deref(),
            Some(format!("ok:{signature}").as_str())
        );
    }
    // 真的绑上了 ⇒ 下次不问、也不重扫（`Pull` 那条路一个字节都不读云端）。
    assert_eq!(daemon.sync_selfcheck("demo").await, Decision::Pull);

    // 换了目标（桶）：结论作废，回到"未定"。没有指纹时照样是"问一次"。
    call(
        &daemon,
        "sync.set_settings",
        r#"{"bucket":"another-bucket"}"#,
    )
    .await;
    assert_eq!(
        daemon.sync_selfcheck("demo").await,
        Decision::Ask {
            found: None,
            trouble: None
        }
    );

    // "关掉这一款的同步"：只关这一款，而且记住"问过了"。
    let value = call(&daemon, "sync.resolve", r#"{"id":"demo","choice":"off"}"#).await;
    assert_eq!(value["result"]["ok"], true, "{value}");
    {
        let config = daemon.config.read().await;
        assert!(!config.games["demo"].sync_enabled);
        let conclusion = config.games["demo"].cloud_conclusion.clone().unwrap();
        assert!(conclusion.starts_with("off:"), "{conclusion}");
    }
    // 关着的时候打开游戏：一个字都不做（不再问第二次）。
    assert_eq!(daemon.sync_selfcheck("demo").await, Decision::Skip);

    // 用户自己把这一款重新打开：**上次那份结论被清掉**（用户 2026-09-24："之后我不论开关
    // 云同步都不会再次弹窗，这也是问题"），于是下一次启动重新自检 —— 指纹认不出就是
    // "再问一次"，不再是"直接新建、再也不问"。
    call(
        &daemon,
        "game.update",
        r#"{"id":"demo","sync_enabled":true}"#,
    )
    .await;
    {
        let config = daemon.config.read().await;
        assert!(
            config.games["demo"].cloud_conclusion.is_none(),
            "重新打开要把上次那份结论清掉"
        );
    }
    assert_eq!(
        daemon.sync_selfcheck("demo").await,
        Decision::Ask {
            found: None,
            trouble: None
        }
    );
}

#[tokio::test]
async fn resolve_can_bind_an_existing_cloud_identity() {
    let (daemon, path) = daemon_at(Keyring::memory());
    let value = call(
        &daemon,
        "sync.resolve",
        r#"{"id":"demo","choice":"pair","cloud_id":"cloud-1","cloud_key":"remote/demo"}"#,
    )
    .await;
    assert_eq!(value["result"]["ok"], true, "{value}");

    let config = crate::config::load_from(&path).unwrap();
    let game = &config.games["demo"];
    assert_eq!(game.cloud_id.as_deref(), Some("cloud-1"));
    assert_eq!(game.cloud_dir.as_deref(), Some("remote/demo"));
    assert!(
        game.cloud_conclusion
            .as_deref()
            .is_some_and(|value| value.starts_with("ok:"))
    );

    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

#[tokio::test]
async fn resolve_pair_without_an_identity_creates_a_new_binding_decision() {
    let (daemon, path) = daemon_at(Keyring::memory());
    let value = call(&daemon, "sync.resolve", r#"{"id":"demo","choice":"pair"}"#).await;
    assert_eq!(value["result"]["ok"], true, "{value}");

    let config = crate::config::load_from(&path).unwrap();
    let game = &config.games["demo"];
    assert!(game.cloud_id.is_none());
    assert!(game.cloud_dir.is_none());
    assert!(
        game.cloud_conclusion
            .as_deref()
            .is_some_and(|value| value.starts_with("new:"))
    );

    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

#[tokio::test]
async fn resolve_rejects_an_unknown_choice_without_mutating_the_binding() {
    let (daemon, path) = daemon_at(Keyring::memory());
    let before = crate::config::load_from(&path).unwrap();

    let value = call(
        &daemon,
        "sync.resolve",
        r#"{"id":"demo","choice":"unknown"}"#,
    )
    .await;
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("不认识的回答"),
        "{value}"
    );

    let after = crate::config::load_from(&path).unwrap();
    assert_eq!(after.games["demo"].cloud_id, before.games["demo"].cloud_id);
    assert_eq!(
        after.games["demo"].cloud_dir,
        before.games["demo"].cloud_dir
    );
    assert_eq!(
        after.games["demo"].cloud_conclusion,
        before.games["demo"].cloud_conclusion
    );

    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

/// **没读到云端 ≠ 云端没有这一款**（用户 2026-09-28 在 Windows 上踩的那次：桶名填成了
/// `kotori-win`，而 Linux 那边是 `kotori-saves`，kopia 回 `bucket not found` —— 弹窗却
/// 只说"云端没有对得上的"，于是他以为是指纹匹配坏了）。
///
/// 夹具里没有 kopia/rclone，"这一款有指纹、云端又读不到"正是它天然的样子：结论必须照旧是
/// `Ask`（自检绝不拦启动），但**要带着原因**。
#[tokio::test]
async fn a_cloud_that_cannot_be_read_says_so_instead_of_claiming_nothing_is_there() {
    use crate::sync::selfcheck::Decision;

    let (daemon, _) = daemon_at(Keyring::memory());
    {
        let mut config = daemon.config.write().await;
        config.games.get_mut("demo").unwrap().exe_fingerprint = Some("v1:1:aa".to_string());
    }

    match daemon.sync_selfcheck("demo").await {
        Decision::Ask { found, trouble } => {
            assert!(found.is_none(), "云端都读不到，就没有'疑似找到的那一条'");
            let trouble = trouble.expect("必须说得出'为什么没认出来'");
            assert!(!trouble.is_empty(), "原因不许是空的");
        }
        other => panic!("有指纹、云端又读不到 ⇒ 该问一次，而不是 {other:?}"),
    }
}

#[tokio::test]
async fn repeating_the_same_pair_is_idempotent() {
    let (daemon, path) = daemon_at(Keyring::memory());
    let body = r#"{"id":"demo","choice":"pair","cloud_id":"cloud-1","cloud_key":"remote/demo"}"#;

    let first = call(&daemon, "sync.resolve", body).await;
    let second = call(&daemon, "sync.resolve", body).await;
    assert_eq!(first["result"]["ok"], true, "{first}");
    assert_eq!(second["result"]["ok"], true, "{second}");

    let config = crate::config::load_from(&path).unwrap();
    let game = &config.games["demo"];
    assert_eq!(game.cloud_id.as_deref(), Some("cloud-1"));
    assert_eq!(game.cloud_dir.as_deref(), Some("remote/demo"));
    assert!(
        game.cloud_conclusion
            .as_deref()
            .is_some_and(|value| value.starts_with("ok:"))
    );

    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}
