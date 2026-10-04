//! 启动前同步的**结论怎么报出去**：回包那几栏、日志与人话文案。
//!
//! 从 `launch_sync.rs` 拆出来（AGENTS.md：单个源码文件尽量 500 行内）—— 那边是"三方比较
//! 怎么走"的流程，这边是"走完之后怎么告诉界面与日志"。**判据一份、文案一份**：`sync_pull`
//! 回包、`tracing` 那一行、以及第 9/10 步要显示的弹窗文案，说的都是这里的字符串 ——
//! 分叉了就意味着"界面上写的"与"日志里说的"不是一回事（用户 2026-09-28 那次排查的根因）。
//!
//! ⚠ 这一层**只把"要问"如实带出去**：弹窗是第 9/10 步的事，这里既不弹、也不替用户决定
//! （PLATFORMS.md §6.8 B：那一问**不阻塞启动**，所以也不能像配对那一问那样提前 return
//! 一个"要决定"给客户端）。

use serde_json::{Value, json};

use crate::sync::decision::{ConflictKind, Decision};
use crate::sync::runner::GameOutcome;

/// 这一次启动前同步的结论 —— `game.launch` 的回包（界面第 9/10 步要读它）。
///
/// ⚠ 这一层**只把"要问"如实带出去**（[`Self::ask`]）：弹窗是第 9/10 步的事，这里既
/// 不弹、也不替用户决定（§6.8 B：那一问**不阻塞启动**，所以也不能像配对那一问那样
/// 提前 return 一个"要决定"给客户端）。
pub(super) struct LaunchReport {
    /// 机器可读的判定名（§6.3 的七个变体；`confirm` 是第 7 格那一态）。
    pub(super) decision: &'static str,
    /// 本次是否已对上账：闸门 b 立不立那块牌子就看它。
    pub(super) settled: bool,
    /// 给人看的一句话。
    pub(super) detail: String,
    /// 要问用户的那一类。
    pub(super) ask: Option<ConflictKind>,
    /// 真去取回过就带上那一次的结果（成功与失败都有）。
    pub(super) outcome: Option<GameOutcome>,
}

impl LaunchReport {
    /// 对上账了（该做的都做成了）。
    pub(super) fn settled(decision: &'static str, detail: impl Into<String>) -> Self {
        Self {
            decision,
            settled: true,
            detail: detail.into(),
            ask: None,
            outcome: None,
        }
    }

    /// 没对上账：本次不下载、也不自动上传。
    pub(super) fn unsettled(decision: &'static str, detail: impl Into<String>) -> Self {
        Self {
            decision,
            settled: false,
            detail: detail.into(),
            ask: None,
            outcome: None,
        }
    }

    /// 要问用户（判定表第 3/5/10 格）。
    pub(super) fn asking(kind: ConflictKind) -> Self {
        Self {
            decision: "ask",
            settled: false,
            detail: ask_detail(kind),
            ask: Some(kind),
            outcome: None,
        }
    }

    /// 取回那条路的结果（铺成功 / 没铺成都算"真去取过"）。
    pub(super) fn pulled(outcome: GameOutcome, settled: bool, detail: String) -> Self {
        Self {
            decision: "pull",
            settled,
            detail,
            ask: None,
            outcome: Some(outcome),
        }
    }

    pub(super) fn json(self, game_id: &str) -> Value {
        let settled = self.settled;
        let detail = self.detail;
        // `ok` / `error` 与从前的回包同名同义（"这一次启动前同步这件事成没成"）：
        // 老的读法不会因为底下多了判定这一层而突然变成另一个意思。
        let error = if settled {
            Value::Null
        } else {
            Value::String(detail.clone())
        };
        json!({
            "game_id": game_id,
            "ok": settled,
            "settled": settled,
            "decision": self.decision,
            "detail": detail,
            "error": error,
            "ask": self.ask.map(|kind| json!({ "kind": conflict_code(kind) })),
            "outcome": self.outcome,
        })
    }
}

/// 判定名（机器可读的那一份，进回包与日志）。
pub(super) fn decision_code(decision: &Decision) -> &'static str {
    match decision {
        Decision::Nothing => "nothing",
        Decision::Pull { .. } => "pull",
        Decision::Confirm { .. } => "confirm",
        Decision::UploadLater => "upload_later",
        Decision::AdoptBaseline { .. } => "adopt_baseline",
        Decision::Ask { .. } => "ask",
        Decision::NoSync { .. } => "no_sync",
    }
}

/// 要问的是哪一类冲突（界面据此挑文案）。
pub(super) fn conflict_code(kind: ConflictKind) -> &'static str {
    match kind {
        ConflictKind::NoBaseline => "no_baseline",
        ConflictKind::UnknownDigest => "unknown_digest",
        ConflictKind::BothChanged => "both_changed",
    }
}

/// 给人看的那一句"为什么问你"。
pub(super) fn ask_detail(kind: ConflictKind) -> String {
    match kind {
        ConflictKind::NoBaseline => {
            "还没有基线，本机与云端的内容不一样 —— 谁新谁旧无从判断，需要你选一次".to_string()
        }
        ConflictKind::UnknownDigest => {
            "云端最新那一版的内容值不知道（老索引）—— 不能拿版本名或时间戳猜，需要你选一次"
                .to_string()
        }
        ConflictKind::BothChanged => "本地和云端都改过 —— 需要你选一次".to_string(),
    }
}
