//! 弱匹配：本机一条档案与云端一条身份"像不像"，以及判据的强弱顺序。
//!
//! 用户 2026-09-24："关于存档位置，我们暂时以父目录名称来规定，这个存档位置我建议
//! 单开一个文件，便于未来拓展更多的算法，更多的弱匹配方式。"
//!
//! * **强判据只有一条**：exe 指纹 —— 同一个可执行文件就是同一款，只有它能自己动手。
//! * 弱判据（名字相同、存档位置的父目录名有交集）**只用来列候选**，永不自动绑：
//!   猜错的代价是把别人的存档铺进本机这一款（用户 2026-09-21："宁可不动，也不猜"）。
//!
//! 判据只看**云端索引里的身份字段**（[`GameIdentity`] / `MachineIdentity`），既不读身份
//! 卡也不碰网络 —— 用户 2026-09-24："其他所有查询都只查本地索引，最大化减少网络请求次数"。
//!
//! 指纹的口径（用户 2026-09-24）："游戏 A 同时有指纹 abc 的情况，假设指纹 b 命中，那么
//! 直接判断命中；但是如果多个游戏共有同一个指纹，也就是指纹 a 同时命中游戏 ABC，这样才
//! 需要问。" ⇒ 同一条身份自己的多个指纹、它名下的多台机器都**不算**歧义；数歧义数的是
//! "有几条**身份**带着这个指纹"（见 [`fingerprint_owners`]）。

use std::collections::HashMap;

use crate::sync::cloud::GameIdentity;

/// 判据的强弱：`Ord` 的顺序就是"谁更硬"，靠前（小）的更硬。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Likeness {
    /// exe 指纹命中（命中这条身份的**任意一个**指纹就算）。
    Fingerprint,
    /// 名字一模一样。
    Name,
    /// 存档位置的父目录名有交集。
    ParentDir,
}

impl Likeness {
    /// 回包里的写法（界面按它选措辞，见 `ui/model/sync.rs` 的 `evidence_label`）。
    ///
    /// ⚠ "父目录名"这一档对外仍然写作 `location`：界面上那句话是"存档位置像"，
    /// 换名字会让界面文案与已有回包一起变，而这一档的**判据**才是新东西。
    pub fn as_str(self) -> &'static str {
        match self {
            Likeness::Fingerprint => "fingerprint",
            Likeness::Name => "name",
            Likeness::ParentDir => "location",
        }
    }
}

/// 本机这一侧参与判断的几栏。
#[derive(Debug, Clone, Copy)]
pub struct LocalSide<'a> {
    /// 本机这一款的名字。
    pub name: &'a str,
    /// 这一款的 exe 指纹（没有就是空 —— **绝不编一个**）。
    pub fingerprints: &'a [String],
    /// 它配的存档位置的父目录名（见 [`crate::sync::remote_paths::parent_dir`]）。
    pub parents: &'a [String],
}

/// 这条身份像不像本机这一条？像就是**最强的那条判据**。
pub fn likeness(local: &LocalSide<'_>, identity: &GameIdentity) -> Option<Likeness> {
    if local
        .fingerprints
        .iter()
        .any(|fingerprint| identity.has_fingerprint(fingerprint))
    {
        return Some(Likeness::Fingerprint);
    }
    if local.name == identity.name {
        return Some(Likeness::Name);
    }
    if shared_parents(local.parents, identity) > 0 {
        return Some(Likeness::ParentDir);
    }
    None
}

/// 父目录名重合了几个（同一档里用它排序：重合越多越像）。
pub fn shared_parents(local: &[String], identity: &GameIdentity) -> usize {
    local
        .iter()
        .filter(|name| {
            identity
                .machines
                .iter()
                .any(|machine| machine.parents.contains(name))
        })
        .count()
}

/// 云端每个指纹各被**几条身份**带着（同一条身份自己的多个指纹、名下的多台机器只算一次）。
///
/// 这是"要不要问用户"的唯一依据：`== 1` 才敢自动绑；`> 1` 就是"多个游戏共有同一个
/// 指纹"，必须问。
pub fn fingerprint_owners<'a>(
    identities: impl IntoIterator<Item = &'a GameIdentity>,
) -> HashMap<String, usize> {
    let mut owners: HashMap<String, usize> = HashMap::new();
    for identity in identities {
        let mut prints: Vec<&str> = identity
            .machines
            .iter()
            .flat_map(|machine| machine.fingerprints.iter().map(String::as_str))
            .collect();
        // 同一条身份里重复的指纹（多台机器各记了一遍）只算一次。
        prints.sort_unstable();
        prints.dedup();
        for print in prints {
            *owners.entry(print.to_string()).or_default() += 1;
        }
    }
    owners
}

