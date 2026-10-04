//! 启动前的「谁新谁旧」：三方比较 → 该拉的才拉 → 写基线 → 立"本次已对上账"的牌子。
//!
//! PLATFORMS.md §6.3（判定表）、§6.4（覆盖前确认）、§6.5（先看再覆盖）、§6.6 b（闸门 b）。
//!
//! ## 这条路从前是什么样
//!
//! 从前 [`Daemon::sync_pull_before_launch`] 是"**无条件**把云端最新那一版拉下来铺上去"：
//! 只要云端有这一款，本机就被覆盖一次。现在换成**三方比较**（本机 / 基线 / 云端最新，
//! 各有一个内容值 `digest` 与一个时间 `mtime_ms`），判定（[`crate::sync::decision`]）说
//! "该拉"才拉、说"问"就问、说"不用动"就一个字节都不动。
//!
//! ## 三条不许动的规矩
//!
//! * **平时这条路绝不读存档内容**：只 `stat` 一遍拿 `mtime_ms`（[`archive::local_mtime_ms`]）。
//!   唯一会为了判定读内容的是 §6.4 那一格（[`Decision::Confirm`]）；
//! * **决定覆盖之后、落盘之前**强制重读一次云端索引并重跑判定（§6.5 第 1 步）：结论变了
//!   就按新结论走，**一个字节都不落盘**；下载之后还要拿包里的内容值与索引核对一次，
//!   对不上绝不铺（§6.5 第 3 步）；
//! * 铺完写基线、并立起"本次已对上账"的牌子（[`LaunchSync`]）；**不许自动上传**的那几格
//!   （`NoSync` / `Ask` / 取回失败）一个字都不写、牌子也不立（§6.6 闸门 b）。
//!
//! 文件分工：本文件是**流程本身**（判定 → 执行 → 写基线 → 立牌子）；
//! `launch_report.rs` 管"走完之后怎么报出去"（回包那几栏与人话文案）；
//! `launch_cloud.rs` 管"云端那一侧现在是什么样"（读索引 → `CloudState`）。

use std::collections::HashSet;
use std::sync::Mutex;

use serde_json::Value;

use super::launch_report::{LaunchReport, conflict_code, decision_code};
use super::{Daemon, PULL_TIMEOUT};
use crate::config::SyncConfig;
use crate::sync::SaveTarget;
use crate::sync::archive;
use crate::sync::baseline::{self, Baseline};
use crate::sync::decision::{self, ConflictKind, Decision, LocalState};
use crate::sync::runner::{GameOutcome, PullResult};

/// 「本次启动前**真的**跟云端对上过账了」的账本（§6.6 闸门 b）。
///
/// ## 生命周期（这块牌子什么时候立、什么时候撤）
///
/// * **立起来**（[`Self::settle`]）：`Nothing` / 取回成功 / `AdoptBaseline` /
///   `UploadLater` —— 也就是"这一局该知道的都知道了"。§6.8 B 的冲突弹窗里选
///   「保留本机」也属于这一列（第 10 步接上时调的就是同一个 `settle`）。
/// * **不立**：`NoSync`（读不到索引）、`Ask` 悬着（要问用户的那一问还没答）、
///   取回失败或被拒（铺不进去就不知道本机现在是什么），以及 §6.8 B 的「稍后再说」。
/// * **撤掉**（[`Self::forget`]）：**每一次启动前的对账一开始**就撤 —— 上一次那一局的
///   对账不能替这一局作证。退出上传**不**撤它：这块牌子的意思一直是"这一局已经对上过
///   账"，一局玩完传上去了，这句话仍然成立（界面上也就不会在刚传完之后反过来说
///   "没对上账"）。
///
/// ⚠ 作用域是**每款游戏 + 从上一次对账到现在**。⚠ **自动追踪**（游戏不是 kotori 启动的，
/// 见 [`crate::daemon::watch`]）从不走对账那条路，所以那一局的退出**不会**自动上传 ——
/// 这是闸门 b 的直接后果，也是"牌子必须由对账立起来"的代价（已提给用户裁决）。
#[derive(Default)]
pub(in crate::daemon) struct LaunchSync {
    settled: Mutex<HashSet<String>>,
}

