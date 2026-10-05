//! 调试面板（`debug-panels` feature）：把**假数据**塞进既有的状态字段，逼那些平时很难
//! 碰到的弹窗与横幅走**原生渲染路径**画出来。
//!
//! ## 这是什么
//!
//! 设置页最下面多一块「调试」，一颗按钮对应一个弹窗/状态（清单就是 [`DebugPanel::ALL`]）：
//!
//! * §6.8 B 的冲突弹窗（本地和云端都改过，三颗按钮，规格见 `PLATFORMS.md`）；
//! * 启动前那一问（疑似同一款云端身份 —— 与冲突弹窗**共用** `SyncAskState` 那副浮层）；
//! * 三个页面共用的那个二次确认弹窗的四种动作（`model::confirm::Confirmation`），
//!   其中「替换」分成 `drifted` 的两半 —— 那是**两种外观**，不是两种动作；
//! * 云同步页那条行内的「删掉主密码凭据文件」确认；
//! * 横幅（§6.8 A）的五种状态：与基线一致 / 云端新 / 本机新 / 冲突 / 本地为空。
//!
//! ## 三条不许破的规矩
//!
//! 1. **只用假数据，不另写界面**。这里一个字都不画：所有东西都塞进既有的字段
//!    （`sync_ask` / `sync_conflict` / `versions` / `sync_form` / `sync_banner` /
//!    `games` / `selected` / `draft`），由既有的 `render` 画出来。想加一颗按钮，就加一个
//!    [`DebugPanel`] 变体、在 [`App::debug_activate`] 里塞状态；**绝不许**在 `.slint`
//!    里新写一个调试专用的弹窗。
//! 2. **不碰用户的真实数据**。假游戏用的是配置里不可能出现的 id（[`GAME_ID`]），而且
//!    调试态下弹窗上那颗确认按钮**一个请求都不发**（拦截在 `App::update` 的入口，
//!    见 [`App::debug_intercept`]）—— 假游戏在 daemon 眼里根本不存在，真发出去只会失败，
//!    还把用户引到一条假路径上。
//! 3. **feature 关掉时这里一行都不存在**：`mod debug;` 本身是 `#[cfg]` 的，而设置页
//!    那一块由 `render` 推一个恒为 `false` 的 `debug-visible` 藏起来（那一块在 `.slint`
//!    里**永远在树里** —— 不赌 Slint 的条件编译，也不会让用户看见）。
//!
//! ## 谁在用
//!
//! 只有 CI 的 `build-linux` job（`.github/workflows/ci.yml`）带这个 feature 编译，
//! 产物挂在 Actions 的 artifact 里给人工看。`verify.yml`（CI 与 Release 共用）与
//! `release.yml` 都不开它 —— 用户从 Release 拿到的包永远没有这一块。

use super::*;
// 推按钮清单那一块要 `Model` 这个 trait 才叫得到 `row_count` / `row_data`
// （照 `render/mod.rs`，那里推表时也只做"先比再写"）。
use slint::{Model, ModelRc, VecModel};

/// 假游戏的 id。
///
/// 刻意用一个**用户配置里不可能有**的名字：它会被临时塞进 `App::games`（启动前那一问与
/// 单游戏页都按 id 在库里认人），所以绝不能与真游戏撞上。
pub(in crate::ui) const GAME_ID: &str = "debug-demo";

/// 假游戏的名字 —— 一眼看得出是调试样张，同时是个像样的中文名（版式要真的量得出来）。
const GAME_NAME: &str = "调试样张《星屑之约》";

/// 假的云端落点（`cloud_key`，只在页面里当一行机器话显示）。
const CLOUD_KEY: &str = "debug-demo-key";

/// 假的版本名。格式与 daemon 那一侧一致（`%Y%m%dT%H%M%SZ`），显示走 `describe_stamp`。
const STAMP_OLD: &str = "20260901T090000Z";
const STAMP_NEW: &str = "20261004T101500Z";