/// 一条候选（[`candidates`] 的产物）：像不像、有多像、以及它是谁。
///
/// 字段是 `pub` 的：调用方要按它判断"能不能自动绑"（**只有 [`Likeness::Fingerprint`]
/// 可以**），也要拿 `item` 去拼界面要显示的那一份事实。
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a, T> {
    /// 命中的最强那条判据。
    pub likeness: Likeness,
    /// 存档位置的父目录名重合了几个（同一档里的排序依据）。
    pub shared: usize,
    /// 云端那一项本身（`IndexGame` / `GameIdentity`…… 由调用方决定）。
    pub item: &'a T,
}

/// 在"云端这一堆身份"里挑出所有**像**本机这一款的，并按"最像"排好序。
///
/// ⚠ **这是弱匹配的唯一入口**（用户 2026-09-28："弱判据拆出来，以后会考虑匹配更好的
/// 算法"）。以后要换算法（名字相似度、厂商、VNDB 对齐……），**只改这一个函数与
/// [`Likeness`] 的排序** —— 调用方（启动前自检、配对页、添加页）一行都不用动。
///
/// `identity_of` 是"从这一项里取出它的身份"：这样这里不必认识 `IndexGame` 之类的东西，
/// 也就能直接拿普通切片做单测。
///
/// 排序：判据越硬越前（[`Likeness`] 的 `Ord` 就是"谁更硬"），同一档里父目录名重合多的在前。
/// 于是"最像的那一条"就是 `[0]`（见 [`best_like`]）。
pub fn candidates<'a, T>(
    local: &LocalSide<'_>,
    items: impl IntoIterator<Item = &'a T>,
    identity_of: impl Fn(&'a T) -> &'a GameIdentity,
) -> Vec<Candidate<'a, T>> {
    let mut found: Vec<Candidate<'a, T>> = items
        .into_iter()
        .filter_map(|item| {
            let identity = identity_of(item);
            let likeness = likeness(local, identity)?;
            let shared = shared_parents(local.parents, identity);
            Some(Candidate {
                likeness,
                shared,
                item,
            })
        })
        .collect();
    found.sort_by_key(|candidate| (candidate.likeness, std::cmp::Reverse(candidate.shared)));
    found
}