impl LaunchSync {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// 立牌子：这一局启动前跟云端对上过账了。
    pub(super) fn settle(&self, game_id: &str) {
        if let Ok(mut settled) = self.settled.lock() {
            settled.insert(game_id.to_string());
        }
    }

    /// 撤牌子：这一局重新开始对账（每次启动前的第一件事）。
    pub(super) fn forget(&self, game_id: &str) {
        if let Ok(mut settled) = self.settled.lock() {
            settled.remove(game_id);
        }
    }

    /// 退出上传的那道闸门问的就是它。锁坏了就按"没对上账"算（保守方向：不传）。
    pub(super) fn is_settled(&self, game_id: &str) -> bool {
        self.settled
            .lock()
            .map(|settled| settled.contains(game_id))
            .unwrap_or(false)
    }

    /// 「保留本机」那条后果（§6.8 B ②）：**什么都不拉**，只把牌子立起来。
    ///
    /// 用户 2026-10-04 定的原话是"只把'本次允许上传'的牌子立起来 ⇒ 退出时上传"。
    /// 所以它与 [`Self::settle`] 是同一件事的两种说法 —— 这一层刻意留一个名字，
    /// 好让"谁把它立起来的"在调用点上看得出来（判定那条路立牌子 vs 用户在冲突弹窗里选的）。
    pub(in crate::daemon) fn allow_upload(&self, game_id: &str) {
        self.settle(game_id);
    }
}

impl Daemon {
    /// 启动前取回。`None` = 这一款根本不需要同步（总开关关着、这一款的开关关着、
    /// 没配存档位置、配置里没有它）—— 那些情况退出上传那边也各有一道自己的闸门。
    ///
    /// ⚠ 失败**绝不拦住启动**（用户要的是玩游戏）：每一种结论都变成回话里的一句话。
    pub(in crate::daemon) async fn sync_pull_before_launch(&self, game_id: &str) -> Option<Value> {
        // 每一次启动都**重新对账**：上一次那一局的牌子不能替这一局作证。
        self.sync.launch_sync.forget(game_id);

        let settings = {
            let config = self.config.read().await;
            if !config.sync.enabled {
                return None;
            }
            let game = config.games.get(game_id)?;
            if game.save_paths.is_empty() {
                return None;
            }
            // 这一款的开关关着 ⇒ **不自动取回**。手动「取回存档」是用户自己按的，
            // 走的是另一条路（`rpc_sync_restore`），不受这个开关限制。
            if !game.sync_enabled {
                tracing::debug!("{game_id}: 这一款的云同步开关关着，启动前不取回");
                return None;
            }
            config.sync.clone()
        };

        let (name, targets) = match self.sync_targets(game_id).await {
            Ok(pair) => pair,
            Err(error) => return Some(LaunchReport::unsettled("no_sync", error).json(game_id)),
        };

        let report = self
            .reconcile_before_launch(game_id, &settings, &name, &targets)
            .await;
        // 每一次启动都留一行：这是"这一局到底同步了什么"的入口（排查时按游戏名搜日志），
        // 也是"为什么没同步"那句答案。它和回包里那几栏说的是同一句话。
        tracing::info!(
            "{game_id}: 启动前同步的结论是 {}（要问的：{}）：{}",
            report.decision,
            report.ask.map(conflict_code).unwrap_or("-"),
            report.detail
        );
        if report.settled {
            self.sync.launch_sync.settle(game_id);
        }
        if report.outcome.is_none() {
            // 没真去取过也要留痕：与退出上传那条路同一条理由（用户 2026-09-28 报的
            // "界面在撒谎"）—— 跳过时什么都不记，界面上留着的还是**上一次**那句
            // "已取回 N 个位置"，而这一局其实什么都没做。
            self.sync
                .remember_note(game_id, "取回", report.settled, report.detail.clone());
        }
        Some(report.json(game_id))
    }

