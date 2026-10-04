//! 横幅与冲突弹窗那两条线（PLATFORMS.md §6.7 / §6.8 A / §6.8 B）。
//!
//! 从 `update/sync.rs` 拆出来（那边本来就贴着 500 行）：这一族的共同点是**"进页面 / 起游戏
//! 那一下看到的三方状态"** —— 横幅是它的只读那一半，冲突弹窗是它要用户拍板的那一半，
//! 「恢复」之前那句"本机有未同步的改动"是它的第三个出口。真正搬存档的动作仍然在
//! `update/sync.rs` 与 `update/versions.rs`（它们各自管一条路）。
//!
//! ## 三条不许动的
//!
//! * **横幅绝不阻塞页面**：进页面时先把它置成「正在核对…」（[`SyncBanner::requested`]），
//!   回包异步到（§6.8 A，用户 2026-10-04 明确要求"不要点进去之前先算"）；
//! * **冲突弹窗绝不阻塞启动**：它是 `game.launch` 回包里那一栏带来的，而 daemon 那条路
//!   本来就"先问再起"，三个回答各自接着把这一局办完（§6.8 B）；
//! * **「稍后再说」= 本次不拉也不传**：这里刻意**什么都不做**（不立牌子、不动存档），
//!   退出上传那一道闸门（§6.6 闸门 b）自然拦下它。

use super::super::*;
use crate::ui::message::ConflictAction;
use crate::ui::model::{restore_confirmation, restore_drifted};

impl App {
    /// 进了某一页（单游戏设置页 / 这一款的云端存档页）：**异步**问一次三方状态。
    ///
    /// 同一款已经问过就不重复问（那两处页面会来回切，而每切一次都真去读一遍索引没必要）；
    /// 回包还在路上的重复请求也不重发（`loading`）。
    pub(super) fn banner_requested(&mut self, game_id: &str) -> Task<Message> {
        if self.sync_banner.game_id == game_id
            && (self.sync_banner.loading || self.sync_banner.lines.is_some())
        {
            return Task::none();
        }
        self.sync_banner.requested(game_id);
        let socket = self.daemon_socket.clone();
        let id = game_id.to_string();
        // ⚠ **不用 `self.activity`**：底部那行"正在…"是给用户主动发起的动作用的，而这一趟
        // 是"进页面顺手核对一次" —— 每次进页面都弹一行状态栏反而像出了什么事。
        Task::perform(async move { sync_snapshot(&socket, &id).await }, |result| {
            Message::SyncSnapshotLoaded(Box::new(result))
        })
    }

