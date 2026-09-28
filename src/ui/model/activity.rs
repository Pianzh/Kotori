//! 底部那条状态栏:"kotori 现在在干什么"。
//!
//! 用户 2026-09-28(在 Windows 上实测之后):"我建议在最下面,添加 kotori 当前状态,比如正在
//! 拉取云端配置就显示正在拉取云端配置,正在下载,正在同步什么的都写上,方便查错"。
//!
//! ## 为什么状态由界面自己攒,而不是 daemon 推
//!
//! 这一栏要回答的是"我点下去之后它到底动了没有"。而**谁发起的谁最清楚**:界面发一条 RPC,
//! 它就知道自己在等什么(拉索引、上传、扫全库、起进程),不必让 daemon 再开一条推送通道
//! —— 那条路还要额外处理"daemon 中途挂了,栏里却还挂着上次那句话"。
//!
//! ## 为什么不报百分比
//!
//! 那要求每个操作自己分块上报(传到第几块、扫到第几个目录)。这一栏现在的任务是"动没动、
//! 动到哪一步";真要百分比,得先给 daemon 加一条进度通道,那是另一件事。
//!
//! ## 为什么这里的 label 是"动作"而不是整句话
//!
//! 同一件事有两句话:跑的时候是「正在拉取云端索引…」,跑完是「拉取云端索引 完成」。所以
//! 这里只存**动作名**(`拉取云端索引`),两句都由 [`crate::ui::App::activity_bar`] 与
//! `Message::ActivityFinished` 那两处拼 —— 一处一个说法,不会分叉。

use std::time::Instant;

/// 一件**正在跑**的耗时事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::ui) struct Activity {
    /// 动作名,例如"拉取云端索引"(不带"正在",也不带省略号)。
    pub label: String,
    /// 起跑时刻。
    pub since: Instant,
}

impl Activity {
    pub(in crate::ui) fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            since: Instant::now(),
        }
    }
}

/// "这件事花了多久"给用户看的那句话。
///
/// 耗时必须说出来:用户 2026-09-28 报过"退出后没有自动上传,手动上传大概要半分钟" ——
/// 半分钟这件事在界面上从来没有一个数字,于是"慢"与"没传"分不清。
pub(in crate::ui) fn took_label(took: std::time::Duration) -> String {
    let seconds = took.as_secs_f64();
    if seconds < 10.0 {
        format!("{seconds:.1} 秒")
    } else {
        format!("{} 秒", took.as_secs())
    }
}
