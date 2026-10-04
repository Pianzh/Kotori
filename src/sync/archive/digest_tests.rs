//! `archive::digest` 的单测：内容值的算法与两条入口（只 stat / 读全文）。
//!
//! 这些测试全部在临时目录里跑，不碰网络、不碰 rclone。最要紧的是第一条：
//! **跨机器可比**是这个算法存在的理由 —— 两台机器上位置 key 不同、内容相同时
//! digest 必须相等，否则"谁新谁旧"整套判定会认错版本（用户现场没法验，全靠这里）。

use std::path::{Path, PathBuf};

use super::*;
use crate::sync::SaveTarget;

fn temp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "kotori-digest-{tag}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 一个存档位置。`key` 就是"这台机器上这个位置叫什么"，**不进 digest**。
fn target(key: &str, local: &Path) -> SaveTarget {
    SaveTarget {
        key: key.to_string(),
        configured: key.to_string(),
        local: local.to_path_buf(),
        exclude: Vec::new(),
    }
}

fn write(dir: &Path, relative: &str, body: &str) {
    let path = dir.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// 每个位置一个 key，内容逐字相同 —— 除了 key，两台机器没有任何差别。
fn machine(root: &Path) -> Vec<SaveTarget> {
    vec![
        target("rel-savedata", &root.join("savedata")),
        target("win-appdata", &root.join("appdata")),
    ]
}

/// 造一台机器：两个位置、各两个文件（其中一个同名同内容）。
fn make_machine(root: &Path) {
    write(&root.join("savedata"), "save01.sav", "one");
    write(&root.join("savedata"), "global.dat", "shared");
    write(&root.join("appdata"), "save02.sav", "two");
    write(&root.join("appdata"), "global.dat", "shared");
}

/// 形状断言：64 个小写十六进制字符（规格 §6.1(a) 钉死的形状）。
fn assert_well_formed(digest: &str) {
    assert_eq!(digest.len(), DIGEST_HEX_LEN, "{digest}");
    assert!(
        digest
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "只许小写十六进制: {digest}"
    );
}

/// 同一个游戏、两台机器：位置 key 不同、路径不同，**内容逐字相同** ⇒ digest 必须相等。
///
/// 这是"跨机器可比"的命根子：Windows 的 AppData 与 wine 的 prefix 上，同一款游戏的
/// 位置 key 与绝对路径几乎总是不同。谁把位置 key 或绝对路径混进算法，这条就红。
#[test]
fn the_same_content_under_different_location_keys_has_the_same_digest() {
    let left = temp("keys-left");
    let right = temp("keys-right");
    make_machine(&left);
    make_machine(&right);

    let here = machine(&left);
    // 另一台机器：key 全换、目录名也全换 —— 只有"位置内的相对路径 + 内容"是一样的。
    let there = vec![
        target("savedata-in-prefix", &right.join("savedata")),
        target("appdata-in-prefix", &right.join("appdata")),
    ];

    let a = local_digest(&here).unwrap();
    let b = local_digest(&there).unwrap();
    assert_well_formed(&a);
    assert_eq!(a, b, "内容相同、key 不同 ⇒ digest 必须相等");

    // 反过来量一遍：算法真的**没有**把绝对路径或 key 掺进去。
    // （`digest_of` 是纯函数，这里直接喂它"只有相对路径与哈希"的输入。）
    let entries = file_hashes(&here).unwrap();
    let pairs: Vec<(&str, &str)> = entries
        .iter()
        .map(|(relative, hash)| (relative.as_str(), hash.as_str()))
        .collect();
    assert_eq!(digest_of(pairs), a, "两条入口必须给出同一个值");

    // 相对路径**不带**位置 key：带上就跨不了机器。
    let relatives: Vec<&str> = entries
        .iter()
        .map(|(relative, _)| relative.as_str())
        .collect();
    assert_eq!(
        relatives,
        vec!["global.dat", "save01.sav", "global.dat", "save02.sav"],
        "只该是位置内那一段（按位置顺序收集）"
    );

    // 同一个文件内容变一个字节 ⇒ 值就变（算法真的在看内容）。
    write(&left.join("savedata"), "save01.sav", "one!");
    let changed = local_digest(&here).unwrap();
    assert_ne!(changed, a, "内容变了 digest 必须变");
    assert_well_formed(&changed);

    std::fs::remove_dir_all(&left).ok();
    std::fs::remove_dir_all(&right).ok();
}

/// 加一个文件、删一个文件，都必须让 digest 变 —— 而且**两条路都对**。
#[test]
fn adding_or_removing_a_file_changes_the_digest() {
    let root = temp("add-remove");
    // ⚠ 目录名必须是 `make_machine` 造的那两个之一（`savedata` / `appdata`）。
    //    写成别的名字，`collect_files` 会把它当成"这个位置在本机不存在"（记进
    //    `missing`，不报错）—— 于是"加一个文件"其实是在一个空目录里加，
    //    后面那句"删掉一个本来就有的文件"直接 `unwrap` 炸掉，而前半段看着是过的。
    let saves = root.join("savedata");
    make_machine(&root);

    let one = vec![target("rel-savedata", &saves)];
    let empty = root.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let none = vec![target("rel-savedata", &empty)];

    // 只有 `save01.sav` + `global.dat` 的那一版（`appdata` 不在配置里 ⇒ 不算）。
    let before = local_digest(&one).unwrap();
    assert_well_formed(&before);

    // 加一个文件。
    write(&saves, "save02.sav", "two");
    let added = local_digest(&one).unwrap();
    assert_ne!(added, before, "多一个文件 digest 必须变");

    // 再把加的那个删掉：回到原来那一版，值也必须回到原来那个。
    std::fs::remove_file(saves.join("save02.sav")).unwrap();
    assert_eq!(
        local_digest(&one).unwrap(),
        before,
        "删掉之后要回到原来那一版的值"
    );

    // 删一个本来就有的。
    std::fs::remove_file(saves.join("global.dat")).unwrap();
    let removed = local_digest(&one).unwrap();
    assert_ne!(removed, before, "少一个文件 digest 必须变");

    // 空集合是**同一个算法**算出来的：Python 的
    // `hashlib.blake2b(b"", digest_size=32).hexdigest()` 就是这个值。
    // 把它钉死在这里，是为了证明"空集合不特例"（规格 §6.1(a) 最后一行）。
    let empty_digest = local_digest(&none).unwrap();
    assert_eq!(
        empty_digest, "0e5751c026e543b2e8ab2eb06099daa1d1e5df47778f7787faab45cdf12fe3a8",
        "空集合走的是同一个算法"
    );
    assert_ne!(empty_digest, removed);

    std::fs::remove_dir_all(&root).ok();
}

/// 一个文件都没有（位置在、但目录是空的）⇒ 一个稳定的值，而且是空字节流的哈希。
#[test]
fn an_empty_save_directory_has_a_stable_digest() {
    let root = temp("empty");
    let first = root.join("first");
    let second = root.join("second");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();

    let empty = vec![target("rel-savedata", &first)];
    let digest = local_digest(&empty).unwrap();
    assert_well_formed(&digest);
    // 值里不许混进绝对路径或 key：同一份空集合必须永远算出同一个值。
    assert_eq!(digest, local_digest(&empty).unwrap(), "同一个输入要稳定");
    assert_eq!(
        digest,
        local_digest(&[target("另一个名字的位置", &second)]).unwrap(),
        "空集合与路径、key 都无关"
    );
    // 本机一个文件都没有 ⇒ mtime 是 0（§6.1(b)），digest 照样有值（不特例）。
    assert_eq!(local_mtime_ms(&empty).unwrap(), 0);

    std::fs::remove_dir_all(&root).ok();
}

/// 只碰修改时间、内容一个字节不动 ⇒ digest 不变，而 mtime 会变。
///
/// 这正是"平时只 stat、需要时才读内容"能成立的原因：mtime 只是**省一次读盘**的
/// 快路，最后的判据永远是 digest。
#[test]
fn touching_mtime_without_changing_content_keeps_the_digest() {
    let root = temp("touch");
    let saves = root.join("saves");
    write(&saves, "save01.sav", "one");
    write(&saves, "nested/save02.sav", "two");
    let targets = vec![target("rel-savedata", &saves)];

    let before = local_digest(&targets).unwrap();
    let mtime_before = local_mtime_ms(&targets).unwrap();
    assert!(mtime_before > 0, "有文件就该有 mtime");

    // 把时间盖到一个明显不同的值上（内容一字不动）。
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(saves.join("save01.sav"))
        .unwrap();
    let later =
        std::time::UNIX_EPOCH + std::time::Duration::from_millis(mtime_before as u64 + 10_000);
    file.set_modified(later).unwrap();
    drop(file);

    let mtime_after = local_mtime_ms(&targets).unwrap();
    assert_ne!(mtime_after, mtime_before, "时间真的被碰过了");
    assert_eq!(
        local_digest(&targets).unwrap(),
        before,
        "只碰时间不改内容 ⇒ digest 必须一模一样"
    );

    std::fs::remove_dir_all(&root).ok();
}

/// 位置在配置里的顺序不影响 digest（条目按 `(relative, hash)` 排序）。
///
/// 两台机器的配置顺序天然可能不同，顺序若进了算法，跨机器比较就废了。
#[test]
fn the_digest_ignores_the_order_the_locations_are_configured_in() {
    let root = temp("order");
    make_machine(&root);
    let forwards = machine(&root);
    let backwards: Vec<SaveTarget> = forwards.iter().rev().cloned().collect();

    let a = local_digest(&forwards).unwrap();
    let b = local_digest(&backwards).unwrap();
    assert_well_formed(&a);
    assert_eq!(a, b, "顺序不该进算法");

    // 收集出来的顺序确实不同（否则这条测试什么也没验到）。
    let first: Vec<String> = file_hashes(&forwards)
        .unwrap()
        .into_iter()
        .map(|(relative, _)| relative)
        .collect();
    let second: Vec<String> = file_hashes(&backwards)
        .unwrap()
        .into_iter()
        .map(|(relative, _)| relative)
        .collect();
    assert_ne!(first, second, "两个顺序下的收集顺序本来就该不同");

    // 只有相对路径与哈希进字节流：位置 key 一个字都不许出现。
    assert!(
        !a.contains("savedata") && !a.contains("appdata"),
        "digest 是哈希，不该长得像路径: {a}"
    );

    std::fs::remove_dir_all(&root).ok();
}

/// 排除规则不同 ⇒ digest 不同（保守方向，规格 §6.1(a) 明确接受）。
///
/// 这一条不进 §6.9 的五条名单，但它是"收集规则与打包完全相同"的直接后果，得有测试守着。
#[test]
fn a_different_exclude_list_produces_a_different_digest() {
    let root = temp("exclude");
    let saves = root.join("saves");
    write(&saves, "save01.sav", "one");
    write(&saves, "debug.log", "noise");

    let all = vec![target("rel-savedata", &saves)];
    let mut filtered = all.clone();
    filtered[0].exclude = vec!["*.log".to_string()];

    let with_log = local_digest(&all).unwrap();
    let without_log = local_digest(&filtered).unwrap();
    assert_ne!(with_log, without_log);
    // 被排除掉的那个文件确实不在清单里（"与打包同一套规则"）。
    let kept: Vec<String> = file_hashes(&filtered)
        .unwrap()
        .into_iter()
        .map(|(relative, _)| relative)
        .collect();
    assert_eq!(kept, vec!["save01.sav".to_string()], "{kept:?}");

    std::fs::remove_dir_all(&root).ok();
}

/// 重复条目**不去重**：同一份内容在两个位置各出现一次，与只出现一次必须不同。
///
/// 去掉重复会让"一个文件"与"两个同内容文件"算出同一个值 —— 那是判定表第 12 格
/// （本机为空 / 本机有东西）最怕的事。这里直接喂纯函数，是算法本身的性质。
#[test]
fn duplicate_entries_are_kept_instead_of_deduplicated() {
    let once = digest_of([("global.dat", "aa")]);
    let twice = digest_of([("global.dat", "aa"), ("global.dat", "aa")]);
    assert_ne!(once, twice, "两条一样的记录也要照留");
    assert_ne!(once, digest_of([("other.dat", "aa")]));
    assert_ne!(once, digest_of([("global.dat", "bb")]));
    // 排序是按 (relative, hash) 两段：相对路径相同的两条，再看哈希。
    assert_eq!(
        twice,
        digest_of([("global.dat", "aa"), ("global.dat", "aa")])
    );
}

/// 逐字节钉死一次算法的编码（`relative` + `\0` + `hash` + `\n` 拼起来再哈希）。
///
/// 参照值由另一个实现（Python `hashlib.blake2b(digest_size=32)`，同为 RFC 7693）
/// 独立算出：`blake2b(b"save01.sav\0" + blake2b(b"one").hexdigest() + b"\n")`。
/// 这条锁住的是"分隔符与拼接顺序"—— 它们一旦变了，两台机器就会算出两个值，而这种错
/// 在单机上永远发现不了（两边都是自己算的）。
#[test]
fn the_byte_stream_is_pinned_against_an_independent_implementation() {
    let root = temp("pinned");
    let saves = root.join("saves");
    write(&saves, "save01.sav", "one");

    let digest = local_digest(&[target("rel-savedata", &saves)]).unwrap();
    assert_eq!(
        digest, "834f5dc70561d07d46a7ba91ad96560a44b6095e64ff0f8a032add62d04c3b54",
        "拼接方式（relative\\0hash\\n）不许变"
    );

    // 换个位置 key、换个目录名，值不变（key 与绝对路径都不进字节流）。
    let elsewhere = root.join("别的地方");
    write(&elsewhere, "save01.sav", "one");
    assert_eq!(
        local_digest(&[target("完全不同的-key", &elsewhere)]).unwrap(),
        digest
    );

    std::fs::remove_dir_all(&root).ok();
}
