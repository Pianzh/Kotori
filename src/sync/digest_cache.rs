//! 本机内容值的**登记簿**：这一款的内容值是哪一刻、按什么时间算出来的（PLATFORMS.md §6.8 A）。
//!
//! ## 它为什么存在
//!
//! 算 [`crate::sync::archive::local_digest`] 要把该游戏**所有存档文件读一遍**，而 §6.1/§6.4
//! 把这条路钉得很死：平时只 `stat`（[`crate::sync::archive::local_mtime_ms`]），只有
//! "要拿云端覆盖本机"那一刻（§6.4 的 `Confirm`）才读内容。可 §6.8 A 的横幅版式里又要求
//! 显示"本机 digest 前 8 位" —— 这是规格里的一处冲突（已提给用户裁决，见回报）。
//!
//! 这里取的是**不重读**的那一半：**只有真的算过一次**（§6.4 的核对、取回成功、手动恢复、
//! 上传成功）才记下来，记的是 `(digest, 算它的那一刻本机的 mtime_ms)`。横幅要显示时先来
//! 这里问一句：**mtime 没变就是同一个值**（可以直接显），mtime 变了就是"本机动过、这个值
//! 过期了"（当作没算过，横幅上写"未核对"）。
//!
//! ## 三条不许忘的
//!
//! * **只活在内存里**：不是落盘缓存。daemon 一重启就没有了 —— 那时横幅上写"未核对"，
//!   而"未核对"永远是**诚实**的（我们确实没核对过）。
//! * **key 只有 game_id 一个**（与基线、启动前那块牌子同一把尺子），不用自己拼文件名，
//!   也就没有"两个地方各算一个 key"那种错位。
//! * **mtime 对不上就是没有**：绝不拿一个过期的内容值去显示"本机是这一版"。

use std::collections::HashMap;
use std::sync::Mutex;

/// 记着的那一个值：内容值 + 算它时的本机 `mtime_ms`。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    digest: String,
    mtime_ms: i64,
}

/// 每款游戏一份的内容值登记簿。
#[derive(Default)]
pub struct DigestCache {
    entries: Mutex<HashMap<String, Entry>>,
}

impl DigestCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记下"这一款在这一刻的内容值"。`mtime_ms` 必须是**算 digest 那一刻**本机的值
    /// （§6.1(b) 的同一个量），否则下一次问就会被判成过期。
    pub fn remember(&self, game_id: &str, digest: &str, mtime_ms: i64) {
        if digest.is_empty() {
            return;
        }
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(
                game_id.to_string(),
                Entry {
                    digest: digest.to_string(),
                    mtime_ms,
                },
            );
        }
    }

    /// 这一款的、**对得上现在这个 mtime** 的内容值；没算过 / 已经过期就是 `None`。
    ///
    /// `None` 的含义只有一个：**这一刻我们不知道本机的内容值** —— 调用方据此显示
    /// "未核对"，绝不许拿一个过期的值顶替。
    pub fn get(&self, game_id: &str, mtime_ms: i64) -> Option<String> {
        let entries = self.entries.lock().ok()?;
        let entry = entries.get(game_id)?;
        (entry.mtime_ms == mtime_ms).then(|| entry.digest.clone())
    }

    /// 忘掉这一款（换存档位置、删游戏之后不认旧账）。今天还没有生产调用点。
    #[allow(dead_code)]
    pub fn forget(&self, game_id: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(game_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 记下来的值只在**同一个 mtime** 下有效：本机动过之后它就是过期账，必须当作没算过。
    #[test]
    fn a_digest_only_counts_while_the_mtime_is_the_same() {
        let cache = DigestCache::new();
        assert!(cache.get("demo", 100).is_none(), "没算过就是不知道");

        cache.remember("demo", &"a".repeat(64), 100);
        assert_eq!(cache.get("demo", 100), Some("a".repeat(64)));
        assert!(
            cache.get("demo", 101).is_none(),
            "mtime 变了 = 本机动过，那个值是过期账"
        );
        assert!(cache.get("other", 100).is_none(), "别的游戏不认这笔账");
    }

    /// 空串不是内容值（`archive::digest_of` 永远给 64 位十六进制）—— 记它等于认一笔假账。
    #[test]
    fn an_empty_digest_is_never_remembered() {
        let cache = DigestCache::new();
        cache.remember("demo", "", 100);
        assert!(cache.get("demo", 100).is_none());
    }

    #[test]
    fn forgetting_a_game_drops_only_that_game() {
        let cache = DigestCache::new();
        cache.remember("demo", &"a".repeat(64), 100);
        cache.remember("other", &"b".repeat(64), 100);
        cache.forget("demo");
        assert!(cache.get("demo", 100).is_none());
        assert!(cache.get("other", 100).is_some());
    }
}
