//! 拼音搜索工具：把中文转成全拼和首字母，供搜索匹配用。
//!
//! 用 `pinyin` crate 做转换，它按最常见读音处理多音字，游戏名场景够用。
//!
//! ⚠ **非汉字字符（英文、数字、符号）不参与拼音**：`pinyin` 那边是逐字符看的，认不得的
//! 直接跳过（`to_pinyin_vec` 里那次 `filter_map`）。所以拼音只是一条**补充判据** ——
//! 英文名与路径那些命中靠调用方自己的 `contains`（见 `parse::games`、`model::cloud`、
//! `model::picker`），而它们都排在拼音之前。
//!
//! 这段文档从前写的是"非汉字原样保留在结果里"，与实现不符，2026-09-28 CI 上因此红过一次
//! （测试照着那句错文档写的）。

use pinyin::{Pinyin, to_pinyin_vec};

/// 把文本转成全拼（小写、无空格）。
/// 例：「王者荣耀」→「wangzherongyao」
pub(in crate::ui) fn to_full_pinyin(text: &str) -> String {
    to_pinyin_vec(text, Pinyin::plain).concat().to_lowercase()
}

/// 把文本转成首字母（小写）。
/// 例：「王者荣耀」→「wzry」
pub(in crate::ui) fn to_initials(text: &str) -> String {
    to_pinyin_vec(text, Pinyin::plain)
        .iter()
        .filter_map(|s| s.chars().next())
        .collect::<String>()
        .to_lowercase()
}

/// 拼音匹配：查询词是否命中文本的全拼或首字母。
/// 调用方需自行对查询词做 `trim().to_lowercase()`。
pub(in crate::ui) fn matches_pinyin(text: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let full = to_full_pinyin(text);
    if full.contains(query) {
        return true;
    }
    let initials = to_initials(text);
    initials.contains(query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_pinyin_converts_chinese() {
        assert_eq!(to_full_pinyin("王者荣耀"), "wangzherongyao");
        assert_eq!(to_full_pinyin("黑魂"), "heihun");
    }

    #[test]
    fn initials_converts_chinese() {
        assert_eq!(to_initials("王者荣耀"), "wzry");
        assert_eq!(to_initials("黑魂"), "hh");
    }

    /// 非汉字**不参与拼音**（`pinyin` 那边认不得的字符会跳过）。
    ///
    /// ⚠ 别把它写成"原样保留"—— 实现给不出那个结果（2026-09-28 CI 上因此红过一次）。
    /// 英文名不受影响：调用方那条"名字本身也 `contains`"排在拼音之前。
    #[test]
    fn non_chinese_is_not_part_of_the_pinyin() {
        assert_eq!(to_full_pinyin("Game"), "");
        assert_eq!(to_initials("Game"), "");
        // 混在一起时：汉字照旧有拼音，中间那个符号被跳过。
        assert_eq!(to_full_pinyin("真·三国无双"), "zhensanguowushuang");
        assert_eq!(to_initials("真·三国无双"), "zsgws");
    }

    #[test]
    fn mixed_text_works() {
        let text = "真·三国无双";
        assert!(matches_pinyin(text, "zsgw"));
        assert!(matches_pinyin(text, "zhensanguowushuang"));
        assert!(matches_pinyin(text, "wushuang"));
    }

    #[test]
    fn empty_query_matches_everything() {
        assert!(matches_pinyin("anything", ""));
    }

    #[test]
    fn no_match_returns_false() {
        assert!(!matches_pinyin("王者荣耀", "lol"));
    }
}
