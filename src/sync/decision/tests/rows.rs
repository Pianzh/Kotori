//! §6.3 判定表的逐格细则：一条测试钉住一条规则（§6.4 的内容核对、第 9/10/11/12 格、
//! 以及规格没写到的「mtime 恰好相等」那一格）。
//!
//! 夹具与 12 格表在父模块（`decision/tests.rs`）里，这里直接复用。

use super::*;

/// 第 7 格：本机看着没动、云端偏离了基线 ⇒ 必须先读内容核对，**绝不**直接 `Pull`。
#[test]
fn an_untouched_local_save_asks_for_a_content_check_before_pulling() {
    let baseline = Baseline::new(STAMP_B, A, MTIME_B);
    let cloud = CloudState::Known {
        stamp: STAMP_C,
        digest: Some(C),
    };
    // 启动时只 `stat` 了一遍：mtime 与基线一致，digest 还没算。
    let local = LocalState {
        mtime_ms: MTIME_B,
        digest: None,
    };
    let first = decide(&local, Some(&baseline), &cloud);
    assert_eq!(
        first,
        Decision::Confirm {
            stamp: STAMP_C.to_string()
        }
    );
    assert!(
        !matches!(&first, Decision::Pull { .. }),
        "第 7 格绝不直接覆盖: {first:?}"
    );

    // §6.4：读完内容、带着 digest 再调一次 —— 这时才允许覆盖。
    let checked = LocalState {
        mtime_ms: MTIME_B,
        digest: Some(A),
    };
    assert_eq!(
        decide(&checked, Some(&baseline), &cloud),
        Decision::Pull {
            stamp: STAMP_C.to_string()
        }
    );
}

/// §6.4：mtime 相同**不等于**本机没动 —— 内容核对说了算，绝不 `Pull`。
#[test]
fn the_mtime_fast_path_is_confirmed_by_content_before_overwriting() {
    let baseline = Baseline::new(STAMP_B, A, MTIME_B);
    let cloud = CloudState::Known {
        stamp: STAMP_C,
        digest: Some(C),
    };
    // 第一步：mtime 一模一样，只能先要一次内容核对。
    let unchecked = LocalState {
        mtime_ms: MTIME_B,
        digest: None,
    };
    assert_eq!(
        decide(&unchecked, Some(&baseline), &cloud),
        Decision::Confirm {
            stamp: STAMP_C.to_string()
        }
    );

    // 第二步：核对完发现内容其实变了（mtime 在骗人）⇒ 双方都改了 ⇒ 问，绝不覆盖本机。
    let changed = LocalState {
        mtime_ms: MTIME_B,
        digest: Some(B),
    };
    let second = decide(&changed, Some(&baseline), &cloud);
    assert_eq!(
        second,
        Decision::Ask {
            kind: ConflictKind::BothChanged
        }
    );
    assert!(
        !matches!(&second, Decision::Pull { .. }),
        "mtime 相同绝不等于可以覆盖本机: {second:?}"
    );
    assert!(
        !matches!(&second, Decision::Confirm { .. }),
        "digest 已经算过，不该再要一次核对: {second:?}"
    );
}

/// 第 11 格：本机落后基线、云端也动了 ⇒ 不询问，结束后上传覆盖云端。
#[test]
fn falling_behind_the_baseline_still_uploads_instead_of_asking() {
    let baseline = Baseline::new(STAMP_B, A, MTIME_B);
    let cloud = CloudState::Known {
        stamp: STAMP_C,
        digest: Some(C),
    };
    let behind = LocalState {
        mtime_ms: MTIME_B - 500,
        digest: Some(B),
    };
    let decision = decide(&behind, Some(&baseline), &cloud);
    assert_eq!(decision, Decision::UploadLater);
    assert!(
        !matches!(&decision, Decision::Ask { .. }),
        "落后不是冲突: {decision:?}"
    );
    assert!(
        !matches!(&decision, Decision::Pull { .. }),
        "本机落后也不下载: {decision:?}"
    );

    // digest 还没算（只看 mtime 那条快路）时也一样。
    let unknown = LocalState {
        mtime_ms: MTIME_B - 500,
        digest: None,
    };
    assert_eq!(
        decide(&unknown, Some(&baseline), &cloud),
        Decision::UploadLater
    );
}

/// 第 10 格：本机超前、云端也动了 ⇒ 双方都动了 ⇒ 冲突。
#[test]
fn being_ahead_while_the_cloud_moved_is_a_conflict() {
    let baseline = Baseline::new(STAMP_B, A, MTIME_B);
    let cloud = CloudState::Known {
        stamp: STAMP_C,
        digest: Some(C),
    };
    let ahead = LocalState {
        mtime_ms: MTIME_B + 5_000,
        digest: Some(B),
    };
    let decision = decide(&ahead, Some(&baseline), &cloud);
    assert_eq!(
        decision,
        Decision::Ask {
            kind: ConflictKind::BothChanged
        }
    );
    assert!(
        !matches!(&decision, Decision::UploadLater),
        "冲突不许悄悄上传覆盖云端: {decision:?}"
    );
    assert!(!matches!(&decision, Decision::Pull { .. }), "{decision:?}");

    // digest 还没算、只有 mtime 超前时也一样：云端偏离基线 + 本机看着动了 ⇒ 冲突。
    let unknown = LocalState {
        mtime_ms: MTIME_B + 5_000,
        digest: None,
    };
    assert_eq!(
        decide(&unknown, Some(&baseline), &cloud),
        Decision::Ask {
            kind: ConflictKind::BothChanged
        }
    );
}