    /// 三方比较 → 按判定办事（§6.3、§6.4、§6.5）。
    async fn reconcile_before_launch(
        &self,
        game_id: &str,
        settings: &SyncConfig,
        name: &str,
        targets: &[SaveTarget],
    ) -> LaunchReport {
        // 本机这一版：**只 `stat`**（`local_mtime_ms` 一个字节的内容都不读）。
        let mtime_ms = match archive::local_mtime_ms(targets) {
            Ok(value) => value,
            // 连"本机现在长什么样"都读不到 ⇒ 这一局什么都不做，也**不立牌子**
            // （与判定表第 1 格同一条道理：不知道就别动）。
            Err(error) => {
                return LaunchReport::unsettled(
                    "no_sync",
                    format!("读不到本机存档的状态（{error}），本次不比较也不自动上传"),
                );
            }
        };

        // 本机缓存那份索引（零网络）+ 基线 + 本机 mtime ⇒ 判定。
        let baseline = baseline::load(game_id);
        let cloud = self.cloud_now(game_id, false).await;
        let mut local_digest: Option<String> = None;
        let mut decision = decision::decide(
            &LocalState {
                mtime_ms,
                digest: None,
            },
            baseline.as_ref(),
            &cloud.state(),
        );

        // §6.4：第 7 格（mtime 与基线一模一样、云端却偏离了基线）—— **唯一**为了判定
        // 读一次存档内容的地方。读完带着 `Some(digest)` 再判一次：那时只会给出
        // `Pull`（本机确实没动）或 `Ask(BothChanged)`（mtime 骗人，本机其实改了）。
        //
        // ⚠ 先问一句登记簿（`digest_cache`）：**同一个 mtime 下**这个值不用重算 —— 刚同步过
        //    的机器上，重启与冷却都不会变成"再读一遍整个存档"。登记簿里没有才真读一遍。
        if let Decision::Confirm { .. } = decision {
            let cached = self.sync.digests.get(game_id, mtime_ms);
            let computed = match cached {
                Some(digest) => Ok(digest),
                None => archive::local_digest(targets),
            };
            match computed {
                Ok(digest) => {
                    // 记下来：横幅（§6.8 A）与本款下一次对账都要用它，而它只在
                    // "真的算过"之后才有值。
                    self.sync.digests.remember(game_id, &digest, mtime_ms);
                    local_digest = Some(digest);
                    decision = decision::decide(
                        &LocalState {
                            mtime_ms,
                            digest: local_digest.as_deref(),
                        },
                        baseline.as_ref(),
                        &cloud.state(),
                    );
                }
                // 算不出内容值 ⇒ 既不敢覆盖本机（不知道它是不是基线那一版），也不敢
                // 上传（不知道本机偏离了多少）。**不立牌子**是第一位的。
                Err(error) => {
                    return LaunchReport::unsettled(
                        "no_sync",
                        format!("读不出本机存档的内容值（{error}），本次不覆盖也不自动上传"),
                    );
                }
            }
        }

        // §6.5 第 1 步：判定说"拿云端覆盖本机"⇒ **落盘之前**强制重读一次云端索引
        // （绕开本机缓存）并重跑判定。
        if let Decision::Pull { .. } = decision {
            let fresh = self.cloud_now(game_id, true).await;
            let again = decision::decide(
                &LocalState {
                    mtime_ms,
                    digest: local_digest.as_deref(),
                },
                baseline.as_ref(),
                &fresh.state(),
            );
            return match again {
                // 结论仍是"覆盖本机" ⇒ 走下载 → 核对 → 铺 → 写基线。
                Decision::Pull { stamp } => {
                    self.lay_down_cloud_version(
                        game_id,
                        settings,
                        name,
                        targets,
                        stamp,
                        fresh.digest(),
                    )
                    .await
                }
                // 结论变了 ⇒ 按新结论走，**一个字节都不落盘**。
                other => {
                    tracing::info!(
                        "{game_id}: 重读云端索引之后结论变了（{}），本次不动本机存档",
                        decision_code(&other)
                    );
                    self.apply_verdict(game_id, other, mtime_ms).await
                }
            };
        }

        self.apply_verdict(game_id, decision, mtime_ms).await
    }

