//! 横幅：三方的"现在长什么样"（PLATFORMS.md §6.8 A）—— 四行文案与那份状态。
//!
//! 两处页面进页面时**异步**问 daemon 一次（`sync.snapshot`），算的过程中显示「正在核对…」，
//! 算完再填这四行。所以这里有两件东西：
//!
//! * [`SyncSnapshot`]：daemon 回的那份**事实**（本机 / 云端 / 基线 / 判定那一格）；
//! * [`banner_lines`]：把事实拼成 §6.8 A 那四行 —— **纯函数**，所以四条文案各自测得出来
//!   （§6.9 的 UI 族要的就是这个）。
//!
//! ## 四行（§6.8 A 逐字对齐）
//!
//! ```text
//! 本地  ⟨超前 N 天 | 落后 N 天 | 与基线一致 | 还没有基线 | 本地为空⟩  <digest 前 8 位>  <本地时间>
//! 云端  ⟨与基线一致 | 比基线新 1 版⟩                                  <digest 前 8 位>  <本地时间>
//! 基线  <stamp>  <digest 前 8 位>
//! 接下来： ⟨游戏结束后上传 | 启动前会下载 | 启动时会问你 | 本次不自动同步⟩
//! ```
//!
//! ## 两处拿的主意（都写在明处）
//!
//! 1. **本机 digest 只显示"已经算过"的那一个**（`—（未核对）` 表示没算过）。理由与代价在
//!    `daemon::sync_rpc::banner` 的文件头写着：§6.8 A 要这一栏显示内容值，而 §6.1/§6.4
//!    要求平时绝不读存档内容 —— 折中就是"算过才显示"，绝不为了横幅去读一遍存档。
//! 2. **digest 相同时时间显示更早的那一个**（用户 2026-10-04 定）：本机与云端 digest 一样
//!    时，云端那一行的时间显示本机这个时间（因为它必然更早 —— 本机是那一刻同步下来的），
//!    并在那一行的措辞上加一句"（本机这一版，比云端本地时间早）"？—— 不：规格要的是**时间**
//!    显示更早的那一个，所以这里把云端那一行的时间换成更早的那个，其余一个字不加。
//!
//!    ⚠ 实现上的细节：云端那一版的时间来自**版本名**（`stamp`），本机的时间来自
//!    `mtime_ms`；两者都用 `describe_stamp` / [`format_ms`] 换成本机时区的人话。digest 相同
//!    时取"更早的那个"，靠的是原始时间戳比较（`stamp_time` 与毫秒），不是比那句人话。

use super::confirm::Confirmation;

/// 本机那一侧（`sync.snapshot` 的 `local`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotLocal {
    pub mtime_ms: i64,
    /// 所有存档位置都不在（§6.11 第 3 条要横幅写明的那一种）。
    pub empty: bool,
    /// **没算过就是 `None`** —— 横幅上写「—（未核对）」，绝不用一个别的值顶替。
    pub digest: Option<String>,
}

/// 云端那一侧（`sync.snapshot` 的 `cloud`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotCloud {
    /// `unknown` / `none` / `known`（判定表第 1/2 格与"有"）。
    pub state: String,
    pub stamp: Option<String>,
    pub digest: Option<String>,
    pub key: String,
}

/// 基线那一侧（`sync.snapshot` 的 `baseline`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotBaseline {
    pub stamp: String,
    pub digest: String,
    pub mtime_ms: i64,
}

/// 索引那一侧（`sync.snapshot` 的 `index`）：横幅上"这份清单是什么时候拿的"那半句。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotIndex {
    pub from_cache: bool,
    pub cached_at: String,
    /// 手上这份索引过期了（§6.8 A 第 2 条：过期就自动刷一次，刷失败才提示）。
    pub stale: bool,
    pub refresh_error: Option<String>,
}

/// `sync.snapshot` 的一整份回包。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncSnapshot {
    pub game_id: String,
    pub local: SnapshotLocal,
    pub cloud: SnapshotCloud,
    pub baseline: Option<SnapshotBaseline>,
    /// 判定给的那一格（`nothing` / `pull` / `confirm` / `upload_later` / `adopt_baseline` /
    /// `ask` / `no_sync`）。
    pub decision: String,
    pub index: SnapshotIndex,
    /// 读取过程里那些"不致命但要说"的事（存档位置解析不出来之类）。空 = 没事。
    pub problem: Option<String>,
}

