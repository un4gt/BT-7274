//! 基于 Unicode 显示宽度的文本工具：换行与截断。
//!
//! TUI 中 CJK 字符占两列，直接按 `char` 计数会导致错位，
//! 这里统一用 `unicode_width` 处理。

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// 把 CRLF 和单独 CR 统一为 LF，同时保留末尾换行信息。
pub fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// 按显示宽度把多行文本拆成单行 `Vec<String>`（贪心填充，不拆单词边界）。
pub fn wrap_lines(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let normalized = normalize_newlines(text);
    for source_line in normalized.split('\n') {
        if width == 0 {
            lines.push(source_line.to_owned());
            continue;
        }
        let mut current = String::new();
        let mut current_width = 0usize;
        for ch in source_line.chars() {
            let ch_width = ch.width().unwrap_or(0);
            if current_width + ch_width > width && !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current_width = 0;
            }
            current.push(ch);
            current_width += ch_width;
        }
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// 按显示宽度截断字符串，超出部分以 `…` 结尾。
pub fn truncate_width(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_owned();
    }
    if max_width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut width = 0usize;
    // 预留省略号的一列
    for ch in text.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if width + ch_width > max_width.saturating_sub(1) {
            break;
        }
        out.push(ch);
        width += ch_width;
    }
    out.push('…');
    out
}

/// 输入框水平滚动：返回 `chars` 在给定可用宽度内、以光标为锚点的可见片段
/// 及光标在片段中的显示列。
///
/// 规则是光标尽量留在可见区内，文本超宽时优先展示尾部。
pub fn visible_slice_with_cursor(chars: &[char], cursor: usize, width: usize) -> (String, usize) {
    let cursor = cursor.min(chars.len());
    let total_width: usize = chars.iter().map(|c| c.width().unwrap_or(0)).sum();
    if total_width <= width {
        let text: String = chars.iter().collect();
        let col: usize = chars[..cursor].iter().map(|c| c.width().unwrap_or(0)).sum();
        return (text, col);
    }
    // 光标左侧宽度超过可用宽度时，把窗口起点推向光标
    let left_width: usize = chars[..cursor].iter().map(|c| c.width().unwrap_or(0)).sum();
    let start = if left_width <= width {
        0
    } else {
        // 找到使窗口内恰好容纳 left_width 的起始字符下标
        let mut skipped = 0usize;
        let mut start = 0usize;
        for (i, c) in chars.iter().enumerate() {
            if skipped >= left_width.saturating_sub(width) {
                start = i;
                break;
            }
            skipped += c.width().unwrap_or(0);
            start = i + 1;
        }
        start
    };
    // 从 start 开始尽量取满 width 列，但必须包含光标所在字符
    let mut out = String::new();
    let mut width_used = 0usize;
    let mut cursor_col = None;
    for (i, c) in chars.iter().enumerate().skip(start) {
        let c_width = c.width().unwrap_or(0);
        if i == cursor {
            cursor_col = Some(width_used);
            if width_used + c_width > width {
                // 光标字符放不下：窗口右边界至少到光标
                break;
            }
        }
        if width_used + c_width > width {
            break;
        }
        out.push(*c);
        width_used += c_width;
    }
    let col = cursor_col.unwrap_or(width_used);
    (out, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_lines_respects_display_width() {
        let lines = wrap_lines("你好世界abcd", 6);
        // 每行不超过 6 列：中文占 2 列
        assert!(lines.iter().all(|l| l.width() <= 6));
        assert_eq!(lines.join(""), "你好世界abcd");
    }

    #[test]
    fn wrap_lines_keeps_empty_and_newlines() {
        assert_eq!(wrap_lines("", 10), vec![""]);
        assert_eq!(wrap_lines("a\nb", 10), vec!["a", "b"]);
        assert_eq!(wrap_lines("a\r\nb", 10), vec!["a", "b"]);
        assert_eq!(wrap_lines("a\rb", 10), vec!["a", "b"]);
    }

    #[test]
    fn truncate_marks_ellipsis() {
        assert_eq!(truncate_width("hello", 10), "hello");
        assert_eq!(truncate_width("hello", 4), "hel…");
        // 中文 2 列：恰好放满时不截断，差 1 列时放 1 个字加省略号
        assert_eq!(truncate_width("你好", 4), "你好");
        assert_eq!(truncate_width("你好", 3), "你…");
    }

    #[test]
    fn cursor_visible_for_long_input() {
        let text: Vec<char> = "0123456789ABCDEF".chars().collect();
        let (visible, col) = visible_slice_with_cursor(&text, 16, 8);
        assert_eq!(visible, "89ABCDEF");
        assert_eq!(col, 8);
        // 光标在中段：从 0 开始能看到光标
        let (visible, col) = visible_slice_with_cursor(&text, 3, 8);
        assert_eq!(visible, "01234567");
        assert_eq!(col, 3);

        let (visible, col) = visible_slice_with_cursor(&text, usize::MAX, 8);
        assert_eq!(visible, "89ABCDEF");
        assert_eq!(col, 8);
    }
}
