//! `debug.rs` 的测试（只有 `debug-panels` 构建里有这一份）。
//!
//! 要钉住的是三件事：
//!
//! 1. 界面上**每一颗按钮**都能被 dispatch，而且真的把对应的状态塞成了"会画出来"的样子
//!    （断言的是语义字段，不是像素 —— 像素那一层由 `render/window_test` 管）；
//! 2. 弹窗上那颗确认按钮**一个请求都不发**（判据是 `Task::into_effects()` 一条都没有）；
//! 3. 那条"吞掉"的表覆盖了所有弹窗按钮 —— 漏一个意味着那颗按钮会去打后端。

use super::*;
use crate::ui::message::ConflictAction;

/// 清单、下标与标签：界面上的第 N 颗按钮与 `at(N)` 必须是同一件事。
#[test]
fn every_button_maps_back_to_the_panel_it_shows() {
    for (index, (panel, label)) in DebugPanel::ALL.iter().enumerate() {
        assert!(!label.is_empty(), "每一颗按钮都要有字");
        assert_eq!(
            DebugPanel::at(index as i32),
            Some(*panel),
            "第 {index} 颗按钮下标对不上"
        );
    }
    assert_eq!(DebugPanel::at(-1), None, "越界不许兜底成第一颗");
    assert_eq!(DebugPanel::at(DebugPanel::ALL.len() as i32), None);

    // 标签不重名：重名就意味着两颗按钮干同一件事（或者标签抄错了）。
    let mut labels: Vec<&str> = DebugPanel::ALL.iter().map(|(_, label)| *label).collect();
    let count = labels.len();
    labels.sort_unstable();
    labels.dedup();
    assert_eq!(labels.len(), count, "有按钮重名: {labels:?}");
}

/// 每一颗按钮按下去：假数据真的进了那些**既有**字段，而且既有 render 认得出要画什么。
#[test]
fn every_button_fills_in_the_state_that_draws_it() {
    for &(panel, label) in DebugPanel::ALL {
        let (mut app, _task) = App::new();
        let _ = app.update(Message::DebugActivate(panel));

        // 每一颗按钮背后都是同一份假世界：假游戏进库、被选中、有草稿。
        assert!(
            app.games.iter().any(|game| game.id == GAME_ID),
            "{label}: 假游戏没进库（启动前那一问与单游戏页都按 id 认人）"
        );
        assert_eq!(app.selected.as_deref(), Some(GAME_ID), "{label}");
        assert!(app.draft.is_some(), "{label}");

        // 横幅每一颗都有：走的是 `sync.snapshot` 回包那条同一条路。
        assert_eq!(app.sync_banner.game_id, GAME_ID, "{label}");
        assert!(app.sync_banner.lines.is_some(), "{label}: 横幅没内容");
        assert!(
            !app.sync_banner.loading && app.sync_banner.error.is_none(),
            "{label}: 假回包不该卡在「正在核对…」或者失败态"
        );

        match panel {
            DebugPanel::Conflict => {
                assert_eq!(
                    app.sync_conflict.as_deref(),
                    Some("both_changed"),
                    "{label}"
                );
                assert_eq!(app.sync_ask.as_deref(), Some(GAME_ID), "{label}");
            }
            DebugPanel::Ask => {
                assert_eq!(app.sync_ask.as_deref(), Some(GAME_ID), "{label}");
                assert!(app.sync_conflict.is_none(), "{label}: 这一问不是冲突那一副");
                assert!(
                    app.sync_ask_cloud.is_some(),
                    "{label}: 「疑似找到的那一条」要画得出来"
                );
            }
            DebugPanel::MasterDelete => {
                assert!(app.sync_form.confirm_master_delete, "{label}");
                assert_eq!(
                    app.sync_status.as_ref().map(SyncStatus::store),
                    Some(CredentialStore::File),
                    "{label}: 那条确认只在「主密码凭据文件」那一级才画"
                );
            }
            other if other.confirmation().is_some() => {
                let action = other.confirmation().expect("上面刚判过");
                assert!(app.versions.open, "{label}: 那一页要开着，弹窗才画得出来");
                assert_eq!(
                    app.versions.pending(),
                    Some(&action),
                    "{label}: 弹窗问的不是这件事"
                );
                assert!(
                    !app.versions.rows.is_empty(),
                    "{label}: 「现在有几版」得说得出数字"
                );
            }
            // 横幅那五颗：状态上面已经断言过，具体措辞见下一条测试。
            _ => {}
        }
    }
}