/// 横幅当前该显示什么。
///
/// ⚠ 它自己**不含判断**（文案在 [`banner_lines`]），只是一份"界面照着画"的东西。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncBanner {
    /// 这一份说的是哪一款（换款之后旧回包要丢掉）。
    pub game_id: String,
    /// 回包还没到（界面上写「正在核对…」）。⚠ **进页面立刻就是它** —— 用户 2026-10-04
    /// 明确要求"不要点进去之前先算"（那会卡住页面）。
    pub loading: bool,
    pub lines: Option<BannerLines>,
    /// **回包本身**（不只是拼好的那四行）：§6.7 第 1 条那条前置提示要拿它的
    /// "本机 mtime vs 基线"判一下（见 `tasks::local_drifted`），而那一问就发生在
    /// 「替换 / 恢复」那一刻 —— 手上这份回包正好够用，不必再问 daemon 一次。
    pub snapshot: Option<SyncSnapshot>,
    /// 问不成的原因（daemon 连不上之类）。空 = 没事。
    pub error: Option<String>,
}

impl SyncBanner {
    /// 进了某一页：清掉上一款，进入「正在核对…」。
    pub(in crate::ui) fn requested(&mut self, game_id: &str) {
        self.game_id = game_id.to_string();
        self.loading = true;
        self.lines = None;
        self.snapshot = None;
        self.error = None;
    }

    /// 回包到了。**只认当前这一款的回包**（换款之后迟到的那些一律丢掉）。
    pub(in crate::ui) fn loaded(&mut self, snapshot: SyncSnapshot) {
        if snapshot.game_id != self.game_id {
            return;
        }
        self.loading = false;
        self.error = None;
        self.lines = Some(banner_lines(&snapshot));
        self.snapshot = Some(snapshot);
    }

    /// 没问成（daemon 不通、云同步没配齐）。横幅上如实说，**不影响别的**。
    pub(in crate::ui) fn failed(&mut self, error: String) {
        self.loading = false;
        self.lines = None;
        self.snapshot = None;
        self.error = Some(error);
    }

    /// 本机偏没偏基线（§6.7 第 1 条那条前置提示）。回包还没到 ⇒ `false`（按"没偏离"算，
    /// 那只是措辞轻一点：覆盖本机这件事本来就还有一道二次确认）。
    pub(in crate::ui) fn drifted(&self) -> bool {
        self.snapshot.as_ref().is_some_and(local_drifted)
    }
}

/// 本机有没有偏离基线（§6.7 第 1 条那条前置提示的判据）。
///
/// **没有基线而本机不是空的**，或者**基线在、而本机的 mtime 与它不一样** —— 两种都是
/// "本机有未同步的改动"。判据只按 `stat` 来的时间，**绝不读存档内容**（§6.1/§6.4：
/// 那是 §6.4 那一格才做的事），所以这一句问得又便宜又快。
///
/// ⚠ 判据取自**基线**而不是判定那一格：判定给 `UploadLater` 也可能是"本机落后"（第 11 格），
/// 那种同样会被恢复覆盖掉。
pub(in crate::ui) fn local_drifted(snapshot: &SyncSnapshot) -> bool {
    if snapshot.local.empty {
        // 本机一个文件都没有：没有"未同步的改动"可覆盖。
        return false;
    }
    match &snapshot.baseline {
        // 还没有基线：本机这一版从来没跟云端对上过账 ⇒ 恢复一定覆盖掉没同步的东西。
        None => true,
        Some(baseline) => snapshot.local.mtime_ms != baseline.mtime_ms,
    }
}

/// 冲突弹窗上那句话（§6.8 B）：按 daemon 报的那一类冲突挑。
///
/// 措辞与 daemon 那份 `launch_report::ask_detail`（进日志的那一份）是**同一套说法的两个
/// 出口** —— 分叉了就意味着"界面上写的"与"日志里说的"不是一回事（用户 2026-09-28 那次
/// 排查的根因）。改这里就该回去看一眼那边。
pub(in crate::ui) fn conflict_message(kind: &str) -> &'static str {
    match kind {
        "no_baseline" => "还没有基线，本机与云端的内容不一样 —— 谁新谁旧无从判断，需要你选一次。",
        "unknown_digest" => {
            "云端最新那一版的内容值不知道（老索引）—— 不能拿版本名或时间戳猜，需要你选一次。"
        }
        // `both_changed`（以及认不出来的那一类：宁可多说一句"两边都改过"）。
        _ => "本地和云端都改过 —— 需要你选一次。",
    }
}