/// 假的基线时刻（毫秒）。几个 fixture 在它上面加减 `DAY_MS`，"超前 / 落后 / 一致"
/// 三种措辞就是这么画出来的。
const BASELINE_MS: i64 = 1_759_570_800_000;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// 设置页那一块「调试」里的一颗按钮（下标就是它在 [`DebugPanel::ALL`] 里的位置）。
///
/// ⚠ 它是 `pub(crate)` 而不是 `pub(in crate::ui)`：`Message::DebugActivate` 那个载荷比它
/// 更宽（`Message` 自己是 `pub(crate)`），否则 rustc 会报"类型比用到它的那个字段更私有"
/// —— 与 `model::MatchReply` / `model::SaveScope` 同一个道理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DebugPanel {
    /// §6.8 B：本地和云端都改过（三颗按钮那一副）。
    Conflict,
    /// 启动前那一问：疑似同一款云端身份（"cloud" 那一副按钮）。
    Ask,
    /// 确认弹窗：替换本机（**本机偏离了基线**，措辞最重的那一版）。
    ReplaceDrifted,
    /// 确认弹窗：替换本机（本机没动过）。
    ReplaceClean,
    /// 确认弹窗：删掉云端某一版。
    DeleteVersion,
    /// 确认弹窗：清空这一款的云端存档（身份留着）。
    ClearVersions,
    /// 确认弹窗：把这一款从云端抹掉（身份连存档）。
    ForgetIdentity,
    /// 行内确认：删掉主密码凭据文件。
    MasterDelete,
    /// 横幅（§6.8 A）：三边一致。
    BannerInSync,
    /// 横幅：云端比基线新一版（本机落后两天）。
    BannerCloudNewer,
    /// 横幅：本机超前两天（云端还是基线那一版）。
    BannerLocalNewer,
    /// 横幅：两边都改过（判定那一格是 `ask`，用警示色）。
    BannerConflict,
    /// 横幅：本机一个存档文件都没有。
    BannerLocalEmpty,
}

/// 按下这一颗按钮之后窗口该跳到哪儿。
///
/// 冲突弹窗与启动前那一问是**窗口级浮层**（`widgets/sync-ask.slint`），在哪儿都看得见；
/// 其余几个住在某一页里，原地多半什么都看不到 —— 不跳过去的话，那颗按钮看着就像没反应。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::ui) enum Place {
    /// 原地就能看见。
    Here,
    /// 单游戏设置页（`tab == 0` 且 `game-open`）。
    GamePage,
    /// 云同步页（`tab == 3`）。
    SyncPage,
}

impl DebugPanel {
    /// 清单。**顺序就是按钮在界面上的顺序**，下标也是它（[`DebugPanel::at`] 按位置取）。
    ///
    /// 标签在这里给、不在 `.slint` 里："按钮上的字"与"它激活的是哪一件事"必须同一个来源，
    /// 否则改一处就会让另一颗按钮去干别的事 —— 那种错编译期看不出来，界面上也不报错。
    pub(in crate::ui) const ALL: &[(DebugPanel, &str)] = &[
        (Self::Conflict, "冲突弹窗：本地和云端都改过"),
        (Self::Ask, "启动前那一问：疑似同一款云端身份"),
        (Self::ReplaceDrifted, "确认弹窗：替换（本机偏离基线）"),
        (Self::ReplaceClean, "确认弹窗：替换（本机没动过）"),
        (Self::DeleteVersion, "确认弹窗：删掉云端某一版"),
        (Self::ClearVersions, "确认弹窗：清空这一款的云端存档"),
        (Self::ForgetIdentity, "确认弹窗：把这一款从云端抹掉"),
        (Self::MasterDelete, "确认弹窗：删掉主密码凭据文件"),
        (Self::BannerInSync, "横幅：与基线一致"),
        (Self::BannerCloudNewer, "横幅：云端新 1 版（本机落后）"),
        (Self::BannerLocalNewer, "横幅：本机超前"),
        (Self::BannerConflict, "横幅：冲突（要你选一次）"),
        (Self::BannerLocalEmpty, "横幅：本地为空"),
    ];

