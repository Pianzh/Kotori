//! 基线 `Baseline`（PLATFORMS.md §6.1(c)）：**我认账的云端那一版**，每个游戏一份、只在本地。
//!
//! 它记的是"上一次跟云端对上账时，本机 = 云端是**哪一版内容**"。有了它，
//! 判定（[`crate::sync::decision`]）才能把三方摆在一起比 —— 本机 / 基线 / 云端最新 ——
//! 而不是拿时间戳去猜"谁新"（§6.0）。
//!
//! 基线**只在两个时刻更新**（§6.0）：① 云端下载成功 ② 上传成功。「恢复到上一版」
//! **不动基线** —— 那是整套设计的支点（§6.7）。
//!
//! 落点：`<data_dir>/sync-baseline/<文件名>.json`，文件名 = game_id 里 `[\\/:*?"<>|]`
//! 换成 `_`（看得懂）+ `-` + BLAKE2b-256(game_id) 前 8 位（防撞）+ `.json` —— game_id
//! 可能是中文、可能很长，两个要求都得满足。
//!
//! ⚠ **读不到基线时的行为是「当作没有基线」（→ 走询问），绝不是猜一个**：文件不在、
//! 读不动、解析失败、格式不认识，四种一律返回 `None` + `warn`。判定那一头据此落到
//! "还没有基线"那两格（第 4/5 格）：内容一样就认账、不一样就问用户，绝不自作主张。
//!
//! 头上一度压着一整行 `#![allow(dead_code)]`（第 4/5 步刚写完时还没有调用方，CI 的
//! `-D warnings` 会红在"没人用"上）。第 6 步把判定接进启动前流程之后它就没必要了 ——
//! 现在只有 [`remove`] 一家还没有生产调用点，那一家单独挂着 `#[cfg(test)]`。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::sync::fingerprint::hash_bytes;

/// 基线文件的格式版本。**不认识就当读不懂**（`None`），绝不按当前字段猜。
pub const BASELINE_FORMAT: u32 = 1;
/// 基线放在数据目录下的这个子目录里（一个游戏一个文件）。
pub const BASELINE_DIR: &str = "sync-baseline";
/// 文件名里哈希取几位十六进制字符。
///
/// ⚠ 规格原文是「BLAKE2b-256(game_id) 前 8 位」，"位"没说清是十六进制字符还是字节
/// （`sync::index_cache` 那边取的是 8 **字节** = 16 个字符）。这里按 **8 个十六进制
/// 字符**实现，并且已经把这个问题提给用户裁决：真要改成 8 字节，只改这一个常量即可
/// （旧文件名读不到 = 当作没有基线，只会多问一次，不会认错人）。
const HASH_CHARS: usize = 8;

/// 我认账的云端那一版。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Baseline {
    /// 写它的时候 [`BASELINE_FORMAT`] 是多少。
    pub format: u32,
    /// 我认账的那一版的版本名（不带扩展名）。
    pub stamp: String,
    /// 那一版的**内容值**（64 个小写十六进制，见 `sync::archive::digest`）——
    /// 当时也就等于本机的内容值。
    pub digest: String,
    /// 写基线那一刻本机的 `mtime_ms`（§6.1(b)）。只用来省一次读盘与横幅措辞。
    pub mtime_ms: i64,
}

impl Baseline {
    /// 一份新基线（`format` 由这里填，调用方不必记它）。
    pub fn new(stamp: impl Into<String>, digest: impl Into<String>, mtime_ms: i64) -> Self {
        Self {
            format: BASELINE_FORMAT,
            stamp: stamp.into(),
            digest: digest.into(),
            mtime_ms,
        }
    }
}

/// 数据目录（基线文件都落在这儿）。
fn root() -> PathBuf {
    crate::config::data_dir()
}

/// 基线文件落在 `<root>/sync-baseline/` 下的哪一个。
pub fn path_in(root: &Path, game_id: &str) -> PathBuf {
    root.join(BASELINE_DIR).join(file_name(game_id))
}