    /// 判定说"不覆盖本机"时的处理（§6.3 里除 `Pull` 以外的每一格）。
    async fn apply_verdict(
        &self,
        game_id: &str,
        decision: Decision,
        mtime_ms: i64,
    ) -> LaunchReport {
        match decision {
            // 第 6 格：三边一致。
            Decision::Nothing => {
                LaunchReport::settled("nothing", "本机、基线、云端三边一致，什么都不用做")
            }
            // 第 2 / 9 / 11 格：不下载，游戏结束后上传。
            Decision::UploadLater => {
                LaunchReport::settled("upload_later", "云端没动，本机这一版在游戏结束后上传")
            }
            // 第 4 格：第一次同步、内容一致 ⇒ **静默**建立基线（不弹窗、不下载）。
            Decision::AdoptBaseline { stamp, digest } => {
                if let Err(error) = baseline::save(game_id, &Baseline::new(stamp, digest, mtime_ms))
                {
                    // 基线没写下去只是"下次启动会重新核对"，不是这一局的问题：本机与云端
                    // 的内容值刚刚比过，一致（§6.6 第 3 步同一条道理）。
                    tracing::warn!("{game_id}: 基线没写下去（下次启动会重新核对）: {error}");
                }
                LaunchReport::settled(
                    "adopt_baseline",
                    "第一次对账：本机与云端内容一致，已记下基线",
                )
            }
            // 第 3 / 5 / 10 格：要问用户。**不下载、不自动上传**。
            Decision::Ask { kind } => LaunchReport::asking(kind),
            // 第 1 格：读不到云端索引 —— 照常启动，但本次不比较也不自动上传。
            Decision::NoSync { reason } => LaunchReport::unsettled("no_sync", reason),
            // `Pull` 那条路在 `reconcile_before_launch` 里走完了；`Confirm` 到不了这里
            // （第 7 格在那边就已经解开了 —— `Pull` 那条路上 digest 一定是 `Some`）。
            // 真到了也只当"要问"处理：绝不在这里悄悄覆盖本机。
            Decision::Pull { .. } | Decision::Confirm { .. } => {
                tracing::warn!(
                    "{game_id}: 判定给出了不该走到这里的结论（{}），按「要问」处理",
                    decision_code(&decision)
                );
                LaunchReport::asking(ConflictKind::BothChanged)
            }
        }
    }

    /// §6.5 的第 2～5 步：下载那一版 → 核对内容值 → 铺进本机 → 写基线。
    async fn lay_down_cloud_version(
        &self,
        game_id: &str,
        settings: &SyncConfig,
        name: &str,
        targets: &[SaveTarget],
        stamp: String,
        expected: Option<&str>,
    ) -> LaunchReport {
        // 索引自己也不知道那一版的内容值 ⇒ 按第 3 格处理：**要问**，绝不硬铺。
        // （第一遍判定就会走这条路；这里是"重读之后才知道"的那一小段窗口。）
        let Some(expected) = expected.map(str::to_string) else {
            return LaunchReport::asking(ConflictKind::UnknownDigest);
        };
        let runner = match self.sync_runner(settings) {
            Ok(runner) => runner,
            Err(error) => return LaunchReport::unsettled("pull", error),
        };
        let cloud_key = match self.cloud_key_of(game_id).await {
            Ok(key) => key,
            Err(error) => return LaunchReport::unsettled("pull", error),
        };
        // ⚠ 取回那条路**绝不认领身份**：认领是上传的事。没认领过就是"还没配对"，
        // 防错配闸据此拒绝铺文件（宁可不动，也不猜）——见 `crate::sync::cloud`。
        let cloud_id = self.cloud_id_of(game_id).await.unwrap_or(None);

        let pulled = tokio::time::timeout(
            PULL_TIMEOUT,
            runner.pull(
                game_id,
                name,
                &cloud_key,
                targets,
                cloud_id.as_deref(),
                // §6.5 第 3 步：包里那一版的内容值必须等于索引里的 `latest_digest`。
                Some(&expected),
            ),
        )
        .await;

        let outcome = match pulled {
            Ok(PullResult::Laid(outcome)) => outcome,
            Ok(PullResult::Refused(outcome)) => {
                self.sync.remember(game_id, "取回", &outcome);
                let detail = outcome
                    .error
                    .clone()
                    .unwrap_or_else(|| "没能取回云端存档".to_string());
                return LaunchReport::pulled(outcome, false, detail);
            }
            Err(_) => {
                tracing::warn!("{game_id}: 启动前拉取超时（{PULL_TIMEOUT:?}），直接启动游戏");
                let detail = format!("拉取超过 {} 秒，已跳过", PULL_TIMEOUT.as_secs());
                let outcome = GameOutcome::failed(game_id, name, detail.clone());
                self.sync.remember(game_id, "取回", &outcome);
                return LaunchReport::pulled(outcome, false, detail);
            }
        };
        self.sync.remember(game_id, "取回", &outcome);

        // §6.5 第 5 步：基线跟上 —— **包的**内容值，和**落地之后重新 stat** 出来的 mtime
        // （不是决定覆盖之前那一次的量：文件刚刚被铺过）。
        let mtime_ms = archive::local_mtime_ms(targets).unwrap_or_else(|error| {
            tracing::warn!("{game_id}: 取回之后 stat 本机存档失败（基线里记 0）: {error}");
            0
        });
        // 铺下去的**就是** `expected` 那一版的内容 ⇒ 本机现在的内容值就是它。记进登记簿：
        // 横幅（§6.8 A）因此不必为了显示它再读一遍存档（见 `sync::digest_cache`）。
        self.sync.digests.remember(game_id, &expected, mtime_ms);
        if let Err(error) =
            baseline::save(game_id, &Baseline::new(stamp.clone(), expected, mtime_ms))
        {
            tracing::warn!("{game_id}: 取回成功但基线没写下去（下次启动会重新核对）: {error}");
        }
        tracing::info!("{game_id}: 已用云端 {stamp} 覆盖本机存档，基线跟着走");
        LaunchReport::pulled(outcome, true, format!("已用云端 {stamp} 覆盖本机存档"))
    }

