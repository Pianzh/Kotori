//! 打开游戏前的自检：**只有一种情况会打断用户**。
//!
//! 全流程（用户 2026-09-22："云同步（打开游戏）前必须自检"）：
//!
//! 1. 目标没配齐 / 这一款的开关关着 ⇒ 什么都不做，直接启动（**自检失败绝不拦启动**）。
//! 2. 这一款在**当前目标**上已确认（结论里的签名与当前签名一致）⇒ 不重扫、不问，
//!    直接取回较新的存档（防错配闸照旧生效）。
//! 3. 未定 ⇒ 先按指纹在**当前桶**里找一次：恰好命中一条 ⇒ 静默认领并盖章；否则问一次。
//!
//! 这个模块是**纯决策**：看云端那一步由调用方给一个闭包（`lookup`），而且**只在第 3 步
//! 才调用** —— kopia 那边"读一次身份 = 一次 restore"，已确认的那条路上一个字都不该读。

use crate::config::GameConfig;
use crate::sync::signature::Conclusion;

/// 云端一条身份里**给用户看的那几栏**（弹窗里"疑似找到的那一条"）。
///
/// ⚠ 与界面的 `CloudIdentityLabel` 一一对应：daemon 只报事实，**名字与摘要由界面用同一个
/// 函数生成**（用户 2026-09-24："弹窗显示的近似游戏信息使用的是和设置页面给出信息一样的
/// 函数就可以了，方便后期统一修改"）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudPeek {
    pub cloud_id: String,
    pub cloud_key: String,
    pub name: String,
    pub versions: u64,
    pub latest: String,
    pub size: u64,
}

/// 指纹在当前云目标上找到了什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    /// 没命中（包括指纹还没算出来）。
    None,
    /// **没能看到云端**：索引读不到、桶名不对、桶里还没有索引、或者刷新失败只剩旧缓存。
    ///
    /// ⚠ 与 [`Found::None`] 分开是刻意的（用户 2026-09-28 在 Windows 上报的"明明是一模一样
    /// 的 exe，却没有匹配，甚至连疑似匹配都没有"）：他那天把桶名填成了 `kotori-win`，而
    /// Linux 那边是 `kotori-saves`，kopia 回的是 `bucket not found` —— 而界面只说"云端没有
    /// 对得上的"，把**没读到**说成了**没有**。两者的下一步完全不同：一个是"云端确实还没有
    /// 这一款（去第一次上传）"，另一个是"你先把桶名/网络弄对"。
    Unavailable(String),
    /// 恰好命中一条，而且这条身份还没被本机别的档案认领。
    One { cloud_id: String, cloud_key: String },
    /// 命中多条，或者命中的那条已经被本机别的档案占着 —— 都要问。带上的那几条是给弹窗挑
    /// "最像的一条"用的（规则在 [`crate::sync::matching::best_like`] 那一个函数里）。
    Many(Vec<CloudPeek>),
}

/// 自检的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// 不打扰：直接启动。
    Skip,
    /// 在当前目标上已确认：照常取回。
    Pull,
    /// 指纹恰好命中一条：静默认领并盖章，然后取回。
    Adopt { cloud_id: String, cloud_key: String },
    /// 不再问：直接新建一条身份（用户自己把"关掉过"的那款开关重新打开了）。
    Fresh,
    /// 问一次。`found` = **疑似找到的那一条**；`None` = 完全没找到，界面照实说、
    /// 让你自己挑。`trouble` = **为什么没认出来**：`Some` 表示"没读到云端"（桶名不对、
    /// 网络不通、索引没建过），与"云端真的没有这一款"是两回事（见 [`Found::Unavailable`]）。
    Ask {
        found: Option<CloudPeek>,
        trouble: Option<String>,
    },
}

/// 要不要为了这次自检去**读云端**（kopia 那边读一次身份 = 一次 restore）。
///
/// 开关关着、目标没配齐、已确认 —— 这三种都不用读。[`decide`] 与它必须说同一件事：
/// 调用方先问这个，再决定要不要花那次读取。
///
/// ⚠ 从前这里还要求"有指纹"，于是**没指纹的档案连一个候选都不会有**：用户 2026-09-28 在
/// Windows 上报的"明明一模一样的 exe，却连疑似匹配都没有"就是这个（那条档案建在指纹功能
/// 落地之前）。现在只要有**弱判据**（名字、存档位置）也去读一次 —— 读的是**本机缓存**那份
/// 索引，没有缓存时才会联网一次（见 `Daemon::cloud_index_view`），与"每次都去云端"是两件事。
pub fn needs_cloud(game: &GameConfig, signature: Option<&str>) -> bool {
    let (Some(signature), true) = (signature, game.sync_enabled) else {
        return false;
    };
    if confirmed_on(game, signature) {
        return false;
    }
    has_fingerprint(game) || has_weak_evidence(game)
}

