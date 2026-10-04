//! 一次同步操作的汇报类型：每个存档位置一条，整个游戏一条。
//!
//! 单独成文件，是因为 UI、CLI、daemon 的日志都直接照着这些字段说话——
//! "哪个位置、在哪、做了什么、为什么"，每一条都要能如实回答，`ok` 才敢说出口。

use serde::Serialize;

/// What happened to one save location.
#[derive(Debug, Clone, Serialize)]
pub struct LocationOutcome {
    /// The location as configured (portable form).
    pub configured: String,
    /// Where it resolved to on this machine.
    pub local: String,
    /// `uploaded` / `pulled` / `kept` / `restored` / `skipped` / `failed`.
    pub action: &'static str,
    /// One line explaining the action, safe to show to the user.
    pub detail: String,
}

impl LocationOutcome {
    pub(super) fn new(
        target: &crate::sync::SaveTarget,
        action: &'static str,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            configured: target.configured.clone(),
            local: target.local.to_string_lossy().to_string(),
            action,
            detail: detail.into(),
        }
    }

    pub fn ok(&self) -> bool {
        self.action != "failed"
    }
}

/// Result of one operation on one game.
#[derive(Debug, Clone, Serialize)]
pub struct GameOutcome {
    pub game_id: String,
    pub name: String,
    pub ok: bool,
    pub locations: Vec<LocationOutcome>,
    /// 这一趟上云的那一版的**内容值**（只有上传成功才有，见 `archive::digest`）。
    ///
    /// 它是"索引里记的 `latest_digest` 该是什么"的唯一来源：调用方（上传成功之后的
    /// 索引更新）拿它写下去，于是索引说的就是**真的上去了的那一版**，而不是"现在再
    /// 去本机算一遍"（那是第二次读盘，而且期间存档可能又变了）。
    ///
    /// 不进 JSON：界面与 CLI 都不需要它（判定那套还没接上来）。
    #[serde(skip)]
    pub digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl GameOutcome {
    pub fn failed(game_id: &str, name: &str, error: impl Into<String>) -> Self {
        Self {
            game_id: game_id.to_string(),
            name: name.to_string(),
            ok: false,
            locations: Vec::new(),
            digest: None,
            error: Some(error.into()),
        }
    }

    pub(super) fn from_locations(
        game_id: &str,
        name: &str,
        locations: Vec<LocationOutcome>,
    ) -> Self {
        let error = locations
            .iter()
            .find(|o| !o.ok())
            .map(|o| format!("{}: {}", o.configured, o.detail));
        Self {
            game_id: game_id.to_string(),
            name: name.to_string(),
            ok: error.is_none(),
            locations,
            digest: None,
            error,
        }
    }

    /// 同一个结果，但带上"这一版的内容值"（上传成功后由 [`super::Runner::upload`] 填）。
    pub(super) fn with_digest(mut self, digest: Option<String>) -> Self {
        self.digest = digest;
        self
    }
}