    /// 界面上第 `index` 颗按钮是哪一件事。越界 ⇒ `None`（照 `wire.rs` 里别的下标映射，
    /// 认不出来就当没点，绝不兜底成第一颗）。
    pub(in crate::ui) fn at(index: i32) -> Option<Self> {
        let index = usize::try_from(index).ok()?;
        Self::ALL.get(index).map(|entry| entry.0)
    }

    /// 这一颗是不是**弹窗**（调试态下它上面的按钮一律被吞掉，见 [`App::debug_intercept`]）。
    fn is_dialog(self) -> bool {
        self.confirmation().is_some()
            || matches!(self, Self::Conflict | Self::Ask | Self::MasterDelete)
    }

    /// 这一颗要问的那件事（`model::confirm` 的措辞）；横幅与另外两种弹窗给 `None`。
    ///
    /// 「替换」的两半用同一个版本名、只差 `drifted` —— 它们要办的事一模一样，变的只是
    /// 那一句话（见 `model::confirm::Confirmation::Replace` 的注释）。
    fn confirmation(self) -> Option<Confirmation> {
        Some(match self {
            Self::ReplaceDrifted => Confirmation::Replace {
                version: STAMP_NEW.to_string(),
                drifted: true,
            },
            Self::ReplaceClean => Confirmation::Replace {
                version: STAMP_NEW.to_string(),
                drifted: false,
            },
            Self::DeleteVersion => Confirmation::DeleteVersion {
                version: STAMP_OLD.to_string(),
            },
            Self::ClearVersions => Confirmation::ClearVersions,
            Self::ForgetIdentity => Confirmation::ForgetIdentity,
            _ => return None,
        })
    }

    /// 这一颗按下去之后窗口跳到哪儿（见 [`Place`]）。
    pub(in crate::ui) fn place(self) -> Place {
        match self {
            // 这两问是窗口级浮层：留在原地就能看见（也就是"点一下立刻有东西"）。
            Self::Conflict | Self::Ask => Place::Here,
            // 那条行内确认长在云同步页的「凭据」那一组里。
            Self::MasterDelete => Place::SyncPage,
            // 其余都长在单游戏设置页（四个确认弹窗在它上面盖着的那一页「这一款的云端
            // 存档」里，横幅在它的「云存档」组与那一页上）。
            _ => Place::GamePage,
        }
    }

    /// 这一颗配的那份假回包。
    ///
    /// **每一颗都给一份**（点弹窗那几颗给"冲突"那一态）：跳过去之后看到的横幅与弹窗说的
    /// 是同一件事，不至于一半是真的一半是假的。
    fn snapshot(self) -> SyncSnapshot {
        match self {
            Self::BannerInSync => snapshot_in_sync(),
            Self::BannerCloudNewer => snapshot_cloud_newer(),
            Self::BannerLocalNewer => snapshot_local_newer(),
            Self::BannerLocalEmpty => snapshot_local_empty(),
            // 冲突那一态，以及点弹窗那几颗时的陪衬。
            _ => snapshot_conflict(),
        }
    }
}