/// 候选里"最像的那一条"。
///
/// ⚠ **现在的规则就是取第一条** —— 而"第一条"的顺序由 [`candidates`] 保证（判据强弱，
/// 同档按父目录名重合数）。用户 2026-09-24："指纹命中多个游戏弹出最像的一个（什么是最像，
/// 现在还没有思路与确定，可以写个空函数默认取第一个或者随机取一个，这个问题就交给以后
/// 了）"。以后有更好的判据，**改 [`candidates`] 那一处**即可，这里不用动。
pub fn best_like<T>(candidates: &[T]) -> Option<&T> {
    candidates.first()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::cloud::MachineIdentity;

    fn machine(id: &str, prints: &[&str], parents: &[&str]) -> MachineIdentity {
        MachineIdentity {
            machine_id: id.to_string(),
            label: format!("host-{id}"),
            fingerprints: prints.iter().map(|print| print.to_string()).collect(),
            locations: Vec::new(),
            parents: parents.iter().map(|parent| parent.to_string()).collect(),
            exe_paths: Vec::new(),
        }
    }

    fn identity(name: &str, machines: Vec<MachineIdentity>) -> GameIdentity {
        let mut identity = GameIdentity::new("cloud-1", name);
        identity.machines = machines;
        identity
    }

    fn side<'a>(name: &'a str, prints: &'a [String], parents: &'a [String]) -> LocalSide<'a> {
        LocalSide {
            name,
            fingerprints: prints,
            parents,
        }
    }

    /// 一条身份带着好几个指纹时，命中**任意一个**就算指纹命中（用户 2026-09-24 的口径）。
    #[test]
    fn any_of_the_fingerprints_counts_as_a_hit() {
        let cloud = identity("某游戏", vec![machine("a", &["p1", "p2", "p3"], &[])]);
        let prints = ["p2".to_string()];
        assert_eq!(
            likeness(&side("别的名字", &prints, &[]), &cloud),
            Some(Likeness::Fingerprint)
        );
    }

    /// 指纹 > 名字 > 父目录名：几样都像时给最硬的那条；一样都不像就是 `None`。
    #[test]
    fn the_strongest_likeness_wins() {
        let cloud = identity("某游戏", vec![machine("a", &["p1"], &["game"])]);
        let prints = ["p1".to_string()];
        let parents = ["game".to_string()];
        assert_eq!(
            likeness(&side("某游戏", &prints, &parents), &cloud),
            Some(Likeness::Fingerprint)
        );
        assert_eq!(
            likeness(&side("某游戏", &[], &parents), &cloud),
            Some(Likeness::Name)
        );
        assert_eq!(
            likeness(&side("别的", &[], &parents), &cloud),
            Some(Likeness::ParentDir)
        );
        let unrelated = ["other".to_string()];
        assert_eq!(likeness(&side("别的", &[], &unrelated), &cloud), None);
    }

    /// 数的是**身份**：同一条身份的多台机器、多个指纹都只算一条；两条身份带同一个指纹
    /// 才算 2（那才是"多个游戏共有同一个指纹"，要问）。
    #[test]
    fn owners_count_identities_not_machines() {
        let one = identity(
            "一号",
            vec![
                machine("a", &["dup"], &[]),
                machine("b", &["dup", "x"], &[]),
            ],
        );
        let two = identity("二号", vec![machine("c", &["dup"], &[])]);
        let owners = fingerprint_owners(&[one, two]);
        assert_eq!(owners.get("dup"), Some(&2), "两条身份带着它 ⇒ 要问");
        assert_eq!(owners.get("x"), Some(&1), "同一条身份里只算一次");
        assert_eq!(owners.get("nope"), None);
    }

    /// 父目录名按"有几个对得上"计数（同一档里排序用它）。
    #[test]
    fn shared_parents_counts_the_overlap() {
        let cloud = identity("某游戏", vec![machine("a", &[], &["game", "vendor"])]);
        assert_eq!(shared_parents(&["game".into(), "vendor".into()], &cloud), 2);
        assert_eq!(shared_parents(&["game".into(), "other".into()], &cloud), 1);
        assert_eq!(shared_parents(&["other".into()], &cloud), 0);
    }

    /// **弱判据也要列出来，而且要排好序**（用户 2026-09-28："连疑似匹配都没有"）。
    ///
    /// 名字相同、存档父目录名重合的那几条都算候选：判据越硬越前，同一档里父目录名重合多的
    /// 在前 —— 于是"最像的那一条"就是 `[0]`（弹窗显示的就是它）。**只有指纹那条允许自动
    /// 绑**，这一条测试只管"列得出来、排得对"。
    #[test]
    fn weak_evidence_lists_candidates_ordered_by_likeness() {
        let by_name = identity("同一款", vec![machine("a", &[], &[])]);
        let by_parent = identity("另一款", vec![machine("b", &[], &["game"])]);
        let by_both = identity("同一款", vec![machine("c", &[], &["game", "vendor"])]);
        let unrelated = identity("无关的", vec![machine("d", &[], &["other"])]);
        let all = [&by_name, &by_parent, &by_both, &unrelated];

        // 本机这一款：没有指纹（所以只能靠弱判据），名字与两条父目录名。
        let prints: Vec<String> = Vec::new();
        let parents = ["game".to_string(), "vendor".to_string()];
        let found = candidates(
            &LocalSide {
                name: "同一款",
                fingerprints: &prints,
                parents: &parents,
            },
            all.iter().copied(),
            |identity| identity,
        );

        let names: Vec<&str> = found.iter().map(|one| one.item.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["同一款", "同一款", "另一款"],
            "命中的都要在，无关的那条不许进：{names:?}"
        );
        assert_eq!(found[0].likeness, Likeness::Name);
        assert_eq!(found[0].shared, 2, "同一档里父目录名重合多的排前面");
        assert_eq!(found[1].shared, 0);
        assert_eq!(found[2].likeness, Likeness::ParentDir);
    }
}
