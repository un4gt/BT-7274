use super::*;
use crate::{config::Theme, editor::ChatEditor, ui::theme};
use ratatui::{
    style::{Modifier, Style},
    widgets::{Paragraph, Widget},
};

fn render(sparkle: &Sparkle, now: Instant, focused: bool) -> Buffer {
    render_editor(sparkle, &ChatEditor::default(), now, focused)
}

pub(crate) fn render_editor(
    sparkle: &Sparkle,
    editor: &ChatEditor,
    now: Instant,
    focused: bool,
) -> Buffer {
    let area = Rect::new(2, 3, 80, 5);
    let palette = theme::palette(Theme::Vanguard);
    let viewport = editor.viewport(80, 5);
    let mut buffer = Buffer::empty(area);
    buffer.set_style(area, palette.panel());
    sparkle.render(area, &viewport, focused, &mut buffer, palette, now);
    buffer
}

fn has_stars(buffer: &Buffer) -> bool {
    buffer
        .content
        .iter()
        .any(|cell| DOTS.contains(&cell.symbol()))
}

#[test]
fn enabled_stars_keep_repainting_with_drafts_without_external_events() {
    let started = Instant::now();
    for draft in ["", "typing 你好 ", "  ", "\n", "你\n好"] {
        let sparkle = Sparkle::new(true);
        let mut editor = ChatEditor::default();
        editor.set_text(draft);
        let mut now = started;
        let mut previous = None;
        let mut changed_after_fifteen_seconds = false;
        while now < started + Duration::from_secs(60) {
            let buffer = render_editor(&sparkle, &editor, now, true);
            if now >= started + FADE_IN {
                assert!(has_stars(&buffer), "stars disappeared with draft {draft:?}");
            }
            if now > started + Duration::from_secs(15) {
                changed_after_fifteen_seconds |= previous.as_ref() != Some(&buffer);
            }
            previous = Some(buffer);
            // 只使用组件安排的截止时间推进时钟，不以键盘或其它事件唤醒它。
            let next = sparkle
                .next_frame()
                .expect("stars must schedule another frame");
            assert!(next > now);
            assert!(next <= now + FRAME_TICK);
            now = next;
        }
        assert!(changed_after_fifteen_seconds);
    }
}

#[test]
fn disabling_during_fade_in_never_brightens_or_extends_the_fade() {
    let now = Instant::now();
    let mut sparkle = Sparkle::new(true);
    sparkle.frame(now);
    let last = sparkle.frame(now + Duration::from_millis(500)).unwrap();
    let disabled = now + Duration::from_millis(510);
    sparkle.set_enabled(false, disabled);
    let first = sparkle.frame(disabled).unwrap();
    assert_eq!(first.elapsed, last.elapsed);
    assert_eq!(first.visibility, last.visibility);
    sparkle.set_enabled(false, disabled + FADE_FRAME_TICK);
    assert!(
        sparkle
            .frame(disabled + FADE_FRAME_TICK)
            .unwrap()
            .visibility
            < first.visibility
    );
    assert!(sparkle.frame(disabled + FADE_OUT).is_none());
}

#[test]
fn disabling_fades_and_stops_frames_until_reenabled() {
    let now = Instant::now();
    let mut sparkle = Sparkle::new(false);
    assert!(!has_stars(&render(&sparkle, now, true)));
    assert!(sparkle.next_frame().is_none());
    sparkle.set_enabled(true, now);
    render(&sparkle, now, true);
    let visible = render(&sparkle, now + FADE_IN, true);
    assert!(has_stars(&visible));
    sparkle.set_enabled(false, now + FADE_IN);
    assert_eq!(render(&sparkle, now + FADE_IN, true), visible);
    assert!(!has_stars(&render(
        &sparkle,
        now + FADE_IN + FADE_OUT,
        true
    )));
    assert!(sparkle.next_frame().is_none());
    assert!(!has_stars(&render(
        &sparkle,
        now + Duration::from_secs(60),
        true
    )));
    assert!(sparkle.next_frame().is_none());
    let restart = now + Duration::from_secs(61);
    sparkle.set_enabled(true, restart);
    render(&sparkle, restart, true);
    let visible = render(&sparkle, restart + FADE_IN, true);
    assert!(has_stars(&visible));
    sparkle.set_enabled(true, restart + FADE_IN);
    assert_eq!(render(&sparkle, restart + FADE_IN, true), visible);
}

#[test]
fn terminal_focus_and_popups_pause_frames_without_restarting_the_animation() {
    let now = Instant::now();
    let mut sparkle = Sparkle::new(true);
    let uninterrupted = Sparkle::new(true);
    render(&sparkle, now, true);
    render(&uninterrupted, now, true);
    assert!(has_stars(&render(&sparkle, now + FADE_IN, true)));
    sparkle.set_terminal_focus(false);
    assert!(!has_stars(&render(
        &sparkle,
        now + Duration::from_secs(2),
        true
    )));
    assert!(sparkle.next_frame().is_none());
    sparkle.set_terminal_focus(true);
    let focused = now + Duration::from_secs(30);
    let expected = render(&uninterrupted, focused, true);
    assert!(has_stars(&expected));
    assert_eq!(render(&sparkle, focused, true), expected);
    assert_eq!(sparkle.next_frame(), Some(focused + FRAME_TICK));
    assert!(!has_stars(&render(&sparkle, focused + FRAME_TICK, false)));
    assert!(sparkle.next_frame().is_none());
    let closed = focused + Duration::from_secs(1);
    assert_eq!(
        render(&sparkle, closed, true),
        render(&uninterrupted, closed, true)
    );
    assert_eq!(sparkle.next_frame(), Some(closed + FRAME_TICK));
}