/// 第 9 格：本机超前、云端没动 ⇒ 不下载，只在游戏结束后上传。
#[test]
fn being_ahead_with_an_untouched_cloud_uploads_later() {
    let baseline = Baseline::new(STAMP_B, A, MTIME_B);
    let cloud = CloudState::Known {
        stamp: STAMP_B,
        digest: Some(A),
    };
    let ahead = LocalState {
        mtime_ms: MTIME_B + 5_000,
        digest: Some(B),
    };
    let decision = decide(&ahead, Some(&baseline), &cloud);
    assert_eq!(decision, Decision::UploadLater);
    assert!(
        !matches!(&decision, Decision::Pull { .. }),
        "云端没动就不下载: {decision:?}"
    );
    assert!(
        !matches!(&decision, Decision::Ask { .. }),
        "云端没动就不是冲突: {decision:?}"
    );

    // 只看 mtime 那条快路（digest 还没算）时也一样。
    let unknown = LocalState {
        mtime_ms: MTIME_B + 5_000,
        digest: None,
    };
    assert_eq!(
        decide(&unknown, Some(&baseline), &cloud),
        Decision::UploadLater
    );
}

/// 第 12 格：本机为空也走 6–11 同一套 —— 空集的 digest 就是一个普通的值。
#[test]
fn an_empty_local_save_is_compared_like_any_other_version() {
    // 基线就是那个空集 ⇒ 与第 6 格一样：什么都不做。
    let baseline = Baseline::new(STAMP_B, EMPTY, 0);
    let empty = LocalState {
        mtime_ms: 0,
        digest: Some(EMPTY),
    };
    assert_eq!(
        decide(
            &empty,
            Some(&baseline),
            &CloudState::Known {
                stamp: STAMP_B,
                digest: Some(EMPTY)
            }
        ),
        Decision::Nothing
    );

    // 本机被清空（基线不是空集）⇒ 与第 9 格一样：本机动了 ⇒ 结束后上传。
    // （"不传不拉、基线不动"是上传那一步的 A0 闸门管的，判定这一层不特例。）
    assert_eq!(
        decide(
            &empty,
            Some(&Baseline::new(STAMP_B, A, MTIME_B)),
            &CloudState::Known {
                stamp: STAMP_B,
                digest: Some(A)
            }
        ),
        Decision::UploadLater
    );

    // 第一次同步、云端就是个空包 ⇒ 与第 4 格一样：认账。
    assert_eq!(
        decide(
            &empty,
            None,
            &CloudState::Known {
                stamp: STAMP_C,
                digest: Some(EMPTY)
            }
        ),
        Decision::AdoptBaseline {
            stamp: STAMP_C.to_string(),
            digest: EMPTY.to_string(),
        }
    );

    // "mtime 说没动、digest 还没算"那一格也照样要读内容核对：本地为空不是免检牌。
    let unchecked = LocalState {
        mtime_ms: 0,
        digest: None,
    };
    assert_eq!(
        decide(
            &unchecked,
            Some(&baseline),
            &CloudState::Known {
                stamp: STAMP_C,
                digest: Some(C)
            }
        ),
        Decision::Confirm {
            stamp: STAMP_C.to_string()
        }
    );
}

/// 规格没覆盖的那一格：本机动了、云端也动了、而 mtime 恰好**相等**。
///
/// §6.3 第 10 格要 `>`、第 11 格要 `<`，相等时两格都不成立。这里按"我们怕的是冲突"
/// 取问用户 —— 绝不悄悄拿哪一边去覆盖另一边。⚠ 这一条是我在规格之外补的判据，已提给
/// 用户裁决（见交接回报）。
#[test]
fn an_equal_mtime_while_both_sides_moved_asks_instead_of_overwriting() {
    let baseline = Baseline::new(STAMP_B, A, MTIME_B);
    let cloud = CloudState::Known {
        stamp: STAMP_C,
        digest: Some(C),
    };
    // digest 已经算过：本机确实动了，云端也动了，方向看不出来 ⇒ 问。
    let moved = LocalState {
        mtime_ms: MTIME_B,
        digest: Some(B),
    };
    assert_eq!(
        decide(&moved, Some(&baseline), &cloud),
        Decision::Ask {
            kind: ConflictKind::BothChanged
        }
    );

    // digest 还没算：mtime 相等只说"看着没动"，那就照第 7 格先读内容核对
    // （核对之后本机内容不等于基线 ⇒ 落到上面的 `Ask`，仍然不会覆盖）。
    let unchecked = LocalState {
        mtime_ms: MTIME_B,
        digest: None,
    };
    assert_eq!(
        decide(&unchecked, Some(&baseline), &cloud),
        Decision::Confirm {
            stamp: STAMP_C.to_string()
        }
    );
    let checked = LocalState {
        mtime_ms: MTIME_B,
        digest: Some(B),
    };
    assert_eq!(
        decide(&checked, Some(&baseline), &cloud),
        Decision::Ask {
            kind: ConflictKind::BothChanged
        }
    );
}
