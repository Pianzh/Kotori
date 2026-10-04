//! 「谁新谁旧」的判定（PLATFORMS.md §6.2、§6.3、§6.4）。
//!
//! 这个文件里**只有数据与纯函数**：不读盘、不碰网络、不认识配置 —— 这正是它的全部价值，
//! 整张 12 格的判定表才能表驱动地测（§6.9）。调用方按 [`Decision`] 去干活：唯一会读盘的
//! 分支是 [`Decision::Confirm`]（§6.4），唯一会再读一次云端的分支是"决定覆盖本机"（§6.5）。
//!
//! ## 三方各有一个内容值与一个时间
//!
//! * **本机**（[`LocalState`]）：`digest` 要读一遍存档才算，平时只有 `mtime_ms`（只 `stat`）；
//! * **基线**（[`Baseline`]）：上一次对上账时的那一版；
//! * **云端最新**（[`CloudState`]）：索引里的 `latest` / `latest_digest`。
//!
//! ## 三条分工（§6.3 里写死的，不许混）
//!
//! * "本机动没动"在 **digest 已知时看 digest**、未知时先看 mtime，mtime 说"没动"的那一格
//!   走 §6.4 读内容核对（第 7 格 ⇒ [`Decision::Confirm`]）；
//! * 第 10 / 11 格的差别**只看 mtime 的方向**（超前 = 冲突，落后 = 结束后上传）；
//! * mtime 只用来**省一次读盘**和横幅措辞 —— 任何"覆盖还是不覆盖"的最后一道关都是 digest。
//!
//! ⚠ 第 4 格（没有基线、本机与内容一致）是 [`Decision::AdoptBaseline`]，**不弹窗**；
//! 第 9 格与第 11 格**都是** [`Decision::UploadLater`]（不下载，游戏结束后上传）。

// ⚠ 第 6 步（§6.10）把判定接进启动前流程之后删掉这一行：这一轮还没有人调用它，
// CI 的 `-D warnings` 会红在"没人用"上。
#![allow(dead_code)]

use super::baseline::Baseline;

/// 读不到云端索引时 [`Decision::NoSync`] 的理由（判定表第 1 格）。
pub const NO_INDEX_REASON: &str = "读不到云端索引";

/// 本机这一款现在的样子。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalState<'a> {
    /// 所有存档文件 `mtime_ms` 的最大值（[`crate::sync::archive::local_mtime_ms`]）。
    pub mtime_ms: i64,
    /// 需要时才算（§6.4）；**没算就是 `None`** —— 绝不许拿一个瞎猜的值顶替。
    pub digest: Option<&'a str>,
}

/// 云端最新那一版的样子（都是从索引里读来的，判定本身不联网）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudState<'a> {
    /// 索引读不到（离线、云端故障）。
    Unknown,
    /// 云端没有这一款，或者它 0 版。
    None,
    /// 云端有这一款：版本名 + 那一版的内容值（`None` = 老索引没记，**不知道**）。
    Known {
        stamp: &'a str,
        digest: Option<&'a str>,
    },
}

/// 要问用户时，问的是哪一类冲突。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// 还没有基线，而本机与云端内容不同：谁新谁旧无从判断。
    NoBaseline,
    /// 云端那一版的内容值不知道（老索引）：绝不拿版本名或时间戳猜。
    UnknownDigest,
    /// 双方都动了。
    BothChanged,
}

/// 判定给调用方的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// 本机、基线、云端三边一致：什么都不用做。
    Nothing,
    /// 拿云端那一版覆盖本机。⚠ 落盘之前还要走 §6.5（先重读索引、再下载核对）。
    Pull { stamp: String },
    /// 判定表第 7/8 格：mtime 看着没动、云端却偏离了基线 ⇒ **必须读过内容才敢覆盖**。
    ///
    /// 调用方算完本机 digest，带着 `Some(digest)` **再调一次** [`decide`] —— 那时它只会
    /// 给出 [`Decision::Pull`] 或 [`Decision::Ask`]（`BothChanged`），绝不直接覆盖（§6.4）。
    Confirm { stamp: String },
    /// 本机这一版留着，**游戏结束后**再上传（第 2 / 9 / 11 格）。
    UploadLater,
    /// 第一次同步且内容一致：把云端那一版认成基线，**不弹窗、不下载**（第 4 格）。
    AdoptBaseline { stamp: String, digest: String },
    /// 要问用户（[`ConflictKind`] 说明问的是哪一类）。
    Ask { kind: ConflictKind },
    /// 本次不自动同步（也**不许**在退出时自动上传，见 §6.6 闸门 b）。
    NoSync { reason: String },
}

