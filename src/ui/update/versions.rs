//! 单游戏页那一页「这一款的云端存档」：列版本、挑一版替换本机、删一版、清空这一款、
//! 把这一款从云端抹掉。
//!
//! 与 `update/cloud.rs`（那一页看的是"云端都有什么"，只读）分开：这一页能做的事都是
//! **破坏性**的（覆盖本机存档目录、删云端的东西），所以走"先记下待确认的那件事、确认之后
//! 才发出去"那条路 —— 四种动作共用同一个弹窗（`Confirmation` 说清是哪一件），与云同步
//! 页那颗「恢复」同一个形状。

use super::super::*;

impl App {
    /// 点了身份那一条（整条可点）：开页，并按当前这一款去列它那条身份的版本。
    ///
    /// 还没绑定身份（算不出 `cloud_key`）就只开页、一个请求都不发 —— 空态那句话由
    /// `bound` 说。用户 2026-09-25：没绑定画不画都行，列不出东西也是正常的。
    pub(super) fn versions_opened(&mut self) -> Task<Message> {
        let Some(game_id) = self.selected.clone() else {
            return Task::none();
        };
        let key = self
            .selected_sync_game()
            .map(|row| row.cloud_key.clone())
            .unwrap_or_default();
        self.versions.opened(&game_id, &key);
        // §6.8 A 的第二处：进这一页也要那块横幅（本机 / 云端 / 基线 + 接下来）。
        // 与"读这一款的版本列表"**并行**跑，谁先回来谁先画 —— 横幅绝不拖住页面。
        let banner = self.banner_requested(&game_id);
        if key.is_empty() {
            return banner;
        }
        let socket = self.daemon_socket.clone();
        Task::batch([
            banner,
            self.activity(
                "读这一款的版本列表",
                async move { cloud_versions(&socket, key).await },
                Message::GameVersionsLoaded,
            ),
        ])
    }

    /// update_versions 负责的那一批消息（路由见 `update/mod.rs`）。
    pub(super) fn update_versions(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::GameVersionsOpened => self.versions_opened(),
            Message::GameVersionsClosed => {
                self.versions.closed();
                Task::none()
            }
            Message::GameVersionsLoaded(result) => {
                match result {
                    Ok(rows) => self.versions.loaded(rows),
                    Err(e) => self.versions.failed(format!("读云端版本失败: {e}")),
                }
                Task::none()
            }
            // 每颗按钮只记下要问哪一件事：真正的动作等弹窗那一下。
            //
            // ⚠ 「替换」这一颗与单游戏页那颗「恢复」是**同一件事**（拿云端某一版覆盖本机），
            // 所以它也吃 §6.7 第 1 条那条前置提示：本机偏离基线时，弹窗要说清"有未同步的
            // 改动，恢复会覆盖它"。判据取自这一页那份横幅（进页面时异步问来的 `sync.snapshot`）
            // —— 它已经是"本机 mtime vs 基线"那一条，不必为了这一颗按钮再问一次。
            Message::GameVersionsReplaceVersion(version) => {
                let drifted = self.drifted_for_this_page();
                self.versions
                    .requested(Confirmation::Replace { version, drifted });
                Task::none()
            }
            Message::GameVersionsDeleteVersion(version) => {
                self.versions
                    .requested(Confirmation::DeleteVersion { version });
                Task::none()
            }
            Message::GameVersionsClearVersions => {
                self.versions.requested(Confirmation::ClearVersions);
                Task::none()
            }
            Message::GameVersionsForgetIdentity => {
                self.versions.requested(Confirmation::ForgetIdentity);
                Task::none()
            }
            Message::GameVersionsCancelled => {
                self.versions.cancelled();
                Task::none()
            }
            // 四种动作共用这一个弹窗：分派按状态里记着的那件事走。
            Message::GameVersionsConfirmed => {
                let Some(action) = self.versions.confirmed() else {
                    return Task::none();
                };
                let key = self.versions.cloud_key.clone();
                let game_id = self.versions.game_id.clone();
                let socket = self.daemon_socket.clone();
                match action {
                    Confirmation::Replace { version, .. } => self.activity(
                        "取回存档",
                        async move {
                            // 这一页的那句话只存文字（`VersionsState::msg` 是 `String`），
                            // 所以在这里把语义那一半丢掉 —— 它不参与颜色判断。
                            sync_restore(&socket, &game_id, Some(version.as_str()))
                                .await
                                .map(|msg| msg.text().to_string())
                        },
                        Message::GameVersionsReplaced,
                    ),
                    Confirmation::DeleteVersion { version } => self.activity(
                        "删除那一版存档",
                        async move { sync_delete_version(&socket, key, version).await },
                        Message::GameVersionsDeleted,
                    ),
                    Confirmation::ClearVersions => self.activity(
                        "删除云端版本",
                        async move { sync_clear_versions(&socket, key).await },
                        Message::GameVersionsDeleted,
                    ),
                    Confirmation::ForgetIdentity => self.activity(
                        "忘掉这条云端身份",
                        async move { sync_forget_identity(&socket, key).await },
                        Message::GameVersionsDeleted,
                    ),
                }
            }
            // ⚠ 结果写在这一页自己那句话上（`versions.replaced`），不是云同步页那句。
            Message::GameVersionsReplaced(result) => {
                self.versions.replaced(result);
                Task::none()
            }
            Message::GameVersionsDeleted(result) => {
                self.versions.deleted(result);
                // 删完顺手把状态重读一次：身份那行摘要（"N 版"）是从 `sync.status` 来的，
                // 不重读就会在刚删空的列表下面挂着旧数字。
                Task::perform(async { load_sync_status().await }, |result| {
                    Message::SyncStatusLoaded(Box::new(result))
                })
            }
            other => unreachable!("update_versions 收到了不该由它处理的消息: {other:?}"),
        }
    }

    /// 这一页（这一款的云端存档）手上那份横幅说本机偏没偏基线（§6.7 第 1 条）。
    ///
    /// 只认**当前这一款**的那一份：换款之后旧回包会被 banner 自己丢掉，而这里再核一次
    /// id 是防"换了款但横幅还是上一款那份"那半秒的窗口。横幅还没回来（「正在核对…」）时
    /// `drifted()` 给 `false` —— 那只是措辞轻一点，覆盖本机这件事**本来就还有一道二次确认**。
    fn drifted_for_this_page(&self) -> bool {
        self.sync_banner.game_id == self.versions.game_id && self.sync_banner.drifted()
    }
}