    /// 冲突弹窗那一问的来处：`game.launch` 回包里 `sync_pull.ask.kind`（§6.8 B）。
    ///
    /// `Some` = 拉起浮层（`SyncAskState` 的另一副按钮），`None` = 这一局没有冲突要问。
    pub(super) fn open_conflict(&mut self, result: &Result<Value, String>) -> bool {
        let Some(game_id) = self.launching.clone() else {
            return false;
        };
        let ask = result
            .as_ref()
            .ok()
            .and_then(|value| value.get("sync_pull"))
            .and_then(|pull| pull.get("ask"))
            .and_then(|ask| ask.get("kind"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let Some(kind) = ask else {
            return false;
        };
        self.sync_ask = Some(game_id);
        self.sync_ask_cloud = None;
        self.sync_ask_trouble = None;
        self.sync_conflict = Some(kind);
        self.error = None;
        true
    }

    /// `update_banner` 负责的那一批消息（路由见 `update/mod.rs`）。
    pub(super) fn update_banner(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SyncSnapshotRequested(game_id) => self.banner_requested(&game_id),
            Message::SyncSnapshotLoaded(result) => {
                match *result {
                    Ok(snapshot) => self.sync_banner.loaded(snapshot),
                    Err(error) => self.sync_banner.failed(error),
                }
                Task::none()
            }
            // 点「恢复」（或那一页的「替换」）：先记下"要恢复哪一版"，真正的动作等确认。
            //
            // ⚠ 这里**顺手把"本机偏没偏基线"判出来**（§6.7 第 1 条）：判据就在手上那份横幅
            // 回包里（只按 mtime，不读内容），所以不必为了这一问再去打一趟 daemon。
            Message::SyncRestoreRequested(game_id, version) => {
                let drifted = self.sync_banner.game_id == game_id && self.sync_banner.drifted();
                // 措辞由 `model::banner::restore_confirmation` 一处给（§6.7 第 1 条），
                // 单游戏页那颗「恢复」与这一页的「替换」说的是同一句话。
                let wording = restore_confirmation(version.clone(), drifted);
                self.sync_restore_pending = Some((game_id, version, restore_drifted(&wording)));
                Task::none()
            }
            Message::SyncRestoreCancelled => {
                self.sync_restore_pending = None;
                Task::none()
            }
            Message::SyncRestoreConfirmed => {
                let Some((game_id, version, _)) = self.sync_restore_pending.take() else {
                    return Task::none();
                };
                self.sync_form.busy = true;
                self.sync_form.msg = Some(FormMsg::ok("正在恢复…"));
                let socket = self.daemon_socket.clone();
                self.activity(
                    "取回存档",
                    async move { sync_restore(&socket, &game_id, version.as_deref()).await },
                    Message::SyncNowDone,
                )
            }
            // ── 冲突弹窗那三颗按钮（§6.8 B）──
            Message::SyncConflictResolve(action) => self.resolve_conflict(action),
            // 「保留本机」那块牌子立好了：**这一局照常启动**（§6.8 B ②：牌子的意思是
            // "退出时上传"，不是"现在做点什么"）。
            Message::SyncConflictAllowed(result) => {
                match result {
                    Ok(()) => self.launch_again(),
                    // 牌子没立成（daemon 不通）：如实说，**游戏照常启动** —— 那是底线
                    // （§6.8 B：不阻塞启动），只是这一局退出后不会自动上传。
                    Err(error) => {
                        let task = self.set_error(format!(
                            "没能记下「保留本机」（这一局退出后不会自动上传）: {error}"
                        ));
                        Task::batch([task, self.launch_again()])
                    }
                }
            }
            // 委派是按变体名精确列的：漏一个就会走到这里，测试会立刻炸。
            other => unreachable!("update_banner 收到了不该由它处理的消息: {other:?}"),
        }
    }

    /// 冲突弹窗的三个后果（§6.8 B，三个都要真的接上）。
    fn resolve_conflict(&mut self, action: ConflictAction) -> Task<Message> {
        let Some(game_id) = self.sync_conflict.take() else {
            return Task::none();
        };
        // 这一问结束了：浮层收掉（`sync_ask` 是它"开着"的那个开关）。
        self.sync_ask = None;
        self.sync_ask_hidden = false;
        let socket = self.daemon_socket.clone();
        match action {
            // ① 用云端覆盖本机 ⇒ 立刻走 §6.5 那条路（先看再覆盖）。
            //    **不启动游戏**：这一颗是"先把存档弄对"，拉完用户自己按「启动」。
            ConflictAction::UseCloud => {
                self.launching = None;
                self.sync_form.busy = true;
                self.sync_form.msg = Some(FormMsg::ok("正在用云端那一版覆盖本机…"));
                self.activity(
                    "取回存档",
                    async move { sync_restore(&socket, &game_id, None).await },
                    Message::SyncNowDone,
                )
            }
            // ② 保留本机（结束后上传）⇒ 什么都不拉，只把"本次允许上传"的牌子立起来。
            ConflictAction::KeepLocal => self.activity(
                "记下「保留本机」",
                async move { allow_upload(&socket, &game_id).await },
                Message::SyncConflictAllowed,
            ),
            // ③ 稍后再说 ⇒ 本次**不拉也不传**，游戏照常启动（牌子不立、存档不动）。
            ConflictAction::Later => {
                tracing::info!("{game_id}: 冲突弹窗里选了「稍后再说」，本次不拉也不传");
                self.launching = Some(game_id);
                self.launch_again()
            }
        }
    }
}