/// 强判据：这一款算过指纹（只有它能自动认领）。
pub fn has_fingerprint(game: &GameConfig) -> bool {
    game.exe_fingerprint
        .as_deref()
        .is_some_and(|fingerprint| !fingerprint.is_empty())
}

/// 弱判据：名字，或者至少一个存档位置（父目录名由它算出来）。
///
/// 弱判据**只用来列候选**（见 [`crate::sync::matching`]），但它决定"要不要读一次索引"：
/// 没有它，没指纹的那一款在界面上连一个候选都不会有。
pub fn has_weak_evidence(game: &GameConfig) -> bool {
    !game.name.trim().is_empty() || !game.save_paths.is_empty()
}

/// 这一款在当前目标上**真的**确认过没有？
///
/// ⚠ 除了签名要对得上，**还必须绑着一条身份**。用户 2026-09-24 报过"没有存档，云同步
/// 点开，但是没有弹出未命中窗口" —— 那正是"结论说确认过、其实没绑"的那一款：没绑的确认
/// 没有落到实处，点启动还是要自检一次（认得出就自动绑上，认不出就问）。
fn confirmed_on(game: &GameConfig, signature: &str) -> bool {
    game.cloud_id.is_some()
        && matches!(
            game.cloud_conclusion.as_deref().and_then(Conclusion::parse),
            Some(Conclusion::Confirmed(known)) if known == signature
        )
}

/// 自检。`lookup` 只在真的需要看云端时被调用一次。
pub fn decide<F>(game: &GameConfig, signature: Option<&str>, lookup: F) -> Decision
where
    F: FnOnce() -> Found,
{
    // 1. 目标没配齐、或者这一款的开关关着：一个字都不做。
    //    （"关掉这一款"这条路上，用户下次打开游戏不该再被问第二次。）
    let (Some(signature), true) = (signature, game.sync_enabled) else {
        return Decision::Skip;
    };
    let conclusion = game.cloud_conclusion.as_deref().and_then(Conclusion::parse);

    // 2. 在当前目标上**真的**确认过（签名对得上，而且绑着一条身份）：不重扫、不问。
    if confirmed_on(game, signature) {
        return Decision::Pull;
    }

    // 3. 问过一次、答案是"以后新建一条"，而且是在**当前这个目标**上问的：他要的就是新建，
    //    云端那条旧身份本来就不再是他要的 —— 不必再扫一遍，直接按"新建"继续。
    //
    //    ⚠ 必须在扫云端**之前**：省下的不只是一次弹窗，还有每次启动都要做的那一次
    //    云端索引读取（缓存过期时就是一次下载）—— 用户 2026-09-26 报的"卡在启动中"
    //    就发生在那一段之后，而它每次都白白重来一遍。
    //
    //    ⚠ 只认 `new`，**不认 `off`**：答"关掉这一款"之后又自己把开关打开，语义是
    //    "我改主意了、想同步了" —— 那就照旧先看一眼云端："认得出旧身份就还是认领它，
    //    匹配不上才新建"（看门测试在 `re_enabling_a_game_that_was_switched_off_...`）。
    if matches!(conclusion, Some(Conclusion::New(known)) if known == signature) {
        return Decision::Fresh;
    }

    // 4. 仍未定：按指纹在当前桶里找一次（这是唯一会读云端的一步）。
    let mut trouble = None;
    if needs_cloud(game, Some(signature)) {
        match lookup() {
            Found::One {
                cloud_id,
                cloud_key,
            } => {
                return Decision::Adopt {
                    cloud_id,
                    cloud_key,
                };
            }
            // 命中多条：弹窗里只显示**最像的那一条**（规则在 `matching::best_like`，
            // 现在是取第一条）。一条都没带（"唯一那条被本机别的档案占着"）就是 `None`。
            Found::Many(candidates) => {
                let found = crate::sync::matching::best_like(&candidates).cloned();
                return ask_or_fresh(conclusion.as_ref(), found, None);
            }
            // 没读到云端：**与"云端没有"分开报**。行为一个字都不变 —— 照旧问一次
            // （自检绝不拦启动，见模块说明第 1 条）。
            Found::Unavailable(reason) => trouble = Some(reason),
            Found::None => {}
        }
    }

    ask_or_fresh(conclusion.as_ref(), None, trouble)
}