/// 三方比一比，给出该干什么（纯函数）。
///
/// 每一格都对着 §6.3 的表实现，注释里的格号就是那张表的行号。
pub fn decide(
    local: &LocalState<'_>,
    baseline: Option<&Baseline>,
    cloud: &CloudState<'_>,
) -> Decision {
    // 第 1 格：云端索引读不到 —— 谁都不知道云端现在是什么，绝不猜（离线时尤其如此）。
    let (stamp, cloud_digest) = match cloud {
        CloudState::Unknown => {
            return Decision::NoSync {
                reason: NO_INDEX_REASON.to_string(),
            };
        }
        // 第 2 格：云端没有这一款（或它 0 版）⇒ 本机这一版等结束后上传。
        CloudState::None => return Decision::UploadLater,
        CloudState::Known { stamp, digest } => (*stamp, *digest),
    };

    // 第 3 格：云端有这一款，但那一版的内容值不知道（老索引）⇒ 问。
    // 绝不拿版本名或时间戳顶替：那是在猜"云端是不是同一版"。
    let Some(cloud_digest) = cloud_digest else {
        return Decision::Ask {
            kind: ConflictKind::UnknownDigest,
        };
    };

    // 第 4 / 5 格：还没有基线 —— 只剩下"本机与云端是不是同一版"这一个问题。
    let Some(baseline) = baseline else {
        return match local.digest {
            // 第 4 格：内容一致 ⇒ 认账（把云端那一版记成基线），**不弹窗、不下载**。
            Some(digest) if digest == cloud_digest => Decision::AdoptBaseline {
                stamp: stamp.to_string(),
                digest: cloud_digest.to_string(),
            },
            // 第 5 格：内容不同（**或者算不出来**）⇒ 没有参照物，只能问。
            _ => Decision::Ask {
                kind: ConflictKind::NoBaseline,
            },
        };
    };

    // 云端动没动：看内容值（`digest` 是唯一权威），不看时间戳。
    let cloud_moved = cloud_digest != baseline.digest;

    match local.digest {
        // 第 6 / 8 格：digest 已经算过、而且等于基线 ⇒ 本机**确实没动**（mtime 相不相同都一样）。
        Some(digest) if digest == baseline.digest => {
            if cloud_moved {
                Decision::Pull {
                    stamp: stamp.to_string(),
                }
            } else {
                Decision::Nothing
            }
        }
        // 第 6 / 7 格：digest 还没算、mtime 与基线一模一样 ⇒ 只有这里会为了判定去读一次内容（§6.4）。
        None if local.mtime_ms == baseline.mtime_ms => {
            if cloud_moved {
                Decision::Confirm {
                    stamp: stamp.to_string(),
                }
            } else {
                Decision::Nothing
            }
        }
        // 其余都是"看着本机动了"（digest 已知且不等于基线，或者 mtime 与基线不同）。
        _ => {
            // 第 9 格：云端没动 ⇒ 不下载，游戏结束后上传（超前、落后都一样）。
            if !cloud_moved {
                return Decision::UploadLater;
            }
            // 第 10 格：云端也动了、而且本机**超前** ⇒ 冲突，问用户。
            // 第 11 格：云端也动了、而本机**落后** ⇒ 结束后上传（本机这一版覆盖云端）。
            //
            // ⚠ mtime 恰好**相等**时规格两格都没覆盖（"超前"要 `>`、"落后"要 `<`）：这里按
            // "我们怕的是冲突"取问用户，绝不悄悄拿哪一边去覆盖另一边（已提给用户裁决）。
            if local.mtime_ms >= baseline.mtime_ms {
                Decision::Ask {
                    kind: ConflictKind::BothChanged,
                }
            } else {
                Decision::UploadLater
            }
        }
    }
}

#[cfg(test)]
// ⚠ 判据层的测试住在 `decision/` 目录里（`tests.rs` + `tests/rows.rs`，与
// `daemon/tests.rs` + `daemon/tests/` 同一个习惯）：§6.9 要求逐格都断言到，连着测试
// 一起数会越过 500 行的线（AGENTS.md）。
mod tests;
