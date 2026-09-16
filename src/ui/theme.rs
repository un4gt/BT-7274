//! 主题到语义颜色的唯一映射；组件只使用语义色，不直接决定具体色值。

use ratatui::style::{Color, Modifier, Style};

use crate::config::Theme;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Palette {
    pub background: Color,
    pub surface: Color,
    pub panel: Color,
    pub text: Color,
    pub muted: Color,
    pub border: Color,
    pub primary: Color,
    pub accent: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
    pub selection: Color,
    pub user: Color,
    pub assistant: Color,
    pub art_body: Color,
}

impl Palette {
    pub fn base(self) -> Style {
        Style::default().fg(self.text).bg(self.background)
    }

    pub fn surface(self) -> Style {
        Style::default().fg(self.text).bg(self.surface)
    }

    pub fn panel(self) -> Style {
        Style::default().fg(self.text).bg(self.panel)
    }

    pub fn border(self, focused: bool) -> Style {
        Style::default()
            .fg(if focused { self.primary } else { self.border })
            .bg(self.surface)
    }

    pub fn selected(self) -> Style {
        Style::default()
            .fg(self.text)
            .bg(self.selection)
            .add_modifier(Modifier::BOLD)
    }
}

pub(crate) fn palette(theme: Theme) -> Palette {
    match theme {
        Theme::Vanguard => Palette {
            background: Color::Rgb(7, 14, 16),
            surface: Color::Rgb(12, 24, 27),
            panel: Color::Rgb(17, 33, 36),
            text: Color::Rgb(224, 239, 237),
            muted: Color::Rgb(128, 151, 149),
            border: Color::Rgb(48, 75, 76),
            primary: Color::Rgb(45, 212, 191),
            accent: Color::Rgb(251, 146, 60),
            success: Color::Rgb(163, 230, 53),
            warning: Color::Rgb(250, 204, 21),
            danger: Color::Rgb(248, 113, 113),
            selection: Color::Rgb(22, 50, 52),
            user: Color::Rgb(163, 230, 53),
            assistant: Color::Rgb(94, 234, 212),
            art_body: Color::Rgb(103, 137, 133),
        },
        Theme::Carbon => Palette {
            background: Color::Rgb(15, 17, 20),
            surface: Color::Rgb(23, 26, 31),
            panel: Color::Rgb(30, 34, 40),
            text: Color::Rgb(232, 234, 237),
            muted: Color::Rgb(144, 149, 158),
            border: Color::Rgb(65, 71, 81),
            primary: Color::Rgb(96, 165, 250),
            accent: Color::Rgb(251, 146, 60),
            success: Color::Rgb(74, 222, 128),
            warning: Color::Rgb(250, 204, 21),
            danger: Color::Rgb(248, 113, 113),
            selection: Color::Rgb(38, 53, 72),
            user: Color::Rgb(74, 222, 128),
            assistant: Color::Rgb(147, 197, 253),
            art_body: Color::Rgb(123, 132, 148),
        },
        Theme::Paper => Palette {
            background: Color::Rgb(243, 247, 246),
            surface: Color::Rgb(255, 255, 255),
            panel: Color::Rgb(232, 240, 238),
            text: Color::Rgb(25, 38, 37),
            muted: Color::Rgb(83, 103, 101),
            border: Color::Rgb(148, 170, 166),
            primary: Color::Rgb(13, 116, 108),
            accent: Color::Rgb(185, 55, 5),
            success: Color::Rgb(15, 115, 50),
            warning: Color::Rgb(145, 80, 0),
            danger: Color::Rgb(185, 28, 28),
            selection: Color::Rgb(235, 248, 246),
            user: Color::Rgb(21, 128, 61),
            assistant: Color::Rgb(13, 116, 108),
            art_body: Color::Rgb(102, 125, 119),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(color: Color) -> f64 {
        let Color::Rgb(red, green, blue) = color else {
            panic!("theme colors must use explicit RGB values");
        };
        let channel = |value: u8| {
            let value = f64::from(value) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(red) + 0.7152 * channel(green) + 0.0722 * channel(blue)
    }

    fn contrast(left: Color, right: Color) -> f64 {
        let left = luminance(left);
        let right = luminance(right);
        (left.max(right) + 0.05) / (left.min(right) + 0.05)
    }

    #[test]
    fn themes_have_distinct_backgrounds_and_focus_colors() {
        let vanguard = palette(Theme::Vanguard);
        let carbon = palette(Theme::Carbon);
        let paper = palette(Theme::Paper);

        assert_ne!(vanguard.background, carbon.background);
        assert_ne!(carbon.background, paper.background);
        assert_ne!(vanguard.primary, carbon.primary);
    }

    #[test]
    fn semantic_text_colors_keep_readable_contrast() {
        for theme in Theme::ALL {
            let palette = palette(theme);
            for background in [
                palette.background,
                palette.surface,
                palette.panel,
                palette.selection,
            ] {
                for foreground in [
                    palette.text,
                    palette.primary,
                    palette.accent,
                    palette.success,
                    palette.warning,
                    palette.danger,
                ] {
                    assert!(
                        contrast(foreground, background) >= 4.5,
                        "{theme:?} contrast too low for {foreground:?} on {background:?}"
                    );
                }
                assert!(
                    contrast(palette.muted, background) >= 3.0,
                    "{theme:?} muted contrast too low on {background:?}"
                );
            }
        }
    }
}