/// 最后一步：问一次 —— **除非**他上次就答过"关掉这一款"或"以后新建一条"（那两次都已经
/// 问过了，用户原话："如果匹配不上还强制打开就建立新游戏存档位置"）。第 3 步只挡掉了
/// 当前目标上的 `new`，这里再兜两处边角：换了目标、结论里那个签名对不上；以及答过 `off`
/// 又自己打开开关、云端却没认出旧身份（那正是"匹配不上"，就新建）。
///
/// `found` 是"疑似找到的那一条"（没有就是完全没找到）—— 界面据此分两种说法，用户
/// 2026-09-24："直接把找到像的和没找到像的打包成函数或者条件，分别显示疑似找到和完全
/// 没找到两个 ui"。
///
/// `trouble` 是**第三种说法**（用户 2026-09-28）：不是"没有像的"，而是"压根没读到云端"。
/// 只在没命中时才可能非空 —— 命中了就是命中了，本机缓存旧一点也是强证据。
fn ask_or_fresh(
    conclusion: Option<&Conclusion>,
    found: Option<CloudPeek>,
    trouble: Option<String>,
) -> Decision {
    match conclusion {
        // 问过一次、答案是"关掉这一款"或者"以后新建一条"：都不再问，直接新建身份。
        Some(Conclusion::Declined(_)) | Some(Conclusion::New(_)) => Decision::Fresh,
        _ => Decision::Ask { found, trouble },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> GameConfig {
        serde_json::from_value(serde_json::json!({
            "name": "demo",
            "exe_path": "/games/demo/game.exe",
            "created_at": "2026-01-01T00:00:00Z",
        }))
        .unwrap()
    }

    const SIG: &str = "v1:kopia::bkt:kotori";
    const OTHER: &str = "v1:kopia::other:kotori";

    fn hit(cloud_id: &str, key: &str) -> Found {
        Found::One {
            cloud_id: cloud_id.to_string(),
            cloud_key: key.to_string(),
        }
    }

    #[test]
    fn a_game_whose_switch_is_off_is_never_asked_about() {
        let mut game = game();
        game.sync_enabled = false;
        game.exe_fingerprint = Some("v1:1:aa".into());
        assert_eq!(decide(&game, Some(SIG), || hit("x", "y")), Decision::Skip);
    }

    #[test]
    fn no_target_means_nothing_to_do() {
        let game = game();
        assert_eq!(decide(&game, None, || Found::None), Decision::Skip);
    }

    /// ⚠ "确认过"必须**绑着一条身份**才算数（用户 2026-09-24 报的场景：结论说确认过、
    /// 其实没绑 ⇒ 点启动什么都不问）。没绑的那种见下一条测试。
    #[test]
    fn a_confirmed_game_is_not_scanned_again() {
        let mut game = game();
        game.cloud_id = Some("cloud-1".into());
        game.cloud_conclusion = Some(Conclusion::confirmed(SIG));
        game.exe_fingerprint = Some("v1:1:aa".into());
        // `lookup` 一被调用就炸：已确认那条路上一个字都不许读云端。
        assert_eq!(
            decide(&game, Some(SIG), || panic!("不该去看云端")),
            Decision::Pull
        );
    }

    /// "确认过、但没绑"不算数：点启动还是要自检 —— 认得出就自动绑上，认不出就问。
    #[test]
    fn a_confirmation_without_an_identity_does_not_count() {
        let mut game = game();
        game.cloud_conclusion = Some(Conclusion::confirmed(SIG));
        game.exe_fingerprint = Some("v1:1:aa".into());
        assert!(
            needs_cloud(&game, Some(SIG)),
            "没绑的确认没有落到实处，该去看一眼"
        );
        assert_eq!(
            decide(&game, Some(SIG), || hit("cloud-1", "demo")),
            Decision::Adopt {
                cloud_id: "cloud-1".into(),
                cloud_key: "demo".into()
            }
        );
        assert_eq!(
            decide(&game, Some(SIG), || Found::None),
            Decision::Ask {
                found: None,
                trouble: None
            }
        );
    }

    #[test]
    fn another_target_makes_the_conclusion_stale_and_the_fingerprint_decides() {
        let mut game = game();
        game.cloud_conclusion = Some(Conclusion::confirmed(OTHER));
        game.exe_fingerprint = Some("v1:1:aa".into());
        assert_eq!(
            decide(&game, Some(SIG), || hit("cloud-1", "demo")),
            Decision::Adopt {
                cloud_id: "cloud-1".into(),
                cloud_key: "demo".into()
            }
        );
        // 换了桶又认不出来：问一次（这次他还没答过"关掉"），而且**不带**"疑似找到的那
        // 一条" —— 完全没找到就不编名字。
        assert_eq!(
            decide(&game, Some(SIG), || Found::None),
            Decision::Ask {
                found: None,
                trouble: None
            }
        );
        assert_eq!(
            decide(&game, Some(SIG), || Found::Many(Vec::new())),
            Decision::Ask {
                found: None,
                trouble: None
            }
        );
    }

    /// 没有指纹**也要问一次** —— 而且问之前会去读一次索引（名字这条弱判据就靠它）。
    ///
    /// ⚠ 这条从前钉的是"没有指纹就不该去查"（闭包直接 `panic!`）：那时 `needs_cloud` 要求
    /// 必须有指纹，于是没指纹的档案连一个候选都不会有 —— 用户 2026-09-28 报的"连疑似匹配
    /// 都没有"就是这个。现在弱判据也算数，所以那个闭包会被**真的调用**。
    #[test]
    fn a_game_without_a_fingerprint_is_asked_instead_of_guessed() {
        let game = game();
        assert!(
            needs_cloud(&game, Some(SIG)),
            "没指纹也要读一次索引：名字这条弱判据就靠它"
        );
        assert_eq!(
            decide(&game, Some(SIG), || Found::None),
            Decision::Ask {
                found: None,
                trouble: None
            }
        );
    }

    /// **没读到云端 ≠ 云端没有这一款**（用户 2026-09-28 在 Windows 上报的那次：桶名填成
    /// `kotori-win`，而 Linux 那边是 `kotori-saves`，kopia 回 `bucket not found`，界面却说
    /// "云端没有对得上的"）。
    ///
    /// 行为与 `Found::None` 一个字都不差（照旧问一次、绝不拦启动），差别只在**带出去的
    /// 原因**：界面照它说"没读到云端"，而不是"云端没有"。
    #[test]
    fn not_reading_the_cloud_is_reported_as_such_not_as_an_empty_cloud() {
        let mut game = game();
        game.exe_fingerprint = Some("v1:1:aa".into());
        assert_eq!(
            decide(&game, Some(SIG), || Found::Unavailable(
                "bucket not found: kotori-win".into()
            )),
            Decision::Ask {
                found: None,
                trouble: Some("bucket not found: kotori-win".into()),
            }
        );
    }

    /// 读不到云端**不许翻案**：他上次答过"关掉这一款"，照旧直接新建 —— 这条原因只影响
    /// 说法，不影响结论。
    #[test]
    fn a_cloud_that_cannot_be_read_does_not_overrule_a_standing_answer() {
        let mut game = game();
        game.exe_fingerprint = Some("v1:1:aa".into());
        game.cloud_conclusion = Some(Conclusion::declined(SIG));
        assert_eq!(
            decide(&game, Some(SIG), || Found::Unavailable("网络不通".into())),
            Decision::Fresh
        );
    }

    /// 用户答过"关掉这一款"，之后自己又把开关打开：**不再问**，直接新建身份。
    ///
    /// ⚠ 界面上那颗开关现在会**顺手清掉结论**（见 `game_rpc::rpc_game_update`），所以这条路
    /// 主要服务"结论还在、开关已经被别的途径打开"的情形（老配置、CLI 直接改配置）。
    #[test]
    fn re_enabling_a_game_that_was_switched_off_creates_a_new_identity_silently() {
        let mut game = game();
        game.sync_enabled = true;
        game.cloud_conclusion = Some(Conclusion::declined(SIG));
        game.exe_fingerprint = Some("v1:1:aa".into());
        assert_eq!(decide(&game, Some(SIG), || Found::None), Decision::Fresh);
        // 认得出旧身份就还是认领它 —— "匹配不上才新建"。
        assert_eq!(
            decide(&game, Some(SIG), || hit("cloud-9", "demo")),
            Decision::Adopt {
                cloud_id: "cloud-9".into(),
                cloud_key: "demo".into()
            }
        );
    }

    /// 用户答过"以后新建一条"：**下次启动不再问**，而且**连云端都不去看**。
    ///
    /// ⚠ 这是 2026-09-26 那个 bug 的看门测试：写结论的那一端（`apply_decision`）曾经把
    /// "新建"盖成 `ok:<签名>`（＝"已确认、而且绑着身份"）—— 而身份刚被清空，于是每次
    /// 启动都重走一遍查云端、再弹一次窗，用户永远建不出档案。
    #[test]
    fn a_game_that_answered_new_is_not_asked_again_nor_scanned() {
        let mut game = game();
        game.sync_enabled = true;
        game.cloud_conclusion = Some(Conclusion::fresh(SIG));
        game.exe_fingerprint = Some("v1:1:aa".into());
        // `lookup` 一被调用就炸：答过"新建"之后一个字都不该读云端。
        assert_eq!(
            decide(&game, Some(SIG), || panic!("答过'新建'就不该再去读云端")),
            Decision::Fresh
        );

        // 换了一个桶：上一次那个结论不作数，该重新看一眼云端（哪怕最后还是走"新建"）。
        let looked = std::cell::Cell::new(false);
        assert_eq!(
            decide(&game, Some(OTHER), || {
                looked.set(true);
                Found::None
            }),
            Decision::Fresh
        );
        assert!(looked.get(), "换了目标就该重新查一次");
    }

    /// 指纹命中多条：问一次，并把**最像的那一条**带上（现在是取第一条，规则在
    /// `matching::best_like`）—— 界面据此显示"疑似找到"。
    #[test]
    fn many_hits_ask_once_and_carry_the_best_one() {
        let mut game = game();
        game.exe_fingerprint = Some("v1:1:aa".into());
        let peek = |cloud_id: &str, name: &str| CloudPeek {
            cloud_id: cloud_id.to_string(),
            cloud_key: format!("games/{cloud_id}"),
            name: name.to_string(),
            versions: 3,
            latest: "20260911T101500Z".to_string(),
            size: 4096,
        };
        assert_eq!(
            decide(&game, Some(SIG), || Found::Many(vec![
                peek("c1", "一号"),
                peek("c2", "二号"),
            ])),
            Decision::Ask {
                found: Some(peek("c1", "一号")),
                trouble: None,
            },
            "带上的必须是第一条（`best_like` 现在的规则）"
        );
    }

    #[test]
    fn a_fingerprint_hit_wins_over_an_unrelated_conclusion() {
        let mut game = game();
        game.cloud_conclusion = Some("看不懂的东西".into());
        game.exe_fingerprint = Some("v1:1:aa".into());
        assert_eq!(
            decide(&game, Some(SIG), || hit("cloud-2", "renamed")),
            Decision::Adopt {
                cloud_id: "cloud-2".into(),
                cloud_key: "renamed".into()
            }
        );
    }

    /// **没指纹也要去读一次云端**（用户 2026-09-28："连疑似匹配都没有"）。
    ///
    /// 从前的判据要求"有指纹"，于是没指纹的档案连一个候选都不会有 —— 他在 Windows 上那条
    /// 档案建在指纹功能落地之前，界面就彻底沉默了。现在名字/存档位置这两条弱判据也算数。
    #[test]
    fn a_game_without_a_fingerprint_still_has_weak_evidence_to_look_up() {
        let mut game = game();
        assert!(game.exe_fingerprint.is_none());
        assert!(!has_fingerprint(&game));
        assert!(has_weak_evidence(&game), "夹具里的名字是有的");
        assert!(
            needs_cloud(&game, Some(SIG)),
            "没指纹也要读一次索引，否则连候选都列不出来"
        );

        // 连名字都空、又没有存档位置：那才是"什么都不用查"。
        game.name = String::new();
        game.save_paths.clear();
        assert!(!has_weak_evidence(&game));
        assert!(!needs_cloud(&game, Some(SIG)));
    }
}