/// 横幅的四行（拼好的字符串；`.slint` 只管画）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BannerLines {
    pub local: String,
    pub cloud: String,
    pub baseline: String,
    pub next: String,
    /// 这一版有"要当心"的地方（冲突 / 本地为空）：那一行用警示色（§6.8 A）。
    pub warning: bool,
    /// 索引过期而且刷失败时那句话（「索引已过期，刷新失败」+ 缓存时间）。
    pub stale_note: Option<String>,
}

/// 内容值的前 8 位（§6.8 A 要显示的那一栏）。没算过就写「—（未核对）」。
pub(in crate::ui) fn digest_prefix(digest: Option<&str>) -> String {
    match digest {
        Some(digest) if !digest.is_empty() => {
            format!("{}…", digest.chars().take(8).collect::<String>())
        }
        _ => "—（未核对）".to_string(),
    }
}

/// 版本名那一栏（基线 / 云端）：认得出时间就写本机时区的人话，认不出原样写。
fn stamp_label(stamp: &str) -> String {
    if stamp.is_empty() {
        return "—".to_string();
    }
    crate::sync::describe_stamp(stamp)
}

/// 毫秒时间戳 → 本机时区的人话，与 [`crate::sync::describe_stamp`] 一个格式。
///
/// `0` 是"一个文件都没有"（§6.1(b) 的定义），显示成「—」而不是 1970 年。
pub(in crate::ui) fn format_ms(mtime_ms: i64) -> String {
    if mtime_ms <= 0 {
        return "—".to_string();
    }
    chrono::DateTime::from_timestamp_millis(mtime_ms)
        .map(|at| {
            at.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "—".to_string())
}

/// 「超前 N 天 / 落后 N 天」：只按天数说，不足一天就不说数字。
///
/// ⚠ 这个方向**只影响横幅措辞**（§6.3 的原话）；"覆盖还是不覆盖"的最后一道关永远是 digest。
fn drift_label(mtime_ms: i64, baseline_ms: i64) -> String {
    let delta_ms = mtime_ms - baseline_ms;
    let days = delta_ms.unsigned_abs() / (24 * 60 * 60 * 1000);
    match delta_ms.cmp(&0) {
        std::cmp::Ordering::Equal => "与基线一致".to_string(),
        std::cmp::Ordering::Greater if days > 0 => format!("超前 {days} 天"),
        std::cmp::Ordering::Greater => "超前".to_string(),
        std::cmp::Ordering::Less if days > 0 => format!("落后 {days} 天"),
        std::cmp::Ordering::Less => "落后".to_string(),
    }
}

/// 「接下来」那一行 —— 由判定那一格翻出来（§6.6 闸门 b 与 §6.8 A 的那四个说法）。
pub(in crate::ui) fn next_label(decision: &str) -> &'static str {
    match decision {
        "upload_later" => "游戏结束后上传",
        "pull" | "confirm" => "启动前会下载",
        "ask" => "启动时会问你",
        // `nothing` / `adopt_baseline` / `no_sync`：本次不自动同步（`adopt_baseline` 是
        // 第一次对账就一致、静默记下基线，`nothing` 是三边一致，`no_sync` 是读不到索引）。
        _ => "本次不自动同步",
    }
}