#[test]
fn stars_preserve_unicode_spaces_selection_cursor_and_borders_in_every_theme() {
    let now = Instant::now();
    for theme in Theme::ALL {
        let palette = theme::palette(theme);
        for selected in [false, true] {
            let outer = Rect::new(0, 0, 82, 7);
            let area = Rect::new(1, 1, 80, 5);
            let mut editor = ChatEditor::default();
            editor.set_text("  你好 e\u{301} 👩‍💻    \n   text with spaces      ");
            if selected {
                editor.select_all();
            }
            let viewport = editor.viewport(80, 5);
            let mut buffer = Buffer::empty(outer);
            buffer.set_style(area, palette.panel());
            for (index, row) in viewport.rows.iter().enumerate() {
                let text = row
                    .cells
                    .iter()
                    .map(|cell| cell.text.as_str())
                    .collect::<String>();
                Paragraph::new(text)
                    .style(if selected {
                        palette.selected()
                    } else {
                        palette.panel()
                    })
                    .render(Rect::new(1, 1 + index as u16, 80, 1), &mut buffer);
            }
            buffer[(65, 4)].set_style(Style::default().add_modifier(Modifier::REVERSED));
            let before = buffer.clone();
            let sparkle = Sparkle::new(true);
            sparkle.frame(now);
            sparkle.frame(now + FADE_IN);
            sparkle.render(area, &viewport, true, &mut buffer, palette, now + FADE_IN);
            assert!(has_stars(&buffer));
            for y in outer.y..outer.bottom() {
                let width = viewport
                    .rows
                    .get(y.saturating_sub(area.y) as usize)
                    .map_or(0, |row| {
                        row.cells
                            .iter()
                            .map(|cell| cell.text.width())
                            .sum::<usize>()
                    });
                for x in outer.x..outer.right() {
                    let protected = !area.contains((x, y).into())
                        || usize::from(x.saturating_sub(area.x)) < width
                        || (x, y)
                            == (
                                area.x + viewport.cursor_column as u16,
                                area.y + viewport.cursor_row as u16,
                            )
                        || (x, y) == (65, 4);
                    if protected {
                        assert_eq!(buffer[(x, y)], before[(x, y)]);
                    }
                }
            }
        }
    }
}

#[test]
fn fading_only_dims_existing_stars_and_clears_the_last_frame() {
    let now = Instant::now();
    for theme in Theme::ALL {
        let palette = theme::palette(theme);
        let area = Rect::new(0, 0, 80, 5);
        let viewport = ChatEditor::default().viewport(80, 5);
        let mut sparkle = Sparkle::new(true);
        sparkle.frame(now);
        let mut before = Buffer::empty(area);
        before.set_style(area, palette.panel());
        sparkle.render(area, &viewport, true, &mut before, palette, now + FADE_IN);
        assert!(has_stars(&before));
        sparkle.set_enabled(false, now + FADE_IN);
        let mut faded = Buffer::empty(area);
        faded.set_style(area, palette.panel());
        sparkle.render(
            area,
            &viewport,
            true,
            &mut faded,
            palette,
            now + FADE_IN + FADE_OUT / 2,
        );
        for (old, new) in before.content.iter().zip(&faded.content) {
            if new.symbol() != " " {
                assert_eq!(new.symbol(), old.symbol());
                let (Color::Rgb(r, g, b), Color::Rgb(nr, ng, nb), Color::Rgb(br, bg, bb)) =
                    (old.fg, new.fg, new.bg)
                else {
                    panic!("RGB theme required")
                };
                assert!(r.abs_diff(br) >= nr.abs_diff(br));
                assert!(g.abs_diff(bg) >= ng.abs_diff(bg));
                assert!(b.abs_diff(bb) >= nb.abs_diff(bb));
            }
        }
        let mut cleared = Buffer::empty(area);
        cleared.set_style(area, palette.panel());
        sparkle.render(
            area,
            &viewport,
            true,
            &mut cleared,
            palette,
            now + FADE_IN + FADE_OUT,
        );
        assert!(!has_stars(&cleared));
        assert!(sparkle.next_frame().is_none());
    }
}

#[test]
fn empty_viewports_never_schedule_frames() {
    let now = Instant::now();
    let palette = theme::palette(Theme::Vanguard);
    for (width, height) in [(0, 0), (0, 2), (1, 0), (1, 1)] {
        let area = Rect::new(4, 5, width, height);
        let sparkle = Sparkle::new(true);
        let viewport = ChatEditor::default().viewport(width as usize, height as usize);
        let mut buffer = Buffer::empty(area);
        sparkle.render(area, &viewport, true, &mut buffer, palette, now);
        assert!(!has_stars(&buffer));
        if area.is_empty() {
            assert!(sparkle.next_frame().is_none());
        }
    }
}
