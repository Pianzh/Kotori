//! 云端那一侧现在是什么样：索引里"本机这一款"的那一条 → 判定要的 `CloudState`。
//!
//! 从 `launch_sync.rs` 拆出来（AGENTS.md：单个源码文件尽量 500 行内）—— 那边是"三方怎么
//! 比、比完干什么"的流程，这边只管**读**：本地缓存那份索引（零网络），或者按 §6.5 第 1 步
//! 强制去云端重读一次。
//!
//! ⚠ 这一层的全部价值是"**绝不猜**"：读不到就是 [`CloudNow::Unknown`]，索引里没有那一条
//! 就是 [`CloudNow::None`]，内容值缺了就带着 `None` 交出去（判定表第 1/2/3 格各自接住）。

use super::Daemon;
use crate::sync::decision::CloudState;

/// 索引里"本机这一款"现在的样子 —— 判定要的 [`CloudState`] 借的是字符串，得有个主人。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CloudNow {
    /// 索引**读不到**（离线、桶名不对、索引坏了、格式不认识）：这一刻不知道云端有没有
    /// 这一款，绝不猜（判定表第 1 格）。
    Unknown,
    /// 云端没有这一款：索引里没有本机认账的那一条（或者桶里压根还没有索引），
    /// 或者它一版都没有（第 2 格）。
    None,
    /// 云端有：版本名 + 那一版的内容值（`None` = 索引自己也不知道，第 3 格 ⇒ 要问）。
    Known {
        stamp: String,
        digest: Option<String>,
    },
}

impl CloudNow {
    pub(super) fn state(&self) -> CloudState<'_> {
        match self {
            Self::Unknown => CloudState::Unknown,
            Self::None => CloudState::None,
            Self::Known { stamp, digest } => CloudState::Known {
                stamp,
                digest: digest.as_deref(),
            },
        }
    }

    /// 索引里那一版的内容值（决定覆盖之后要拿它去核对包，§6.5 第 3 步）。
    pub(super) fn digest(&self) -> Option<&str> {
        match self {
            Self::Known { digest, .. } => digest.as_deref(),
            _ => None,
        }
    }
}

impl Daemon {
    /// 索引里"本机这一款"现在的样子。
    ///
    /// `refresh = true` 是**强制去云端重读一次**（绕开本机缓存，§6.5 第 1 步）；
    /// 平时走本机缓存那份（零网络，用户 2026-09-24 定的"其他所有查询都只查本地索引"）。
    pub(super) async fn cloud_now(&self, game_id: &str, refresh: bool) -> CloudNow {
        let view = match self.cloud_index_view(refresh).await {
            Ok(view) => view,
            Err(error) => {
                tracing::warn!("{game_id}: 读不到云端索引，本次不比较也不自动上传: {error}");
                return CloudNow::Unknown;
            }
        };
        // 桶里**还没有**索引 —— 这不是"读不到"，而是一个明确的事实：还没有任何一台机器
        // 往这个桶里传过东西（`Runner::read_index` 把这两种情况分得很清楚：`Ok(None)`
        // 是"从没写过"，`Err` 才是"读不动/读不懂"）。索引是**每次上传都写**的，所以
        // "没有索引"就等于"云端没有这一款"（判定表第 2 格 ⇒ 结束后上传，第一次上传
        // 因此照旧自动发生）。
        //
        // ⚠ 另一条读法（更保守：当成 [`CloudNow::Unknown`]，于是第一次上传必须手动点一次
        // 「立即同步」才有索引）问过用户了（2026-10-05）：按这一条实现（`Ok(None)` 与
        // `Err` 本来就是两件事，第 1 格写的是"索引**读不到**"）。
        let Some(index) = view.index else {
            tracing::debug!("{game_id}: 桶里还没有云端索引（当作云端没有这一款）");
            return CloudNow::None;
        };
        // 本机认账的那一条 = 配置里记着的 `cloud_id`。**还没认领过身份就没有"我那一款"**：
        // 按落点去认别人的词条，会让判定拿别人的内容值去比 —— 那正是"静默损坏存档"那条路
        // （用户答过"以后新建一条"之后就是这个状态，那时该走的是"结束后上传"）。
        let Ok(Some(cloud_id)) = self.cloud_id_of(game_id).await else {
            return CloudNow::None;
        };
        let Some(entry) = index
            .games
            .iter()
            .find(|game| !game.gone && game.identity.cloud_id == cloud_id)
        else {
            return CloudNow::None;
        };
        match &entry.latest {
            Some(stamp) => CloudNow::Known {
                stamp: stamp.clone(),
                digest: entry.latest_digest.clone(),
            },
            // 这一条在，但一版都没有（云端认得它、只是没有存档）：判定表第 2 格。
            None => CloudNow::None,
        }
    }
}