    /// 上传成功 ⇒ 基线跟上（§6.6 第 3 步）。
    ///
    /// 这是"更新基线"的另一半（一半是取回成功，见 [`Self::lay_down_cloud_version`]）。
    /// 上传**失败或跳过**时一个字都不动：下次启动会把同一件事重新问一遍（§6.6 第 4 步）。
    ///
    /// ⚠ 手动「立即同步」也走这里（`actions::rpc_sync_now`）：它同样真的把这一版推上了云，
    /// 不写基线的话，下一次启动会把"我自己刚传上去的那一版"当成"云端动了"再拉回来。
    pub(in crate::daemon) fn note_uploaded_baseline(
        &self,
        game_id: &str,
        outcome: &GameOutcome,
        targets: &[SaveTarget],
    ) {
        if !outcome.ok {
            return;
        }
        let (Some(stamp), Some(digest)) = (outcome.stamp.as_deref(), outcome.digest.as_deref())
        else {
            // 没真的上云（空包 A0、或者引擎压根没传）：没有"新的一版"可以认。
            return;
        };
        let mtime_ms = archive::local_mtime_ms(targets).unwrap_or_else(|error| {
            tracing::warn!("{game_id}: 上传后 stat 本机存档失败（基线里记 0）: {error}");
            0
        });
        // 刚上云的就是**本机这一版**（打包时算出来的那个值）⇒ 登记簿跟着走，
        // 横幅（§6.8 A）于是不必为了显示"本机的内容值"再读一遍存档。
        self.sync.digests.remember(game_id, digest, mtime_ms);
        if let Err(error) = baseline::save(game_id, &Baseline::new(stamp, digest, mtime_ms)) {
            tracing::warn!("{game_id}: 上传成功但基线没写下去（下次启动会重新核对）: {error}");
        }
    }

