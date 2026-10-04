//! 凭据那一族消息：解锁 / 锁定 / 设主密码 / 删掉主密码文件。
//!
//! 从 `update/sync.rs` 拆出来（那边本来就顶到 500 行的软线），分法与 daemon 侧的
//! `sync_rpc` 一致：那边管"把存档搬来搬去"，这边管"凭据放在哪、能不能打开"，
//! 两者的失败方式完全不同（一个是网络，一个是密码）。

use super::super::*;

/// 这批消息归本文件管吗（`update_sync` 据此分派过来）。
///
/// ⚠ 按变体名**精确**列：漏一个就会掉进 `update_sync` 的 `unreachable!`，
/// 委派测试会立刻炸出是哪一个。
pub(super) fn handles(message: &Message) -> bool {
    matches!(
        message,
        Message::SyncMasterPasswordChanged(_)
            | Message::SyncUnlock
            | Message::SyncUnlocked(_)
            | Message::SyncSetMasterPassword
            | Message::SyncMasterSaved(_)
            | Message::SyncLockCredentials
            | Message::SyncCredentialsLocked(_)
            | Message::SyncMasterDeleteRequested
            | Message::SyncMasterDeleteCancelled
            | Message::SyncMasterDeleteConfirmed
            | Message::SyncMasterDeleted(_)
    )
}

impl App {
    /// `update_sync` 委派过来的那一批。
    ///
    /// 每一处 `FormMsg` 都在说"这句话是成功还是失败"（B13）—— 从前那件事是 `render`
    /// 靠 `contains("失败")` 猜的，于是「主密码不对」这种错会被显示成绿色。
    pub(super) fn update_sync_credentials(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SyncMasterPasswordChanged(value) => {
                self.sync_form.master_password = value;
                Task::none()
            }
            Message::SyncUnlock => {
                let password = self.sync_form.master_password.clone();
                if password.is_empty() {
                    self.sync_form.msg = Some(FormMsg::error("请先输入主密码"));
                    return Task::none();
                }
                self.sync_form.busy = true;
                self.sync_form.msg = None;
                let socket = self.daemon_socket.clone();
                self.activity(
                    "解锁凭据",
                    async move { unlock_credentials(&socket, &password).await },
                    Message::SyncUnlocked,
                )
            }
            Message::SyncUnlocked(result) => {
                self.sync_form.busy = false;
                match result {
                    Ok(()) => {
                        self.sync_form.master_password.clear();
                        self.sync_form.msg = Some(FormMsg::ok("已解锁"));
                    }
                    // ⚠ "主密码不对（或者凭据文件损坏了）"里没有"失败"两个字，从前的
                    //    字符串判据会把它显示成**绿色**。
                    Err(e) => self.sync_form.msg = Some(FormMsg::error(e)),
                }
                self.reload_sync()
            }
            Message::SyncSetMasterPassword => {
                let password = self.sync_form.master_password.clone();
                if password.chars().count() < self.min_master_password() {
                    self.sync_form.msg = Some(FormMsg::error(format!(
                        "主密码至少要 {} 个字符",
                        self.min_master_password()
                    )));
                    return Task::none();
                }
                self.sync_form.busy = true;
                self.sync_form.msg = None;
                let socket = self.daemon_socket.clone();
                self.activity(
                    "设置主密码",
                    async move { set_master_password(&socket, &password).await },
                    Message::SyncMasterSaved,
                )
            }
            Message::SyncMasterSaved(result) => {
                self.sync_form.busy = false;
                match result {
                    Ok(msg) => {
                        self.sync_form.master_password.clear();
                        // 好消息还是警告，`set_master_password` 已经说清楚了（B2：
                        // 明文文件没能删掉时那句话必须显眼）。
                        self.sync_form.msg = Some(msg);
                    }
                    Err(e) => self.sync_form.msg = Some(FormMsg::error(e)),
                }
                self.reload_sync()
            }
            Message::SyncLockCredentials => {
                if self.sync_form.busy {
                    return Task::none();
                }
                self.sync_form.busy = true;
                self.sync_form.msg = None;
                let socket = self.daemon_socket.clone();
                self.activity(
                    "锁定凭据",
                    async move { lock_credentials(&socket).await },
                    Message::SyncCredentialsLocked,
                )
            }
            Message::SyncCredentialsLocked(result) => {
                self.sync_form.busy = false;
                self.sync_form.msg = Some(match result {
                    Ok(()) => FormMsg::ok("凭据文件已锁定；再要用它得重新输入主密码"),
                    Err(e) => FormMsg::error(format!("锁定失败: {e}")),
                });
                self.reload_sync()
            }
            Message::SyncMasterDeleteRequested => {
                self.sync_form.confirm_master_delete = true;
                Task::none()
            }
            Message::SyncMasterDeleteCancelled => {
                self.sync_form.confirm_master_delete = false;
                Task::none()
            }
            Message::SyncMasterDeleteConfirmed => {
                // 破坏性操作:确认过一次就够了,别再让用户点第三下。
                if !self.sync_form.confirm_master_delete {
                    return Task::none();
                }
                self.sync_form.confirm_master_delete = false;
                self.sync_form.busy = true;
                self.sync_form.msg = None;
                let socket = self.daemon_socket.clone();
                self.activity(
                    "删除主密码文件",
                    async move { clear_master_file(&socket).await },
                    Message::SyncMasterDeleted,
                )
            }
            Message::SyncMasterDeleted(result) => {
                self.sync_form.busy = false;
                self.sync_form.msg = Some(match result {
                    Ok(()) => FormMsg::ok("已删除主密码凭据文件（存在里面的凭据一起消失了）"),
                    Err(e) => FormMsg::error(format!("删除凭据文件失败: {e}")),
                });
                self.reload_sync()
            }
            other => unreachable!("update_sync_credentials 收到了不该由它处理的消息: {other:?}"),
        }
    }
}
