//! 判据层的测试（PLATFORMS.md §6.9）：把 §6.3 那张判定表**表驱动**地钉住。
//!
//! 从 `decision.rs` 拆出来（那边连着测试一起数会越过 500 行的线，见 AGENTS.md）：
//! 本文件放夹具（三方状态 + 12 格表）与表驱动的那一条，逐格细则在 `rows` 子模块里。
//! 测试一律只调纯函数 —— 不读盘、不联网，所以判定表能被逐格摆出来跑。

use super::*;
use crate::sync::archive::DIGEST_HEX_LEN;

// 这一层把 digest 当**不透明字符串**（形状由 `sync::archive::digest` 保证），但测试里
// 仍写成 64 个小写十六进制 —— 免得测出来的东西在真实数据里根本不存在。8 × 8 = 64。
const A: &str = concat!(
    "aaaaaaaa", "aaaaaaaa", "aaaaaaaa", "aaaaaaaa", "aaaaaaaa", "aaaaaaaa", "aaaaaaaa", "aaaaaaaa",
);
const B: &str = concat!(
    "bbbbbbbb", "bbbbbbbb", "bbbbbbbb", "bbbbbbbb", "bbbbbbbb", "bbbbbbbb", "bbbbbbbb", "bbbbbbbb",
);
const C: &str = concat!(
    "cccccccc", "cccccccc", "cccccccc", "cccccccc", "cccccccc", "cccccccc", "cccccccc", "cccccccc",
);
/// 本机一个文件都没有时算出来的那个内容值（空集合也走同一条算法，§6.1(a)）。
const EMPTY: &str = concat!(
    "eeeeeeee", "eeeeeeee", "eeeeeeee", "eeeeeeee", "eeeeeeee", "eeeeeeee", "eeeeeeee", "eeeeeeee",
);

/// 基线认账的那一版，与云端现在最新的那一版（两个不同的版本名）。
const STAMP_B: &str = "20261001T090000Z";
const STAMP_C: &str = "20261002T090000Z";
/// 基线那一版的 mtime；第 10/11 格比大小用的就是它。
const MTIME_B: i64 = 1_000;

/// 云端索引那一侧（`Unknown` / 没有这一款 / 有这一款）。
#[derive(Clone, Copy)]
enum CloudSpec {
    /// 索引读不到。
    Unknown,
    /// 云端没有这一款，或者它 0 版。
    NoSave,
    /// `(版本名, 内容值)`；内容值 `None` = 老索引没记。
    Known(&'static str, Option<&'static str>),
}

/// §6.3 表里的一格：三方各自的输入，以及它该给出的结论。
struct Row {
    /// §6.3 的格号 —— 断言里带上它，红了能一眼对回规格。
    number: u32,
    mtime_ms: i64,
    digest: Option<&'static str>,
    /// 基线：`(stamp, digest, mtime_ms)`；`None` = 没有基线。
    baseline: Option<(&'static str, &'static str, i64)>,
    cloud: CloudSpec,
    expected: Decision,
}

fn run(row: &Row) -> Decision {
    let local = LocalState {
        mtime_ms: row.mtime_ms,
        digest: row.digest,
    };
    let baseline = row
        .baseline
        .map(|(stamp, digest, mtime_ms)| Baseline::new(stamp, digest, mtime_ms));
    let cloud = match row.cloud {
        CloudSpec::Unknown => CloudState::Unknown,
        CloudSpec::NoSave => CloudState::None,
        CloudSpec::Known(stamp, digest) => CloudState::Known { stamp, digest },
    };
    decide(&local, baseline.as_ref(), &cloud)
}

