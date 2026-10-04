//! 启动前取回：把云端最新那一版里**比本机新**的文件取回来（`pull`）。
//!
//! ADR-012 的落点。从前这条不变量是 `rclone --update` 保证的——它逐文件比较，
//! 只覆盖更新的那些。改成一版一包之后，rclone 不再看文件，所以这条保证搬到了
//! 我们自己手里：下载最新包、按清单逐文件比较、只铺 `plan.take`。
//!
//! 报价必须诚实：超时或失败要报成"没取完"，**绝不能报成"云端没有存档"**——
//! 后者听起来像是一切正常，用户会以为自己的进度已经在云上了。

use std::collections::HashMap;

use super::staging::Staging;
use super::{GameOutcome, LocationOutcome, PULL_TIMEOUT, Runner};
use crate::sync::archive::{self, Merge};
use crate::sync::cloud::identity_match;

/// 一次启动前取回的结果（PLATFORMS.md §6.5）。
///
/// 两分是刻意的：**只有 [`PullResult::Laid`] 表示本机存档真的动过**。
/// `Refused` 覆盖了"引擎/网络出事"、"防错配闸拦下"、"内容值核对没通过"以及
/// "云端还没有这一款" —— 最后那一种的 `GameOutcome::ok` 是 `true`（没什么可做不是
/// 错误），所以**调用方光看 `ok` 分不出"铺了"与"什么都没干"**。而这件事要紧：
/// 铺过 ⇒ 要写基线（§6.5 第 5 步），没铺 ⇒ 一个字节都不许写。
#[derive(Debug)]
pub enum PullResult {
    /// 铺完了（本机存档动过）。
    Laid(GameOutcome),
    /// **一个字节都没铺**：原因在 `outcome.error` 里（给人看的那一句）。
    Refused(GameOutcome),
}

impl PullResult {
    /// 摊平成那一次的结果（两种情况下都有 `GameOutcome`：`Refused` 那一半里带着给人看的
    /// 原因，`Laid` 那一半里带着每个位置做了什么）。
    ///
    /// ⚠ 只给测试用：生产那两个调用方（`daemon::sync_rpc::launch_sync`）必须**先分**这两
    /// 种——"铺过了"要写基线、"没铺"一个字都不许写，只看 `GameOutcome::ok` 分不出来
    /// （"云端还没有这一款"也是 `ok == true`）。
    #[cfg(all(test, unix))]
    pub fn into_outcome(self) -> GameOutcome {
        match self {
            Self::Laid(outcome) | Self::Refused(outcome) => outcome,
        }
    }
}

