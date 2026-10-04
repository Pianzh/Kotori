//! **启动前那条路**：三方比较之后才动手（PLATFORMS.md §6.3 / §6.4 / §6.5）。
//!
//! 从 `sync.rs` / `sync_exit_upload.rs` 拆出来的第三块（它们各自管"上传"与"为什么
//! 没上传"，这一块管**启动前**）：那条路上有两件只有端到端才验得出来的事 ——
//!
//! 1. **决定覆盖之后、落盘之前要重读一次云端索引**（§6.5 第 1 步）：第二次读到的索引
//!    不再是"该覆盖" ⇒ 本机**一个字节都不许动**；
//! 2. **包的内容值必须等于索引里那一版的值**（§6.5 第 3 步）：对不上就是包被人换了，
//!    或者索引在骗人 ⇒ 丢弃 staging、报错，**绝不铺**。
//!
//! 两件都靠"把桶里那份索引改掉"来构造 —— 假 rclone 的桶就是磁盘上一个目录，所以
//! 这种"云端被人动了手脚"的场景可以真的造出来，而不是靠桩返回一个假响应。
//!
//! 第三条是**观测会话**（游戏不是 kotori 启动的）那条路的边界：它也"对账"，但只判定、
//! 绝不取回、**绝不读正在跑的存档** —— 于是 `Confirm` 那一格不立牌子。

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;

use crate::fixture::Fixture;
use crate::helpers::{cloud_packages, wait_until};

