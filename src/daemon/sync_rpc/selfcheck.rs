//! 打开游戏前的自检：接线（纯决策在 [`crate::sync::selfcheck`]）。
//!
//! 这一层只做三件事：把配置读成"这一款现在是什么状态"、需要时才读一次**云端索引**
//! （本机缓存那份，不读身份卡）、把结论落盘。**它绝不拦启动**：读不到就当作"未定"，
//! 用户照样能开始玩。

use serde_json::{Value, json};

use super::Daemon;
use crate::config::GameConfig;
use crate::sync::index::IndexGame;
use crate::sync::selfcheck::{CloudPeek, Decision, Found};
use crate::sync::signature::{self, Conclusion};

impl Daemon {
    /// 这一款的当前目标签名；目标没配齐就是 `None`。
    async fn target_signature(&self) -> Option<String> {
        signature::of(&self.config.read().await.sync)
    }

    /// 自检。**只有 `Ask` 会打断用户**（见 `sync::selfcheck`）。
    pub(in crate::daemon) async fn sync_selfcheck(&self, game_id: &str) -> Decision {
        let (mut game, signature) = {
            let config = self.config.read().await;
            let Some(game) = config.games.get(game_id).cloned() else {
                return Decision::Skip;
            };
            (game, signature::of(&config.sync))
        };

        // ⚠ **先补一次指纹**（用户 2026-09-28 在 Windows 上报的"明明一模一样的 exe 却没
        // 自动匹配，连疑似都没有"）：`needs_cloud` 的第一条判据就是"本机有指纹"，而补指纹
        // 从前只发生在"配对扫描前"与"上传时" —— 于是点「启动」的自检在本机没指纹时**干脆
        // 不查云端**，直接判"认不出"，界面只会说"云端没有对得上的"，可云端明明有一条。
        // 那条档案建在指纹功能落地之前（09-21），此后一直没人问过它指纹。
        //
        // 算不出（盘没插、文件不在）**绝不拦启动**：照旧往下走"问一次"，让用户自己挑
        // —— 自检的规矩是"只有一种情况会打断用户"，而"读不到 exe"不在那一种里。
        if game.exe_fingerprint.is_none() {
            self.ensure_fingerprint(game_id).await.ok();
            let refreshed = self.config.read().await.games.get(game_id).cloned();
            if let Some(refreshed) = refreshed {
                game.exe_fingerprint = refreshed.exe_fingerprint;
            }
        }

        // 已确认、开关关着、没目标：都不用去云端。
        if !crate::sync::selfcheck::needs_cloud(&game, signature.as_deref()) {
            return crate::sync::selfcheck::decide(&game, signature.as_deref(), || Found::None);
        }

        let found = self.survey_cloud(game_id, &game).await;
        crate::sync::selfcheck::decide(&game, signature.as_deref(), || found)
    }