/// §6.3 的 12 格，逐格一行（顺序就是表里的顺序）。
fn table() -> Vec<Row> {
    let baseline = Some((STAMP_B, A, MTIME_B));
    vec![
        // 1：云端索引读不到 ⇒ 本次不自动同步（也不许"顺手"上传，§6.6 闸门 b）。
        Row {
            number: 1,
            mtime_ms: MTIME_B,
            digest: None,
            baseline,
            cloud: CloudSpec::Unknown,
            expected: Decision::NoSync {
                reason: NO_INDEX_REASON.to_string(),
            },
        },
        // 2：云端没有这一款（或它 0 版）⇒ 结束后上传。
        Row {
            number: 2,
            mtime_ms: MTIME_B,
            digest: None,
            baseline: None,
            cloud: CloudSpec::NoSave,
            expected: Decision::UploadLater,
        },
        // 3：云端那一版的内容值不知道 ⇒ 问，绝不猜。
        Row {
            number: 3,
            mtime_ms: MTIME_B,
            digest: None,
            baseline,
            cloud: CloudSpec::Known(STAMP_C, None),
            expected: Decision::Ask {
                kind: ConflictKind::UnknownDigest,
            },
        },
        // 4：第一次同步、内容一致 ⇒ 认账（不弹窗、不下载）。
        Row {
            number: 4,
            mtime_ms: MTIME_B,
            digest: Some(A),
            baseline: None,
            cloud: CloudSpec::Known(STAMP_C, Some(A)),
            expected: Decision::AdoptBaseline {
                stamp: STAMP_C.to_string(),
                digest: A.to_string(),
            },
        },
        // 5：第一次同步、内容不同 ⇒ 没有参照物，问用户。
        Row {
            number: 5,
            mtime_ms: MTIME_B,
            digest: Some(B),
            baseline: None,
            cloud: CloudSpec::Known(STAMP_C, Some(A)),
            expected: Decision::Ask {
                kind: ConflictKind::NoBaseline,
            },
        },
        // 6：云端与基线一致、本机看着也没动（digest 还没算）⇒ 什么都不做。
        Row {
            number: 6,
            mtime_ms: MTIME_B,
            digest: None,
            baseline,
            cloud: CloudSpec::Known(STAMP_B, Some(A)),
            expected: Decision::Nothing,
        },
        // 7：云端偏离了基线、本机看着没动、digest 还没算 ⇒ 先读内容核对（§6.4），绝不直接覆盖。
        Row {
            number: 7,
            mtime_ms: MTIME_B,
            digest: None,
            baseline,
            cloud: CloudSpec::Known(STAMP_C, Some(C)),
            expected: Decision::Confirm {
                stamp: STAMP_C.to_string(),
            },
        },
        // 8：同上，但 digest 已经算过、等于基线 ⇒ 本机确实没动 ⇒ 拉（mtime 相不相同都一样）。
        Row {
            number: 8,
            mtime_ms: MTIME_B + 5_000,
            digest: Some(A),
            baseline,
            cloud: CloudSpec::Known(STAMP_C, Some(C)),
            expected: Decision::Pull {
                stamp: STAMP_C.to_string(),
            },
        },
        // 9：云端没动、本机动了 ⇒ 不下载，结束后上传（把本机这一版固化）。
        Row {
            number: 9,
            mtime_ms: MTIME_B + 5_000,
            digest: Some(B),
            baseline,
            cloud: CloudSpec::Known(STAMP_B, Some(A)),
            expected: Decision::UploadLater,
        },
        // 10：云端也动了、本机超前 ⇒ 冲突。
        Row {
            number: 10,
            mtime_ms: MTIME_B + 5_000,
            digest: Some(B),
            baseline,
            cloud: CloudSpec::Known(STAMP_C, Some(C)),
            expected: Decision::Ask {
                kind: ConflictKind::BothChanged,
            },
        },
        // 11：云端也动了、本机落后 ⇒ 结束后上传（用户："只要本地落后基线，本地就会上传覆盖云存档"）。
        Row {
            number: 11,
            mtime_ms: MTIME_B - 500,
            digest: Some(B),
            baseline,
            cloud: CloudSpec::Known(STAMP_C, Some(C)),
            expected: Decision::UploadLater,
        },
        // 12：本机一个文件都没有 —— 空集的 digest 也是一个值，走 6–11 同一套，没有特例。
        Row {
            number: 12,
            mtime_ms: 0,
            digest: Some(EMPTY),
            baseline: Some((STAMP_B, EMPTY, 0)),
            cloud: CloudSpec::Known(STAMP_B, Some(EMPTY)),
            expected: Decision::Nothing,
        },
    ]
}

/// 把表里某一格的基线换掉再跑一遍（规格写"任意"的格子靠它把两种都覆盖到）。
fn with_baseline(number: u32, baseline: Option<(&'static str, &'static str, i64)>) -> Decision {
    let mut row = table()
        .into_iter()
        .find(|row| row.number == number)
        .expect("表里该有这一格");
    row.baseline = baseline;
    run(&row)
}

