//! **退出后自动上传：到底传了没有、没传为什么**—— 端到端那一族。
//!
//! 从 `sync.rs` 拆出来（那边已经 437 行，这一族再加就要越过 600 的硬线）。
//!
//! 起因是用户 2026-09-28 在 Windows 上的实测："推出后没有自动上传，需要手动上传，而且
//! 上传时间大于 1 分钟"。这里验两件事，各自对应那个现象的一层：
//!
//! 1. **没上传时，界面说得出是哪一道闸门关着** —— 不用翻日志就能自查。判据本身的
//!    单测在 `sync_rpc/tests/exit_upload.rs`，那边不碰网络。
//! 2. **引擎卡死不会把 daemon 拖住** —— 那一分钟到底花在哪，第一步先分出"是不是同步把
//!    整机堵了"：只读的 `daemon.status` 还能不能秒回。
//!
//! ⚠ **全部靠假 rclone**（`fixture::enable_fake_sync`）：它真搬文件但零网络，所以
//! "传了没有"验的是真的打包与真的合并，而不是"参数长得像不像"。

use std::time::{Duration, Instant};

use serde_json::json;

use crate::fixture::Fixture;
use crate::helpers::{cloud_packages, wait_until, write_script};

/// 建一款游戏：回传 **id、存档目录、exe**。
///
/// ⚠ id **不许在调用方硬写**：它是 daemon 从名字推出来的（"ExitGame" → `exitgame`，带空格的
/// "Life Game" 才是 `life-game`）。硬写过一次就红了一轮 —— 断言拿着一个不存在的 id，
/// 看不出是"没建档成功"还是"找错了那一款"。
fn make_game(fixture: &Fixture, name: &str) -> (String, std::path::PathBuf, std::path::PathBuf) {
    let game_dir = fixture.dir.join(name);
    let saves = game_dir.join("savedata");
    std::fs::create_dir_all(&saves).unwrap();
    let exe = game_dir.join("game.exe");
    std::fs::write(&exe, b"").unwrap();

    // ⚠ `game.create` 回的是 `{id, name}`，**没有** `success` 那一栏。
    let response = fixture.rpc(
        "game.create",
        json!({ "name": name, "exe_path": exe, "game_dir": game_dir }),
    );
    let id = response["result"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("建档要回 id: {response}"))
        .to_string();
    let response = fixture.rpc(
        "game.update",
        json!({ "id": id, "save_paths": ["savedata"] }),
    );
    assert_eq!(response["result"]["success"], true, "{response}");
    let response = fixture.rpc(
        "sync.set_credentials",
        json!({ "key_id": "id", "app_key": "key" }),
    );
    assert_eq!(response["result"]["stored"], true, "{response}");
    (id, saves, exe)
}