    /// 云端那份索引里"这一款"对应的是谁 —— **三条判据，指纹优先**。
    ///
    /// ⚠ 读的是**本机缓存里那份索引**，不是所有身份卡 —— 用户 2026-09-24："其他所有查询
    /// 都只查本地索引，最大化减少网络请求次数"。索引是身份卡的镜像，指纹这一栏本来就在
    /// 里面；本地还没有缓存时那条读路径会下载一次（见 `Daemon::cloud_index_view`）。
    ///
    /// ⚠ **读不到 ≠ 没有**（用户 2026-09-28）：桶名填错、网络不通、桶里还没索引，这三种
    /// 一律包成 [`Found::Unavailable`]，让界面说得出原因，而不是跟着说"云端没有"。
    ///
    /// ⚠ **弱判据也要列出来**（同一批里的第二次纠正）：只按指纹筛的话，**没指纹的档案一个
    /// 候选都不会有** —— 他在 Windows 上那条档案建在指纹功能落地之前，界面于是彻底沉默
    /// （"连疑似匹配都没有"）。现在名字相同、存档位置的父目录名重合的那几条也会进候选，
    /// **但只有指纹能自动认领**：弱判据只让弹窗说一句"云端有一条像的"，绑不绑由用户点头
    /// （用户 09-21："宁可不动，也不猜"）。
    async fn survey_cloud(&self, game_id: &str, game: &GameConfig) -> Found {
        let view = match self.cloud_index_view(false).await {
            Ok(view) => view,
            Err(error) => {
                tracing::warn!("{game_id}: 自检读不到云端索引，当作未配对: {error}");
                return Found::Unavailable(format!("读不到云端索引：{error}"));
            }
        };
        // 桶里还没有这份索引（第一次用）：这一刻我们**不知道**云端有没有这一款 ——
        // 与"索引里有、但没有这一款"不是一回事。
        let Some(index) = view.index else {
            return Found::Unavailable(
                "这个桶里还没有云端索引（还没有任何一台机器传过）".to_string(),
            );
        };
        // 有缓存、但这次刷新失败：下面这份是**旧**的。命中照样算数（旧证据也是证据），
        // 没命中就必须说清"手上这份是旧的" —— 否则用户会以为云端真的没有。
        let stale = view
            .refresh_error
            .map(|error| format!("这次没能刷新云端索引（{error}），手上是本机缓存的旧索引"));

        // 本机**别的**档案已经认领的那些身份：指过去只会让用户撞墙（一个身份只配一款）。
        let claimed_by_others: std::collections::HashSet<String> = {
            let config = self.config.read().await;
            config
                .games
                .iter()
                .filter(|(id, _)| id.as_str() != game_id)
                .filter_map(|(_, other)| other.cloud_id.clone())
                .collect()
        };

        let fingerprint = game.exe_fingerprint.clone().unwrap_or_default();
        // 1. 指纹恰好命中一条、而且那条没被本机别的档案占着 ⇒ **可以自动认领**。
        //    （指纹命中**任意一个**即算命中 —— 用户 2026-09-24 的口径。）
        let hits: Vec<&IndexGame> = index
            .games
            .iter()
            .filter(|candidate| {
                !fingerprint.is_empty() && candidate.identity.has_fingerprint(&fingerprint)
            })
            .collect();
        if let [only] = hits.as_slice()
            && !claimed_by_others.contains(only.identity.cloud_id.as_str())
        {
            return Found::One {
                cloud_id: only.identity.cloud_id.clone(),
                cloud_key: only.cloud_key.clone(),
            };
        }

        // 2. 其余一律"列候选"：指纹命中多条（或那条被别人占着），或者一条指纹都没中 ——
        //    后者按弱判据（名字、存档位置的父目录名）再找一遍。
        //
        //    筛选与排序都在 `matching::candidates` 里（弱匹配的唯一入口：以后换更好的算法
        //    只动那一处，这里一行都不用改），这里只负责把数据喂进去、把结果搬成界面要的
        //    那几栏。
        let prints: Vec<String> = if fingerprint.is_empty() {
            Vec::new()
        } else {
            vec![fingerprint.clone()]
        };
        let parents: Vec<String> = game
            .save_paths
            .iter()
            .filter_map(|save| crate::sync::parent_dir(&save.path))
            .collect();
        let local = crate::sync::matching::LocalSide {
            name: &game.name,
            fingerprints: &prints,
            parents: &parents,
        };
        let candidates = crate::sync::matching::candidates(
            &local,
            index.games.iter().filter(|candidate| {
                !claimed_by_others.contains(candidate.identity.cloud_id.as_str())
            }),
            |candidate| &candidate.identity,
        );

        if candidates.is_empty() {
            return match stale {
                Some(reason) => Found::Unavailable(reason),
                None => Found::None,
            };
        }
        Found::Many(
            candidates
                .iter()
                .map(|candidate| peek(candidate.item))
                .collect(),
        )
    }

    /// 把自检的结论落盘（`Skip` / `Pull` / `Ask` 不用落任何东西）。
    pub(in crate::daemon) async fn apply_decision(
        &self,
        game_id: &str,
        decision: &Decision,
    ) -> Result<(), String> {
        let Some(signature) = self.target_signature().await else {
            return Ok(());
        };
        match decision {
            Decision::Adopt {
                cloud_id,
                cloud_key,
            } => {
                self.remember_identity(game_id, cloud_id, cloud_key).await?;
                self.stamp_conclusion(game_id, Conclusion::confirmed(&signature))
                    .await
            }
            // "不再问，直接新建一条身份"：把本机这份身份清掉，上传时就会新建一条。
            // 防错配闸照旧生效 —— 没有身份就取不回任何东西。
            Decision::Fresh => {
                let owner = game_id.to_string();
                self.mutate_config(move |config| {
                    if let Some(game) = config.games.get_mut(&owner) {
                        game.cloud_id = None;
                        game.cloud_dir = None;
                    }
                    Ok(Value::Null)
                })
                .await?;
                // ⚠ 这里**不能**盖 `confirmed`：那个结论的意思是"已确认，**而且绑着
                // 一条身份**"（见 [`Conclusion::Confirmed`]），而上面刚把身份清空 ——
                // 于是 `confirmed_on` 判它不成立，下一次自检又去查云端、又弹一次窗，
                // 用户永远建不出这条档案（2026-09-26 报的"点了新建还是被问"）。
                // 他答的是"以后新建一条"，记下的就该是它：下次自检直接走 Fresh。
                self.stamp_conclusion(game_id, Conclusion::fresh(&signature))
                    .await
            }
            Decision::Skip | Decision::Pull | Decision::Ask { .. } => Ok(()),
        }
    }