impl App {
    /// 一颗调试按钮按下去：把假数据塞进**既有的**状态字段（见模块文件头）。
    ///
    /// 这里只碰内存里的状态，**一个请求都不发** —— 真发出去的那些由
    /// [`App::debug_intercept`] 拦在入口。
    pub(in crate::ui) fn debug_activate(&mut self, panel: DebugPanel) -> Task<Message> {
        // 先收掉上一颗留下的东西：两颗弹窗叠在一起就没法看了。
        self.debug_reset();
        self.debug_prepare_game();
        self.debug_panel = Some(panel);

        // 横幅：每一颗都给一份假回包。走的是 `sync.snapshot` 回包**同一条**路
        // （`requested` → `loaded`），所以四行文案还是 `model::banner` 拼的。
        self.sync_banner.requested(GAME_ID);
        self.sync_banner.loaded(panel.snapshot());

        match panel {
            DebugPanel::Conflict => {
                self.sync_ask = Some(GAME_ID.to_string());
                self.sync_conflict = Some("both_changed".to_string());
            }
            DebugPanel::Ask => {
                self.sync_ask = Some(GAME_ID.to_string());
                self.sync_ask_cloud = Some(SyncAskCloud {
                    cloud_id: "8f2c1234-0000-0000-0000-000000000000".to_string(),
                    cloud_key: CLOUD_KEY.to_string(),
                    name: GAME_NAME.to_string(),
                    versions: 2,
                    latest: STAMP_NEW.to_string(),
                    size: 4096,
                });
            }
            DebugPanel::MasterDelete => {
                // 那条行内确认只在 `store-kind == 1`（主密码凭据文件）时才画
                // （`slint/pages/sync-credentials.slint`），这是这一颗**唯一**的前置条件。
                // ⚠ 它会盖住用户真实的 `sync.status`：下一次进「云同步 / 设置」页时那次
                // 重读（`TabChanged`）就回到真实值。
                self.sync_status = Some(fake_sync_status());
                self.sync_form.confirm_master_delete = true;
            }
            // 四个确认弹窗：它们住在单游戏页盖着的那一页「这一款的云端存档」里，所以先摆出
            // "云端有几版"（`loaded` 那条路与真回包同一条），再记下"要问哪一件事"。
            _ => {
                if let Some(action) = panel.confirmation() {
                    self.versions.opened(GAME_ID, CLOUD_KEY);
                    self.versions.loaded(vec![
                        version_row(STAMP_NEW, 4096),
                        version_row(STAMP_OLD, 3072),
                    ]);
                    self.versions.requested(action);
                }
            }
        }
        Task::none()
    }

    /// 调试态下**吞掉**弹窗上那一颗按钮（这一族唯一的拦截点）。
    ///
    /// 返回 `Some` = 这条消息到此为止（只把假态与弹窗收掉，一个请求都不发）；`None` =
    /// 不是调试态、或者不是弹窗上的按钮，照常走后面的处理。
    ///
    /// ⚠ 为什么拦在 `App::update` 的**入口**、而不是在每一族 handler 里各加一条：那一族
    /// 分支有十几处（三个页面的确认弹窗、冲突弹窗三颗、启动前那一问四颗、主密码删除），
    /// 漏掉任何一处都只会在"按下确认"那一刻才暴露 —— 而那时人已经在看假弹窗了。入口这
    /// 一处只有一条规则：**调试态 + 弹窗按钮 ⇒ 收掉假态、什么都不做**。
    pub(in crate::ui) fn debug_intercept(&mut self, message: &Message) -> Option<Task<Message>> {
        let panel = self.debug_panel?;
        if !panel.is_dialog() || !is_dialog_button(message) {
            return None;
        }
        tracing::info!("调试面板：{message:?} 不走后端（假弹窗，见 ui::debug）");
        self.debug_reset();
        Some(Task::none())
    }

    /// 把假游戏摆进库里、选中它、给它一份草稿（启动前那一问与单游戏页都按 id 认人）。
    ///
    /// ⚠ 这里**不发** `Message::GameSelected`：那会顺手打一趟 `sync.snapshot`
    /// （见 `App::banner_requested`），而横幅那份假数据是我们自己塞的 —— 真打一趟只会把
    /// 它盖成"这一款在 daemon 眼里不存在"。库里那一条也一样是临时的：下一次刷新游戏库
    /// （`GamesLoaded`）就没了。
    fn debug_prepare_game(&mut self) {
        let game = debug_game();
        match self
            .games
            .iter_mut()
            .find(|existing| existing.id == GAME_ID)
        {
            Some(existing) => *existing = game.clone(),
            None => self.games.push(game.clone()),
        }
        self.selected = Some(GAME_ID.to_string());
        self.draft = Some(Draft::from_game(&game));
    }