impl Runner {
    /// Fetch anything that is *newer* in the cloud, keeping newer local files.
    ///
    /// Used before a launch: a slow network or a broken package must never turn
    /// into "the game did not start", so every failure here is reported, not
    /// raised, and the caller launches anyway.
    ///
    /// ⚠ `local_cloud_id` 是**防错配闸**：云端那一版的身份与本机这一款不一致
    /// （或者两边缺一头）时，这一版**一个文件都不会铺** —— 见 [`identity_match`]。
    /// 这条是踩过的坑的反面：两台机器给两款不同游戏起的名字撞在一起时，静默铺过去
    /// 就是**静默损坏存档**。
    ///
    /// ⚠ `expected_digest` 是 §6.5 第 3 步那道**内容值核对**：`Some(d)` 表示"索引说
    /// 云端最新那一版的内容值是 `d`"，于是包里的 `manifest.digest` 必须**也**是它 ——
    /// 对不上就是"包被人换了，或者索引在骗人"，这一版绝不铺。闸门在**铺文件之前**：
    /// 那时清单已经读出来了，本机还一个字节都没动。
    pub async fn pull(
        &self,
        game_id: &str,
        name: &str,
        cloud_key: &str,
        targets: &[crate::sync::SaveTarget],
        local_cloud_id: Option<&str>,
        expected_digest: Option<&str>,
    ) -> PullResult {
        if let Err(error) = self.ready() {
            return PullResult::Refused(GameOutcome::failed(game_id, name, error.to_string()));
        }

        let stamp = match self.latest_package(cloud_key).await {
            Ok(Some(stamp)) => stamp,
            // Nothing has ever been uploaded: not an error, just nothing to do.
            // ⚠ 但"没铺过"这件事必须如实说出去（所以仍然是 `Refused`）：调用方据此
            //   决定不写基线、也不立"本次已对上账"的牌子。
            Ok(None) => {
                return PullResult::Refused(GameOutcome::from_locations(
                    game_id,
                    name,
                    targets
                        .iter()
                        .map(|target| {
                            LocationOutcome::new(target, "skipped", "云端还没有这个游戏的存档")
                        })
                        .collect(),
                ));
            }
            Err(error) => {
                return PullResult::Refused(GameOutcome::failed(game_id, name, error.to_string()));
            }
        };

        let staging = match Staging::new(self.work_dir()) {
            Ok(staging) => staging,
            Err(error) => return PullResult::Refused(GameOutcome::failed(game_id, name, error)),
        };
        let manifest = match self
            .fetch_version(cloud_key, &stamp, &staging.unpacked(), PULL_TIMEOUT)
            .await
        {
            Ok(manifest) => manifest,
            Err(error) => {
                return PullResult::Refused(GameOutcome::failed(
                    game_id,
                    name,
                    format!("没能取回云端存档 {stamp}（这一局照常启动）: {error}"),
                ));
            }
        };
        // 闸门在**铺文件之前**：清单已经读出来了，本机还一个字节都没动。
        if let Some(refusal) = identity_match(local_cloud_id, manifest.identity.as_ref()).refusal()
        {
            return PullResult::Refused(GameOutcome::failed(
                game_id,
                name,
                format!("{refusal}（云端最新那一版是 {stamp}，这一局照常启动）"),
            ));
        }
        // §6.5 第 3 步：包的内容值必须等于索引里那一版的值。对不上就**丢弃 staging**
        // （`staging` 在这里析构，整个临时目录跟着走）+ 报错，绝不铺。
        // `expected_digest` 是 `None` 表示这次不核对（老调用方/手动那条路）—— 判定那条
        // 路永远带着 `Some`，它的 `None`（索引自己也不知道）在第 3 格就变成"要问"了。
        if let Some(expected) = expected_digest
            && manifest.digest.as_deref() != Some(expected)
        {
            return PullResult::Refused(GameOutcome::failed(
                game_id,
                name,
                format!(
                    "云端 {stamp} 那一版的内容值与索引对不上（索引说 {}，包里是 {}）—— \
                     这一版可能被人换过，本机存档一个都没动",
                    short_digest(Some(expected)),
                    short_digest(manifest.digest.as_deref())
                ),
            ));
        }
        let plan = match archive::plan(&manifest, targets, Merge::Newer) {
            Ok(plan) => plan,
            Err(error) => {
                return PullResult::Refused(GameOutcome::failed(
                    game_id,
                    name,
                    format!("合并判定失败: {error}"),
                ));
            }
        };
        if let Err(error) = staging.lay_down(targets, &plan) {
            return PullResult::Refused(GameOutcome::failed(
                game_id,
                name,
                format!("写入本机存档失败: {error}"),
            ));
        }

        let mut taken: HashMap<&str, usize> = HashMap::new();
        for entry in &plan.take {
            *taken.entry(entry.key.as_str()).or_default() += 1;
        }
        let mut kept: HashMap<&str, usize> = HashMap::new();
        for entry in &plan.kept {
            *kept.entry(entry.key.as_str()).or_default() += 1;
        }

        let outcomes = targets
            .iter()
            .map(|target| {
                if !manifest.has_location(&target.key) {
                    return LocationOutcome::new(target, "skipped", "云端还没有这个位置的存档");
                }
                let taken = taken.get(target.key.as_str()).copied().unwrap_or(0);
                let kept = kept.get(target.key.as_str()).copied().unwrap_or(0);
                if taken > 0 {
                    LocationOutcome::new(
                        target,
                        "pulled",
                        format!("已取回云端较新的 {taken} 个文件（{stamp}）"),
                    )
                } else if kept > 0 {
                    // 这一条是 ADR-012 说出口的地方：本机更新就不动。
                    LocationOutcome::new(
                        target,
                        "kept",
                        format!("本机的 {kept} 个文件更新，保持不动"),
                    )
                } else {
                    LocationOutcome::new(target, "pulled", format!("云端 {stamp} 里这个位置是空的"))
                }
            })
            .collect();

        PullResult::Laid(GameOutcome::from_locations(game_id, name, outcomes))
    }
}

/// 一个内容值的头 8 位（给人看的错误里用它）；没有就是"没有"。
///
/// ⚠ 用 `get(..8)` 而不是切片下标：桶里的清单是**外部输入**，一个多字节字符卡在第 8 个
/// 字节上就会让 `&value[..8]` 直接 panic —— 报错的那条路不该能被打断。
fn short_digest(digest: Option<&str>) -> &str {
    match digest {
        Some(value) if !value.is_empty() => value.get(..8).unwrap_or(value),
        _ => "没有",
    }
}

#[cfg(all(test, unix))]
// ⚠ 测试住在同一目录的另一个文件里（与 `index_tests` / `digest_tests` 同一个习惯）：
// `pull.rs` 连着那三百行测试会越过 500 行的线（AGENTS.md）。
#[path = "pull_tests.rs"]
mod pull_tests;