    /// 写下"这一款在这个目标上的结论"。
    async fn stamp_conclusion(&self, game_id: &str, value: String) -> Result<(), String> {
        let owner = game_id.to_string();
        self.mutate_config(move |config| {
            if let Some(game) = config.games.get_mut(&owner) {
                game.cloud_conclusion = Some(value);
            }
            Ok(Value::Null)
        })
        .await
        .map(|_| ())
    }

    /// 用户在启动前的对话框里选了什么（`ok` / `off` / `pair`）。
    pub(in crate::daemon) async fn rpc_sync_resolve(
        &self,
        game_id: &str,
        choice: &str,
        cloud_id: Option<&str>,
        cloud_key: Option<&str>,
    ) -> Result<Value, String> {
        let Some(signature) = self.target_signature().await else {
            return Err("云同步还没配好目标（bucket）".to_string());
        };
        match choice {
            // ⚠ **没有"没问题"这一项**（用户 2026-09-24："不允许没问题出现，我们要么处理好
            // 存档问题，要么就直接关掉存档，不允许模糊的存在"）。三条回答各自都有确定的结果：
            // 绑上某一条 / 新建一条 / 关掉这一款的同步。
            //
            // "改配对…"：用户挑了一条云端身份（浮层里"新建"那一项就是不给 cloud_id）。
            "pair" => {
                let stamped = match (cloud_id, cloud_key) {
                    (Some(cloud_id), Some(cloud_key)) => {
                        self.remember_identity(game_id, cloud_id, cloud_key).await?;
                        Conclusion::confirmed(&signature)
                    }
                    // 新建：清掉本机身份，上传时新建一条 —— 结论记成"以后新建一条"。
                    _ => {
                        let owner = game_id.to_string();
                        self.mutate_config(move |config| {
                            if let Some(game) = config.games.get_mut(&owner) {
                                game.cloud_id = None;
                                game.cloud_dir = None;
                            }
                            Ok(Value::Null)
                        })
                        .await?;
                        Conclusion::fresh(&signature)
                    }
                };
                self.stamp_conclusion(game_id, stamped).await?;
            }
            // "关掉这一款的同步"：只关这一款，而且**记住问过了** —— 他自己再打开时
            // 直接新建身份、不再问（见 `sync::selfcheck`）。
            "off" => {
                let owner = game_id.to_string();
                let value = Conclusion::declined(&signature);
                self.mutate_config(move |config| {
                    let game = config
                        .games
                        .get_mut(&owner)
                        .ok_or_else(|| format!("配置中找不到游戏: {owner}"))?;
                    game.sync_enabled = false;
                    game.cloud_conclusion = Some(value);
                    Ok(Value::Null)
                })
                .await?;
            }
            other => return Err(format!("不认识的回答: {other}")),
        }
        tracing::info!("{game_id}: 启动前自检的回答 {choice}");
        Ok(json!({ "ok": true }))
    }
}

/// 索引里那一条 → 弹窗要显示的那几栏（[`CloudPeek`]）。
///
/// ⚠ 只搬事实、不拼文案：名字与摘要由界面用**同一个函数**生成（见 `CloudPeek` 的注释）。
fn peek(game: &IndexGame) -> CloudPeek {
    CloudPeek {
        cloud_id: game.identity.cloud_id.clone(),
        cloud_key: game.cloud_key.clone(),
        name: game.identity.name.clone(),
        versions: game.versions as u64,
        latest: game.latest.clone().unwrap_or_default(),
        size: game.size,
    }
}
