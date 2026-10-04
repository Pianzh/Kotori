//! `digest`：一款游戏的存档"内容值"（PLATFORMS.md §6.1(a)、§6.1(b)）。
//!
//! 用途是**跨机器比"我们说的是不是同一版"**。两台机器上同一款游戏的位置 key 常常
//! 不同（Windows 的 AppData vs wine 的 prefix），而内容相同时 digest 必须相等 —— 所以
//! 条目里**只有位置内的相对路径，没有位置的 key**（这是本算法的命根子）。排除规则
//! （`exclude`）不同则 digest 不同：保守方向，可以接受。
//!
//! 算法（逐条照规格，**空集合也走同一条路**，不特例）：
//!
//!  1. 每个位置用**与打包完全相同**的收集规则列文件
//!     （[`gather::collect_files`]，不跟随符号链接）；
//!  2. 每条记成 `(relative, BLAKE2b-256(文件全文))`；
//!  3. 全部条目按 `(relative, hash)` 字典序排序，**保留重复条目**（不去重）；
//!  4. 拼字节流：每条 `relative` + `\0` + `hash` + `\n`；
//!  5. `digest` = BLAKE2b-256(该字节流) 的小写十六进制，64 个字符。
//!
//! ## 两个入口：读内容的与不读内容的（这是性能上的硬要求）
//!
//! 算 digest 要**把存档全读一遍**，而启动前的判定平时只想 `stat` 一遍拿个时间。
//! 所以这里刻意分成两条路：
//!
//!   * [`local_mtime_ms`] —— 只 `stat`，一个字节的内容都不读（走 [`gather`] 的收集
//!     规则，但只量修改时间）；
//!   * [`local_digest`] —— 读全文算 digest（只在"要拿云端覆盖本机"时才算，见 §6.4）。
//!
//! [`digest_of`] 是纯函数（给定 `(relative, hash)` 就算），打包那条路拿它把
//! 自己**已经读过一遍的字节**变成 digest，不必为了这个值再读一遍盘。

use blake2::{Blake2b256, Digest};

use super::gather::{collect_all, gather};
use crate::sync::SaveTarget;
use crate::sync::fingerprint::hash_bytes;

/// 一条存档内容值的十六进制长度（BLAKE2b-256 = 32 字节 = 64 个十六进制字符）。
pub const DIGEST_HEX_LEN: usize = 64;

/// 把 `(位置内的相对路径, 内容哈希)` 算成 digest —— 规格第 3～5 步（纯函数）。
///
/// `relative` 是**位置内**的相对路径（`/` 分隔），**不含存档位置的 key**：含了它，
/// 两台机器上位置 key 不同就会算出两个值，跨机器比较立刻失效。
///
/// 排序按 `(relative, hash)` 两段；**重复条目照留**：同一份内容在两个位置各出现一次、
/// 甚至同一个位置里两条同路径的记录（配置写重了），都要如实进字节流 —— 这条路上的
/// "去重"会让"一份文件和两份文件"算出同一个值，那是判定表第 12 格最怕的事。
pub fn digest_of<'a>(entries: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    let mut sorted: Vec<(&str, &str)> = entries.into_iter().collect();
    sorted.sort_unstable();

    let mut stream = Vec::new();
    for (relative, hash) in sorted {
        stream.extend_from_slice(relative.as_bytes());
        stream.push(0);
        stream.extend_from_slice(hash.as_bytes());
        stream.push(b'\n');
    }
    hex(&blake2b(&stream))
}

/// 算出本机这一款的内容值 —— **会读全部存档内容**。
///
/// `Err` 表示"算不出来"（有文件读不动）：调用方必须把它当成"还没有 digest"，绝不许
/// 拿一个残缺的值去比 —— 那会让"本机没动"与"本机动了"认错。
pub fn local_digest(targets: &[SaveTarget]) -> Result<String, String> {
    let entries = file_hashes(targets)?;
    let pairs: Vec<(&str, &str)> = entries
        .iter()
        .map(|(relative, hash)| (relative.as_str(), hash.as_str()))
        .collect();
    Ok(digest_of(pairs))
}

/// 本机这一款所有文件的 `(位置内的相对路径, BLAKE2b-256(全文))`。
///
/// 顺序是收集顺序（按位置、按相对路径），**没有排序**：算 digest 的那一步自己会排。
/// 单独暴露出来是为了"同一批字节只读一遍"：打包那条路已经把它们读进内存了，将来要把
/// "读文件"与"算 digest"合成一次读时，接口就在这里（[`digest_of`] 随时能接住那批哈希）。
pub fn file_hashes(targets: &[SaveTarget]) -> Result<Vec<(String, String)>, String> {
    let mut locations = Vec::new();
    let mut missing = Vec::new();
    let mut excluded = 0;
    let files = collect_all(targets, &mut locations, &mut missing, &mut excluded)?;

    let mut entries = Vec::with_capacity(files.len());
    for (_key, absolute, relative) in files {
        // 读不到（盘拔了、没权限）就如实报错：调用方必须把它当成"算不出来"，
        // 绝不许拿一个残缺的值去比。
        let bytes = std::fs::read(&absolute)
            .map_err(|e| format!("读取 {} 失败: {e}", absolute.display()))?;
        entries.push((relative, hash_bytes(&bytes)));
    }
    Ok(entries)
}

/// 本机这一款的 `mtime_ms`（§6.1(b)）—— **只 `stat`，不读内容**。
///
/// 所有位置下所有文件的 [`mtime_ms`] 的最大值；一个文件都没有（位置都不在、或者
/// 目录是空的）就是 0。每个文件的尺寸也顺手量了（走的是打包那套收集规则），但内容
/// 一个字节不读 —— 启动前的判定就靠这条快路。
pub fn local_mtime_ms(targets: &[SaveTarget]) -> Result<i64, String> {
    let gathered = gather(targets)?;
    Ok(gathered
        .entries
        .iter()
        .map(|entry| entry.mtime_ms)
        .max()
        .unwrap_or(0)
        .max(0i64))
}

/// BLAKE2b-256 一个字节流，给回 32 字节。
fn blake2b(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Blake2b256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

/// 32 字节 → 64 个小写十六进制字符。
fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

#[cfg(test)]
// ⚠ 测试住在同一目录的另一个文件里（与 `index_tests` 同一个习惯）：`digest.rs`
// 连着测试一起数会越过 500 行的线（AGENTS.md）。
#[path = "digest_tests.rs"]
mod digest_tests;
