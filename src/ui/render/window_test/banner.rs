//! 横幅（§6.8 A）与冲突弹窗（§6.8 B）那两块 —— §6.9 的 UI 族。
//!
//! 四行文案**本身**是纯函数（`ui::model::banner` 的单测逐条钉着），这里验的是**另一件事**：
//! 那四行真的被推到了窗口属性上、两处页面都画得出、弹窗三个按钮真的在、以及"还没回包"
//! 那一态写的是「正在核对…」。
//!
//! ⚠ 与 `window_test` 里其它几段一样，**不是**独立的 `#[test]`：Slint 的测试后端在同一个
//! 进程里建第二个窗口，本地能过、CI 上不成立（见 `settings.rs` 的文件头）。

use super::*;
use crate::ui::message::ConflictAction::{KeepLocal, Later, UseCloud};
use crate::ui::{SnapshotBaseline, SnapshotCloud, SnapshotIndex, SnapshotLocal, SyncSnapshot};

fn digest(seed: u8) -> String {
    format!("{seed:02x}").repeat(32)
}

/// 一份"三边一致"的回包（各条测试在它上面改出一格）。
fn snapshot() -> SyncSnapshot {
    SyncSnapshot {
        game_id: "demo".into(),
        local: SnapshotLocal {
            mtime_ms: 1_759_570_000_000,
            empty: false,
            digest: Some(digest(0xaa)),
        },
        cloud: SnapshotCloud {
            state: "known".into(),
            stamp: Some("20261004T101500Z".into()),
            digest: Some(digest(0xaa)),
            key: "demo".into(),
        },
        baseline: Some(SnapshotBaseline {
            stamp: "20261004T101500Z".into(),
            digest: digest(0xaa),
            mtime_ms: 1_759_570_000_000,
        }),
        decision: "nothing".into(),
        index: SnapshotIndex::default(),
        problem: None,
    }
}

/// 让横幅走一遍"进页面 → 回包到"：先把消息喂进 `App`，再渲染。
fn banner_with(ui: &mut Ui, snapshot: SyncSnapshot) {
    let _ = ui
        .app
        .update(Message::SyncSnapshotRequested(snapshot.game_id.clone()));
    render(ui);
    let _ = ui
        .app
        .update(Message::SyncSnapshotLoaded(Box::new(Ok(snapshot))));
    render(ui);
}

