//! Plain data shared by the UI. The message/page enums live in
//! [`super::message`]; everything a daemon answer is turned into lives here.
//!
//! 按「这块状态归谁」拆成子模块:本机环境(`environment`)、游戏与它的草稿
//! (`game`)、添加页的云端匹配(`add`)、云同步(`sync`)、云端存档的浏览(`cloud`)、
//! 会话与连接参数(`session`)、单游戏页那一页云端版本(`versions`)、云端一个存档的管理页
//! (`cloud_version`)、破坏性动作的弹窗措辞(`confirm`)。每个子模块顶部写清它负责什么、
//! 为什么和邻居分开。
//!
//! 本文件只是门面:本来 `pub` 的六个类型在这里重新 `pub use`,其余按原来的
//! `pub(super)` 可见性重导出 —— `crate::ui::model::X` 这些路径照旧可用,调用方
//! 一行都不用改。

mod activity;
mod add;
mod banner;
mod cloud;
mod cloud_label;
mod cloud_pick;
mod cloud_version;
mod confirm;
mod environment;
// `FormMsg` 是 `Message` 的载荷之一（几处 `Result<FormMsg, String>`），同样要放宽到
// `pub(crate)`（同上面的 `MatchReply`）。
mod form_msg;
mod game;
mod picker;
mod session;
mod sync;
mod versions;

pub use environment::{ConfigSource, WineStatus};
pub use game::{SavePathDraft, UiGame};
pub use picker::ProcessRow;
pub use session::SessionInfo;
pub use sync::{SyncGameRow, SyncStatus};

pub(super) use activity::{Activity, took_label};
pub(super) use add::{AddMatch, MATCH_DEBOUNCE, MatchPhase};
// 横幅（§6.8 A）：状态 + 四行文案。`SyncSnapshot` 是 `Message::SyncSnapshotLoaded` 的载荷
// 之一（`Message` 自己是 `pub(crate)`），所以它跟着放宽 —— 同下面的 `MatchReply`。
pub(super) use banner::{
    SnapshotIndex, SyncBanner, conflict_message, restore_confirmation, restore_drifted,
};
// 解析那一层要自己拼这几块（`parse/banner.rs` 从 `ui::*` 拿不到子模块里的名字，
// 它们只对 `crate::ui` 可见）。
pub(crate) use banner::SyncSnapshot;
pub(in crate::ui) use banner::{SnapshotBaseline, SnapshotCloud, SnapshotLocal};
// `MatchReply` 是 `Message` 的载荷之一（`Message` 自己是 `pub(crate)`），所以它得跟着放宽到
// `pub(crate)`，否则 clippy 报"类型比用到它的那个字段更私有"（同下面的 `SaveScope`）。
pub(crate) use add::MatchReply;
pub(super) use cloud::{CloudGameRow, CloudListReply, CloudState, CloudVersionRow};
pub(super) use cloud_version::CloudVersionState;
// `Confirmation` 是三个页面共用的弹窗措辞；它只活在 `crate::ui` 里。
pub(super) use cloud_label::{identity_label, identity_summary};
pub(super) use cloud_pick::{CloudPick, CloudPickPurpose};
pub(in crate::ui) use confirm::Confirmation;
pub(super) use environment::{EnvCheck, Environment};
pub(crate) use form_msg::FormMsg;
pub(super) use game::{AUTOSAVE_DEBOUNCE, Draft, MountRef, SAVE_PATH_KINDS, SaveAttempt};
// `SaveScope` 是 `Message` 的载荷之一（`Message` 自己是 `pub(crate)`），所以它得跟着
// 放宽到 `pub(crate)`，否则 clippy 报"类型比用到它的那个字段更私有"。
pub(crate) use game::SaveScope;
pub(super) use picker::ProcessPicker;
pub(super) use session::{MAX_AUTO_RETRIES, STATUS_POLL};
pub(super) use sync::{CredentialStore, SyncAskCloud, SyncForm, engine_switched_note};
pub(super) use versions::VersionsState;
