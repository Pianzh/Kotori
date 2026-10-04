//! `App::update`: the message loop.
//!
//! 这个 `match` 曾经有 900 多行，所以按**消息属于哪一块**拆成了几个文件：
//! 本文件管跳转、游戏库与库里的增删改，`settings` 管服务/wine/单游戏设置/环境，
//! `sync` 管云同步，「云端存档」的浏览在 `cloud`。几处改的仍然是同一个 `App`，
//! 只是每个文件不再长到读不完。

use super::*;

mod add;
mod banner;
mod cloud;
mod picker;
mod profile;
// 「启动 / 停止」那一族住在 `run.rs`;`pub(in crate::ui)` 只为单元测试叫得到它。
pub(in crate::ui) mod run;
mod settings;
mod sync;
// 凭据那一族从 `sync.rs` 拆出来（那边顶到 500 行软线了），分法与 daemon 侧
// `sync_rpc` 一致：一边是"把存档搬来搬去"，一边是"凭据放在哪、能不能打开"。
mod sync_credentials;
mod versions;

use settings::is_settings_message;

impl App {
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::TabChanged(tab) => {
                self.tab = tab;
                self.selected = None;
                self.draft = None;
                self.confirm_delete = false;
                self.confirm_stop = false;
                // 那一层覆盖跟着"当前这一款"走：切页 / 回库 / 删条目时都不能留着，
                // 否则下次进另一款，它带着上一款的版本直接盖上来。
                self.versions.closed();
                // 「云端存档」那两层覆盖同理（切页之后再切回来是另一份清单）。
                self.cloud_version.closed();
                self.error = None;
                if tab == Tab::Cloud {
                    return self.cloud_entered();
                }
                if tab == Tab::Settings || tab == Tab::Sync {
                    // Re-read them all, they may have changed on disk (or in the
                    // daemon, which is the only writer).
                    let mut tasks = vec![
                        self.activity(
                            "探测 wine",
                            async { load_wine_status().await },
                            Message::WineStatusLoaded,
                        ),
                        Task::perform(async { load_sync_status().await }, |result| {
                            Message::SyncStatusLoaded(Box::new(result))
                        }),
                    ];
                    // 环境检查只在设置页问:它会真去跑几个外部程序(见 `platform`)。
                    if tab == Tab::Settings {
                        tasks.push(self.activity(
                            "检查运行环境",
                            async { load_environment().await },
                            Message::EnvironmentLoaded,
                        ));
                    }
                    return Task::batch(tasks);
                }
                Task::none()
            }
            // 「从正在运行的进程里挑」那一族消息在 `update/picker.rs`。
            m @ (Message::ProcessPickerOpen
            | Message::ProcessesLoaded(..)
            | Message::ProcessQueryChanged(..)
            | Message::ProcessPicked(..)
            | Message::ProcessPickerClose) => self.update_picker(m),
            Message::Refresh => {
                self.error = None;
                self.loading = true;
                // 后台服务是用户自己停的:刷新只看看它在不在,不许顺手把它拉起来。
                if self.daemon_paused {
                    self.activity(
                        "重新载入游戏库",
                        async { load_without_booting().await },
                        Message::GamesLoaded,
                    )
                } else {
                    self.activity(
                        "重新载入游戏库",
                        async { connect_and_load().await },
                        Message::GamesLoaded,
                    )
                }
            }
            Message::GamesLoaded(Ok(games)) => {
                self.games = games;
                self.loading = false;
                self.daemon_connected = Some(true);
                // 连上了就是连上了 —— 无论它是我们拉起来的还是用户从别处起的。
                self.daemon_paused = false;
                // ⚠ 这里**不许**清错误条：保存失败之后紧跟着就是这一次刷新，从前那句
                // `self.error = None` 会把刚弹出来的提示在 0.2 秒内吃掉（用户 2026-09-27
                // 报的"顶部错误条一闪就没"）。现在统一由 `set_error` 的 3 秒定时器收尾。
                self.retry_attempts = 0;
                Task::none()
            }
            Message::GamesLoaded(Err(e)) => {
                self.loading = false;
                self.daemon_connected = Some(false);
                let error_task = self.set_error(e);
                // 用户亲手停掉的服务不该被退避重试一次次拉起来(那才叫"停不掉")。
                if self.daemon_paused {
                    self.retry_attempts = 0;
                    return error_task;
                }
                // Self-heal: keep retrying with backoff, so the UI recovers on
                // its own once the daemon is back.
                self.retry_attempts = self.retry_attempts.saturating_add(1);
                if self.retry_attempts <= MAX_AUTO_RETRIES {
                    let delay = retry_delay(self.retry_attempts);
                    return Task::batch([
                        error_task,
                        Task::perform(async move { tokio::time::sleep(delay).await }, |_| {
                            Message::Refresh
                        }),
                    ]);
                }
                error_task
            }
            // 一颗按钮两种时候:该启动还是该停由 `run_action` 说了算(文案也从同一
            // 份会话表来,所以两者不可能再说两套话)。
            Message::ToggleRun(game_id) => self.toggle_run(game_id),
            Message::LaunchDone(result) => {
                // 自检认不出云端那一条：daemon 先没起游戏，把问题交回来了。这一问由
                // 浮层接（`SyncAskState`），回答走 `SyncAskAnswered`。
                if result
                    .as_ref()
                    .is_ok_and(|value| value["needs_sync_decision"] == serde_json::json!(true))
                {
                    self.sync_ask = self.launching.take();
                    // 云端"疑似找到的那一条"：有就显示（名字与摘要由界面用同一个函数生成），
                    // `None` 就是"完全没找到" —— 界面分两种说法，**不编名字**。
                    self.sync_ask_cloud = result
                        .as_ref()
                        .ok()
                        .and_then(|value| value.get("cloud"))
                        .and_then(parse_sync_ask_cloud);
                    // 第三种情况：**没读到云端**（桶名不对、网络不通、索引没建过）。它与
                    // "云端没有这一款"必须分开说 —— 用户 2026-09-28 在 Windows 上就是因为
                    // 两者混在一起，以为指纹匹配坏了（实情是桶名填成了 `kotori-win`）。
                    self.sync_ask_trouble = result
                        .as_ref()
                        .ok()
                        .and_then(|value| value.get("cloud_trouble"))
                        .and_then(|value| value.as_str())
                        .filter(|text| !text.is_empty())
                        .map(str::to_string);
                    self.error = None;
                    return Task::none();
                }
                // §6.8 B：冲突弹窗。启动前那条路判定"本地和云端都改过"时**不启动游戏**，
                // 把这一问交回来（`sync_pull.ask.kind`）—— 复用同一个浮层（`SyncAskState`），
                // 由 `sync_conflict` 那一栏换成三颗按钮。游戏照常等这一问答完再起。
                if self.open_conflict(&result) {
                    return Task::none();
                }
                self.launching = None;
                match result {
                    Ok(value) => {
                        let sid = value
                            .get("session_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("?");
                        tracing::info!("game session started: {sid}");
                        self.error = None;
                        let socket = self.daemon_socket.clone();
                        Task::perform(
                            async move { load_status(&socket).await },
                            Message::StatusLoaded,
                        )
                    }
                    Err(e) => self.set_error(e),
                }
            }
            Message::GameSelected(id) => {
                // 换款之前把上一款那笔编辑交出去：防抖窗口里挂着的那一次会被这一笔
                // 取代（世代 +1），而 `begin_auto_save` 拿的是**当前**草稿 —— 也就是
                // 上一款的。`self.draft` 下面就被换成新款了，不先发这一笔，上一款的
                // 编辑就只留在内存里（BUG-18）。手里没有草稿时什么都别动：世代空转一次
                // 会让"这一笔是第几代"这种断言失去参照。
                let flush = if self.draft.is_some() {
                    let task = self.begin_auto_save();
                    self.cancel_auto_save();
                    task
                } else {
                    Task::none()
                };
                let mut load_sync = false;
                if let Some(g) = self.games.iter().find(|g| g.id == id) {
                    self.selected = Some(g.id.clone());
                    self.saved_msg = None;
                    self.confirm_delete = false;
                    // 二次确认属于上一款,别带过来。
                    self.confirm_stop = false;
                    // 同理,"确认新建"那个两段式状态也不许跟着换款。
                    self.sync_new_pending = false;
                    // 换款也跟着换掉那一问（它是上一款的事）。
                    self.sync_conflict = None;
                    // Seed the form from the *stored* profile. Anything else
                    // means a plain "open + save" silently rewrites settings.
                    self.draft = Some(Draft::from_game(g));
                    // 这一页现在也显示这个游戏的云存档状况,而 `sync.status` 平时只在
                    // 打开「云同步」/「设置」页时才读 —— 直接从游戏库点进来时补一次。
                    load_sync = self.sync_status.is_none();
                }
                // 进这一页（§6.8 A 的第一处）：**异步**问一次三方状态，横幅先写「正在核对…」。
                // ⚠ 它与上面那份 `sync.status` 是两回事：那份说的是"这一款参不参与、上次怎么样"，
                // 这一份说的是"本机 / 云端 / 基线现在各是什么"。
                let banner = match self.selected.clone() {
                    Some(id) => self.banner_requested(&id),
                    None => Task::none(),
                };
                if load_sync {
                    return Task::batch([
                        flush,
                        banner,
                        Task::perform(async { load_sync_status().await }, |result| {
                            Message::SyncStatusLoaded(Box::new(result))
                        }),
                    ]);
                }
                Task::batch([flush, banner])
            }
            Message::BackToList => {
                // 离开这一页之前先把草稿交出去：`draft` 下面就被清掉了，不先发这一笔，
                // 防抖窗口里那次编辑就只留在内存里（BUG-18 的另一半）。没有草稿时什么都
                // 别动（理由同上：世代不要空转）。
                let flush = if self.draft.is_some() {
                    let task = self.begin_auto_save();
                    self.cancel_auto_save();
                    task
                } else {
                    Task::none()
                };
                self.selected = None;
                self.draft = None;
                self.saved_msg = None;
                self.confirm_delete = false;
                self.confirm_stop = false;
                self.versions.closed();
                flush
            }
            Message::SearchChanged(query) => {
                self.search = query;
                Task::none()
            }
            // 单游戏设置页那一族"改一笔就自动保存":十二个分支只差写哪个字段,
            // 整族住在 `update/profile.rs`(见那边的文件头)。
            m @ (Message::AlgoChanged(..)
            | Message::SharpnessChanged(..)
            | Message::InternalWChanged(..)
            | Message::InternalHChanged(..)
            | Message::OutputWChanged(..)
            | Message::OutputHChanged(..)
            | Message::ScaleRatioChanged(..)
            | Message::FullscreenToggled(..)
            | Message::FramerateChanged(..)
            | Message::ExePathChanged(..)
            | Message::LaunchArgsChanged(..)
            | Message::GamescopeArgsChanged(..)) => self.update_profile_edit(m),
            Message::DeleteRequested => {
                self.confirm_delete = true;
                Task::none()
            }
            Message::DeleteCancelled => {
                self.confirm_delete = false;
                Task::none()
            }
            Message::DeleteConfirmed => {
                let Some(game_id) = self.selected.clone() else {
                    return Task::none();
                };
                self.confirm_delete = false;
                let socket = self.daemon_socket.clone();
                self.activity(
                    "删除档案",
                    async move { remove_game(&socket, &game_id).await },
                    Message::Deleted,
                )
            }
            Message::Deleted(result) => match result {
                Ok(()) => {
                    self.selected = None;
                    self.draft = None;
                    self.versions.closed();
                    self.error = None;
                    self.activity(
                        "重新载入游戏库",
                        async { connect_and_load().await },
                        Message::GamesLoaded,
                    )
                }
                Err(e) => self.set_error(e),
            },
            m @ (Message::NewNameChanged(..)
            | Message::NewGameDirChanged(..)
            | Message::NewExeChanged(..)
            | Message::NewGameDirDiskChanged(..)
            | Message::NewGameDirRelativeChanged(..)
            | Message::NewExeDiskChanged(..)
            | Message::NewExeRelativeChanged(..)
            | Message::NewMountInferred(..)
            | Message::CreateRequested
            | Message::CreateFinished(..)
            | Message::MatchExeReady(..)
            | Message::MatchLoaded(..)
            | Message::MatchChoose(..)
            | Message::MatchDecline
            | Message::MatchUndoDecline
            | Message::MatchClearPick
            | Message::CloudPickOpen(..)
            | Message::CloudPickLoaded(..)
            | Message::CloudPickSearch(..)
            | Message::CloudPickChoose(..)
            | Message::CloudPickDismiss
            | Message::CloudPickNewIdentity
            | Message::GamePaired(..)) => self.update_add(m),
            // ── 云同步（处理在 `update::update_sync`） ──
            m @ (Message::SyncStatusLoaded(..)
            | Message::SyncToggleEnabled(..)
            | Message::SyncToggleEnabledSaved(..)
            | Message::SyncField(..)
            | Message::SyncEngineSelected(..)
            | Message::SyncEngineSaved(..)
            | Message::SyncKopiaPasswordChanged(..)
            | Message::SyncSaveKopiaPassword
            | Message::SyncKopiaPasswordSaved(..)
            | Message::SyncSaveSettings
            | Message::SyncSettingsSaved(..)
            | Message::SyncSaveCredentials
            | Message::SyncCredentialsSaved(..)
            | Message::SyncClearCredentials
            | Message::SyncCredentialsCleared(..)
            | Message::SyncTest
            | Message::SyncTested(..)
            | Message::SyncParticipatingToggled(..)
            | Message::SyncParticipatingSaved(..)
            | Message::SyncRebindRequested
            | Message::SyncNewIdentityConfirmed
            | Message::SyncNewIdentityCancelled
            | Message::SyncBindingChanged(..)
            | Message::SyncMasterPasswordChanged(..)
            | Message::SyncUnlock
            | Message::SyncUnlocked(..)
            | Message::SyncSetMasterPassword
            | Message::SyncMasterSaved(..)
            | Message::SyncLockCredentials
            | Message::SyncCredentialsLocked(..)
            | Message::SyncMasterDeleteRequested
            | Message::SyncMasterDeleteCancelled
            | Message::SyncMasterDeleteConfirmed
            | Message::SyncMasterDeleted(..)
            | Message::SyncNow(..)
            | Message::SyncNowDone(..)) => self.update_sync(m),

            // ── 横幅与冲突弹窗（处理在 `update::update_banner`） ──
            // §6.8 A（横幅）与 §6.8 B（冲突弹窗）走**同一条线**：都是"进页面 / 起游戏那一下
            // 看到的三方状态"，只读的那半与要用户拍板的那半。
            m @ (Message::SyncSnapshotRequested(..)
            | Message::SyncSnapshotLoaded(..)
            | Message::SyncConflictResolve(..)
            | Message::SyncConflictAllowed(..)
            | Message::SyncRestoreRequested(..)
            | Message::SyncRestoreCancelled
            | Message::SyncRestoreConfirmed) => self.update_banner(m),

            // ── 「云端存档」的浏览（处理在 `update::update_cloud`） ──
            // 它只读云端、一个字都不改本机配置，所以与上面那一族分开列。
            m @ (Message::CloudRefresh
            | Message::CloudLoaded(..)
            | Message::CloudScan
            | Message::CloudScanned(..)
            | Message::CloudSearch(..)
            | Message::CloudOpenGame(..)
            | Message::CloudBack
            | Message::CloudVersionsLoaded(..)
            | Message::CloudDeleteVersions
            | Message::CloudDeleteIdentity
            | Message::CloudDeleteConfirmed
            | Message::CloudDeleteCancelled
            | Message::CloudDeleted(..)
            | Message::CloudVersionOpened(..)
            | Message::CloudVersionClosed
            | Message::CloudVersionDeleteRequested
            | Message::CloudVersionConfirmed
            | Message::CloudVersionCancelled
            | Message::CloudVersionDeleted(..)) => self.update_cloud(m),

            // ── 单游戏页那一页「这一款的云端存档」（处理在 `update::update_versions`） ──
            // 与上面那一族分开：那一页只读，这一页能覆盖本机存档、还能删云端的东西，是**写**动作。
            m @ (Message::GameVersionsOpened
            | Message::GameVersionsClosed
            | Message::GameVersionsLoaded(..)
            | Message::GameVersionsReplaceVersion(..)
            | Message::GameVersionsDeleteVersion(..)
            | Message::GameVersionsClearVersions
            | Message::GameVersionsForgetIdentity
            | Message::GameVersionsConfirmed
            | Message::GameVersionsCancelled
            | Message::GameVersionsReplaced(..)
            | Message::GameVersionsDeleted(..)) => self.update_versions(m),

            // ── 服务、wine 与单游戏设置（处理在 `update::update_settings`） ──
            // 这一族有哪些变体由 `is_settings_message` 说了算（它就在 handler 旁边，
            // 两处挨着改，不会漏）。
            m if is_settings_message(&m) => self.update_settings(m),
            Message::StopDone(result) => {
                let error_task = match result {
                    Err(e) => self.set_error(e),
                    Ok(()) => Task::none(),
                };
                let socket = self.daemon_socket.clone();
                Task::batch([
                    error_task,
                    Task::perform(
                        async move { load_status(&socket).await },
                        Message::StatusLoaded,
                    ),
                ])
            }
            Message::SyncAskAnswered(choice) => self.sync_ask_answered(choice),
            // 「改配对…」：收起这一问、打开云端清单（挑完接着启动，见 `update/run.rs`）。
            Message::SyncAskPairRequested => self.sync_ask_pair_requested(),
            // 「就绑这一条」：绑上弹窗里显示的那一条，然后启动。
            Message::SyncAskBindFound => self.sync_ask_bind_found(),
            Message::StopCancelled => {
                self.confirm_stop = false;
                Task::none()
            }
            Message::AutoSave(generation) => {
                // 世代对不上 = 这 700ms 里又改过,这一次作废(防抖就是靠它)。
                if generation != self.autosave_generation {
                    return Task::none();
                }
                self.begin_auto_save()
            }
            Message::SaveGroup(scope) => self.begin_save(scope),
            Message::ClearError(generation) => {
                // 世代号对不上说明这 3 秒里又冒出一条新的错误，别把新的那条清掉。
                if generation == self.error_generation {
                    self.error = None;
                }
                Task::none()
            }
            // 后台任务 panic 之后的复位:见 `driver::spawn` 的注释 —— 不把这两条链接上,
            // 界面会"活着但什么都不做"(状态不再刷新,设置改了也永远不会保存)。
            Message::EffectPanicked => {
                tracing::error!("一个后台任务 panic 了，正在复位界面状态并接回轮询");
                self.save_in_flight = None;
                self.saving = false;
                self.service_busy = false;
                self.sync_form.busy = false;
                self.poll_status()
            }
            // 一件耗时的事跑完了：摘掉底部那行"正在…"，把它记成"刚做完什么、花了多久"，
            // 然后照常处理它自己的回包（`App::activity` 是唯一挂它的地方）。
            //
            // ⚠ 世代号对不上就是**上一件事迟到的收尾**：那时什么都不动 —— 用户连着点两下
            // 时，"上一件跑完了"不许把"下一件正在跑"抹成空闲。
            Message::ActivityFinished(generation, label, inner) => {
                if generation == self.activity_generation {
                    let took = self
                        .activity
                        .take()
                        .map(|activity| activity.since.elapsed());
                    self.activity_done = Some(match took {
                        // 耗时必须说出来：用户报过"手动上传要半分钟"，而界面上从来没有一个
                        // 数字，"慢"与"没传"因此分不清。
                        Some(took) if took.as_secs() >= 1 => {
                            format!("{label} 完成，用了 {}", took_label(took))
                        }
                        _ => format!("{label} 完成"),
                    });
                }
                self.update(*inner)
            }
            Message::ProfileSaved(generation, result) => self.profile_saved(generation, result),
            Message::ResetProfile => self.reset_profile(),
            Message::PickerProbed(result) => {
                if let Err(reason) = &result {
                    // 不是"出错了",是这台机器上确实没有 —— 记一条,界面据此灰掉按钮。
                    tracing::info!("系统文件选择框不可用：{reason}");
                }
                self.picker = Some(result);
                Task::none()
            }
            Message::PickPath(target) => {
                // 框已经开着:再来一次只会弹出第二个(用户点的是同一个按钮)。
                if self.picking {
                    return Task::none();
                }
                self.picking = true;
                let request = self.pick_request(target);
                self.activity(
                    "打开文件选择器",
                    async move { crate::picker::pick(request).await },
                    move |result| Message::PathPicked(target, result),
                )
            }
            Message::PathPicked(target, result) => {
                self.picking = false;
                match result {
                    // 用户按了取消:什么都不改(这不是失败)。
                    Ok(None) => Task::none(),
                    Err(e) => {
                        // 这一次没成而已:顶部错误条说一句就够了。
                        // ⚠ **不许**动 `self.picker` —— 它说的是"这台机器上有没有文件对话框",
                        //    是开机探出来的结论。一次失败(何况用户取消)不代表它从此没有了:
                        //    真机上点一次叉号就把「浏览…」永久灰掉了(用户 2026-09-13 报的),
                        //    原因正是这里曾把它写成 `Some(Err(e))`。
                        tracing::warn!("打开文件选择框失败：{e}");
                        self.set_error(format!("打开文件选择框失败：{e}"))
                    }
                    Ok(Some(path)) => self.apply_picked_path(target, &path),
                }
            }
            // 每一族都按变体精确列了，所以走到这里只可能是"新加了一条消息而忘了挂到
            // 某一族"。与其静默丢掉它，不如立刻炸出来 —— 整页渲染测试会先撞上。
            other => unreachable!("没有 handler 认领这条消息: {other:?}"),
        }
    }
}
