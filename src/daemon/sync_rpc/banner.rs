//! `sync.snapshot`：横幅要的那一份"三方现在长什么样"（PLATFORMS.md §6.8 A）。
//!
//! 两处页面（单游戏设置页的「云存档」组、云端清单点开某一款的那一页）进页面时**异步**问它
//! 一次，算的过程中界面显示「正在核对…」，算完再填那四行。⚠ **绝不**在"点进去之前先算"
//! （用户 2026-10-04 明确要求：那会卡住页面）。
//!
//! ## 回包里放什么、不放什么
//!
//! 只放**事实**（本机 / 云端 / 基线各是什么）与判定给的那一格（`decision`），**不放文案**：
//! 四行怎么写由界面层 `ui::model::banner` 决定（那样四条文案才测得住，见 §6.9 的 UI 族）。
//! 判定本身走 [`crate::sync::decision::decide`] —— **判据只有一份**，这里一个分支都不加。
//!
//! ## 那一处规格冲突（§6.8 A 的版式 vs §6.1/§6.4 的"平时绝不读存档内容"）
//!
//! §6.8 A 要横幅显示"本机 digest 前 8 位"，而 §6.1/§6.4 反复强调**平时绝不读存档内容**
//! （读一次 = 把该游戏所有存档全文读一遍）。这里取的是：
//!
//! * 本机那一栏的 `digest` **只从** [`crate::sync::digest_cache`] 里取 —— 也就是**只有真的
//!   算过**（§6.4 的核对、取回成功、手动恢复之后顺手记下的那一笔）才有值；
//! * 没算过就如实交 `None`，界面上写「—（未核对）」；
//! * 本机存在且**还没有基线**时，基线那一行的内容值就是本机的内容值（§6.3 第 4/5 格那张
//!   表的语义：没有基线时"本机 == 云端"是全等的），所以那一格直接填基线的值；
//! * **不为了横幅去读一遍存档**：这条路平时只 `stat`（`archive::local_mtime_ms`），
//!   云端那一栏只读**本机缓存**那份索引（零网络）。
//!
//! 代价写在明处：**第一次进页面时本机那一栏的 digest 通常是空的**（那一趟没核对过），要等
//! 一次真正的核对 / 取回 / 恢复之后才会填上。取这个做法的理由是 §6.1/§6.4 那两条硬要求：
//! 横幅是**每次进页面**都要画的东西，而读一遍存档不是（另一条路"进页面读一遍存档"与
//! "平时绝不读内容"直接冲突，已提给用户裁决，见交接回报）。
//!
//! ## 缓存过期就自动刷新一次（§6.11 第 2 条）
//!
//! 手上那份索引的 `cached_at` 超过 [`CACHE_TTL`] ⇒ **自动去云端读一次**（`refresh = true`）；
//! 只有"**过期且刷新失败**"才在回包里带上 `stale` + `refresh_error` + `cached_at`，界面据此
//! 写「索引已过期，刷新失败」。⚠ 刷新**绝不阻塞页面**：这条路本身是异步的，失败就带着手上
//! 那份旧索引照常出横幅。

use super::launch_cloud::CloudNow;
use super::launch_report::decision_code;
use super::*;
use crate::sync::archive;
use crate::sync::baseline::{self, Baseline};
use crate::sync::decision::{self, Decision, LocalState};
use crate::sync::index_cache::CACHE_TTL;

/// `sync.snapshot` 的一个回包（字段名与 §6.3 的三方一一对应）。
///
/// ⚠ 字段是 `pub(super)`：`CloudNow` 本身就只在这一族里可见（`launch_cloud`），
/// 把字段放到 `crate::daemon` 那一层会让这个类型比它自己还公开。
pub(in crate::daemon) struct Snapshot {
    pub(super) local_mtime_ms: i64,
    /// **本机为空**（所有存档位置都不在）：既没有可传的，也没有可覆盖的（§6.11 第 3 条）。
    pub(super) local_empty: bool,
    /// 本机的内容值 —— **只有算过才有**（见文件头那段）。
    pub(super) local_digest: Option<String>,
    pub(super) cloud: CloudNow,
    pub(super) cloud_key: String,
    pub(super) baseline: Option<Baseline>,
    pub(super) decision: Decision,
    /// 这份索引来自本机缓存（这一趟**没有**打网络）。
    pub(super) from_cache: bool,
    /// 缓存什么时候拿下来的（`stamp` 形状）。
    pub(super) cached_at: String,
    /// 手上这份索引已经**过期**（超过 [`CACHE_TTL`]，或者本地压根还没有缓存）。
    pub(super) stale: bool,
    /// 这一次**为了横幅**去刷新索引失败了（只有过期才会去刷）。
    pub(super) refresh_error: Option<String>,
    /// 读取过程里那些"不致命但要说"的事（存档位置解析不出来之类）。
    pub(super) problem: Option<String>,
}