/// 文件名：可读的那一半 + 防撞的那一半（§6.1(c)）。
pub fn file_name(game_id: &str) -> String {
    let readable: String = game_id
        .chars()
        .map(|ch| if is_illegal_in_file_name(ch) { '_' } else { ch })
        .collect();
    // 哈希取十六进制**字符**（`hash_bytes` 给的就是小写十六进制），所以切片不会落在字符中间。
    let hash: String = hash_bytes(game_id.as_bytes())
        .chars()
        .take(HASH_CHARS)
        .collect();
    format!("{readable}-{hash}.json")
}

/// Windows 上不能进文件名的九个字符（规格逐字列的）。
fn is_illegal_in_file_name(ch: char) -> bool {
    matches!(ch, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
}

/// 读这个游戏的基线 —— 不在 / 读不动 / 解析失败 / 格式不认识 ⇒ `None` + `warn`。
///
/// ⚠ `None` 的唯一含义是「**当作没有基线**」：判定那一头会去问用户，绝不猜一个出来。
pub fn load(game_id: &str) -> Option<Baseline> {
    load_at(&root(), game_id)
}

/// [`load`] 落在指定目录上的那一半（测试靠它待在临时目录里，绝不碰用户真实的 data 目录）。
pub fn load_at(root: &Path, game_id: &str) -> Option<Baseline> {
    let path = path_in(root, game_id);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        // 还没有基线（第一次同步、或者刚删过）：这是**常态**，不是"坏了"。
        // ⚠ 所以它是 `debug!` 而不是 `warn!`：每一款还没同步过的游戏每次启动都会走到这里，
        //    走 warn 的话日志里全是"还没有基线"，真正要看的"读不动 / 读不懂"会被淹掉。
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!("还没有基线（当作没有基线，走询问）: {}", path.display());
            return None;
        }
        Err(error) => {
            tracing::warn!(
                "基线读不动（当作没有基线，走询问）: {}: {error}",
                path.display()
            );
            return None;
        }
    };
    let baseline: Baseline = match serde_json::from_str(&text) {
        Ok(baseline) => baseline,
        Err(error) => {
            tracing::warn!(
                "基线读不懂（当作没有基线，走询问）: {}: {error}",
                path.display()
            );
            return None;
        }
    };
    // 未来版本写的基线不该被这一版按当前字段解释（`Manifest` / 索引那边同一条规矩）。
    if baseline.format != BASELINE_FORMAT {
        tracing::warn!(
            "基线的格式不认识（当作没有基线，走询问）: {}（格式 {}）",
            path.display(),
            baseline.format
        );
        return None;
    }
    Some(baseline)
}

/// 写这个游戏的基线（**原子写**）。`Err` 里是给人看的中文。
pub fn save(game_id: &str, baseline: &Baseline) -> Result<(), String> {
    save_at(&root(), game_id, baseline)
}

/// [`save`] 落在指定目录上的那一半。
///
/// 原子写：同目录的临时文件 + `sync_all` + `rename`（照 `config::save_to` 的形状）。
/// 直接写目标文件会先把它截断 —— 崩在半路就只剩半份 JSON，而读方只能"当作没有基线"
/// （安全，但那意味着把上一次的对账结果白扔了，用户被多问一次）。
pub fn save_at(root: &Path, game_id: &str, baseline: &Baseline) -> Result<(), String> {
    let path = path_in(root, game_id);
    let dir = path
        .parent()
        .ok_or_else(|| format!("基线路径没有父目录: {}", path.display()))?;
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("建基线目录 {} 失败: {error}", dir.display()))?;
    let bytes =
        serde_json::to_vec_pretty(baseline).map_err(|error| format!("基线序列化失败: {error}"))?;

    let tmp = path.with_extension("json.tmp");
    let mut file = std::fs::File::create(&tmp)
        .map_err(|error| format!("建基线临时文件 {} 失败: {error}", tmp.display()))?;
    let written = std::io::Write::write_all(&mut file, &bytes).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = written {
        // 失败就把它收掉：目录里留一个半截的临时文件，只会让人以为基线坏了。
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("写基线临时文件 {} 失败: {error}", tmp.display()));
    }
    std::fs::rename(&tmp, &path).map_err(|error| {
        format!(
            "基线改名 {} → {} 失败: {error}",
            tmp.display(),
            path.display()
        )
    })
}