    /// 收掉调试态与它摆出来的那些假东西。
    ///
    /// 不管清的是哪一颗（一次性清干净最简单：调试态下不会同时存在两个真的待确认动作），
    /// 顺手把三个页面的确认弹窗都收掉 —— 假弹窗被按过之后，那一页本来就该回到原样。
    fn debug_reset(&mut self) {
        self.debug_panel = None;
        self.sync_ask = None;
        self.sync_ask_hidden = false;
        self.sync_ask_cloud = None;
        self.sync_ask_trouble = None;
        self.sync_conflict = None;
        self.sync_restore_pending = None;
        self.sync_form.confirm_master_delete = false;
        self.cloud.cancelled();
        self.cloud_version.closed();
        self.versions.cancelled();
        self.versions.closed();
    }
}

/// 弹窗上那一类"用户按了按钮"的消息。
///
/// 取消类本来就不发请求，也一并列上：规则只有一条（弹窗上的按钮一律吞掉），比"这个吞
/// 那个不吞"好记，也不会漏 —— 漏一个就意味着那颗按钮会去打后端。
fn is_dialog_button(message: &Message) -> bool {
    matches!(
        message,
        Message::SyncConflictResolve(_)
            | Message::SyncAskAnswered(_)
            | Message::SyncAskBindFound
            | Message::SyncAskPairRequested
            | Message::GameVersionsConfirmed
            | Message::GameVersionsCancelled
            | Message::CloudDeleteConfirmed
            | Message::CloudDeleteCancelled
            | Message::CloudVersionConfirmed
            | Message::CloudVersionCancelled
            | Message::SyncMasterDeleteConfirmed
            | Message::SyncMasterDeleteCancelled
            | Message::SyncRestoreConfirmed
            | Message::SyncRestoreCancelled
    )
}

/// 设置页那一块「调试」：可见性 + 按钮清单。由 `render` 每帧推一次。
///
/// 清单整表只在真的不一样时才写（照 `render::settings` 的规矩）：每帧重建一个新模型会
/// 让悬停状态抖一下，而这种"每帧都推"的属性最容易犯这个错。
pub(super) fn push_debug_panel(ui: &mut Ui) {
    let w = &ui.window;
    if !w.get_debug_visible() {
        w.set_debug_visible(true);
    }
    let wanted: Vec<DebugPanelItem> = DebugPanel::ALL
        .iter()
        .enumerate()
        .map(|(index, (_, label))| DebugPanelItem {
            index: index as i32,
            label: (*label).into(),
        })
        .collect();
    let model = w.get_debug_panels();
    let same = model.row_count() == wanted.len()
        && wanted
            .iter()
            .enumerate()
            .all(|(index, row)| model.row_data(index).as_ref() == Some(row));
    if !same {
        w.set_debug_panels(ModelRc::new(VecModel::from(wanted)));
    }
}

/// 假游戏：像样的中文名 + 一个不在任何用户配置里的 id（[`GAME_ID`]）。
fn debug_game() -> UiGame {
    UiGame {
        id: GAME_ID.to_string(),
        name: GAME_NAME.to_string(),
        game_dir: "/调试样张/星屑之约".to_string(),
        exe: "/调试样张/星屑之约/game.exe".to_string(),
        game_dir_mount: MountRef::default(),
        exe_mount: MountRef::default(),
        launch_args: Vec::new(),
        save_paths: Vec::new(),
        auto_watch: false,
        direct_launch: false,
        sync_enabled: true,
        process_name: String::new(),
        profile_name: "默认".to_string(),
        algo: "Fsr".to_string(),
        sharpness: 2,
        internal: (Some(1280), Some(720)),
        output: (Some(1920), Some(1080)),
        scale_ratio: None,
        fullscreen: true,
        framerate: None,
        gamescope_args: Vec::new(),
    }
}