/// 建一款游戏：回传 **id、存档目录、exe**。
///
/// ⚠ id **不许在调用方硬写**：它是 daemon 从名字推出来的（"Shared Game" → `shared-game`）。
/// 硬写过一次就红了一轮 —— 断言拿着一个不存在的 id，看不出是"没建档成功"还是"找错了
/// 那一款"。
fn make_game(fixture: &Fixture, name: &str, exe_body: &[u8]) -> (String, PathBuf, PathBuf) {
    let game_dir = fixture.dir.join(name);
    let saves = game_dir.join("savedata");
    std::fs::create_dir_all(&saves).unwrap();
    let exe = game_dir.join("game.exe");
    std::fs::write(&exe, exe_body).unwrap();

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

/// 桶里那份**合并快照**在磁盘上的位置。
///
/// `remote` 是假 rclone 的远端根（`<夹具目录>/test-bucket/kotori`），索引就在它下面的
/// `index/` 里（`sync::remote_paths::index_main_path`）。
fn index_main(remote: &Path) -> PathBuf {
    remote.join("index/kotori-index.json")
}

/// 把索引里某一款的那一条改掉（**云端被人动了手脚**就是用这个造的）。
///
/// 收的是 `cloud_key`（这一款在云端的落点 —— 第一次上传就是本机 id）。
fn tamper_index(remote: &Path, cloud_key: &str, edit: impl FnOnce(&mut serde_json::Value)) {
    let path = index_main(remote);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut index: serde_json::Value = serde_json::from_str(&text).expect("索引该是 JSON");
    let entry = index["games"]
        .as_array_mut()
        .expect("索引里该有 games")
        .iter_mut()
        .find(|game| game["cloud_key"] == cloud_key);
    // ⚠ 找不到就自己造一个**能读的**诊断（把 `index` 借进闭包会与上面那个可变借冲突）。
    let Some(entry) = entry else {
        panic!(
            "索引里没有落点为 {cloud_key} 的那一条：{}",
            index_main(remote).display()
        );
    };
    edit(entry);
    std::fs::write(&path, serde_json::to_vec_pretty(&index).unwrap()).unwrap();
}

/// 本机这个存档文件的**字节 + 修改时间** —— "一个字节都没动"要连时间一起断言。
fn fingerprint_of(path: &Path) -> (Vec<u8>, std::time::SystemTime) {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mtime = std::fs::metadata(path).unwrap().modified().unwrap();
    (bytes, mtime)
}

/// 一台机器先传一版（顺手写下基线），第二台机器再传一版 —— 于是第一台的
/// "本机 = 基线、云端已经前进"那个状态就成立了。
///
/// 返回 `(第一台, 它的 id, 它的存档目录, 远端根, 云端那一款的包目录)`。
fn two_machines_one_step_ahead(tag: &str) -> (Fixture, String, PathBuf, PathBuf, PathBuf) {
    const EXE: &[u8] = b"\x7fELF the same game on both machines";

    let mut machine_a = Fixture::new(&format!("{tag}-a"));
    let remote = machine_a.enable_fake_sync(true);
    machine_a.start();
    let (id_a, saves_a, _exe_a) = make_game(&machine_a, "Shared Game", EXE);
    std::fs::write(saves_a.join("save.dat"), b"version-one").unwrap();
    assert_eq!(
        machine_a.rpc("sync.now", json!({ "id": id_a }))["result"]["ok"],
        true,
        "第一版该传得上去"
    );

    // 第二台机器：**exe 一模一样**（指纹因此认出同一条身份），存档内容不同 —— 它传上去
    // 的那一版就是"云端前进了一版"。
    let mut machine_b = Fixture::new(&format!("{tag}-b"));
    machine_b.enable_fake_sync(true);
    machine_b.share_bucket_with(&machine_a);
    machine_b.start();
    let (id_b, saves_b, _exe_b) = make_game(&machine_b, "Shared Copy", EXE);
    std::fs::write(saves_b.join("save.dat"), b"version-two").unwrap();
    assert_eq!(
        machine_b.rpc("sync.now", json!({ "id": id_b }))["result"]["ok"],
        true,
        "第二版该传得上去"
    );

    let packages = remote.join(format!("games/{id_a}"));
    assert_eq!(
        cloud_packages(&packages).len(),
        2,
        "云端该有两版（第一台一版、第二台一版）"
    );

    // 第一台把索引缓存刷到最新：它的**缓存**里现在写着"云端最新是第二版"。
    machine_a.rpc("sync.cloud_list", json!({ "refresh": true }));
    (machine_a, id_a, saves_a, remote, packages)
}

/// ⚠ §6.5 第 1 步：**决定覆盖之后、落盘之前**强制重读一次云端索引。
///
/// 第二次读到的那份索引不再是"该覆盖"（这里造的是"索引自己不知道那一版的内容值"，
/// 判定表第 3 格 ⇒ 要问）⇒ 本机**一个字节都不许动**，也**不许**自动上传。
#[test]
fn a_second_index_read_that_no_longer_says_pull_touches_nothing() {
    let (machine_a, id_a, saves_a, remote, packages) =
        two_machines_one_step_ahead("launch-conflict");
    let save_file = saves_a.join("save.dat");
    let before = fingerprint_of(&save_file);

    // 云端那份索引被改了：这一条现在**没有** `latest_digest`（老索引、或者被人抹了）。
    // 第一次判定读的是第一台的**缓存**（里面有第二版的内容值），第二次读的是**桶里这份**。
    tamper_index(&remote, &id_a, |entry| {
        let object = entry.as_object_mut().expect("一条索引该是个对象");
        assert!(
            object.remove("latest_digest").is_some(),
            "这一条本来就该有内容值（上传时写的）: {entry}"
        );
    });

    // 触发启动前那条路。假 gamescope 立刻退出，所以回包是不是成功无所谓 ——
    // 要看的全在**副作用**上。
    machine_a.rpc("game.launch", json!({ "id": id_a, "selfcheck": true }));

    assert_eq!(
        fingerprint_of(&save_file),
        before,
        "第二次读到的索引不再是「该覆盖」⇒ 本机一个字节都不许动"
    );
    assert_eq!(
        cloud_packages(&packages).len(),
        2,
        "没对上账就不许自动上传（闸门 b）：云端不该多出第三版"
    );
    // 而且说得出为什么：日志里那行是"这一局到底同步了什么"的入口。
    let logs = machine_a.logs();
    assert!(
        logs.contains("unknown_digest"),
        "该落在「要问」那一格（索引不知道那一版的内容值）\n--- daemon log ---\n{logs}"
    );
}

/// ⚠ §6.5 第 3 步：包的 `manifest.digest` 与索引里的 `latest_digest` 对不上
/// ⇒ **丢弃 staging + 报错，绝不铺**。
///
/// 造法：把索引里那条的值改成一个别的 64 位十六进制（包本身一个字节都没动）。判定
/// 仍然认为"该覆盖本机"（索引说云端是新的一版），所以它会真的去下载、真的核对 ——
/// 而这正是要验的那一步。
#[test]
fn a_package_whose_digest_disagrees_with_the_index_is_not_laid_down() {
    let (machine_a, id_a, saves_a, remote, packages) = two_machines_one_step_ahead("launch-digest");
    let save_file = saves_a.join("save.dat");
    let before = fingerprint_of(&save_file);

    // 索引在骗人：它说这一版的内容值是别的什么东西。
    tamper_index(&remote, &id_a, |entry| {
        entry["latest_digest"] = json!("ab".repeat(32));
    });

    machine_a.rpc("game.launch", json!({ "id": id_a, "selfcheck": true }));

    assert_eq!(
        fingerprint_of(&save_file),
        before,
        "包与索引对不上 ⇒ 绝不铺（本机存档连时间都不许变）"
    );
    assert_eq!(
        cloud_packages(&packages).len(),
        2,
        "取回被拒之后也不该自动上传"
    );
    let logs = machine_a.logs();
    assert!(
        logs.contains("内容值与索引对不上"),
        "日志里要看得见是哪一道核对拦下来的\n--- daemon log ---\n{logs}"
    );
}

/// 把 `game.exe` 换成一个一直跑下去的进程并等 daemon 自己认出它（**不点「启动」**）。
///
/// 与 `sync_exit_upload.rs` 里那条同一个做法：自动追踪认人的凭据是进程的 exe 完整路径
/// （用户 2026-09-20），所以真跑起来的必须就是档案里那个 exe。
fn spawn_watched(fixture: &Fixture, game_id: &str, exe: &Path) -> std::process::Child {
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

/// ⚠ **观测会话那条路的边界**（用户 2026-10-05 裁决）：它也"跟云端对上账"（否则"双击图标
/// 玩完没传"那个 bug 就回来了），但它**只判定、绝不取回、更不读正在跑的存档**。
///
/// 这一条造的正是那条边界上最要紧的一格：**云端前进了一版、本机看着没动**（mtime 与基线
/// 一模一样）⇒ 判定给 `Confirm`（"要不要覆盖本机"得读一遍内容才知道）⇒ 观测会话**不立
/// 牌子** ⇒ 这一局退出后**不自动上传**（拿本机去盖新云端正是冲突）。
///
/// 三件事一起断言：本机存档一个字节都没动（没偷偷取回）、云端没多出第三版（没偷偷上传）、
/// 界面上说得出这一局为什么没传。
#[test]
fn an_observation_never_reads_or_lays_down_and_stays_unsettled_on_confirm() {
    let (machine_a, id_a, saves_a, _remote, packages) =
        two_machines_one_step_ahead("observe-confirm");
    let save_file = saves_a.join("save.dat");
    let before = fingerprint_of(&save_file);

    // 本机存档在这之前、之后都不去碰它 —— mtime 因此仍等于基线里那个值。
    let mut child = spawn_watched(
        &machine_a,
        &id_a,
        &machine_a.dir.join("Shared Game/game.exe"),
    );
    child.kill().unwrap();
    child.wait().unwrap();

    assert_eq!(
        fingerprint_of(&save_file),
        before,
        "观测会话绝不取回：本机存档连时间都不许变"
    );
    // ⚠ 每一次都**重新问一次**状态：退出钩子是异步的，拿一份快照轮询会一直看着旧的
    // `last`（第一版就是这么红的 —— 断言审的是一份过期的 JSON）。
    let state = |fixture: &Fixture| -> serde_json::Value {
        let status = fixture.rpc("sync.status", json!({}));
        status["result"]["games"]
            .as_array()
            .unwrap()
            .iter()
            .find(|game| game["id"] == id_a)
            .cloned()
            .unwrap_or_else(|| panic!("状态里没有 {id_a}：{status}"))
    };
    // Confirm 不立牌子 ⇒ 退出上传的那道闸门拦下它，而且说得出是哪一件事。
    assert!(
        wait_until(Duration::from_secs(30), || {
            state(&machine_a)["last"]["action"] == "上传"
                && state(&machine_a)["last"]["detail"]
                    .as_str()
                    .is_some_and(|detail| detail.contains("对上账"))
        }),
        "这一局退出后该被闸门 b 拦下，并说清原因：{}\n--- daemon log ---\n{}",
        state(&machine_a),
        machine_a.logs()
    );
    assert_eq!(
        state(&machine_a)["auto_upload_blocked"]["reason"],
        "launch_sync_not_done",
        "没立牌子就是这一道闸门关着：{}",
        state(&machine_a)
    );
    assert_eq!(
        cloud_packages(&packages).len(),
        2,
        "没立牌子就不许自动上传：云端不该多出第三版"
    );
}

/// 基线文件在磁盘上的位置（§6.1(c)：`<data_dir>/sync-baseline/<名字>.json`）。
///
/// 名字里有 game_id 的哈希，所以**不自己拼**：拿目录里唯一那个文件（夹具里一款游戏就一份
/// 基线），找不到就报错。这样它跟 `baseline::file_name` 的实现不会各写一份规则。
fn baseline_file(fixture: &Fixture) -> PathBuf {
    let dir = fixture.dir.join("data/sync-baseline");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| entry.expect("读目录项").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    assert_eq!(files.len(), 1, "夹具里这一款该正好有一份基线：{files:?}");
    files.remove(0)
}

/// 基线文件的**字节**（"一字不动"要按字节断言，不是按解析出来的字段）。
fn baseline_bytes(fixture: &Fixture) -> Vec<u8> {
    let path = baseline_file(fixture);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// ⚠ §6.6 第 4 步 / §6.9：**上传失败（或跳过）⇒ 基线一字不动**。
///
/// 造法：先手动传一版（基线因此存在），再让假 rclone 在上传那一步失败
/// （`KOTORI_FAKE_FAIL` 命中 `copyto`，见 `tests/support/tool.rs`），最后再传一次。
///
/// 这一条同时钉住两件事：失败**不留**半截基线，也**不把**基线推到一个没上云的名字上 ——
/// 不然下次启动会把"我自己那次没传成的"当成"云端最新"，去拉一个不存在的版本。
#[test]
fn an_upload_that_failed_leaves_the_baseline_alone() {
    let mut fixture = Fixture::new("baseline-upload-failed");
    fixture.enable_fake_sync(true);
    fixture.start();
    let (id, saves, _exe) = make_game(&fixture, "Baseline Game", b"\x7fELF failed upload");

    // ① 先传一版：基线因此存在（上传成功是更新基线的两个时机之一）。
    std::fs::write(saves.join("save.dat"), b"version-one").unwrap();
    assert_eq!(
        fixture.rpc("sync.now", json!({ "id": id }))["result"]["ok"],
        true,
        "第一版该传得上去"
    );
    let before = baseline_bytes(&fixture);

    // ② 让上传那一步失败，再改一次存档并再传一次。
    std::fs::write(fixture.dir.join("fail"), "copyto").unwrap();
    std::fs::write(saves.join("save.dat"), b"version-two").unwrap();
    assert_eq!(
        fixture.rpc("sync.now", json!({ "id": id }))["result"]["ok"],
        false,
        "注入了失败之后这一次上传不该成功"
    );

    assert_eq!(
        baseline_bytes(&fixture),
        before,
        "上传失败 ⇒ 基线一个字都不许动（下次启动会把同一件事重新问一遍）"
    );
}

/// ⚠ §6.8 B ③ / §6.9：**冲突留给"稍后再说"⇒ 退出不上传**。
///
/// 构造与闸门 b（`SkipReason::LaunchSyncNotDone`）那条测试同源：启动前那条路判定"要问用户"
/// ⇒ **不立牌子**；用户在弹窗里选「稍后再说」（客户端一个字段都不改）⇒ 退出上传照样被拦。
/// 这里验的是**端到端那一半**：真跑一局、真退出，云端的版数一版都不许多。
#[test]
fn a_conflict_left_for_later_does_not_upload_on_exit() {
    let (machine_a, id_a, saves_a, _remote, packages) =
        two_machines_one_step_ahead("conflict-later");
    let save_file = saves_a.join("save.dat");
    let before = cloud_packages(&packages).len();
    let fingerprint = fingerprint_of(&save_file);

    // 本机在"云端已经前进了一版、本机看着没动"那一刻：启动前的判定是**要问**（第 7 格
    // ⇒ `Confirm` ⇒ 读一遍内容核对 ⇒ 两边都动了 ⇒ `Ask(BothChanged)`），
    // 于是**不立牌子**。用户在弹窗里选「稍后再说」= 客户端什么都不做，游戏照常启动。
    let mut child = spawn_watched(
        &machine_a,
        &id_a,
        &machine_a.dir.join("Shared Game/game.exe"),
    );
    child.kill().unwrap();
    child.wait().unwrap();

    // 退出钩子走完之后：云端不许多出第三版，本机存档也不许被动过。
    let settle = |fixture: &Fixture| -> bool {
        fixture.rpc("sync.status", json!({}))["result"]["games"]
            .as_array()
            .unwrap()
            .iter()
            .any(|game| {
                game["id"] == id_a
                    && game["last"]["action"] == "上传"
                    && game["last"]["detail"]
                        .as_str()
                        .is_some_and(|detail| detail.contains("对上账"))
            })
    };
    assert!(
        wait_until(Duration::from_secs(30), || settle(&machine_a)),
        "退出钩子该把这一次如实记下来（闸门 b 拦下了它）\n--- daemon log ---\n{}",
        machine_a.logs()
    );
    assert_eq!(
        cloud_packages(&packages).len(),
        before,
        "「稍后再说」⇒ 本次不拉也不传：云端一版都不许多"
    );
    assert_eq!(
        fingerprint_of(&save_file),
        fingerprint,
        "「稍后再说」也绝不动本机存档"
    );
}

/// ⚠ §6.7 / §6.9 的**支点**：**恢复不动基线**。
///
/// 先传两版（基线停在第二版），再手动恢复到**第一版**：本机的存档换了、mtime 也变了，
/// 而基线文件的**字节**必须一模一样。整套设计的支点就在这一条上 —— 基线一旦被恢复推动，
/// "本机被恢复成旧版"就会被认成"本机自己改的"，下一次启动的判定整盘皆错。
#[test]
fn restoring_an_old_version_does_not_move_the_baseline() {
    let mut fixture = Fixture::new("restore-baseline");
    let remote = fixture.enable_fake_sync(true);
    fixture.start();
    let (id, saves, _exe) = make_game(&fixture, "Restore Game", b"\x7fELF restore");

    // ① 第一版。
    let save_file = saves.join("save.dat");
    std::fs::write(&save_file, b"version-one").unwrap();
    assert_eq!(
        fixture.rpc("sync.now", json!({ "id": id }))["result"]["ok"],
        true
    );
    // 版本名的毫秒精度让两次上传不会撞车（从前撞过，见 `snapshots::version_stamp`）。
    let first = cloud_packages(&remote.join(format!("games/{id}")));
    assert_eq!(first.len(), 1, "先有第一版：{first:?}");
    // `cloud_packages` 给的是桶里的**文件名**（带 `.zip`），而 `sync.restore` 收的是
    // **版本名**（不带扩展名）—— 差这一个后缀就是"不是合法的版本名"。
    let oldest = first[0]
        .strip_suffix(".zip")
        .expect("包名该以 .zip 结尾")
        .to_string();

    // ② 第二版（基线因此停在第二版）。
    std::fs::write(&save_file, b"version-two").unwrap();
    assert_eq!(
        fixture.rpc("sync.now", json!({ "id": id }))["result"]["ok"],
        true
    );
    let packages = remote.join(format!("games/{id}"));
    assert_eq!(cloud_packages(&packages).len(), 2, "云端该有两版");
    let before = baseline_bytes(&fixture);

    // ③ 手动恢复到**第一版**（用户按的那一下，`sync.restore` 带上版本名）。
    let response = fixture.rpc("sync.restore", json!({ "id": id, "version": oldest }));
    assert_eq!(response["result"]["ok"], true, "恢复该成功：{response}");
    assert_eq!(
        std::fs::read_to_string(&save_file).unwrap(),
        "version-one",
        "本机存档该被换成第一版"
    );

    // ④ 支点：基线文件的**字节**完全一样。
    assert_eq!(
        baseline_bytes(&fixture),
        before,
        "恢复绝不动基线（§6.7：那是整套设计的支点）"
    );
}