pub(super) fn banner_and_conflict(ui: &mut Ui, game_id: &str) {
    // 进页面之前（`sync_ask` 还没有）先把这一款那份状态准备好：`banner_requested` 记的是
    // 当前那一款，所以先让 App 知道"现在看的是哪一款"。
    ui.app.selected = Some(game_id.to_string());
    ui.app.games = vec![ui_game()];
    ui.app.draft = Some(Draft::from_game(&ui_game()));
    ui.window.set_game_open(true);
    ui.window
        .window()
        .set_size(slint::LogicalSize::new(1120.0, 2600.0));
    show_tab(ui, Tab::Games);

    // ── ① 「正在核对…」：进页面就置起、回包还没到 ──
    let _ = ui
        .app
        .update(Message::SyncSnapshotRequested(game_id.to_string()));
    render(ui);
    let board = ui.window.global::<SyncBannerBoard>();
    assert_eq!(board.get_game_id(), game_id, "横幅认的是当前这一款");
    assert!(board.get_loading(), "回包还没到 ⇒「正在核对…」那一态");
    assert_eq!(board.get_local_line(), "", "还没算完不许画半截旧内容");
    assert_eq!(board.get_next_line(), "");
    fits(ui, &["SyncBannerCard"]);

    // ── ② 与基线一致（§6.8 A 第一条文案）──
    banner_with(ui, snapshot());
    let board = ui.window.global::<SyncBannerBoard>();
    assert!(!board.get_loading());
    assert!(
        board.get_local_line().starts_with("本地  与基线一致"),
        "{}",
        board.get_local_line()
    );
    assert!(
        board.get_cloud_line().starts_with("云端  与基线一致"),
        "{}",
        board.get_cloud_line()
    );
    assert!(
        board.get_baseline_line().starts_with("基线  "),
        "{}",
        board.get_baseline_line()
    );
    assert_eq!(board.get_next_line(), "接下来：本次不自动同步");
    assert!(!board.get_warning(), "三边一致不该有警示色");
    assert_eq!(board.get_stale_note(), "", "没过期就一个字都不许说");
    fits(ui, &["SyncBannerCard"]);

    // ── ③ 云端新（第二条文案）──
    let mut newer_cloud = snapshot();
    newer_cloud.cloud.digest = Some(digest(0xbb));
    newer_cloud.cloud.stamp = Some("20261005T101500Z".into());
    newer_cloud.decision = "pull".into();
    banner_with(ui, newer_cloud);
    let board = ui.window.global::<SyncBannerBoard>();
    assert!(
        board.get_cloud_line().contains("比基线新 1 版"),
        "{}",
        board.get_cloud_line()
    );
    assert_eq!(board.get_next_line(), "接下来：启动前会下载");
    assert!(!board.get_warning());
    fits(ui, &["SyncBannerCard"]);

    // ── ④ 本机新（第三条文案）──
    let mut newer_local = snapshot();
    newer_local.local.mtime_ms += 2 * 24 * 60 * 60 * 1000;
    newer_local.local.digest = Some(digest(0xcc));
    newer_local.decision = "upload_later".into();
    banner_with(ui, newer_local);
    let board = ui.window.global::<SyncBannerBoard>();
    assert!(
        board.get_local_line().contains("超前 2 天"),
        "{}",
        board.get_local_line()
    );
    assert_eq!(board.get_next_line(), "接下来：游戏结束后上传");
    assert!(!board.get_warning());
    fits(ui, &["SyncBannerCard"]);

    // ── ⑤ 冲突（第四条文案）：警示色 + "启动时会问你" ──
    let mut conflict = snapshot();
    conflict.local.mtime_ms += 24 * 60 * 60 * 1000;
    conflict.local.digest = Some(digest(0xcc));
    conflict.cloud.digest = Some(digest(0xbb));
    conflict.cloud.stamp = Some("20261005T101500Z".into());
    conflict.decision = "ask".into();
    banner_with(ui, conflict.clone());
    let board = ui.window.global::<SyncBannerBoard>();
    assert!(board.get_warning(), "冲突那一行要用警示色（§6.8 A）");
    assert_eq!(board.get_next_line(), "接下来：启动时会问你");
    fits(ui, &["SyncBannerCard"]);

    // ── ⑥ 索引过期而且刷新失败（§6.11 第 2 条）──
    let mut stale = conflict.clone();
    stale.index = SnapshotIndex {
        from_cache: true,
        cached_at: "20261001T101500Z".into(),
        stale: true,
        refresh_error: Some("网络不通".into()),
    };
    banner_with(ui, stale);
    let note = ui.window.global::<SyncBannerBoard>().get_stale_note();
    assert!(note.contains("索引已过期，刷新失败"), "{note}");
    assert!(note.contains("网络不通"), "{note}");
    fits(ui, &["SyncBannerCard"]);

    // ── ⑦ 失败态：如实说 + 那颗「重新核对」真的发得出消息 ──
    let _ = ui.app.update(Message::SyncSnapshotLoaded(Box::new(Err(
        "连不上 daemon".into()
    ))));
    render(ui);
    let board = ui.window.global::<SyncBannerBoard>();
    assert!(
        board.get_error().contains("连不上 daemon"),
        "{}",
        board.get_error()
    );
    assert_eq!(board.get_local_line(), "");
    fits(ui, &["SyncBannerCard"]);
    // 那颗按钮真的接在一个回调上（回调表在 `wire.rs`，两个页面的横幅共用一块）。
    ui.window
        .global::<SyncBannerBoard>()
        .invoke_retry_requested();
    ui.window
        .global::<SyncAskState>()
        .invoke_conflict_resolved("later".into());
    render(ui);

    // ── ⑧ 冲突弹窗三个按钮都在（§6.8 B）──
    ui.app.launching = Some(game_id.to_string());
    let reply = serde_json::json!({
        "sync_pull": { "decision": "ask", "ask": { "kind": "both_changed" } }
    });
    let _ = ui.app.update(Message::LaunchDone(Ok(reply.clone())));
    render(ui);
    let ask = ui.window.global::<SyncAskState>();
    assert!(ask.get_open(), "冲突那一问要把浮层立起来");
    assert_eq!(ask.get_mode(), "conflict");
    assert!(
        ask.get_message().contains("本地和云端都改过"),
        "{}",
        ask.get_message()
    );
    fits(ui, &["SyncAskDialog"]);

    // 三个按钮的**字**都要在（§6.8 B 要求三个都在），而且它们发的是三个不同的 choice ——
    // `wire.rs` 靠这三个字符串认人，改一个就会静默落到"稍后再说"上（最轻的那一个后果）。
    let ask_slint = include_str!("../../slint/widgets/sync-ask.slint");
    for (choice, label) in [
        ("use_cloud", "用云端覆盖本机"),
        ("keep_local", "保留本机（结束后上传）"),
        ("later", "稍后再说"),
    ] {
        assert!(
            ask_slint.contains(&format!("label: \"{label}\"")),
            "弹窗上少了「{label}」那颗按钮"
        );
        assert!(
            ask_slint.contains(&format!("conflict-resolved(\"{choice}\")")),
            "「{label}」那颗按钮没接上 {choice}"
        );
        assert!(
            crate::ui::model::conflict_message("both_changed").contains("都改过"),
            "冲突那一问的话要说清两边都改过"
        );
    }

    // 三个后果各自真的发得出来（`wire.rs` 那张字符串 → 枚举的表）。
    for (choice, want) in [
        ("use_cloud", "use_cloud"),
        ("keep_local", "keep_local"),
        ("later", "later"),
    ] {
        match choice {
            "use_cloud" => {
                let _ = ui.app.update(Message::SyncConflictResolve(UseCloud));
                // 「用云端覆盖本机」= 立刻走 §6.5 那条路（先看再覆盖），而且**不启动游戏**。
                assert!(ui.app.launching.is_none(), "那一颗是先把存档弄对，不启动");
                assert!(ui.app.sync_conflict.is_none(), "弹窗当场收掉");
                let _ = want;
            }
            "keep_local" => {
                let _ = ui.app.update(Message::SyncConflictResolve(KeepLocal));
                assert!(ui.app.sync_conflict.is_none());
                let _ = ui.app.update(Message::SyncConflictAllowed(Ok(())));
                assert!(ui.app.launching.is_some(), "牌子立好就照常启动");
            }
            _ => {
                let _ = ui.app.update(Message::SyncConflictResolve(Later));
                assert!(ui.app.sync_conflict.is_none());
                assert!(ui.app.launching.is_some(), "「稍后再说」也要把这一局起起来");
            }
        }
        render(ui);
        assert!(
            !ui.window.global::<SyncAskState>().get_open(),
            "答完就该收掉"
        );
        // 再摆一次那一问，验下一个后果。
        if choice != "later" {
            ui.app.launching = Some(game_id.to_string());
            let _ = ui.app.update(Message::LaunchDone(Ok(reply.clone())));
            render(ui);
        }
    }
    ui.app.launching = None;

    // ── ⑨ 没有冲突时（`Confirm` / `Nothing` / ...）绝不弹窗 ──
    let quiet = serde_json::json!({ "sync_pull": { "decision": "nothing", "settled": true } });
    ui.app.launching = Some(game_id.to_string());
    let _ = ui.app.update(Message::LaunchDone(Ok(quiet)));
    render(ui);
    assert!(
        !ui.window.global::<SyncAskState>().get_open(),
        "没冲突就不许弹窗，游戏照常起"
    );
    ui.app.launching = None;

    // ── ⑩ 第二处页面：这一款的云端存档页上也要那块横幅 ──
    ui.app.versions.opened(game_id, "demo-key");
    ui.app.versions.loaded(Vec::new());
    ui.window
        .global::<GameVersionsBoard>()
        .set_rows(ui.game_version_rows.clone().into());
    banner_with(ui, snapshot());
    render(ui);
    assert!(
        ui.window.global::<GameVersionsBoard>().get_open(),
        "这一页该画出来"
    );
    assert!(
        ui.window
            .global::<SyncBannerBoard>()
            .get_local_line()
            .starts_with("本地  "),
        "这一页上那块横幅也要被推下去"
    );
    fits(ui, &["GameVersionsPage"]);

    // ── ⑪ 「恢复」那一问：本机偏离基线时那句话要变重（§6.7 第 1 条）──
    let mut drifted = snapshot();
    drifted.local.mtime_ms += 60_000;
    banner_with(ui, drifted);
    let _ = ui
        .app
        .update(Message::SyncRestoreRequested(game_id.to_string(), None));
    render(ui);
    assert!(
        ui.window.get_detail_sync_drifted(),
        "本机动过 ⇒ 确认那一行要说清会覆盖未同步的改动"
    );
    let wording = crate::ui::model::restore_confirmation(None, true);
    assert!(wording.title().contains("未同步"), "{}", wording.title());
    let _ = ui.app.update(Message::SyncRestoreCancelled);
    render(ui);
    assert!(!ui.window.get_detail_sync_drifted());
    assert!(ui.app.sync_restore_pending.is_none());
}