impl Snapshot {
    /// 交给客户端的 JSON。**只放事实与判定名**，四行文案由界面拼（见文件头）。
    pub(in crate::daemon) fn json(self, game_id: &str) -> Value {
        let (cloud_state, cloud_stamp, cloud_digest) = match &self.cloud {
            CloudNow::Unknown => ("unknown", Value::Null, Value::Null),
            CloudNow::None => ("none", Value::Null, Value::Null),
            CloudNow::Known { stamp, digest } => (
                "known",
                json!(stamp),
                digest.as_ref().map_or(Value::Null, |d| json!(d)),
            ),
        };
        let baseline = self.baseline.as_ref().map(|baseline| {
            json!({
                "stamp": baseline.stamp,
                "digest": baseline.digest,
                "mtime_ms": baseline.mtime_ms,
            })
        });
        json!({
            "game_id": game_id,
            "local": {
                "mtime_ms": self.local_mtime_ms,
                "empty": self.local_empty,
                // `null` = **没算过**（不是"算出来是空"）—— 界面据此写「—（未核对）」。
                "digest": self.local_digest,
            },
            "cloud": {
                "state": cloud_state,
                "stamp": cloud_stamp,
                "digest": cloud_digest,
                "key": self.cloud_key,
            },
            "baseline": baseline,
            "decision": decision_code(&self.decision),
            "index": {
                "from_cache": self.from_cache,
                "cached_at": self.cached_at,
                "stale": self.stale,
                "refresh_error": self.refresh_error,
            },
            "problem": self.problem,
        })
    }
}

impl Daemon {
    /// `sync.snapshot`：三方各是什么 + 判定给哪一格（**只读**，不动任何存档、不写基线）。
    ///
    /// 读不到本机 / 读不到云端都**不报错**：横幅是只读的展示，读不到就如实说读不到 ——
    /// 界面上那句「本次不自动同步」正是它。
    pub(in crate::daemon) async fn rpc_sync_snapshot(
        &self,
        game_id: &str,
    ) -> Result<Value, String> {
        Ok(self.sync_snapshot(game_id).await.json(game_id))
    }

    /// [`Self::rpc_sync_snapshot`] 的实体（测试直接叫它，免得为了一个 `Value` 再解一遍 JSON）。
    pub(in crate::daemon) async fn sync_snapshot(&self, game_id: &str) -> Snapshot {
        // 本机这一版：**只 `stat`**（一个字节的内容都不读）。
        let targets = self.sync_targets(game_id).await.map(|(_, targets)| targets);
        let (mtime_ms, local_empty, problem) = match &targets {
            Ok(targets) => match archive::local_mtime_ms(targets) {
                // 「本地为空」= **所有存档位置都不在**。空**目录**（位置在、里面一个文件都没有）
                // 也会给出 `mtime_ms == 0`，但那是"位置配好了、游戏还没写过存档" —— 与
                // "位置本身不在"（盘没插）不是一件事，不当作空的。
                Ok(value) => (
                    value,
                    value == 0 && targets.iter().all(|target| !target.local.is_dir()),
                    None,
                ),
                Err(error) => (0, false, Some(format!("读不到本机存档的状态（{error}）"))),
            },
            Err(error) => (0, false, Some(error.clone())),
        };

        let baseline = baseline::load(game_id);
        // 本机的内容值：**只从登记簿里取**（见文件头那段冲突）；没有基线时基线那一行就是
        // 本机这一版（§6.3 第 4/5 格），所以那一格可以直接填基线的值。
        let local_digest = problem
            .is_none()
            .then(|| {
                self.sync
                    .digests
                    .get(game_id, mtime_ms)
                    .or_else(|| baseline.as_ref().map(|b| b.digest.clone()))
            })
            .flatten();

        // ── 索引：本地缓存优先；过期就自动刷新一次（§6.11 第 2 条）──
        // ⚠ **一次采样，一处真相**：这一趟用哪一份索引，就报那一份的 `cached_at` / `from_cache`；
        //   刷新失败时照旧用**刷新前**手上那份（它才是横幅上显示的那个云端）。
        let cached = self.cloud_index_view(false).await.ok();
        let mut index_from_cache = cached.as_ref().map(|view| view.from_cache).unwrap_or(false);
        let mut cached_at = cached
            .as_ref()
            .map(|view| view.cached_at.clone())
            .unwrap_or_default();
        let stale = match &cached {
            Some(view) => cache_expired(&view.cached_at),
            // 连缓存都读不到（云同步没配齐）：当作过期，去刷一次。
            None => true,
        };
        let mut refresh_error = None;
        if stale {
            match self.cloud_index_view(true).await {
                Ok(fresh) => {
                    index_from_cache = fresh.from_cache;
                    cached_at = fresh.cached_at;
                }
                Err(error) => {
                    tracing::debug!(
                        "{game_id}: 索引过期了，自动刷新失败（照旧用手上那份）: {error}"
                    );
                    refresh_error = Some(error);
                }
            }
        }

        // 云端那一侧：就用上面这一份（**不再多读一次**）。
        let cloud = self.cloud_now(game_id, false).await;
        let cloud_key = self.cloud_key_of(game_id).await.unwrap_or_default();
        let decision = match &problem {
            // 连"本机现在长什么样"都读不到 ⇒ 什么都不做（与启动前那条路同一个方向）。
            Some(problem) => Decision::NoSync {
                reason: problem.clone(),
            },
            None => decision::decide(
                &LocalState {
                    mtime_ms,
                    digest: local_digest.as_deref(),
                },
                baseline.as_ref(),
                &cloud.state(),
            ),
        };
        Snapshot {
            local_mtime_ms: mtime_ms,
            local_empty,
            local_digest,
            cloud,
            cloud_key,
            baseline,
            decision,
            from_cache: index_from_cache,
            cached_at,
            stale,
            refresh_error,
            problem,
        }
    }
}

