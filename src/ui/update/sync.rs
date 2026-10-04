//! 云同步那一块的消息：设置、测试连接、立即同步、恢复。
//!
//! 从 `update/mod.rs` 拆出来（那里本来是 900 多行的单个 `match`）。语义一字未动。
//! 凭据那一族（解锁 / 锁定 / 设主密码 / 删主密码文件）后来又拆到了
//! [`super::sync_credentials`]：它们只跟"凭据放在哪、能不能打开"有关，与"存哪、
//! 传什么"是两件事（同 daemon 侧 `sync_rpc` 的分法）。
//!
//! ⚠ 这个文件里每一处 `FormMsg` 都在回答"这句话是成功还是失败"（B13）—— 从前那件事
//! 是 `render` 靠 `contains("失败")` 猜的，写出这句提示的人根本不知道自己在动一个判据。

use super::super::*;

impl App {
    /// update_sync 负责的那一批消息。
    ///
    /// 拆出来只是因为 `update` 那个 match 太长：**这里改的仍然是同一个 `App`**，
    /// 语义一字未动。
    pub(super) fn update_sync(&mut self, message: Message) -> Task<Message> {
        // 凭据那一族先分派出去（见文件头）。`handles` 按变体名精确列，漏一个就会
        // 掉进下面那个 `unreachable!`，委派的测试会立刻炸。
        if super::sync_credentials::handles(&message) {
            return self.update_sync_credentials(message);
        }
        match message {
            Message::SyncStatusLoaded(result) => match *result {
                Ok(status) => {
                    self.sync_form.apply(&status, &status.settings);
                    self.sync_status = Some(status);
                    Task::none()
                }
                Err(e) => {
                    self.sync_form.loaded = true;
                    self.sync_form.msg = Some(FormMsg::error(format!("读取同步状态失败: {e}")));
                    Task::none()
                }
            },
            Message::SyncToggleEnabled(value) => {
                // **点下去就生效**（用户 2026-09-28 报的"开关前后端不对应"）：它是布尔开关，
                // 没有"打字中间态"，与旁边那两颗引擎按钮同一种东西 —— 而从前这里只改
                // `form.enabled` 再 `Task::none()`，于是界面显示"已启用"而后端还是 `false`
                // （退出游戏自然不上传）；`settings_dirty` 一置起还会让 `form.apply()` 跳过
                // **所有**字段，界面就冻结在用户点的那一份上，连"未保存"都不说。
                //
                // 只提交 `enabled` 一个字段：用户手上那些还没保存的编辑（bucket、prefix…）
                // 一个字都不会被带上去，`settings_dirty` 也**不动**（与换引擎那条一致）。
                self.sync_form.enabled = value;
                self.sync_form.busy = true;
                self.sync_form.msg = None;
                let socket = self.daemon_socket.clone();
                self.activity(
                    "切换云同步总开关",
                    async move { save_sync_enabled(&socket, value).await },
                    Message::SyncToggleEnabledSaved,
                )
            }
            Message::SyncToggleEnabledSaved(result) => {
                self.sync_form.busy = false;
                match result {
                    Ok(()) => {
                        self.sync_form.msg = Some(if self.sync_form.enabled {
                            FormMsg::ok("云同步已启用：退出游戏后会自动上传")
                        } else {
                            FormMsg::ok("云同步已关闭")
                        });
                    }
                    Err(error) => {
                        self.sync_form.msg =
                            Some(FormMsg::error(format!("改云同步总开关失败: {error}")));
                        // 没写进去就别让界面继续装着改过了：清掉 dirty，好让下面这次刷新把
                        // 配置里那个真正的值拉回来（与 `SyncEngineSaved` 同一条规矩）。而且
                        // **挂横幅** —— 这颗开关是最容易"以为生效了"的那一个。
                        self.sync_form.settings_dirty = false;
                        let message = format!("改「启用云同步」失败: {error}");
                        return Task::batch([self.set_error(message), self.reload_sync()]);
                    }
                }
                self.reload_sync()
            }
            Message::SyncField(field, value) => {
                let form = &mut self.sync_form;
                match field {
                    SyncField::Endpoint => form.endpoint = value,
                    SyncField::Bucket => form.bucket = value,
                    SyncField::Prefix => form.prefix = value,
                    SyncField::KeepVersions => form.keep_versions = value,
                    SyncField::KeyId => form.key_id = value,
                    SyncField::AppKey => form.app_key = value,
                    SyncField::RcloneBinary => form.rclone_binary = value,
                    SyncField::KopiaBinary => form.kopia_binary = value,
                }
                if matches!(
                    field,
                    SyncField::Endpoint
                        | SyncField::Bucket
                        | SyncField::Prefix
                        | SyncField::KeepVersions
                        | SyncField::RcloneBinary
                        | SyncField::KopiaBinary
                ) {
                    form.settings_dirty = true;
                }
                Task::none()
            }
            Message::SyncEngineSelected(engine) => {
                // **点下去就生效**：引擎是二选一的开关，不该还要用户再去找一个「保存设置」
                // ——从前就是那样，界面上按钮立刻变成「kopia √」，而 config 里一个字节都
                // 没动，重开 GUI 就又回到 rclone（用户 2026-09-16 报的就是这个）。
                //
                // 只提交 engine 一个字段：用户手上那些还没保存的编辑（bucket、prefix…）
                // 一个字都不会被带上去，`settings_dirty` 在这里也**不动**。
                self.sync_form.engine = engine.clone();
                self.sync_form.busy = true;
                self.sync_form.msg = None;
                let socket = self.daemon_socket.clone();
                self.activity(
                    "切换同步引擎",
                    async move { save_sync_engine(&socket, &engine).await },
                    Message::SyncEngineSaved,
                )
            }
            Message::SyncEngineSaved(result) => {
                self.sync_form.busy = false;
                match result {
                    // daemon 说这一笔真的换了引擎 ⇒ 那条"对面数据看不见"的警告要说出来。
                    Ok(true) => {
                        self.sync_form.msg =
                            Some(FormMsg::ok(engine_switched_note(&self.sync_form.engine)));
                    }
                    Ok(false) => self.sync_form.msg = Some(FormMsg::ok("同步方式已保存")),
                    Err(error) => {
                        self.sync_form.msg =
                            Some(FormMsg::error(format!("切换同步方式失败: {error}")));
                        // 没写进去就别让界面继续装着已经换了：清掉 dirty，好让下面这次
                        // 刷新把配置里那个真正的值拉回来。
                        self.sync_form.settings_dirty = false;
                    }
                }
                self.reload_sync()
            }
            Message::SyncKopiaPasswordChanged(value) => {
                self.sync_form.kopia_password = value;
                Task::none()
            }
            Message::SyncSaveKopiaPassword => {
                let password = self.sync_form.kopia_password.clone();
                self.sync_form.busy = true;
                self.sync_form.msg = None;
                let socket = self.daemon_socket.clone();
                self.activity(
                    "保存仓库密码",
                    async move { save_kopia_password(&socket, password.trim()).await },
                    Message::SyncKopiaPasswordSaved,
                )
            }
            Message::SyncKopiaPasswordSaved(result) => {
                self.sync_form.busy = false;
                match result {
                    Ok(using_default) => {
                        // 密码进了凭据库就立刻从输入框里消失,与 B2 那两条同一套规矩。
                        self.sync_form.kopia_password.clear();
                        self.sync_form.msg = Some(if using_default {
                            FormMsg::ok(
                                "已改回默认密码 kotori —— 任何拿到这个 bucket 的人都能解开仓库",
                            )
                        } else {
                            FormMsg::ok("kopia 仓库密码已保存")
                        });
                    }
                    Err(error) => {
                        self.sync_form.msg = Some(FormMsg::error(format!("保存失败: {error}")));
                    }
                }
                self.reload_sync()
            }
            Message::SyncSaveSettings => {
                let form = self.sync_form.clone();
                // 非法输入就地拦下:以前 `patch()` 用 `unwrap_or(0)` 把 "abc" 变成 0,而 0
                // 的语义是**永不删版本** —— 与用户想限制保留份数的意图正好相反,界面却报
                // "已保存"。
                let patch = match form.patch() {
                    Ok(patch) => patch,
                    Err(error) => {
                        // ⚠ 这是"没存下去"，从前的字符串判据（`contains("失败")`）看不出
                        //    来，会把它显示成绿色。
                        self.sync_form.msg = Some(FormMsg::error(error));
                        return Task::none();
                    }
                };
                self.sync_form.busy = true;
                self.sync_form.msg = None;
                let socket = self.daemon_socket.clone();
                self.activity(
                    "保存同步设置",
                    async move { save_sync_settings(&socket, patch).await },
                    Message::SyncSettingsSaved,
                )
            }
            Message::SyncSettingsSaved(result) => {
                self.sync_form.busy = false;
                match &result {
                    Ok(true) => {
                        self.sync_form.msg =
                            Some(FormMsg::ok(engine_switched_note(&self.sync_form.engine)));
                    }
                    Ok(false) => self.sync_form.msg = Some(FormMsg::ok("已保存")),
                    Err(e) => {
                        self.sync_form.msg = Some(FormMsg::error(format!("保存失败: {e}")));
                    }
                }
                if result.is_ok() {
                    // The daemon now holds exactly what the form holds, so a
                    // later status reply may refill the form again.
                    self.sync_form.settings_dirty = false;
                }
                self.reload_sync()
            }
            Message::SyncSaveCredentials => {
                let key_id = self.sync_form.key_id.trim().to_string();
                let app_key = self.sync_form.app_key.trim().to_string();
                if key_id.is_empty() && app_key.is_empty() {
                    self.sync_form.msg =
                        Some(FormMsg::error("两个字段都空着：这只会清掉已保存的凭据"));
                    return Task::none();
                }
                // 内存那一级只是"过渡":没有可持久化的后端时,先把主密码设起来,
                // 否则凭据活不过这个守护进程 —— 静默接受等于骗用户(ADR-014)。
                if self.credential_store() == CredentialStore::Session {
                    self.sync_form.msg =
                        Some(FormMsg::error(CredentialStore::needs_master_password()));
                    return Task::none();
                }
                self.sync_form.busy = true;
                self.sync_form.msg = None;
                let socket = self.daemon_socket.clone();
                self.activity(
                    "保存云端凭据",
                    async move { save_sync_credentials(&socket, &key_id, &app_key).await },
                    Message::SyncCredentialsSaved,
                )
            }
            Message::SyncCredentialsSaved(result) => {
                self.sync_form.busy = false;
                self.sync_form.msg = Some(match &result {
                    Ok(()) => FormMsg::ok(self.credential_store().saved_note("凭据")),
                    Err(e) => FormMsg::error(format!("保存凭据失败: {e}")),
                });
                if result.is_ok() {
                    // The daemon consumed them; never echo secrets back.
                    self.sync_form.key_id.clear();
                    self.sync_form.app_key.clear();
                }
                self.reload_sync()
            }
            Message::SyncClearCredentials => {
                self.sync_form.busy = true;
                self.sync_form.msg = None;
                let socket = self.daemon_socket.clone();
                self.activity(
                    "清除云端凭据",
                    async move { save_sync_credentials(&socket, "", "").await },
                    Message::SyncCredentialsCleared,
                )
            }
            Message::SyncCredentialsCleared(result) => {
                self.sync_form.busy = false;
                self.sync_form.msg = Some(match &result {
                    Ok(()) => FormMsg::ok(format!(
                        "已从{}里删除 B2 凭据",
                        self.credential_store().name()
                    )),
                    Err(e) => FormMsg::error(format!("删除凭据失败: {e}")),
                });
                if result.is_ok() {
                    self.sync_form.key_id.clear();
                    self.sync_form.app_key.clear();
                }
                self.reload_sync()
            }
            Message::SyncTest => {
                self.sync_form.busy = true;
                // 这一条**必须**先给句话:它背后可能是一次真的网络往返(kopia 连桶、
                // 建仓库、列一次快照),最长能到几十秒,而 busy 只把按钮变灰 ——
                // 用户看到的就是"点了没反应"(2026-09-18 报的)。「立即同步全部」一直
                // 都有这句,是这一个漏了。
                self.sync_form.msg = Some(FormMsg::ok("正在测试连接…"));
                let socket = self.daemon_socket.clone();
                self.activity(
                    "测试云连接",
                    async move { sync_test(&socket).await },
                    Message::SyncTested,
                )
            }
            // 单游戏页那颗「参与云同步」开关：立即存（它只发 `game.update` 的一个字段，
            // 不走那条自动保存的草稿路 —— 别的编辑一个都不会被带上）。
            Message::SyncParticipatingToggled(enabled) => {
                let Some(id) = self.selected.clone() else {
                    return Task::none();
                };
                // 先在界面上生效（开关自己已经翻过去了）；存不成再翻回来。
                if let Some(game) = self.games.iter_mut().find(|game| game.id == id) {
                    game.sync_enabled = enabled;
                }
                let socket = self.daemon_socket.clone();
                self.activity(
                    "改这一款的同步开关",
                    async move { set_game_sync_enabled(&socket, id, enabled).await },
                    move |result| Message::SyncParticipatingSaved(enabled, result),
                )
            }
            Message::SyncParticipatingSaved(enabled, result) => {
                match result {
                    Ok(()) => {
                        self.saved_ok = true;
                        self.saved_msg = Some(if enabled {
                            "这一款已参与云同步。".to_string()
                        } else {
                            "这一款已暂时关掉云同步（随时能在这一页再打开）。".to_string()
                        });
                    }
                    Err(e) => {
                        // 存不成 ⇒ 把界面上那个值翻回去，别让人以为已经生效。
                        if let Some(id) = self.selected.clone()
                            && let Some(game) = self.games.iter_mut().find(|game| game.id == id)
                        {
                            game.sync_enabled = !enabled;
                        }
                        self.saved_ok = false;
                        self.saved_msg = Some(format!("改这一款的云同步失败: {e}"));
                    }
                }
                Task::none()
            }
            // ── 单游戏页的绑定（用户 2026-09-24：显示当前绑定、换绑、新建） ──
            // 「更改绑定…」：打开云端清单，挑中的那条成为新的绑定（**不**启动游戏）。
            Message::SyncRebindRequested => {
                self.sync_new_pending = false;
                self.cloud_pick.open(CloudPickPurpose::Rebind);
                let socket = self.daemon_socket.clone();
                self.activity(
                    "拉取云端清单",
                    async move { cloud_list(&socket, false).await },
                    Message::CloudPickLoaded,
                )
            }
            Message::SyncNewIdentityCancelled => {
                self.sync_new_pending = false;
                Task::none()
            }
            Message::SyncNewIdentityConfirmed => {
                self.sync_new_pending = false;
                self.change_binding(None)
            }
            Message::SyncBindingChanged(result) => {
                match result {
                    Ok(()) => {
                        self.saved_ok = true;
                        self.saved_msg = Some("绑定已更新。".to_string());
                    }
                    Err(e) => {
                        self.saved_ok = false;
                        self.saved_msg = Some(format!("改绑定失败: {e}"));
                    }
                }
                // 绑定变了 ⇒ 那一行要跟着变（状态是 daemon 算的，重新问一次）。
                self.reload_sync()
            }
            Message::SyncTested(result) => {
                self.sync_form.busy = false;
                self.sync_form.msg = Some(match result {
                    Ok(remote) => FormMsg::ok(format!("连接正常：{remote}")),
                    Err(e) => FormMsg::error(format!("连接失败: {e}")),
                });
                Task::none()
            }
            Message::SyncNow(game_id) => {
                self.sync_form.busy = true;
                self.sync_form.msg = Some(FormMsg::ok("正在同步…"));
                let socket = self.daemon_socket.clone();
                self.activity(
                    "同步这一款的存档",
                    async move { sync_now(&socket, game_id).await },
                    Message::SyncNowDone,
                )
            }
            Message::SyncNowDone(result) => {
                self.sync_form.busy = false;
                match result {
                    // ⚠ 好消息还是坏消息由 `describe_sync_outcome` 判定（B13）：
                    //    "某个位置失败了"与"没有需要同步的变化"从前长得一样。
                    Ok(msg) => {
                        self.sync_form.msg = Some(msg);
                        self.reload_sync()
                    }
                    // ⚠ 失败**必须挂顶部横幅**（用户 2026-09-28："算不了直接横幅报错"）：
                    // 表单里那行小字常常在一屏之外，同步失败时他根本看不见 —— 而"上传必须
                    // 带指纹"这类拒绝正是从这里冒出来的（见 `sync_rpc::pack_identity`）。
                    Err(error) => {
                        let message = format!("同步失败: {error}");
                        self.sync_form.msg = Some(FormMsg::error(message.clone()));
                        Task::batch([self.set_error(message), self.reload_sync()])
                    }
                }
            }
            // 「恢复」那两颗（`SyncRestoreRequested` / `Cancelled` / `Confirmed`）与冲突弹窗
            // 都在 `update_banner` 里 —— 那条线管的是"三方状态与要用户拍板的那一问"，
            // 这里只留真正搬存档的那几个动作（见 `update/banner.rs` 的文件头）。
            // 委派是按变体名精确列的：漏一个就会走到这里，测试会立刻炸。
            other => unreachable!("update_sync 收到了不该由它处理的消息: {other:?}"),
        }
    }
}
