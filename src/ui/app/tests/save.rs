//! 保存状态机那一族:防抖、在途、排队、迟到回包。
//!
//! 从 `app/tests/mod.rs` 拆出来 —— 那边加上 `SC-02` 的回归之后越过了 600 行硬线。
//!
//! 三条规矩值得记住(每一条都有回归测试):
//! 1. 同一时刻只允许一笔在途(两次全量写并发时,后到的旧快照会盖掉新的);
//! 2. 被挡下的那一笔**排队**,带着它自己那份草稿(`SC-02`:切款之后当前草稿是别人的);
//! 3. 过期回包要么发排队那笔,要么拿当前草稿补一次(`BUG-18`)。

use crate::ui::app::App;
use crate::ui::model::SaveScope;
use crate::ui::test_support::ui_game;
use crate::ui::{Message, UiGame};

/// 自动保存:连改几笔只会存最后一笔,而且过期回包必须让**新的**那份再存一次 ——
/// 否则配置里留下的是用户已经改掉的值。
#[test]
fn the_latest_edit_is_the_one_that_gets_saved() {
    let (mut app, _task) = App::new();
    app.games = vec![ui_game()];
    app.update(Message::GameSelected("demo".into()));

    // 第一笔:挂一个防抖定时器,而不是立刻写。
    app.update(Message::AlgoChanged("Nis".into()));
    assert_eq!(app.autosave_generation, 1);
    assert!(
        app.save_in_flight.is_none() && !app.saving,
        "还没到点,不该发出去"
    );

    // 用户还在改:旧的定时器醒来时世代已经对不上,作废。
    app.update(Message::SharpnessChanged(4.0));
    app.update(Message::AutoSave(1));
    assert!(app.save_in_flight.is_none(), "过期的定时器不许写");

    // 最后那一笔到点,才真的发出去。
    app.update(Message::AutoSave(2));
    assert!(app.saving);
    let in_flight = app
        .save_in_flight
        .as_ref()
        .expect("一笔应该在路上")
        .draft
        .clone();
    assert_eq!(in_flight.algo, "Nis");
    assert_eq!(in_flight.sharpness, 4);

    // 上一笔还没回来时又改了一笔:不发第二笔(并发写会互相覆盖)……
    app.update(Message::FramerateChanged("60".into()));
    app.update(Message::AutoSave(3));
    assert_eq!(
        app.save_in_flight
            .as_ref()
            .map(|a| a.draft.framerate.clone()),
        Some(String::new()),
        "在路上的那笔不该被替换"
    );

    // ……等它回来时发现世代变了,于是拿手上的草稿再存一次。
    app.update(Message::ProfileSaved(2, Ok(())));
    assert!(app.saving);
    assert_eq!(
        app.save_in_flight
            .as_ref()
            .map(|a| a.draft.framerate.clone()),
        Some("60".to_string()),
        "过期回包之后要把最新的那份补上"
    );

    let task = app.update(Message::ProfileSaved(3, Ok(())));
    assert!(!app.saving && app.save_in_flight.is_none());
    assert_eq!(app.saved_msg.as_deref(), Some("已自动保存"));
    // 存成功后要重读一遍库,列表里的"已存值"才跟得上。
    assert_eq!(task.into_effects().len(), 1);
}

/// 一条回包只能落在它自己那一份草稿上:用户中途翻到别的游戏时,不能把别人的
/// `*_original` 写成这个游戏的值。
#[test]
fn a_late_save_never_touches_another_games_draft() {
    let (mut app, _task) = App::new();
    app.games = vec![
        ui_game(),
        UiGame {
            id: "other".into(),
            name: "Other".into(),
            ..ui_game()
        },
    ];
    app.update(Message::GameSelected("demo".into()));
    app.update(Message::ExePathChanged("/games/demo/renamed.exe".into()));
    // 路径那一组归按钮管（不再自动保存）—— 这里就是"用户按了保存路径"。
    app.update(Message::SaveGroup(SaveScope::Paths));
    assert!(app.save_in_flight.is_some());

    app.update(Message::GameSelected("other".into()));
    app.update(Message::ProfileSaved(1, Ok(())));
    let draft = app.draft.as_ref().expect("换过去的游戏也有草稿");
    assert_eq!(draft.game_id, "other");
    assert_eq!(draft.exe_original, draft.exe, "别人的书签不许被动");
    assert!(app.saved_msg.is_none(), "已经离开那一页了,别在这儿报");
}