/// 把 `game.exe` 换成一个会一直跑下去的进程，并让它跑起来。
///
/// ⚠ 真跑起来的**必须就是档案里那个 exe**：自动追踪认人的凭据是进程的 exe 完整路径
/// （用户 2026-09-20），记一个空壳 `game.exe`、真跑另一个名字的那种"按名字认"已经删了。
fn spawn_watched_game(
    fixture: &Fixture,
    game_id: &str,
    exe: &std::path::Path,
) -> std::process::Child {
    std::fs::copy("/bin/sleep", exe).expect("copy /bin/sleep");
    let child = std::process::Command::new(exe)
        .arg("30")
        .spawn()
        .expect("spawn the watched process");
    let watched = {
        let game_id = game_id.to_string();
        move |fixture: &Fixture| {
            fixture.rpc("daemon.status", json!({}))["result"]["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["game_id"] == game_id)
        }
    };
    assert!(
        wait_until(Duration::from_secs(60), || watched(fixture)),
        "daemon 没有自己认出这个进程\n--- daemon log ---\n{}",
        fixture.logs()
    );
    child
}

/// 状态里这一款的那一条。
fn game_in_status(fixture: &Fixture, game_id: &str) -> serde_json::Value {
    let status = fixture.rpc("sync.status", json!({}));
    status["result"]["games"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == game_id)
        .cloned()
        .unwrap_or_else(|| panic!("状态里没有 {game_id}：{status}"))
}

/// ⚠ **关掉单款开关之后退出，界面必须说清是"这一款的开关关着"。**
///
/// 用户 2026-09-28 的现象：退出后没自动上传，手动上传却好使。手动那条路不受单款开关限制
/// （`actions::rpc_sync_now`），所以"能不能传"与"会不会自动传"本来就是两件事 —— 而从前
/// 界面上**一个字都不说**，用户只能靠猜。修复前那条早退路的日志是 `debug` 级，默认日志
/// 级别下根本看不见。
#[test]
fn an_exit_with_the_switch_off_explains_itself_in_the_status() {
    let mut fixture = Fixture::new("exit-off");
    let remote = fixture.enable_fake_sync(true);
    fixture.start();

    let (id, saves, exe) = make_game(&fixture, "ExitGame");
    let packages = remote.join(format!("games/{id}"));

    // 先手动传一版，于是界面上有一条"上传成功" —— 用户看到的就是这个状态。
    assert_eq!(
        fixture.rpc("sync.now", json!({ "id": id }))["result"]["ok"],
        true
    );
    assert_eq!(cloud_packages(&packages).len(), 1, "先有第一版");

    // 用户把这一款的参与开关关掉。
    let response = fixture.rpc("game.update", json!({ "id": id, "sync_enabled": false }));
    assert_eq!(response["result"]["success"], true, "{response}");

    // ⚠ 界面**在游戏还没跑的时候**就该说得出这件事 —— 不用等退出、不用翻日志。
    let game = game_in_status(&fixture, &id);
    assert_eq!(
        game["auto_upload_blocked"]["reason"], "per_game_off",
        "这一款关了自动上传，界面必须报出是哪一道闸门：{game}"
    );
    assert!(
        game["auto_upload_blocked"]["detail"]
            .as_str()
            .is_some_and(|d| d.contains("开关")),
        "要说清是那颗开关，不是「引擎没装」之类：{game}"
    );

    // 真跑一局再退出。
    let mut child = spawn_watched_game(&fixture, &id, &exe);
    std::fs::write(saves.join("save.dat"), b"played-a-while").unwrap();
    child.kill().unwrap();
    child.wait().unwrap();

    // 退出钩子把这一次**跳过**如实记下来（而不是留着一句"上传成功"）。
    // `wait_until` 给足 `POLL_INTERVAL` 2s + `SETTLE_DELAY` 3s 的余量。
    assert!(
        wait_until(Duration::from_secs(30), || {
            game_in_status(&fixture, &id)["last"]["detail"]
                .as_str()
                .is_some_and(|d| d.contains("开关"))
        }),
        "退出后什么也没记 —— 界面上留着的还是上一次那条「上传成功」，用户在撒谎的界面上\
         什么线索都没有\n--- daemon log ---\n{}",
        fixture.logs()
    );
    let game = game_in_status(&fixture, &id);
    // 跳过**不是**失败：它是用户自己选的，不该标成错误。
    assert_eq!(
        game["last"]["ok"], true,
        "用户自己关的开关不该被报成错误：{game}"
    );
    assert_eq!(
        cloud_packages(&packages).len(),
        1,
        "开关关着就不该多出第二版"
    );
}

/// **"没传成"与"不该传"是两个判决**，界面上必须分得开。
///
/// 同一个夹具、同一款游戏、同一份配置，唯一变的是引擎会不会成功：`KOTORI_FAKE_FAIL`
/// 让任何含 `copyto`（真上传那一步）的 rclone 调用当场失败（见 `tests/support/tool.rs`）。
/// 于是引擎报错 ⇒ `last.ok == false`，**那是错**，该去修。
///
/// ⚠ 这里**不先跑一次 `sync.now`**：那也会留下一条 `ok == false`，于是"退出钩子记了失败"
/// 这条断言会在退出还没发生时提前满足 —— 绿灯是假的。只跑退出那一条路。
///
/// # ⚠ 这一条同时钉着闸门 b 在"自动追踪"那一局上怎么算
///
/// 闸门 b（`SkipReason::LaunchSyncNotDone`，PLATFORMS.md §6.6）要求"这一局跟云端对上过账"
/// 才允许退出时自动上传，而这一条用的是**自动追踪**那条路 —— 游戏不是 kotori 启动的
/// （用户双击图标），没有"启动前"可言。用户 2026-10-05 裁决：**观测会话建立时也跑一次
/// 判定**（`sync_rpc::launch_sync::settle_from_observation`，只立牌子、绝不取回、也不读
/// 存档内容）—— 否则"双击图标玩完没传"那个 bug（用户 2026-09-28 报的）就回来了。
///
/// 于是这条测试的断言一个字都没改：牌子立得起来 ⇒ 退出钩子走得到引擎 ⇒ copyto 失败 ⇒
/// `last.ok == false`，而闸门是**开着**的（`auto_upload_blocked == null`）。
#[test]
fn an_engine_failure_after_the_exit_is_recorded_as_a_failure() {
    let mut fixture = Fixture::new("exit-fail");
    let remote = fixture.enable_fake_sync(true);
    fixture.start();

    let (id, saves, exe) = make_game(&fixture, "FailGame");
    let packages = remote.join(format!("games/{id}"));

    // 让假 rclone 一调用 `copyto`（真上传那一步）就失败。
    std::fs::write(fixture.dir.join("fail"), "copyto").unwrap();

    let mut child = spawn_watched_game(&fixture, &id, &exe);
    std::fs::write(saves.join("save.dat"), b"played-a-while").unwrap();
    child.kill().unwrap();
    child.wait().unwrap();

    assert!(
        wait_until(Duration::from_secs(30), || {
            game_in_status(&fixture, &id)["last"]["ok"] == false
        }),
        "引擎报错之后退出上传该记成**失败**，而不是一声不吭\n--- daemon log ---\n{}",
        fixture.logs()
    );
    let game = game_in_status(&fixture, &id);
    // 要说清是**引擎**出的问题，不是一句"同步失败"。
    assert!(
        game["last"]["detail"]
            .as_str()
            .is_some_and(|d| d.contains("injected failure")),
        "界面上要看得见引擎到底报的什么：{game}"
    );
    // 失败与"不该传"是两回事：这一款的闸门是**开着**的。
    assert_eq!(
        game["auto_upload_blocked"],
        serde_json::Value::Null,
        "引擎报错不等于闸门关着，界面不该把两件事混成一件：{game}"
    );
    assert!(
        cloud_packages(&packages).is_empty(),
        "传没成功先不论，桶里不该凭空多出东西"
    );
}

/// **引擎卡死不许把 daemon 拖住。**
///
/// 那一分钟到底花在哪，第一步先分出"是不是同步把整机堵了"：只读的 `daemon.status` 还能
/// 不能秒回。⚠ `COMMAND_TIMEOUT` 是 **300 秒**（`sync/runner/mod.rs`）—— 一条挂死的传输
/// 能占住一个任务五分钟；它绝不该占住别的。
///
/// 假 rclone 被换成一支只会睡的脚本，所以这一局的上传**必定**卡住。
#[test]
fn a_hung_engine_does_not_wedge_the_daemon() {
    let mut fixture = Fixture::new("exit-hang");
    fixture.enable_fake_sync(true);
    fixture.start();

    // 换掉假 rclone：睡到被打死为止（`--kotori-warmup` 那条立刻退，
    // `write_script` 要靠它确认这个文件真的能执行）。
    write_script(
        &fixture.dir.join("bin/rclone"),
        "#!/bin/sh\n[ \"$1\" = \"--kotori-warmup\" ] && exit 0\nsleep 600\n",
    );

    let (id, saves, exe) = make_game(&fixture, "HangGame");
    let mut child = spawn_watched_game(&fixture, &id, &exe);
    std::fs::write(saves.join("save.dat"), b"played-a-while").unwrap();
    child.kill().unwrap();
    child.wait().unwrap();

    // 引擎现在必定卡住（`SETTLE_DELAY` 3s 之后进 `copyto`，然后睡 600 秒）。
    // 这段时间里 daemon 必须照常回话。
    let started = Instant::now();
    for _ in 0..3 {
        let response = fixture.rpc("daemon.status", json!({}));
        assert_eq!(response["result"]["running"], true, "{response}");
    }
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "一条挂死的上传把只读请求也拖住了（{:?}）—— 同步绝不许占住别的",
        started.elapsed()
    );
}