/// 横幅那五颗各自画出**不一样**的措辞 —— 这正是它们存在的理由。
#[test]
fn each_banner_button_draws_its_own_wording() {
    let cases = [
        DebugPanel::BannerInSync,
        DebugPanel::BannerCloudNewer,
        DebugPanel::BannerLocalNewer,
        DebugPanel::BannerConflict,
        DebugPanel::BannerLocalEmpty,
    ];
    let mut locals: Vec<String> = Vec::new();
    for panel in cases {
        let (mut app, _task) = App::new();
        let _ = app.update(Message::DebugActivate(panel));
        let lines = app.sync_banner.lines.clone().expect("横幅要有四行");
        locals.push(lines.local.clone());
        match panel {
            DebugPanel::BannerInSync => {
                assert!(lines.local.contains("与基线一致"), "{}", lines.local);
                assert!(lines.cloud.contains("与基线一致"), "{}", lines.cloud);
                assert_eq!(lines.next, "接下来：本次不自动同步");
                assert!(!lines.warning, "三边一致不该有警示色");
            }
            DebugPanel::BannerCloudNewer => {
                assert!(lines.local.contains("落后 2 天"), "{}", lines.local);
                assert!(lines.cloud.contains("比基线新 1 版"), "{}", lines.cloud);
                assert_eq!(lines.next, "接下来：启动前会下载");
                assert!(!lines.warning);
            }
            DebugPanel::BannerLocalNewer => {
                assert!(lines.local.contains("超前 2 天"), "{}", lines.local);
                assert_eq!(lines.next, "接下来：游戏结束后上传");
                assert!(!lines.warning);
            }
            DebugPanel::BannerConflict => {
                assert_eq!(lines.next, "接下来：启动时会问你");
                assert!(lines.warning, "冲突那一行要用警示色（§6.8 A）");
            }
            _ => {
                assert!(lines.local.contains("本地为空"), "{}", lines.local);
                assert!(lines.warning, "本地为空也要警示色（§6.8 A）");
            }
        }
    }
    locals.sort_unstable();
    locals.dedup();
    assert_eq!(
        locals.len(),
        cases.len(),
        "五颗按钮画出来的本机那行必须各不相同"
    );
}

/// 每一颗弹窗按钮挑一个"用户会按的那一下"（横幅那几颗没有按钮，给 `None`）。
fn dialog_button(panel: DebugPanel) -> Option<Message> {
    Some(match panel {
        DebugPanel::Conflict => Message::SyncConflictResolve(ConflictAction::UseCloud),
        DebugPanel::Ask => Message::SyncAskAnswered("pair".to_string()),
        DebugPanel::MasterDelete => Message::SyncMasterDeleteConfirmed,
        _ if panel.confirmation().is_some() => Message::GameVersionsConfirmed,
        _ => return None,
    })
}

/// **这个 feature 最要紧的一条**：假弹窗上的按钮一个请求都不许发出去。
///
/// "发 RPC"与"什么都不做"的区别就落在 `App::update` 返回的那个 `Task` 上 —— 拦住了就
/// 一个 effect 都不剩。顺带断言假态被收掉了（不然界面上留着一个再也点不动的框）。
#[test]
fn no_button_on_a_fake_dialog_ever_reaches_the_backend() {
    for &(panel, label) in DebugPanel::ALL {
        let Some(button) = dialog_button(panel) else {
            continue;
        };
        let (mut app, _task) = App::new();
        let _ = app.update(Message::DebugActivate(panel));
        assert!(panel.is_dialog(), "{label}: 挑得出按钮就该是弹窗那一类");

        let task = app.update(button);
        assert!(
            task.into_effects().is_empty(),
            "{label}: 调试态下弹窗按钮不许发任何请求"
        );
        assert!(app.debug_panel.is_none(), "{label}: 按过就该把假态收掉");
        assert!(
            app.sync_conflict.is_none()
                && app.sync_ask.is_none()
                && app.sync_restore_pending.is_none()
                && !app.sync_form.confirm_master_delete
                && app.versions.pending().is_none(),
            "{label}: 弹窗要跟着收掉，不然界面上会留着一个点不动的框"
        );
    }
}

