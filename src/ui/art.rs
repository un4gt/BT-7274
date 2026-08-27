//! BT-7274 泰坦像素画背景（铺满聊天区）。
//!
//! 像素位图：`#` 机体、`:` 面板线（驾驶舱门/下颚/膝甲）、`o` 灯效
//! （光学镜头与核心）、`.` 留空。渲染时按最近邻采样把位图拉伸到
//! 目标区域的完整尺寸：终端列 ← 位图列，半块像素行（每终端行两行）
//! ← 位图行，因此任意终端大小都能铺满。机体亮灰、面板线暗一档、
//! 灯效青色发光。

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};

use super::theme::Palette;

/// 先锋级泰坦正面像：天线、独眼镜头、宽肩、双臂、驾驶舱与核心、粗腿、宽脚掌。
const TITAN_PIXELS: &[&str] = &[
    "...........................##...........................",
    "...........................##...........................",
    "......................############......................",
    ".....................##oooooooooo##.....................",
    ".....................####::::::####.....................",
    "......................::::::::::::......................",
    ".........................######.........................",
    ".........................######.........................",
    "............################################............",
    "........########################################........",
    ".....##############################################.....",
    "...##################################################...",
    "..####################################################..",
    "..#####o######....####################....######o#####..",
    "....##########....####################....##########....",
    "....###::::###....#####::::::::::#####....###::::###....",
    "....###::::###....#####::oooooo::#####....###::::###....",
    "....##########....#####:::oooo:::#####....##########....",
    "....##########....#####::::::::::#####....##########....",
    "...##########.......################.......##########...",
    "....########..........############..........########....",
    ".........................######.........................",
    "........................########........................",
    ".................######################.................",
    ".................##########..##########.................",
    ".................#########....#########.................",
    ".................#########....#########.................",
    ".................###:::###....###:::###.................",
    ".................####:####....####:####.................",
    "..................#######......#######..................",
    "..................#######......#######..................",
    "..................#######......#######..................",
    "...................##o##........##o##...................",
    "..............#############..#############..............",
];

const ART_WIDTH: usize = 56;

fn body_style(palette: Palette) -> Style {
    palette.surface().fg(palette.art_body)
}

fn panel_style(palette: Palette) -> Style {
    palette.surface().fg(palette.art_panel)
}

fn glow_style(palette: Palette) -> Style {
    palette
        .surface()
        .fg(palette.primary)
        .add_modifier(Modifier::BOLD)
}

fn block_char(up: bool, down: bool) -> char {
    match (up, down) {
        (true, true) => '█',
        (true, false) => '▀',
        (false, true) => '▄',
        (false, false) => ' ',
    }
}

/// 采样某个像素行列的字符（越界按留空处理）。
fn pixel(row: usize, col: usize) -> u8 {
    TITAN_PIXELS[row]
        .as_bytes()
        .get(col)
        .copied()
        .unwrap_or(b'.')
}

