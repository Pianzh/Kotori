//! `model::banner` 的单测：四行文案、digest 相同取更早的时间、以及「偏没偏基线」。

use super::*;

fn digest(seed: u8) -> String {
    format!("{:02x}", seed).repeat(32)
}

fn snapshot() -> SyncSnapshot {
    SyncSnapshot {
        game_id: "demo".into(),
        local: SnapshotLocal {
            mtime_ms: 1_759_570_000_000,
            empty: false,
            digest: Some(digest(0xaa)),
        },
        cloud: SnapshotCloud {
            state: "known".into(),
            stamp: Some("20261004T101500Z".into()),
            digest: Some(digest(0xaa)),
            key: "demo".into(),
        },
        baseline: Some(SnapshotBaseline {
            stamp: "20261004T101500Z".into(),
            digest: digest(0xaa),
            mtime_ms: 1_759_570_000_000,
        }),
        decision: "nothing".into(),
        index: SnapshotIndex::default(),
        problem: None,
    }
}

/// 与基线一致：三行都说"同一版"，接下来什么都不做。
#[test]
fn an_untouched_game_says_everything_lines_up() {
    let lines = banner_lines(&snapshot());
    assert!(
        lines.local.starts_with("本地  与基线一致"),
        "{}",
        lines.local
    );
    assert!(lines.local.contains(&digest(0xaa)[..8]), "{}", lines.local);
    assert!(
        lines.cloud.starts_with("云端  与基线一致"),
        "{}",
        lines.cloud
    );
    assert!(lines.baseline.starts_with("基线  "), "{}", lines.baseline);
    assert_eq!(lines.next, "接下来：本次不自动同步");
    assert!(!lines.warning, "三边一致不该有警示色");
    assert!(lines.stale_note.is_none());
}

/// 云端新：本机与基线一致、云端换了一版 ⇒ 启动前会下载。
#[test]
fn a_newer_cloud_says_it_will_download_before_launch() {
    let mut snapshot = snapshot();
    snapshot.cloud.digest = Some(digest(0xbb));
    snapshot.cloud.stamp = Some("20261005T101500Z".into());
    snapshot.decision = "pull".into();

    let lines = banner_lines(&snapshot);
    assert!(lines.cloud.contains("比基线新 1 版"), "{}", lines.cloud);
    assert!(lines.cloud.contains(&digest(0xbb)[..8]), "{}", lines.cloud);
    assert_eq!(lines.next, "接下来：启动前会下载");
    assert!(!lines.warning);
}

/// 本机新：本机超前、云端没动 ⇒ 游戏结束后上传。
#[test]
fn a_newer_local_says_it_uploads_after_the_game() {
    let mut snapshot = snapshot();
    // 本机比基线晚两天改过。
    snapshot.local.mtime_ms += 2 * 24 * 60 * 60 * 1000;
    snapshot.local.digest = Some(digest(0xcc));
    snapshot.decision = "upload_later".into();

    let lines = banner_lines(&snapshot);
    assert_eq!(lines.local.split_whitespace().nth(1), Some("超前"));
    assert!(lines.local.contains("超前 2 天"), "{}", lines.local);
    assert!(lines.local.contains(&digest(0xcc)[..8]), "{}", lines.local);
    assert_eq!(lines.next, "接下来：游戏结束后上传");
    assert!(!lines.warning);
}

/// 冲突：两边都动了 ⇒ 那一行警示色，接下来是"启动时会问你"。
#[test]
fn a_conflict_warns_and_says_it_will_ask_on_launch() {
    let mut snapshot = snapshot();
    snapshot.local.mtime_ms += 24 * 60 * 60 * 1000;
    snapshot.local.digest = Some(digest(0xcc));
    snapshot.cloud.digest = Some(digest(0xbb));
    snapshot.cloud.stamp = Some("20261005T101500Z".into());
    snapshot.decision = "ask".into();

    let lines = banner_lines(&snapshot);
    assert!(lines.warning, "冲突必须用警示色");
    assert_eq!(lines.next, "接下来：启动时会问你");
    assert!(lines.cloud.contains("比基线新 1 版"), "{}", lines.cloud);
}

/// 本地为空（§6.11 第 3 条）：横幅写明，且是警示色；本机那一栏没有时间可写。
#[test]
fn an_empty_local_says_so_and_warns() {
    let mut snapshot = snapshot();
    snapshot.local = SnapshotLocal {
        mtime_ms: 0,
        empty: true,
        digest: None,
    };
    snapshot.decision = "upload_later".into();

    let lines = banner_lines(&snapshot);
    assert!(lines.local.starts_with("本地  本地为空"), "{}", lines.local);
    assert!(lines.warning, "本地为空要警示色（§6.8 A）");
    assert!(lines.local.contains("—"), "{}", lines.local);
}

/// 没有算过本机的内容值时如实写「—（未核对）」，绝不拿别的值顶替。
#[test]
fn an_unchecked_local_digest_is_shown_as_unchecked() {
    let mut snapshot = snapshot();
    snapshot.local.digest = None;
    // 没有基线时基线那一行就是本机这一版（§6.3 第 4/5 格），所以本机那一栏是"未核对"。
    snapshot.baseline = None;
    snapshot.cloud.digest = Some(digest(0xaa));

    let lines = banner_lines(&snapshot);
    assert!(lines.local.contains("还没有基线"), "{}", lines.local);
    assert!(lines.local.contains("—（未核对）"), "{}", lines.local);
    assert!(lines.baseline.contains("还没有"), "{}", lines.baseline);
}