/// §6.3 的 12 格，一格一条断言（表驱动）。
#[test]
fn every_row_of_the_decision_table() {
    // 先让那几个"内容值"常量自己过一遍形状这一关。
    for digest in [A, B, C, EMPTY] {
        assert_eq!(digest.len(), DIGEST_HEX_LEN);
        assert!(
            digest
                .chars()
                .all(|ch| ch.is_ascii_hexdigit() && !ch.is_uppercase())
        );
    }

    let rows = table();
    assert_eq!(rows.len(), 12, "§6.3 是 12 格，一格都不许少");
    for row in &rows {
        assert_eq!(run(row), row.expected, "§6.3 第 {} 格", row.number);
    }

    // 第 1 / 2 / 3 格写的是"基线任意"：基线在不在，结论都不许变。
    for number in [1_u32, 2, 3] {
        let without = with_baseline(number, None);
        let with = with_baseline(number, Some((STAMP_B, A, MTIME_B)));
        assert_eq!(without, with, "第 {number} 格：基线不该影响结论");
    }
}

/// 第 1 格：读不到云端索引时**绝不猜** —— 本机与基线什么样子都一样。
#[test]
fn a_cloud_we_cannot_read_never_guesses() {
    let baseline = Baseline::new(STAMP_B, A, MTIME_B);
    for digest in [None, Some(A), Some(B)] {
        for mtime_ms in [0, MTIME_B, MTIME_B + 5_000] {
            let local = LocalState { mtime_ms, digest };
            let decision = decide(&local, Some(&baseline), &CloudState::Unknown);
            assert!(
                matches!(&decision, Decision::NoSync { .. }),
                "第 1 格：{digest:?}/{mtime_ms} ⇒ {decision:?}"
            );
            // 也不许顺手"上传"：读不到索引 = 这次没跟云端对上账（§6.6 闸门 b）。
            assert!(!matches!(&decision, Decision::UploadLater), "{decision:?}");
            assert_eq!(
                decision,
                Decision::NoSync {
                    reason: NO_INDEX_REASON.to_string()
                }
            );
        }
    }
    // 连基线都没有时也是同一句话。
    let local = LocalState {
        mtime_ms: 0,
        digest: None,
    };
    assert_eq!(
        decide(&local, None, &CloudState::Unknown),
        Decision::NoSync {
            reason: NO_INDEX_REASON.to_string()
        }
    );
}

/// 第 4 格：第一次同步、内容一致 ⇒ 认账，而且**不弹窗**。
#[test]
fn a_first_sync_with_identical_content_adopts_the_baseline() {
    let local = LocalState {
        mtime_ms: MTIME_B,
        digest: Some(A),
    };
    let cloud = CloudState::Known {
        stamp: STAMP_C,
        digest: Some(A),
    };
    let decision = decide(&local, None, &cloud);
    assert_eq!(
        decision,
        Decision::AdoptBaseline {
            stamp: STAMP_C.to_string(),
            digest: A.to_string(),
        }
    );
    // 不弹窗：既不问用户，也不要"先读一遍内容再决定"。
    assert!(
        !matches!(
            &decision,
            Decision::Ask { .. } | Decision::Confirm { .. } | Decision::NoSync { .. }
        ),
        "第 4 格不许弹窗: {decision:?}"
    );
    // 也不下载：本机与云端本来就是同一版。
    assert!(!matches!(&decision, Decision::Pull { .. }), "{decision:?}");
}

/// 第 5 格：第一次同步、内容不同（或算不出来）⇒ 问。
#[test]
fn a_first_sync_with_different_content_asks() {
    let cloud = CloudState::Known {
        stamp: STAMP_C,
        digest: Some(A),
    };
    let different = LocalState {
        mtime_ms: MTIME_B,
        digest: Some(B),
    };
    assert_eq!(
        decide(&different, None, &cloud),
        Decision::Ask {
            kind: ConflictKind::NoBaseline
        }
    );
    // "算不出来"（有文件读不动）也不许当成"一样"。
    let unknown = LocalState {
        mtime_ms: MTIME_B,
        digest: None,
    };
    assert_eq!(
        decide(&unknown, None, &cloud),
        Decision::Ask {
            kind: ConflictKind::NoBaseline
        }
    );
}

/// 逐格细则（§6.4 的内容核对、第 9~12 格、以及规格没覆盖的那一格）。
mod rows;