/// 那条"吞掉"的表要覆盖**所有**弹窗按钮：漏一个就意味着那颗按钮会去打后端。
#[test]
fn every_dialog_button_is_covered_by_the_interception() {
    let buttons = [
        Message::SyncConflictResolve(ConflictAction::UseCloud),
        Message::SyncConflictResolve(ConflictAction::KeepLocal),
        Message::SyncConflictResolve(ConflictAction::Later),
        Message::SyncAskAnswered("off".to_string()),
        Message::SyncAskBindFound,
        // 这一颗会去开「自己选…」那个浮层（读云端清单）：调试态下也得拦住。
        Message::SyncAskPairRequested,
        Message::GameVersionsConfirmed,
        Message::GameVersionsCancelled,
        Message::CloudDeleteConfirmed,
        Message::CloudDeleteCancelled,
        Message::CloudVersionConfirmed,
        Message::CloudVersionCancelled,
        Message::SyncMasterDeleteConfirmed,
        Message::SyncMasterDeleteCancelled,
        Message::SyncRestoreConfirmed,
        Message::SyncRestoreCancelled,
    ];
    for button in buttons {
        let shown = format!("{button:?}");
        let (mut app, _task) = App::new();
        // 随便摆一个弹窗就够：吞不吞只看"调试态 + 弹窗按钮"。
        let _ = app.update(Message::DebugActivate(DebugPanel::Conflict));
        let task = app.update(button);
        assert!(
            task.into_effects().is_empty(),
            "{shown} 漏在了拦截之外（调试态下它会去打后端）"
        );
    }
}

/// 拦截只在**调试态**下生效：没摆假弹窗时，真的确认照旧发出去。
///
/// 少了这一条，一个写宽了的判据（比如"见到确认类消息就吞"）会把正常功能整个吃掉，
/// 而那种错在调试面板里完全看不出来。
#[test]
fn a_real_confirmation_still_reaches_the_backend() {
    let (mut app, _task) = App::new();
    app.versions.opened("demo", "demo-key");
    app.versions.requested(Confirmation::DeleteVersion {
        version: "20260901T090000Z".to_string(),
    });
    let task = app.update(Message::GameVersionsConfirmed);
    assert_eq!(
        task.into_effects().len(),
        1,
        "没有调试态挡着，真的确认就该发出去（`sync.delete_version`）"
    );
}

/// 摆着的是**横幅**（不是弹窗）时，弹窗按钮照常走真实那条路 —— 拦截只属于弹窗。
#[test]
fn a_banner_panel_does_not_swallow_anything() {
    let (mut app, _task) = App::new();
    let _ = app.update(Message::DebugActivate(DebugPanel::BannerInSync));
    app.versions.opened("demo", "demo-key");
    app.versions.requested(Confirmation::ClearVersions);
    let task = app.update(Message::GameVersionsConfirmed);
    assert_eq!(task.into_effects().len(), 1);
}

/// 每一颗按钮该跳到哪一页：跳错了那颗按钮看着就像没反应（而页面本身不会报错）。
#[test]
fn every_button_knows_where_it_shows_up() {
    for &(panel, label) in DebugPanel::ALL {
        match panel {
            // 窗口级浮层：原地就看得见。
            DebugPanel::Conflict | DebugPanel::Ask => {
                assert_eq!(panel.place(), Place::Here, "{label}")
            }
            // 那条行内确认长在云同步页的「凭据」组里。
            DebugPanel::MasterDelete => assert_eq!(panel.place(), Place::SyncPage, "{label}"),
            // 四个确认弹窗与横幅都在单游戏设置页里。
            _ => assert_eq!(panel.place(), Place::GamePage, "{label}"),
        }
    }
}

/// 再按第二颗按钮时，第一颗留下的假东西必须被收干净（两颗弹窗叠在一起没法看）。
#[test]
fn activating_another_button_clears_the_previous_one() {
    let (mut app, _task) = App::new();
    let _ = app.update(Message::DebugActivate(DebugPanel::Conflict));
    let _ = app.update(Message::DebugActivate(DebugPanel::ReplaceDrifted));

    assert!(
        app.sync_conflict.is_none() && app.sync_ask.is_none(),
        "上一颗的弹窗还在"
    );
    assert_eq!(app.debug_panel, Some(DebugPanel::ReplaceDrifted));
    assert!(app.versions.pending().is_some(), "这一颗的弹窗要立起来");
    // 假游戏不会因为连点两次就在库里堆两条。
    assert_eq!(
        app.games.iter().filter(|game| game.id == GAME_ID).count(),
        1
    );
}