/// 这一份缓存**过期了没有**：超过 [`CACHE_TTL`] 就是过期（§6.11 第 2 条）。
///
/// 时间戳读不懂（不该发生）也算过期 —— 读不懂就别拿它当"新鲜"，去刷一次是唯一安全的动作。
pub(in crate::daemon) fn cache_expired(cached_at: &str) -> bool {
    match crate::sync::stamp_time(cached_at) {
        Some(at) => chrono::Utc::now()
            .signed_duration_since(at)
            .to_std()
            .map(|age| age > CACHE_TTL)
            // 时钟倒着走（缓存"来自未来"）也算过期：宁可信它旧。
            .unwrap_or(true),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::index::stamp;

    #[test]
    fn a_cache_older_than_the_ttl_is_expired() {
        // 刚写的当然不过期（`stamp()` 是秒精度，TTL 是一个钟头 —— 差几秒不可能越界）。
        assert!(!cache_expired(&stamp()), "刚拿到的缓存不该算过期");
        assert!(cache_expired(""), "读不懂的时间戳按过期算");
        assert!(cache_expired("not-a-stamp"), "读不懂的时间戳按过期算");
        // 一个明显的旧时间（2020 年）当然过期。
        assert!(cache_expired("20200101T000000Z"));
    }

    /// 云同步**没配齐**（桶名空着）时 `sync.snapshot` 照样回话：云端那一栏是"读不到索引"、
    /// 判定落到 [`Decision::NoSync`]、而且它**不报错** —— 横幅是只读的展示，读不到就如实
    /// 说读不到（界面上那句「本次不自动同步」正是它）。
    ///
    /// ⚠ 这一条**不碰网络**：走的是"索引缓存读不到"那条早退（签名都算不出来）。
    #[tokio::test]
    async fn an_unconfigured_target_still_answers_with_no_sync() {
        let mut config = crate::config::Config::default();
        config.sync.enabled = true;
        // 故意留空 bucket：`crate::sync::signature::of` 会给 Err。
        let fake = crate::secrets::testing::FakeTool::new("sync-banner");
        let daemon = crate::daemon::Daemon::with_keyring(config, fake.keyring());
        let reply = daemon.rpc_sync_snapshot("demo").await.expect("不许报错");
        assert_eq!(reply["decision"], "no_sync", "{reply}");
        assert_eq!(reply["cloud"]["state"], "unknown", "{reply}");
        // 本机读不到存档（这一款压根不在配置里）⇒ 那一栏没有内容值可显示，
        // **绝不是**一个空串当"算出来是空的"。
        assert!(reply["local"]["digest"].is_null(), "{reply}");
        assert!(reply["baseline"].is_null(), "{reply}");
    }
}
