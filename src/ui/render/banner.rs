//! 横幅那一块（§6.8 A）：三方状态 → 窗口那一排属性。
//!
//! 唯一推的地方是这里（两个页面共用 `SyncBannerBoard`），所以"进页面显示「正在核对…」、
//! 回包到了填四行"这条顺序只有一份实现。四行**文案**在 `model::banner` 里拼好，这里只搬。

use super::*;

pub(super) fn push_sync_banner(ui: &mut Ui) {
    let banner = &ui.app.sync_banner;
    let board = ui.window.global::<SyncBannerBoard>();

    push_str(board.get_game_id(), &banner.game_id, |v| {
        board.set_game_id(v)
    });
    push_bool(board.get_loading(), banner.loading, |v| {
        board.set_loading(v)
    });
    push_str(
        board.get_error(),
        banner.error.as_deref().unwrap_or_default(),
        |v| board.set_error(v),
    );

    // 四行：回包还没到 / 没问成时一律空 —— 那两态由 `loading` 与 `error` 说了算
    // （页面上分别是「正在核对…」与那句失败原因，绝不画半截旧内容）。
    let (local, cloud, baseline, next, warning, stale) = match &banner.lines {
        Some(lines) => (
            lines.local.as_str(),
            lines.cloud.as_str(),
            lines.baseline.as_str(),
            lines.next.as_str(),
            lines.warning,
            lines.stale_note.as_deref().unwrap_or_default(),
        ),
        None => ("", "", "", "", false, ""),
    };
    push_str(board.get_local_line(), local, |v| board.set_local_line(v));
    push_str(board.get_cloud_line(), cloud, |v| board.set_cloud_line(v));
    push_str(board.get_baseline_line(), baseline, |v| {
        board.set_baseline_line(v)
    });
    push_str(board.get_next_line(), next, |v| board.set_next_line(v));
    push_bool(board.get_warning(), warning, |v| board.set_warning(v));
    push_str(board.get_stale_note(), stale, |v| board.set_stale_note(v));
}