/// `BUG-18` 的回归：A 的保存还在路上时切到 B、改 B —— B 那一笔不能没人发。
///
/// 从前 `ProfileSaved` 只在"还是同一款"时才补发，于是切款之后 B 的编辑会因为
/// "上一笔还在路上"被退回，而且再也没人发它 —— 用户看到的是"改了、没保存"
/// （GAP-3 一直说这条没有回归测试）。
#[test]
fn switching_games_mid_save_still_persists_the_new_edits() {
    let (mut app, _task) = App::new();
    app.games = vec![
        ui_game(),
        UiGame {
            id: "other".into(),
            name: "Other".into(),
            ..ui_game()
        },
    ];

    // A 改一笔，让保存上路。
    app.update(Message::GameSelected("demo".into()));
    app.update(Message::AlgoChanged("Nis".into()));
    app.update(Message::AutoSave(app.autosave_generation));
    assert!(app.save_in_flight.is_some(), "A 那一笔该在路上");

    // 切到 B 再改 B：这一笔因为"上一笔还在路上"发不出去（`begin_auto_save` 退回）。
    app.update(Message::GameSelected("other".into()));
    app.update(Message::SharpnessChanged(4.0));
    app.update(Message::AutoSave(app.autosave_generation));
    assert!(
        app.save_in_flight
            .as_ref()
            .is_some_and(|attempt| attempt.draft.game_id == "demo"),
        "在路上的仍然是 A 那一笔"
    );

    // A 的回包到了：世代已经变了（切款 + 改 B）⇒ 必须**补发**手上这份（B 的）。
    app.update(Message::ProfileSaved(1, Ok(())));
    let resend = app
        .save_in_flight
        .as_ref()
        .expect("B 那一笔必须被补发，否则它就丢了");
    assert_eq!(resend.draft.game_id, "other");
    assert_eq!(resend.draft.sharpness, 4);
}

/// `SC-02`：**切款前最后一笔还没发出去的编辑**必须跟着走，不能被"当前草稿"顶掉。
///
/// 场景：A 改一笔 → 上路；再改一笔（被挡下）；马上切到 B。从前被挡下的那笔直接丢弃，
/// 而补发拿的是**换款之后**的草稿（B 的），于是 A 的第二次编辑蒸发，界面还报"已保存"。
#[test]
fn the_last_edit_before_switching_games_is_still_saved() {
    let (mut app, _task) = App::new();
    app.games = vec![
        ui_game(),
        UiGame {
            id: "other".into(),
            name: "Other".into(),
            ..ui_game()
        },
    ];

    // A 的第一笔上路；第二笔因为"上一笔还在路上"被挡下 → 排队。
    app.update(Message::GameSelected("demo".into()));
    app.update(Message::AlgoChanged("Nis".into()));
    app.update(Message::AutoSave(app.autosave_generation));
    assert!(app.save_in_flight.is_some(), "A 的第一笔该在路上");
    app.update(Message::SharpnessChanged(4.0));
    app.update(Message::AutoSave(app.autosave_generation));
    assert_eq!(
        app.pending_save
            .as_ref()
            .map(|a| (a.draft.game_id.as_str(), a.draft.sharpness)),
        Some(("demo", 4)),
        "第二笔该排队，而且手里是 A 的那份草稿"
    );

    // 切款：`self.draft` 换成 B 的，排队那笔不受影响。
    app.update(Message::GameSelected("other".into()));
    assert_eq!(app.draft.as_ref().unwrap().game_id, "other");

    // 第一笔回来 ⇒ 发出去的必须是 A 的第二笔，不是 B 的草稿。
    app.update(Message::ProfileSaved(1, Ok(())));
    let next = app
        .save_in_flight
        .as_ref()
        .expect("排队那笔必须被发出去，否则 A 的最后一次编辑就丢了");
    assert_eq!(next.draft.game_id, "demo", "不许拿 B 的草稿顶替");
    assert_eq!(next.draft.sharpness, 4, "A 第二次改的锐度要真的落盘");
    assert_eq!(next.draft.algo, "Nis");
}
