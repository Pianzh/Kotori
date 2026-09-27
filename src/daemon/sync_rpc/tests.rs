//! `sync.*` RPC 的单测：设置怎么校验、状态里能出现什么、以及凭据那几级存储
//! 各自的行为。

use super::super::*;
// `SettingsPatch` 住在 `sync_rpc` 自己那一层（`pub(super)`），不随 `daemon::*` 过来。
use crate::config::{Config, GameConfig, SavePath, ScaleProfile};
use crate::secrets::Keyring;
#[cfg(unix)]
use crate::secrets::SecretKey;
use crate::secrets::testing::FakeTool;
use std::path::PathBuf;

// 自检与配对结论那一族单开了一个文件（`tests/resolve.rs`）：这个文件顶过 500 行软线，
// 拆开之后两边都留了余量。夹具（`call` / `daemon*`）仍是 `pub(super)`，那边 `use super::…`。
mod resolve;

/// Send a raw JSON-RPC request through the real dispatcher.
pub(super) async fn call(daemon: &Daemon, method: &str, params: &str) -> Value {
    let request = if params.is_empty() {
        format!(r#"{{"jsonrpc":"2.0","id":1,"method":"{method}"}}"#)
    } else {
        format!(r#"{{"jsonrpc":"2.0","id":1,"method":"{method}","params":{params}}}"#)
    };
    let reply = daemon.handle_request(&request).await;
    serde_json::from_str(&reply.body).unwrap_or_else(|e| panic!("bad reply {}: {e}", reply.body))
}

/// The sync settings every test here starts from.
pub(super) fn daemon_config() -> Config {
    let mut config = Config::default();
    config.sync.enabled = true;
    config.sync.bucket = "bkt".to_string();
    config
}

pub(super) fn daemon(keyring: Keyring) -> Daemon {
    daemon_at(keyring).0
}

/// 同上，但把配置文件的路径也交出来 —— 单测要断言"写下去的东西真的落盘了"。
pub(super) fn daemon_at(keyring: Keyring) -> (Daemon, PathBuf) {
    let mut config = Config::default();
    config.sync.enabled = true;
    config.sync.endpoint = String::new();
    config.sync.bucket = "bkt".to_string();
    config.games.insert(
        "demo".into(),
        GameConfig {
            cloud_id: None,
            game_dir_mount: None,
            exe_mount: None,
            exe_fingerprint: None,
            cloud_dir: None,
            cloud_rejected: Vec::new(),
            sync_enabled: true,
            cloud_conclusion: None,
            name: "demo".into(),
            game_dir: PathBuf::from("/games/demo"),
            exe_path: PathBuf::from("/games/demo/game.exe"),
            launch_args: Vec::new(),
            save_paths: vec![SavePath::inferred("savedata")],
            wine_prefix: None,
            auto_watch: false,
            direct_launch: false,
            process_name: None,
            scale_profile: ScaleProfile::default_for(),
            created_at: chrono::Utc::now(),
        },
    );
    // Tests own a throw-away config file: the daemon persists every settings
    // change, and the machine-wide config is not theirs to touch.
    let dir = std::env::temp_dir().join(format!(
        "kotori-sync-rpc-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let path = dir.join("config.toml");
    // 先把这份配置**写进那个文件**:daemon 改配置时会先重读磁盘(见
    // `Daemon::mutate_config`),而"内存里有、磁盘上没有"的 daemon 在生产里不
    // 存在 —— 启动时它就是从这个文件读出来的。
    crate::config::save_to(&path, &config).unwrap();
    (
        Daemon::with_keyring(config, keyring).with_config_path(path.clone()),
        path,
    )
}

/// `sync.status` 不许把**配置读锁**跨着 `await` 持有 —— 那会和写请求形成**自死锁**。
///
/// 用户 2026-09-26 报的"点『给这一款新建一条』之后卡在启动中"就是它（整台 daemon 一起
/// 僵住）。交错是这样的：`sync.status` 先拿到配置读锁，然后带着它去读云端索引
/// （`cloud_index_view`），而那一步自己**还要再取一次**读锁；此时写请求（`sync.resolve`）
/// 在写锁上排队，tokio 读写锁的公平性让那次**新的读**也排在写者后面 —— 读者等的是自己
/// 手里的锁，两边都回不来，之后所有读配置的请求一起排队。
///
/// 这条测试把那个窗口**撑成确定的**：读者的路上有一把同步锁（`records`），测试先把它攥住，
/// 读者就停在"已经持有读锁"的状态上；这时放写请求进来（它会排在写锁上），最后松开那把
/// 同步锁。修复前：读者继续往前走、去取第二次读锁、排在写者后面 —— 死锁成立。
/// `timeout` 收口，所以它只会失败，不会把 CI 挂住。
// ⚠ 这条测试**故意**把那把同步锁跨着 `await` 攥住 —— 要的就是那个交错。clippy 的
// `await_holding_lock` 在这里是误报（被测的锁是 `records`，不是配置那把）。
#[allow(clippy::await_holding_lock)]
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sync_status_does_not_hold_the_config_lock_across_its_await() {
    let fake = FakeTool::new("sync-status-lock");
    let (daemon, _path) = daemon_at(fake.keyring());
    let daemon = std::sync::Arc::new(daemon);

    // ① 攥住 `records`：`sync.status` 会停在它上面 —— 那一刻它已经拿着配置读锁。
    let held = daemon.sync.records.lock().unwrap();

    // ② 让读者跑起来，并给它足够时间停在那把锁上。
    let reader = {
        let daemon = daemon.clone();
        tokio::spawn(async move { daemon.rpc_sync_status().await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // ③ 放一个写请求进来：它会在配置写锁上排队。
    let writer = {
        let daemon = daemon.clone();
        tokio::spawn(async move {
            daemon
                .mutate_config(|config| {
                    config.sync.enabled = !config.sync.enabled;
                    Ok(Value::Null)
                })
                .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // ④ 松开：读者继续走，去取它那第二次读锁 —— 修复前这里就再也回不来了。
    drop(held);

    let finished = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let _ = reader.await;
        let _ = writer.await;
    })
    .await;

    assert!(
        finished.is_ok(),
        "sync.status 与写请求撞在一起时死锁了：配置读锁被跨着 await 持有"
    );
}

/// 没有存档位置就开不了云同步 —— 用户 2026-09-26 定的规则：**身份是身份，路径是路径**。
///
/// 界面把那颗开关置灰（`ui::render::detail`），但规则得在 daemon 上成立 —— 界面之外还有
/// CLI 和别的客户端。允许"只填身份、不填路径"，代价就是开不了同步；而**身份没填照样能开**，
/// 那时启动会弹那一问，所以"弹窗"这条路上必然已经有路径了。
#[tokio::test]
async fn cloud_sync_cannot_be_switched_on_without_a_save_location() {
    let fake = FakeTool::new("sync-needs-a-path");
    let (daemon, path) = daemon_at(fake.keyring());

    // 造出"没有存档位置、同步也关着"的那一款。
    //
    // 开关先**真的**关一次："关"不看路径，所以这一步会成功，内存与磁盘一起落到"关着"
    // （生产里这两份总是一致的 —— 启动时 daemon 读的就是这个文件）。存档位置再单独从
    // **磁盘上**抹掉：daemon 改配置时先重读磁盘（见 `Daemon::mutate_config`），内存里
    // 那份不再算数。
    let rewrite = |save_paths: Vec<SavePath>, sync_enabled: bool| {
        let mut on_disk = crate::config::load_at(&path).unwrap();
        let game = on_disk.games.get_mut("demo").unwrap();
        game.save_paths = save_paths;
        game.sync_enabled = sync_enabled;
        crate::config::save_to(&path, &on_disk).unwrap();
    };
    let off = call(
        &daemon,
        "game.update",
        r#"{"id":"demo","sync_enabled":false}"#,
    )
    .await;
    assert_eq!(off["result"]["success"], true, "{off}");
    rewrite(Vec::new(), false);

    // 想开：拒绝，并说清原因。
    let refused = call(
        &daemon,
        "game.update",
        r#"{"id":"demo","sync_enabled":true}"#,
    )
    .await;
    let message = refused["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("存档位置"), "{refused}");
    assert!(
        !daemon.config.read().await.games["demo"].sync_enabled,
        "被拒之后开关不该留下"
    );

    // 填上路径之后就能开 —— 校验看的是"这一刻"的档案，不认死哪一次请求。
    rewrite(vec![SavePath::inferred("savedata")], false);
    let allowed = call(
        &daemon,
        "game.update",
        r#"{"id":"demo","sync_enabled":true}"#,
    )
    .await;
    assert_eq!(allowed["result"]["success"], true, "{allowed}");
    assert!(daemon.config.read().await.games["demo"].sync_enabled);

    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

// 假 secret-tool(FakeTool)是 shell 脚本,Unix 限定——Windows 的密钥环后端
// 还没实现,这两条在 Windows 上 spawn 不出来(os error 193)。
#[cfg(unix)]
#[tokio::test]
async fn status_never_returns_a_secret_value() {
    let fake = FakeTool::new("status-secrets");
    let keyring = fake.keyring();
    keyring.set(SecretKey::B2KeyId, "keyid123").unwrap();
    keyring.set(SecretKey::B2AppKey, "appkey456").unwrap();

    let daemon = daemon(keyring);
    let value = call(&daemon, "sync.status", "").await;
    let body = value.to_string();
    let result = &value["result"];
    assert_eq!(result["enabled"], true);
    assert_eq!(result["settings"]["bucket"], "bkt");
    assert_eq!(result["remote"], "kotori:bkt/kotori");
    // Names only: the UI has no business holding the key.
    assert_eq!(result["secrets"][0], "b2-key-id");
    assert!(!body.contains("keyid123"), "{body}");
    assert!(!body.contains("appkey456"), "{body}");
    // 同步密码随 crypt 层一起没了：status 里也不该再有它的取回提示。
    assert!(result.get("password_hint").is_none(), "{result}");
    assert_eq!(result["games"][0]["locations"], 1);
}

#[tokio::test]
async fn status_explains_what_is_missing_instead_of_failing() {
    let fake = FakeTool::new("status-missing");
    let value = call(&daemon(fake.keyring()), "sync.status", "").await;
    assert_eq!(value["result"]["ready"], false);
    assert!(
        value["result"]["problem"]
            .as_str()
            .unwrap()
            .contains("B2 凭据"),
        "{value}"
    );
}

#[tokio::test]
async fn settings_are_validated_before_they_are_stored() {
    let fake = FakeTool::new("settings");
    let daemon = daemon(fake.keyring());

    // An endpoint the backend cannot use, and paths that could escape the
    // prefix or the bucket.
    for (body, needle) in [
        // The B2 console shows the S3 endpoint first, and it is the wrong
        // one for this backend; say so instead of failing later with a 404.
        (
            r#"{"endpoint":"s3.us-west-004.backblazeb2.com"}"#,
            "S3 兼容接口",
        ),
        // rclone does not add a scheme, so a bare host cannot work.
        (r#"{"endpoint":"api001.backblazeb2.com"}"#, "https://"),
        (r#"{"bucket":"my/bucket"}"#, "bucket"),
        (r#"{"prefix":"../other"}"#, ".."),
        (r#"{"prefix":"  "}"#, "prefix"),
        (r#"{"keep_versions":100000}"#, "最多"),
        // Enabling with nowhere to sync to is refused.
        (r#"{"enabled":true,"bucket":""}"#, "bucket"),
    ] {
        let value = call(&daemon, "sync.set_settings", body).await;
        assert!(
            value["error"]["message"]
                .as_str()
                .is_some_and(|m| m.contains(needle)),
            "expected {needle:?} in {value}"
        );
    }

    // Turning sync off and clearing the fields together is fine: an
    // unfinished setup is only a problem while sync is on.
    let value = call(
        &daemon,
        "sync.set_settings",
        r#"{"enabled":false,"endpoint":""}"#,
    )
    .await;
    assert_eq!(value["result"]["settings"]["enabled"], false);
    assert_eq!(value["result"]["settings"]["endpoint"], "");
    let value = call(&daemon, "sync.set_settings", r#"{"enabled":true}"#).await;
    assert_eq!(value["result"]["settings"]["enabled"], true);

    // A good patch is stored, and persisted.
    let value = call(
        &daemon,
        "sync.set_settings",
        r#"{"bucket":"new-bucket","prefix":"/saves/kotori/"}"#,
    )
    .await;
    assert_eq!(value["result"]["settings"]["bucket"], "new-bucket");
    assert_eq!(
        value["result"]["settings"]["prefix"], "saves/kotori",
        "the prefix is normalised, not rejected"
    );
}

/// 本机的机器身份：一台机器只有一个，而且**落盘**（重开进程读到的必须是同一个）。
#[tokio::test]
async fn the_machine_identity_is_created_once_and_persisted() {
    let fake = FakeTool::new("machine-id");
    let (daemon, config_path) = daemon_at(fake.keyring());
    assert_eq!(daemon.config.read().await.daemon.machine_id, None);

    let first = daemon.machine_id().await.unwrap();
    assert_eq!(first.len(), 36, "机器身份是个 uuid: {first}");
    assert_eq!(daemon.machine_id().await.unwrap(), first, "一台机器一个");

    let persisted = crate::config::load_at(&config_path).unwrap();
    assert_eq!(
        persisted.daemon.machine_id.as_deref(),
        Some(first.as_str()),
        "身份卡上要拿它对账，所以必须落盘"
    );
}
