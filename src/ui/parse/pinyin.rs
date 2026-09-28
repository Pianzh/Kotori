//! 拼音搜索工具：把中文转成全拼和首字母，供搜索匹配用。
//!
//! 用 `pinyin` crate 做转换，它按最常见读音处理多音字，游戏名场景够用。
//! 非汉字字符（英文、数字、符号）原样保留在结果里。

use pinyin::{Pinyin, to_pinyin_vec};

/// 把文本转成全拼（小写、无空格）。
/// 例：「王者荣耀」→「wangzherongyao」
pub(in crate::ui) fn to_full_pinyin(text: &str) -> String {
    to_pinyin_vec(text, Pinyin::plain)
        .concat()
        .to_lowercase()
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

    #[test]
    fn non_chinese_passes_through() {
        assert_eq!(to_full_pinyin("Game"), "game");
        assert_eq!(to_initials("Game"), "g");
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
