//! **退出后自动上传**为什么没跑 —— 一份能问出确切答案的东西。
//!
//! 这条路的入口只有一个：会话的 `Ended` 事件（见 `daemon::spawn_sync_events`），而它在
//! 碰到网络之前要过四道闸门：总开关、这一款的开关、存档位置、引擎程序。
//!
//! ⚠ **为什么单独一个文件、为什么返回"原因"而不是什么都不返回**（用户 2026-09-28 在
//! Windows 上实测："推出后没有自动上传，需要手动上传"，四条早退路里有一条**连日志都
//! 没有**）：
//!
//! * 从前 [`crate::daemon::Daemon::sync_after_game_exit`] 返回 `()`，于是"开关关着"与
//!   "引擎没装"在调用方眼里**一模一样**。手动「立即同步」走的是另一个函数、不受单款
//!   开关限制，所以用户看到的现象正是"手动能传、自动不传"—— 而没有任何一处说得清原因。
//! * 更要命的是**测试写不出来**：单测夹具里没有 kopia/rclone 二进制，那个函数**必然**
//!   在"引擎没装"那里掉头。于是"开关关着时不该上传"这条测试即使写了，也会因为**错误的
//!   理由**变绿 —— 它根本没走到开关那一关。`exit_upload_gate` 是纯函数（只看配置，不碰
//!   引擎、不碰云端），就是为了让这几道闸门**各自**能被单独问出确切答案。
//!
//! 判据与文案都只在这里一份：`sync.status` 报的是同一个 [`SkipReason`]，日志说的也是它。

use super::SyncConfig;
use crate::config::Config;

/// 退出后自动上传没跑的原因。
///
/// 每个变体对应**一件用户能做的事**：这是"为什么界面不传、也不说是哪一步"的那份答案。
/// 顺序按闸门实际问的先后排。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::daemon) enum SkipReason {
    /// 云同步总开关关着（`[sync] enabled = false`）。
    SyncOff,
    /// 这一款的「参与云同步」关着（`GameConfig::sync_enabled`）。
    ///
    /// ⚠ 启动前那一问里选「关掉这一款的同步」会**把它持久化**（`sync_rpc/selfcheck.rs`），
    /// 所以这个状态常常是用户自己点过一次弹窗留下的，界面上看不出因果。
    PerGameOff,
    /// 这一款还没填存档位置。
    NoSavePaths,
    /// 存档位置配了，但**这一刻解析不出来**（盘没插、挂载丢了）。
    LocationsUnresolved,
    /// 配置里根本没有这一款（`Ended` 事件与配置对不上）。
    UnknownGame,
}

impl SkipReason {
    /// 机器可读的那一份，进 `sync.status`。
    pub(in crate::daemon) fn code(self) -> &'static str {
        match self {
            Self::SyncOff => "sync_off",
            Self::PerGameOff => "per_game_off",
            Self::NoSavePaths => "no_save_paths",
            Self::LocationsUnresolved => "locations_unresolved",
            Self::UnknownGame => "unknown_game",
        }
    }
}

/// 一次拒绝：原因是**枚举**（可比对），话是**给人看的**（进日志与界面）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::daemon) struct Refusal {
    pub reason: SkipReason,
    pub detail: String,
}

impl Refusal {
    fn new(reason: SkipReason, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
        }
    }
}

/// 退出后自动上传的结果。
///
/// `Uploaded` / `Skipped` / `Failed` 三分是刻意的：**"没上传"有两种完全不同的含义** ——
/// 用户自己关掉了（不是错），和它压根没传成（是错）。合成一个 `()` 就分不出来了。
#[derive(Debug)]
pub(in crate::daemon) enum ExitUpload {
    /// 传完了。
    Uploaded,
    /// 没传，**而且是有原因的没传**。
    Skipped(Refusal),
    /// 想传，没传成（引擎没装、网络坏、引擎报错）。
    Failed(String),
}

/// 退出后上传在**碰网络之前**要过的那几道闸门；过了就把该用的设置交出去。
///
/// 纯函数：只看 [`Config`]，不碰引擎、不碰云端、不读密钥环。所以每一道闸门都能在单测里
/// 单独问出答案（`tests/exit_upload.rs`），而 [`crate::daemon::Daemon::sync_after_game_exit`]
/// 与 `sync.status` 报的是同一个 [`SkipReason`] —— 界面说的和日志说的因此不会分叉。
pub(in crate::daemon) fn exit_upload_gate(
    game_id: &str,
    config: &Config,
) -> Result<SyncConfig, Refusal> {
    if !config.sync.enabled {
        return Err(Refusal::new(
            SkipReason::SyncOff,
            "云同步总开关关着（设置页「连接与保留」）",
        ));
    }
    // ⚠ 顺序刻意与 [`crate::daemon::Daemon::sync_after_game_exit`] 从前的样子一致：先是
    //   这一款的开关，再是存档位置。换人读日志时，"报了存档位置没填"却"开关其实是关着的"
    //   那才是真的误导。
    let Some(game) = config.games.get(game_id) else {
        return Err(Refusal::new(
            SkipReason::UnknownGame,
            format!("配置中找不到游戏: {game_id}"),
        ));
    };
    // 这一款的开关关着 ⇒ **不自动上传**（手动「立即同步」不受限制）。
    if !game.sync_enabled {
        return Err(Refusal::new(
            SkipReason::PerGameOff,
            format!("《{}》这一款的云同步开关关着", game.name),
        ));
    }
    if game.save_paths.is_empty() {
        return Err(Refusal::new(
            SkipReason::NoSavePaths,
            format!("《{}》还没有配置存档位置", game.name),
        ));
    }
    Ok(config.sync.clone())
}