/// ⚠ digest 相同时**时间显示更早的那一个**（用户 2026-10-04 定）。
///
/// 造法：云端那一版的时间在**未来**（本机那一刻更早）⇒ 云端那一行的时间该显示本机那个。
#[test]
fn when_both_digests_match_the_earlier_time_is_shown() {
    let mut snapshot = snapshot();
    let cloud_stamp = "20990101T000000Z";
    snapshot.cloud.stamp = Some(cloud_stamp.to_string());
    snapshot.cloud.digest = Some(digest(0xaa));
    snapshot.local.digest = Some(digest(0xaa));

    let lines = banner_lines(&snapshot);
    assert!(
        lines.cloud.contains(&format_ms(snapshot.local.mtime_ms)),
        "digest 相同时该显示更早的那个时间：{}",
        lines.cloud
    );
    assert!(
        !lines.cloud.contains(&stamp_label(cloud_stamp)),
        "更晚的那个时间不该出现：{}",
        lines.cloud
    );
}

/// 索引过期而且刷新失败 ⇒ 横幅上写明那句话（§6.11 第 2 条）。
#[test]
fn a_stale_index_that_could_not_be_refreshed_says_so() {
    let mut snapshot = snapshot();
    snapshot.index = SnapshotIndex {
        from_cache: true,
        cached_at: "20261001T101500Z".into(),
        stale: true,
        refresh_error: Some("网络不通".into()),
    };

    let lines = banner_lines(&snapshot);
    let note = lines.stale_note.expect("过期且刷新失败必须提示");
    assert!(note.contains("索引已过期，刷新失败"), "{note}");
    assert!(note.contains("网络不通"), "{note}");
    assert!(
        note.contains(&stamp_label("20261001T101500Z")),
        "要带上缓存时间：{note}"
    );
}

/// 过期但刷成功了（`refresh_error` 是空的）⇒ 一个字都不提示。
#[test]
fn a_refreshed_index_says_nothing_about_being_stale() {
    let mut snapshot = snapshot();
    snapshot.index = SnapshotIndex {
        from_cache: false,
        cached_at: "20261005T101500Z".into(),
        stale: true,
        refresh_error: None,
    };
    assert!(banner_lines(&snapshot).stale_note.is_none());
}

/// 云端索引读不到 ⇒ 接下来"本次不自动同步"，而且**不说**云端与基线一致。
#[test]
fn an_unreadable_index_never_claims_the_cloud_is_the_same() {
    let mut snapshot = snapshot();
    snapshot.cloud = SnapshotCloud {
        state: "unknown".into(),
        stamp: None,
        digest: None,
        key: String::new(),
    };
    snapshot.decision = "no_sync".into();

    let lines = banner_lines(&snapshot);
    assert!(lines.cloud.contains("云端索引读不到"), "{}", lines.cloud);
    assert!(!lines.cloud.contains("与基线一致"), "{}", lines.cloud);
    assert_eq!(lines.next, "接下来：本次不自动同步");
}

/// 换款之后迟到的回包必须丢掉（否则乙页上会画着甲的那份横幅）。
#[test]
fn a_late_reply_for_another_game_is_dropped() {
    let mut banner = SyncBanner::default();
    banner.requested("demo");
    assert!(
        banner.loading && banner.lines.is_none(),
        "进页面就是「正在核对…」"
    );

    let mut other = snapshot();
    other.game_id = "other".into();
    banner.loaded(other);
    assert!(banner.loading, "别人的回包不许把它从「正在核对…」里推出来");

    let mut mine = snapshot();
    mine.game_id = "demo".into();
    banner.loaded(mine);
    assert!(!banner.loading);
    assert!(banner.lines.is_some());
}

/// 偏没偏基线决定「恢复」那一问的措辞（§6.7 第 1 条）。
#[test]
fn the_restore_question_mentions_unsynced_changes_when_the_local_drifted() {
    let steady = restore_confirmation(None, false);
    assert_eq!(steady.title(), "用这一版替换本机的存档？");
    assert!(!steady.danger(), "没偏离时它就是原来那个「替换」");

    let drifted = restore_confirmation(None, true);
    assert_eq!(drifted.title(), "本机有未同步的改动，恢复会覆盖它");
    assert!(
        drifted.body().contains("还没同步过"),
        "要说清「没同步过的进度会被盖掉」：{}",
        drifted.body()
    );
    assert!(drifted.body().contains("取消"), "{}", drifted.body());
    assert!(drifted.danger(), "偏离基线时那颗按钮要走警示色");
    // 两种都是同一个动作（替换），只是问得重一点 —— 动作名不许分叉。
    assert_eq!(steady.verb(), drifted.verb());
    assert_eq!(steady.label(), drifted.label());
}

/// 点「恢复」之前那一下：本机偏没偏基线（§6.7 第 1 条）。**只 stat，不读内容**。
#[test]
fn drift_is_decided_by_the_baseline_and_the_local_mtime() {
    // 三边一致（本机 = 基线）。
    assert!(!local_drifted(&snapshot()));
    // 本机动过（mtime 与基线不同）。
    let mut moved = snapshot();
    moved.local.mtime_ms += 1000;
    assert!(local_drifted(&moved));
    // 还没有基线（本机这一版从没对上过账）。
    let mut fresh = snapshot();
    fresh.baseline = None;
    assert!(local_drifted(&fresh));
    // 本机为空：没有"未同步的改动"可覆盖。
    let mut empty = snapshot();
    empty.local.empty = true;
    empty.local.mtime_ms = 0;
    empty.baseline = None;
    assert!(!local_drifted(&empty));
}

/// 时间换算：0（一个文件都没有）显示成「—」，正常的毫秒值给人话。
#[test]
fn zero_mtime_is_not_1970() {
    assert_eq!(format_ms(0), "—");
    assert_eq!(format_ms(-1), "—");
    assert!(format_ms(1_759_570_000_000).starts_with("2025-"));
}
