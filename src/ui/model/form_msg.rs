//! 表单下面那句话：**内容 + 它是不是好消息**（B13）。
//!
//! 从前的判据是猜出来的 —— `render` 里写着
//! `let ok = !message.contains("失败") && !message.contains("不一样") && !message.contains("请先")`，
//! 于是任何一句新文案（"没成功"、"不一样"、英文错误、以后新增的措辞）都可能悄悄变成
//! 绿色，而**改文案的人根本不知道自己在动一个判据**。
//!
//! 现在反过来：话是**写的人**给的，好坏的判断也由他显式说出来，`render` 只读标记。
//! 一句话属于哪一类，在写入的那一行就能看懂；想知道"哪些情况算失败"，grep
//! `FormMsg::error` 就是完整答案。

/// 给用户看的一句话，外加它是好消息还是坏消息。
///
/// ⚠ 它是 `pub(crate)` 而不是 `pub(in crate::ui)`：`Message` 里有几处
/// `Result<FormMsg, String>` 的载荷，而 `Message` 自己是 `pub(crate)` ——
/// 载荷类型比枚举更私有时编译器会拒绝（同 `model::MatchReply` 那条注释）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FormMsg {
    text: String,
    ok: bool,
}

impl FormMsg {
    /// 事情办成了（或至少没有坏消息）。
    pub(crate) fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ok: true,
        }
    }

    /// 事情没办成，或者用户还得再做点什么。
    pub(crate) fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ok: false,
        }
    }

    /// 要显示的那句话。
    pub(in crate::ui) fn text(&self) -> &str {
        &self.text
    }

    /// 是不是好消息（决定界面用哪种颜色）。
    pub(in crate::ui) fn is_ok(&self) -> bool {
        self.ok
    }
}

impl Default for FormMsg {
    /// 「没有消息」= 空文本 + 好消息 —— 与 `render` 里 `None` 那一支的行为一致，
    /// 这样"还没说话"和"说了一句空话"在界面上长得一样。
    fn default() -> Self {
        Self {
            text: String::new(),
            ok: true,
        }
    }
}

/// 只解引用到**文字**那一半：`msg.contains("…")`、`as_deref()`、
/// `unwrap_or_default()` 这些老写法因此照旧能用，判定用的 `ok` 仍然只能显式取
/// （见 [`FormMsg::is_ok`]）—— 想让它变绿，得有人写 `FormMsg::ok`。
impl std::ops::Deref for FormMsg {
    type Target = str;

    fn deref(&self) -> &str {
        &self.text
    }
}

impl std::fmt::Display for FormMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl From<FormMsg> for String {
    fn from(msg: FormMsg) -> Self {
        msg.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 这条测试钉的是**这个类型存在的理由**：好坏是写的人说的，不是从文字里猜的。
    #[test]
    fn the_flag_is_not_guessed_from_the_text() {
        // 一句话里带"失败"两个字，但它被显式标成好消息 ⇒ 就是好消息。
        let reassuring = FormMsg::ok("上次失败的原因已经修好了");
        assert!(reassuring.is_ok(), "好坏由写入点决定，不从文字里猜");
        assert_eq!(reassuring.text(), "上次失败的原因已经修好了");

        // 反过来也一样：一句看起来中性的错误。
        let broken = FormMsg::error("主密码不对（或者凭据文件损坏了）");
        assert!(!broken.is_ok());
        assert_eq!(broken.text(), "主密码不对（或者凭据文件损坏了）");
    }
}