/// 把泰坦像素画拉伸铺满 `area`，直接写入缓冲区。
///
/// 空白像素会把单元格重置为默认（透明效果），消息文本随后覆盖其上。
/// 高度足够时底部保留两行空白行放题注，整机不与题注重叠。
pub fn render_fill(area: Rect, buf: &mut Buffer, palette: Palette) {
    let width = area.width as usize;
    let height = area.height as usize;
    if width < 4 || height < 4 {
        return;
    }
    let art_height = TITAN_PIXELS.len();
    let caption_rows = if height >= 8 { 2 } else { 0 };
    let art_rows = height - caption_rows;

    for y in 0..height {
        if y >= art_rows {
            // 题注区：清空
            for x in 0..width {
                buf[(area.x + x as u16, area.y + y as u16)]
                    .set_char(' ')
                    .set_style(palette.surface());
            }
            continue;
        }
        // 每个终端行对应两个位图行（半块字符）；
        // 用保端点映射，降采样时首尾（天线与脚掌）不会被丢掉
        let pixel_rows = 2 * art_rows;
        let up_row = y * 2 * (art_height - 1) / (pixel_rows - 1);
        let down_row = ((y * 2 + 1) * (art_height - 1) / (pixel_rows - 1)).min(art_height - 1);
        for x in 0..width {
            let col = x * (ART_WIDTH - 1) / (width - 1);
            let up = pixel(up_row, col);
            let down = pixel(down_row, col);
            let glyph = block_char(up != b'.', down != b'.');
            let cell = &mut buf[(area.x + x as u16, area.y + y as u16)];
            if glyph == ' ' {
                cell.set_char(' ').set_style(palette.surface());
                continue;
            }
            // 半块字符只能单色：灯效 > 面板线 > 机体
            let style = if up == b'o' || down == b'o' {
                glow_style(palette)
            } else if up == b':' || down == b':' {
                panel_style(palette)
            } else {
                body_style(palette)
            };
            cell.set_char(glyph).set_style(style);
        }
    }

    // 底部两行题注（独立区域，不遮挡机体）
    let captions = [
        "BT-7274 · VANGUARD-CLASS TITAN",
        "PROTOCOL 3: PROTECT THE PILOT",
    ];
    for (offset, caption) in captions.iter().rev().enumerate() {
        let y = area.bottom() - 1 - offset as u16;
        let text_width = caption.chars().count();
        if text_width > width {
            continue;
        }
        let x0 = area.x + (area.width as usize - text_width).div_ceil(2) as u16;
        for (i, ch) in caption.chars().enumerate() {
            buf[(x0 + i as u16, y)]
                .set_char(ch)
                .set_style(body_style(palette));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 在给定尺寸的缓冲区里渲染并返回逐行文本（用于断言与预览）。
    fn render_to_text(width: u16, height: u16) -> Buffer {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        render_fill(
            area,
            &mut buf,
            super::super::theme::palette(crate::config::Theme::Vanguard),
        );
        buf
    }

    #[test]
    fn bitmap_is_symmetric_and_fixed_width() {
        for (index, row) in TITAN_PIXELS.iter().enumerate() {
            assert_eq!(row.len(), ART_WIDTH, "row {index} width");
            let rev: String = row.chars().rev().collect();
            assert_eq!(rev, *row, "row {index} asymmetric");
        }
    }

    #[test]
    fn fill_covers_area_with_blocks_and_glow() {
        // 原始比例（1:1 采样）
        let buf = render_to_text(56, 17);
        let blocks = buf
            .content
            .iter()
            .filter(|c| c.symbol() == "█" || c.symbol() == "▀" || c.symbol() == "▄")
            .count();
        assert!(blocks > 200, "expect a filled image, got {blocks} blocks");
        // 光学镜头的青色灯效应存在
        assert!(
            buf.content.iter().any(|c| {
                c.symbol() != " "
                    && c.fg == super::super::theme::palette(crate::config::Theme::Vanguard).primary
            }),
            "eye/core glow missing"
        );

        // 拉伸到 2 倍：块数应明显增多（铺满更大区域）
        let stretched = render_to_text(112, 34);
        let blocks2 = stretched
            .content
            .iter()
            .filter(|c| c.symbol() == "█" || c.symbol() == "▀" || c.symbol() == "▄")
            .count();
        assert!(blocks2 > blocks * 3);
    }

    #[test]
    fn captions_rendered_at_bottom() {
        let buf = render_to_text(60, 20);
        let bottom: String = (0..60).map(|x| buf[(x, 19)].symbol().to_string()).collect();
        assert!(bottom.contains("PROTOCOL 3"));
        let above: String = (0..60).map(|x| buf[(x, 18)].symbol().to_string()).collect();
        assert!(above.contains("VANGUARD"));
    }

    /// `cargo test titan_visual -- --nocapture` 预览原始比例渲染。
    #[test]
    fn titan_visual() {
        let buf = render_to_text(56, 17);
        for y in 0..17 {
            let row: String = (0..56).map(|x| buf[(x, y)].symbol().to_string()).collect();
            println!("{row}");
        }
    }
}