/// 「删掉主密码凭据文件」那一颗要的假状态（见 `debug_activate` 里的说明）。
fn fake_sync_status() -> SyncStatus {
    SyncStatus {
        store_kind: "encrypted-file".to_string(),
        store_locked: false,
        master_file: "/home/user/.config/kotori/secrets.json".to_string(),
        keyring: "主密码加密文件（已解锁）".to_string(),
        ..SyncStatus::default()
    }
}

/// 云端的一版（`CloudVersionRow`：名字、字节数、给人看的时间在 `label()` 里现算）。
fn version_row(name: &str, size: u64) -> CloudVersionRow {
    CloudVersionRow {
        name: name.to_string(),
        size,
        time: String::new(),
    }
}

/// 64 个小写十六进制的假内容值（§6.1(a) 的形状）。
fn digest(byte: u8) -> String {
    format!("{byte:02x}").repeat(32)
}

/// 一份"三边一致"的假回包（§6.8 A 第一条文案）。
fn snapshot_in_sync() -> SyncSnapshot {
    SyncSnapshot {
        game_id: GAME_ID.to_string(),
        local: SnapshotLocal {
            mtime_ms: BASELINE_MS,
            empty: false,
            digest: Some(digest(0xaa)),
        },
        cloud: SnapshotCloud {
            state: "known".to_string(),
            stamp: Some(STAMP_NEW.to_string()),
            digest: Some(digest(0xaa)),
            key: CLOUD_KEY.to_string(),
        },
        baseline: Some(SnapshotBaseline {
            stamp: STAMP_NEW.to_string(),
            digest: digest(0xaa),
            mtime_ms: BASELINE_MS,
        }),
        decision: "nothing".to_string(),
        index: SnapshotIndex::default(),
        problem: None,
    }
}

/// 云端比基线新一版、本机落后两天 —— "落后 N 天"与"比基线新 1 版"两种措辞一起画出来。
fn snapshot_cloud_newer() -> SyncSnapshot {
    let mut snapshot = snapshot_in_sync();
    snapshot.local.mtime_ms = BASELINE_MS - 2 * DAY_MS;
    snapshot.local.digest = Some(digest(0xbb));
    snapshot.cloud.digest = Some(digest(0xcc));
    snapshot.decision = "pull".to_string();
    snapshot
}

/// 本机超前两天、云端还是基线那一版（§6.6 闸门 b：这一局退出会上传）。
fn snapshot_local_newer() -> SyncSnapshot {
    let mut snapshot = snapshot_in_sync();
    snapshot.local.mtime_ms = BASELINE_MS + 2 * DAY_MS;
    snapshot.local.digest = Some(digest(0xdd));
    snapshot.decision = "upload_later".to_string();
    snapshot
}

/// 两边都改过（判定那一格是 `ask`）：横幅那一行用警示色，接下来写"启动时会问你"。
fn snapshot_conflict() -> SyncSnapshot {
    let mut snapshot = snapshot_in_sync();
    snapshot.local.mtime_ms = BASELINE_MS + DAY_MS;
    snapshot.local.digest = Some(digest(0xdd));
    snapshot.cloud.digest = Some(digest(0xcc));
    snapshot.decision = "ask".to_string();
    snapshot
}

/// 本机一个存档文件都没有（§6.11 第 3 条要横幅写明的那一种）。
fn snapshot_local_empty() -> SyncSnapshot {
    let mut snapshot = snapshot_in_sync();
    snapshot.local = SnapshotLocal {
        mtime_ms: 0,
        empty: true,
        digest: None,
    };
    snapshot.decision = "no_sync".to_string();
    snapshot
}

#[cfg(test)]
// ⚠ 测试住在同目录的另一个文件里（照 `model/banner.rs` 与 `app.rs` 的习惯）：这个文件
// 连着测试一起数会往 500 行的软线（AGENTS.md）上贴。
#[path = "debug/tests.rs"]
mod tests;