    /// 观测会话（游戏**不是** kotori 启动的）建立时的那一次"对账"。
    ///
    /// 用户 2026-10-05 裁决：这条路也要立牌子，否则"双击图标启动的游戏"那一局退出后
    /// **不会**自动上传 —— 那正是用户 2026-09-28 报过的 bug（`PLATFORMS.md` §0.1 第 16 条
    /// 与 `exit_upload.rs` 文件头），而 [`crate::daemon::watch`] 存在的意义就是覆盖这条路。
    ///
    /// 与启动前那条路共用判定（[`decision::decide`]）与那块牌子（[`LaunchSync`]），
    /// 但有三条只属于这里的硬要求：
    ///
    /// * **绝不取回**：游戏已经在跑了，铺存档比不铺危险得多。所以 `Pull` **不立牌子**
    ///   （云端有更新的一版时，这一局退出后也不自动上传 —— 拿本机去盖新云端正是冲突）；
    /// * **绝不读存档内容**：只 `stat` 一遍拿 `mtime_ms`。所以 `Confirm` 那一格**不立牌子**
    ///   —— 它要读一遍内容才知道"本机动没动"，而为了这一局的自动上传去读一个**正在被
    ///   写**的存档，不值得（读到半截还会得出一个错的"本机改了"）；
    /// * **安静**：这条挂在自动追踪那圈 2 秒轮询上，只留 `debug`/`warn` 日志，绝不弹窗、
    ///   绝不报错给用户（"为什么这一局没立牌子"从日志里看得出来就够了）。
    ///
    /// ⚠ 判定层的判据一个字都没改：这里只是**换一种调用方式**（不带 digest），于是第 7
    /// 格给出的 `Confirm` 天然落进"不立牌子"那一档。
    pub(in crate::daemon) async fn settle_from_observation(&self, game_id: &str) {
        // 与启动前那条路同一套早退判据。
        {
            let config = self.config.read().await;
            if !config.sync.enabled {
                tracing::debug!("{game_id}: 观测会话不对账（云同步总开关关着）");
                return;
            }
            let Some(game) = config.games.get(game_id) else {
                tracing::debug!("{game_id}: 观测会话不对账（配置里没有这一款）");
                return;
            };
            if game.save_paths.is_empty() {
                tracing::debug!("{game_id}: 观测会话不对账（这一款没填存档位置）");
                return;
            }
            if !game.sync_enabled {
                tracing::debug!("{game_id}: 观测会话不对账（这一款的云同步开关关着）");
                return;
            }
        }

        // **先撤掉上一次那一局的牌子**：这一次对账说"不知道"的时候，那块旧牌子不许继续
        // 作数（生命周期见 [`LaunchSync`] 头上那一段）。
        self.sync.launch_sync.forget(game_id);

        let targets = match self.sync_targets(game_id).await {
            Ok((_, targets)) => targets,
            Err(error) => {
                tracing::debug!("{game_id}: 观测会话不对账（存档位置解析不出来）: {error}");
                return;
            }
        };
        let mtime_ms = match archive::local_mtime_ms(&targets) {
            Ok(value) => value,
            // 连"本机现在长什么样"都读不到：这一局什么都不做，牌子也不立。
            Err(error) => {
                tracing::warn!(
                    "{game_id}: 观测会话读不到本机存档的状态（本次不自动上传）: {error}"
                );
                return;
            }
        };

        let baseline = baseline::load(game_id);
        let cloud = self.cloud_now(game_id, false).await;
        let verdict = decision::decide(
            &LocalState {
                mtime_ms,
                digest: None,
            },
            baseline.as_ref(),
            &cloud.state(),
        );

        match verdict {
            // 该知道的都知道了：立牌子，退出时照旧自动上传。
            Decision::Nothing | Decision::UploadLater => {
                self.sync.launch_sync.settle(game_id);
                tracing::debug!(
                    "{game_id}: 观测会话已对上账（{}），这一局退出后照旧自动上传",
                    decision_code(&verdict)
                );
            }
            // 第 4 格在这条路上**到不了**（它要求本机的 digest 已知，而这里绝不读内容）；
            // 真到了就照启动前那条路办：静默记下基线，然后立牌子。
            Decision::AdoptBaseline { stamp, digest } => {
                if let Err(error) = baseline::save(game_id, &Baseline::new(stamp, digest, mtime_ms))
                {
                    tracing::warn!("{game_id}: 观测会话没能记下基线（下次会重新核对）: {error}");
                }
                self.sync.launch_sync.settle(game_id);
                tracing::debug!("{game_id}: 观测会话第一次对账，已记下基线");
            }
            // `Pull` / `Confirm` / `Ask` / `NoSync`：**不立牌子**，并说清是哪一格。
            other => {
                tracing::debug!(
                    "{game_id}: 观测会话没能对上账（{}），这一局退出后不自动上传",
                    decision_code(&other)
                );
            }
        }
    }
}