/// 删掉这个游戏的基线（删游戏、关同步时调用）。文件不在 = 已经删过了，不是错。
///
/// ⚠ **今天还没有生产调用点**（§6.1(c) 说的那两个触发点里，"关同步"的语义还没定：
/// 关的是全局开关还是单款？要不要连身份一起忘掉？——已提给用户裁决，见交接报告）。
/// 所以它是这个文件里**唯一**挂着 `allow(dead_code)` 的条目：只标在它头上，而不是像
/// 从前那样在文件顶上压一整行 `#![allow(dead_code)]` 把整个文件盖住（那会连"以后真写死的
/// 代码"一起藏起来）。触发点定下来之后去掉这一行、在那儿调它。
#[allow(dead_code)]
pub fn remove(game_id: &str) {
    remove_at(&root(), game_id)
}

/// [`remove`] 落在指定目录上的那一半。
pub fn remove_at(root: &Path, game_id: &str) {
    let path = path_in(root, game_id);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!("删基线 {} 失败: {error}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::archive::DIGEST_HEX_LEN;

    fn sample() -> Baseline {
        Baseline::new(
            "20261004T101500Z",
            "a".repeat(DIGEST_HEX_LEN),
            1_759_570_000_000,
        )
    }

    /// 目录里现在有哪些名字（排序）—— "不许有残留的临时文件"靠它断言。
    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("目录该在")
            .map(|entry| {
                entry
                    .expect("读目录项")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    /// 不在 / 读不动 / 读不懂 / 格式不认识 —— 四种都必须是 `None`（当作没有基线，走询问）。
    #[test]
    fn a_missing_or_broken_baseline_reads_as_none() {
        let root = crate::config::test_scratch("baseline-broken");
        let game_id = "示例游戏";
        let path = path_in(&root, game_id);

        // ① 从来没有过（第一次同步、或者刚删过）。
        assert!(load_at(&root, game_id).is_none(), "不在 = 当作没有基线");
        // 删一个不存在的基线不该炸。
        remove_at(&root, game_id);

        // ② 写坏了（半截 JSON、被谁改过）：绝不许把半份东西当成基线。
        std::fs::create_dir_all(path.parent().expect("基线路径总有父目录")).unwrap();
        std::fs::write(&path, b"{ \"format\": 1, \"stamp\": ").unwrap();
        assert!(load_at(&root, game_id).is_none(), "读不懂 = 当作没有基线");

        // ③ 格式不认识（未来版本写的）。
        let future = Baseline {
            format: BASELINE_FORMAT + 1,
            ..sample()
        };
        std::fs::write(&path, serde_json::to_vec(&future).unwrap()).unwrap();
        assert!(load_at(&root, game_id).is_none(), "不认识的格式绝不瞎读");

        // ④ 读不动（路径其实是个目录）。
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir_all(&path).unwrap();
        assert!(load_at(&root, game_id).is_none(), "读不动 = 当作没有基线");

        // ⑤ 写下去读得回来、删掉之后又回到"没有基线"。
        std::fs::remove_dir_all(&path).unwrap();
        save_at(&root, game_id, &sample()).expect("写基线");
        assert!(load_at(&root, game_id).is_some(), "刚写的该读得回来");
        remove_at(&root, game_id);
        assert!(load_at(&root, game_id).is_none(), "删掉就是没有");
    }

    /// 写完：目标文件已经存在、内容**完整**，而且目录里没有残留的临时文件。
    #[test]
    fn saving_a_baseline_is_atomic() {
        let root = crate::config::test_scratch("baseline-atomic");
        let game_id = "示例:游戏/甲";
        let path = path_in(&root, game_id);
        assert!(!path.exists(), "还没写过就是不在");

        let first = sample();
        save_at(&root, game_id, &first).expect("写基线");
        assert!(path.is_file(), "写完之后目标文件就该在");
        // 完整：整份 JSON 读得回来，与写下去的一模一样（不是半截、也不是别人的形状）。
        let text = std::fs::read_to_string(&path).expect("读回刚写的基线");
        let parsed: Baseline = serde_json::from_str(&text).expect("写下去的就是一份完整的 JSON");
        assert_eq!(parsed, first);
        assert_eq!(load_at(&root, game_id), Some(first));
        assert_eq!(
            entries(path.parent().expect("基线路径总有父目录")),
            vec![file_name(game_id)],
            "临时文件不许留下"
        );

        // 再写一次（换一版）：目标文件被换掉，目录里依然只有它一个。
        let second = Baseline::new(
            "20261005T101500Z",
            "b".repeat(DIGEST_HEX_LEN),
            1_759_572_000_000,
        );
        save_at(&root, game_id, &second).expect("覆盖写基线");
        assert_eq!(load_at(&root, game_id), Some(second));
        assert_eq!(
            entries(path.parent().expect("基线路径总有父目录")),
            vec![file_name(game_id)],
            "覆盖写也不许留下临时文件"
        );
    }

    /// 文件名规则（§6.1(c)）：非法字符换掉、哈希补上、不同 game_id 不撞名。
    #[test]
    fn the_file_name_is_readable_and_collision_free() {
        const ILLEGAL: [char; 9] = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];

        let game_id = "C:\\Games/甲:乙*丙?丁\"戊<己>庚|辛";
        let expected_readable = "C__Games_甲_乙_丙_丁_戊_己_庚_辛";
        let name = file_name(game_id);
        assert!(!name.contains(ILLEGAL), "九个非法字符一个都不许剩: {name}");
        assert!(name.ends_with(".json"), "{name}");
        assert!(name.starts_with(expected_readable), "{name}");

        // 结尾是 8 位**小写十六进制**，而且就是 game_id 的 BLAKE2b-256 前几位。
        let stem = name.strip_suffix(".json").expect("以 .json 结尾");
        let (readable, hash) = stem.rsplit_once('-').expect("可读部分与哈希之间有 -");
        assert_eq!(readable, expected_readable);
        assert_eq!(hash.len(), HASH_CHARS);
        assert!(
            hash.chars()
                .all(|ch| ch.is_ascii_digit() || ('a'..='f').contains(&ch)),
            "哈希得是小写十六进制: {hash}"
        );
        assert_eq!(&hash_bytes(game_id.as_bytes())[..HASH_CHARS], hash);

        // 可读部分相同的两个 game_id **绝不撞名** —— 中文名、很长的 id 全靠哈希这几位分开。
        let slash = file_name("游戏/甲");
        let colon = file_name("游戏:甲");
        assert!(slash.starts_with("游戏_甲-"), "{slash}");
        assert!(colon.starts_with("游戏_甲-"), "{colon}");
        assert_ne!(slash, colon, "可读部分一样，就得靠哈希分开");

        // 同一个 game_id 每次都得到同一个名字（写与读必须认同一个文件）。
        assert_eq!(file_name(game_id), name);

        // 长 id（很长的中文名 + 带符号的路径）：可读那半照留，方便一眼看出是哪一款。
        let long = format!(
            "{}-{}",
            "很长的一款游戏名".repeat(20),
            "非常长的目录/路径:带符号"
        );
        assert!(file_name(&long).starts_with(&long.replace(ILLEGAL, "_")));
        // 换一个非法字符（'/' → ':'）之后可读部分一模一样，名字仍然不同。
        assert_ne!(file_name(&long), file_name(&long.replace('/', ":")));
    }
}