/// 把一份 `sync.snapshot` 拼成那四行（纯函数，§6.9 的四条文案就靠它测）。
pub(in crate::ui) fn banner_lines(snapshot: &SyncSnapshot) -> BannerLines {
    let baseline = snapshot.baseline.as_ref();
    let local_digest = snapshot.local.digest.as_deref();
    let cloud_digest = snapshot.cloud.digest.as_deref();

    // ── 第一行：本机 ──
    let local_state = if snapshot.local.empty {
        "本地为空".to_string()
    } else {
        match baseline {
            None => "还没有基线".to_string(),
            Some(baseline) => drift_label(snapshot.local.mtime_ms, baseline.mtime_ms),
        }
    };
    let local = format!(
        "本地  {}  {}  {}",
        local_state,
        digest_prefix(local_digest),
        if snapshot.local.empty {
            "—".to_string()
        } else {
            format_ms(snapshot.local.mtime_ms)
        }
    );

    // ── 第二行：云端 ──
    let cloud_state = match snapshot.cloud.state.as_str() {
        "unknown" => "云端索引读不到".to_string(),
        "none" => "云端没有这一款".to_string(),
        _ => match baseline {
            // ⚠ 云端动没动看 **digest**，不看时间戳（§6.3 的原话）。
            Some(baseline) => match cloud_digest {
                Some(digest) if digest == baseline.digest => "与基线一致".to_string(),
                // 内容值不知道（老索引）：绝不说成"与基线一致"，也不说"比基线新"。
                None => "内容值不知道".to_string(),
                Some(_) => "比基线新 1 版".to_string(),
            },
            // 还没有基线：云端这一版就是"第一次见到的那一版"。
            None => "还没有基线".to_string(),
        },
    };
    // digest 相同时（同一存档、时间不同）**时间显示更早的那一个**（用户 2026-10-04 定）。
    // 这里比的是原始时间戳（版本名 vs 毫秒），不是那句人话。
    let same_digest = match (cloud_digest, local_digest) {
        (Some(cloud), Some(local)) => cloud == local,
        _ => false,
    };
    let cloud_time = match (&snapshot.cloud.stamp, same_digest) {
        (Some(stamp), true) => {
            let cloud_at = crate::sync::stamp_time(stamp);
            // 谁更早：云端那一刻 vs 本机那一刻。本机早（或说不清）就显示本机的。
            let earlier_local = match cloud_at {
                Some(at) => snapshot.local.mtime_ms <= at.timestamp_millis(),
                None => false,
            };
            if earlier_local {
                format_ms(snapshot.local.mtime_ms)
            } else {
                stamp_label(stamp)
            }
        }
        (Some(stamp), false) => stamp_label(stamp),
        (None, _) => "—".to_string(),
    };
    let cloud = format!(
        "云端  {}  {}  {}",
        cloud_state,
        digest_prefix(cloud_digest),
        cloud_time
    );

    // ── 第三行：基线 ──
    let baseline_line = match baseline {
        Some(baseline) => format!(
            "基线  {}  {}",
            stamp_label(&baseline.stamp),
            digest_prefix(Some(&baseline.digest))
        ),
        None => "基线  还没有（第一次同步会记下一条）".to_string(),
    };

    // ── 第四行：接下来 ──
    let next = format!("接下来：{}", next_label(&snapshot.decision));

    // 冲突或本地为空那一行用警示色（§6.8 A）。冲突 = 判定说"要问你"（第 3/5/10 格），
    // 而"还没有基线"那两格问的也是同一件事（第一次同步、两边内容不一样）。
    let conflict = snapshot.decision == "ask";
    BannerLines {
        local,
        cloud,
        baseline: baseline_line,
        next,
        warning: conflict || snapshot.local.empty,
        stale_note: stale_note(&snapshot.index),
    }
}

/// 索引过期而且刷失败时那句话（§6.11 第 2 条：**只有**这一种情况才提示）。
pub(in crate::ui) fn stale_note(index: &SnapshotIndex) -> Option<String> {
    let error = index.refresh_error.as_deref()?;
    Some(format!(
        "索引已过期，刷新失败：{error}（缓存时间 {}）",
        stamp_label(&index.cached_at)
    ))
}

/// 单游戏页那颗「恢复」现在该问哪一句话（§6.7 第 1 条）。
///
/// 两种说法都来自 `model::confirm`（同一件事在各入口上必须说同一句话）：
/// 本机没动过就是原来那句「用这一版替换本机的存档？」，动过就换成
/// 「本机有未同步的改动，恢复会覆盖它」并把那段后果写成"还没同步过的进度会被盖掉"。
pub(in crate::ui) fn restore_confirmation(version: Option<String>, drifted: bool) -> Confirmation {
    Confirmation::Replace {
        // 单游戏页那颗是"云端最新那一版"，所以版本名可以是空的 —— 页面那边只画标题与后果。
        version: version.unwrap_or_default(),
        drifted,
    }
}

/// 那一问要不要按"偏离基线"重写（页面只拿得到一个布尔：确认那一行与那句话都得跟着变）。
pub(in crate::ui) fn restore_drifted(wording: &Confirmation) -> bool {
    matches!(wording, Confirmation::Replace { drifted: true, .. })
}
#[cfg(test)]
// ⚠ 测试住在同目录的另一个文件里（照 `sync/archive/digest_tests.rs` 与
// `daemon/tests.rs` 的习惯）：这个文件连着测试一起数会越过 500 行的软线（AGENTS.md）。
#[path = "banner_tests.rs"]
mod tests;
