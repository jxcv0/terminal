use crate::model::{Position, Terminal};
use egui::{Color32, FontId, Painter, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};

pub const BACKGROUND: Color32 = Color32::from_rgb(20, 23, 28);
const FOREGROUND: Color32 = Color32::from_rgb(221, 225, 231);
const SELECTION: Color32 = Color32::from_rgb(54, 76, 106);
const FONT_SIZE: f32 = 16.0;

pub fn install_fonts(context: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, bytes) in [
        (
            "DejaVu Sans Mono",
            include_bytes!("../assets/fonts/DejaVuSansMono.ttf").as_slice(),
        ),
        (
            "Droid Sans Fallback",
            include_bytes!("../assets/fonts/DroidSansFallbackFull.ttf").as_slice(),
        ),
    ] {
        fonts.font_data.insert(
            name.into(),
            std::sync::Arc::new(egui::FontData::from_static(bytes)),
        );
    }
    let monospace = fonts
        .families
        .get_mut(&egui::FontFamily::Monospace)
        .unwrap();
    monospace.splice(
        0..0,
        ["DejaVu Sans Mono".into(), "Droid Sans Fallback".into()],
    );
    context.set_fonts(fonts);
}

#[derive(Clone, Copy, Debug)]
pub struct GridMetrics {
    pub origin: Pos2,
    pub cell: Vec2,
    pub baseline: f32,
    pub rows: u16,
    pub cols: u16,
}

impl GridMetrics {
    fn new(rect: Rect, width: f32, height: f32, ascent: f32, scale: f32) -> Self {
        let snap = |value: f32| (value * scale).round() / scale;
        let origin = pos2(snap(rect.min.x), snap(rect.min.y));
        let cell = vec2(
            (width * scale).ceil().max(1.0) / scale,
            ((height + 4.0) * scale).ceil().max(1.0) / scale,
        );
        Self {
            origin,
            cell,
            baseline: snap(ascent + 2.0),
            rows: ((rect.max.y - origin.y) / cell.y)
                .floor()
                .clamp(f32::from(crate::model::MIN_ROWS), 1000.0) as u16,
            cols: ((rect.max.x - origin.x) / cell.x)
                .floor()
                .clamp(f32::from(crate::model::MIN_COLS), 1000.0) as u16,
        }
    }

    pub fn cell_rect(&self, row: u16, col: u16, width: u16) -> Rect {
        Rect::from_min_size(
            self.origin + vec2(f32::from(col) * self.cell.x, f32::from(row) * self.cell.y),
            vec2(f32::from(width) * self.cell.x, self.cell.y),
        )
    }

    fn position(&self, pos: Pos2) -> Position {
        Position {
            row: ((pos.y - self.origin.y) / self.cell.y)
                .floor()
                .clamp(0.0, f32::from(self.rows - 1)) as u16,
            col: ((pos.x - self.origin.x) / self.cell.x)
                .floor()
                .clamp(0.0, f32::from(self.cols - 1)) as u16,
        }
    }
}

pub struct TerminalView {
    pub focused: bool,
    first_frame: bool,
    scroll_remainder: f32,
    pub metrics: Option<GridMetrics>,
}

impl Default for TerminalView {
    fn default() -> Self {
        Self {
            focused: true,
            first_frame: true,
            scroll_remainder: 0.0,
            metrics: None,
        }
    }
}

impl TerminalView {
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        terminal: &mut Terminal,
        preedit: &str,
        cursor_on: bool,
        demo: bool,
    ) -> Option<(u16, u16)> {
        let mut resized = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BACKGROUND).inner_margin(8.0))
            .show(ctx, |ui| {
                let (rect, response) =
                    ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
                if self.first_frame || response.clicked() || response.drag_started() {
                    response.request_focus();
                    self.first_frame = false;
                }
                self.focused = response.has_focus();
                let font = FontId::monospace(FONT_SIZE);
                // All cell positions use the embedded primary font's metrics.
                let (width, height, ascent) = ui.fonts_mut(|fonts| {
                    let sample = fonts.layout_no_wrap("M".into(), font.clone(), FOREGROUND);
                    let ascent = sample.rows[0].glyphs[0].pos.y;
                    (
                        fonts.glyph_width(&font, 'M'),
                        fonts.row_height(&font),
                        ascent,
                    )
                });
                let metrics = GridMetrics::new(rect, width, height, ascent, ctx.pixels_per_point());
                if terminal.resize(metrics.rows, metrics.cols) {
                    resized = Some((metrics.rows, metrics.cols));
                    if demo {
                        terminal.demo();
                    }
                }
                self.metrics = Some(metrics);
                if response.hovered() {
                    self.scroll_remainder +=
                        ui.input(|input| input.raw_scroll_delta.y) / metrics.cell.y;
                    let rows = self.scroll_remainder.trunc() as isize;
                    if rows != 0 {
                        terminal.scroll(rows);
                        self.scroll_remainder -= rows as f32;
                    }
                }
                if (response.drag_started_by(egui::PointerButton::Primary) || response.clicked())
                    && let Some(pos) = ui
                        .input(|input| input.pointer.press_origin())
                        .or(response.interact_pointer_pos())
                {
                    let anchor = metrics.position(pos);
                    terminal.selection = Some((anchor, anchor));
                }
                if (response.dragged_by(egui::PointerButton::Primary)
                    || response.drag_stopped_by(egui::PointerButton::Primary))
                    && let (Some((anchor, _)), Some(pos)) =
                        (terminal.selection, response.interact_pointer_pos())
                {
                    terminal.selection = Some((anchor, metrics.position(pos)));
                }
                let painter = ui.painter().with_clip_rect(rect);
                paint(
                    &painter,
                    terminal,
                    metrics,
                    &font,
                    cursor_on && self.focused,
                    preedit,
                );
                if self.focused {
                    let (row, col) = terminal.screen().cursor_position();
                    ctx.output_mut(|output| {
                        output.ime = Some(egui::output::IMEOutput {
                            rect,
                            cursor_rect: metrics.cell_rect(
                                row.min(metrics.rows - 1),
                                col.min(metrics.cols - 1),
                                1,
                            ),
                        });
                    });
                }
            });
        resized
    }
}

fn palette(color: vt100::Color, default: Color32) -> Color32 {
    const ANSI: [[u8; 3]; 16] = [
        [0, 0, 0],
        [205, 49, 49],
        [13, 188, 121],
        [229, 229, 16],
        [36, 114, 200],
        [188, 63, 188],
        [17, 168, 205],
        [229, 229, 229],
        [102, 102, 102],
        [241, 76, 76],
        [35, 209, 139],
        [245, 245, 67],
        [59, 142, 234],
        [214, 112, 214],
        [41, 184, 219],
        [255, 255, 255],
    ];
    let [r, g, b] = match color {
        vt100::Color::Default => return default,
        vt100::Color::Rgb(r, g, b) => [r, g, b],
        vt100::Color::Idx(index @ 0..=15) => ANSI[usize::from(index)],
        vt100::Color::Idx(index @ 16..=231) => {
            let index = index - 16;
            let level = |n| if n == 0 { 0 } else { 55 + n * 40 };
            [level(index / 36), level(index / 6 % 6), level(index % 6)]
        }
        vt100::Color::Idx(index) => [8 + (index - 232) * 10; 3],
    };
    Color32::from_rgb(r, g, b)
}

fn colors(cell: &vt100::Cell) -> (Color32, Color32) {
    let mut fg = palette(cell.fgcolor(), FOREGROUND);
    let mut bg = palette(cell.bgcolor(), BACKGROUND);
    if cell.inverse() {
        std::mem::swap(&mut fg, &mut bg);
    }
    if cell.dim() {
        fg = Color32::from_rgb(fg.r() / 2, fg.g() / 2, fg.b() / 2);
    }
    (fg, bg)
}

fn glyph(
    painter: &Painter,
    cell: &vt100::Cell,
    rect: Rect,
    baseline: f32,
    font: &FontId,
    color: Color32,
) {
    if !cell.has_contents() || cell.is_wide_continuation() {
        return;
    }
    let format = egui::TextFormat {
        font_id: font.clone(),
        color,
        italics: cell.italic(),
        ..Default::default()
    };
    let job = egui::text::LayoutJob::single_section(cell.contents().to_owned(), format);
    let galley = painter.layout_job(job);
    let ascent = galley
        .rows
        .first()
        .and_then(|row| row.glyphs.first())
        .map_or(baseline, |glyph| glyph.pos.y);
    let origin = rect.min + vec2(0.0, baseline - ascent);
    painter.galley(origin, galley.clone(), color);
    if cell.bold() {
        painter.galley(
            origin + vec2(1.0 / painter.ctx().pixels_per_point(), 0.0),
            galley,
            color,
        );
    }
    if cell.underline() {
        let y = rect.min.y + baseline + 1.0;
        painter.line_segment(
            [pos2(rect.min.x, y), pos2(rect.max.x, y)],
            Stroke::new(1.0_f32, color),
        );
    }
}

fn paint(
    painter: &Painter,
    terminal: &Terminal,
    metrics: GridMetrics,
    font: &FontId,
    cursor_on: bool,
    preedit: &str,
) {
    let screen = terminal.screen();
    // Equal adjacent backgrounds become one rectangle; only visible cells are visited.
    for row in 0..metrics.rows {
        let mut start = 0;
        while start < metrics.cols {
            let background = colors(screen.cell(row, start).unwrap()).1;
            let mut end = start + 1;
            while end < metrics.cols && colors(screen.cell(row, end).unwrap()).1 == background {
                end += 1;
            }
            painter.rect_filled(metrics.cell_rect(row, start, end - start), 0.0, background);
            start = end;
        }
    }
    for row in 0..metrics.rows {
        for col in 0..metrics.cols {
            if terminal.selected(row, col) {
                painter.rect_filled(metrics.cell_rect(row, col, 1), 0.0, SELECTION);
            }
        }
    }
    for row in 0..metrics.rows {
        for col in 0..metrics.cols {
            let cell = screen.cell(row, col).unwrap();
            glyph(
                painter,
                cell,
                metrics.cell_rect(row, col, if cell.is_wide() { 2 } else { 1 }),
                metrics.baseline,
                font,
                colors(cell).0,
            );
        }
    }
    let (row, mut col) = screen.cursor_position();
    col = col.min(metrics.cols - 1); // Pending autowrap may put the cursor past the last column.
    if row < metrics.rows && screen.scrollback() == 0 && !screen.hide_cursor() {
        if screen.cell(row, col).unwrap().is_wide_continuation() {
            col = col.saturating_sub(1);
        }
        let cell = screen.cell(row, col).unwrap();
        let rect = metrics.cell_rect(row, col, if cell.is_wide() { 2 } else { 1 });
        if cursor_on {
            painter.rect_filled(rect, 0.0, FOREGROUND);
            glyph(painter, cell, rect, metrics.baseline, font, BACKGROUND);
        }
        if !preedit.is_empty() {
            let galley = painter.layout_no_wrap(preedit.to_owned(), font.clone(), FOREGROUND);
            let overlay = Rect::from_min_size(
                rect.min,
                vec2(galley.size().x.max(rect.width()), metrics.cell.y),
            );
            painter.rect_filled(overlay, 0.0, SELECTION);
            painter.galley(overlay.min + vec2(0.0, 2.0), galley, FOREGROUND);
            painter.line_segment(
                [overlay.left_bottom(), overlay.right_bottom()],
                Stroke::new(1.0_f32, FOREGROUND),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_cell_origins_have_no_cumulative_fractional_drift() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let metrics = GridMetrics::new(
                Rect::from_min_size(pos2(8.1, 8.1), vec2(2400.0, 1800.0)),
                9.3,
                17.2,
                13.1,
                scale,
            );
            for col in 0..metrics.cols {
                let physical = metrics.cell_rect(0, col, 1).min.x * scale;
                assert!((physical - physical.round()).abs() < 0.001);
            }
            assert!(
                metrics
                    .cell_rect(metrics.rows - 1, metrics.cols - 1, 1)
                    .max
                    .x
                    <= 2408.1
            );
            assert_eq!(
                metrics.position(pos2(-100.0, -100.0)),
                Position { row: 0, col: 0 }
            );
        }
        let tiny = GridMetrics::new(Rect::ZERO, 9.0, 17.0, 13.0, 1.0);
        assert_eq!((tiny.rows, tiny.cols), (2, 2));
    }

    #[test]
    fn view_paints_unicode_and_cursor_without_a_gpu() {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let mut view = TerminalView::default();
        let mut terminal = Terminal::new(24, 80);
        terminal.demo();
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(816.0, 592.0))),
                ..Default::default()
            },
            |ctx| {
                view.show(ctx, &mut terminal, "compose", true, true);
            },
        );
        assert!(!output.shapes.is_empty());
        assert!(
            !ctx.tessellate(output.shapes, output.pixels_per_point)
                .is_empty()
        );
        assert!(terminal.screen().contents().contains("Wide:"));
        ctx.fonts_mut(|fonts| {
            assert!(fonts.has_glyphs(&FontId::monospace(FONT_SIZE), "界語e\u{301}a\u{308}λ→∑"))
        });
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1.0, 1.0))),
                ..Default::default()
            },
            |ctx| {
                view.show(ctx, &mut terminal, "", true, true);
            },
        );
        assert_eq!(terminal.screen().size(), (2, 2));
    }

    #[test]
    fn pointer_click_and_drag_select_the_expected_cells() {
        for (start, end, expected) in [(2, 2, "c"), (1, 4, "bcde")] {
            let ctx = egui::Context::default();
            install_fonts(&ctx);
            let mut view = TerminalView::default();
            let mut terminal = Terminal::new(2, 8);
            let screen_rect = Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 160.0)));
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect,
                    ..Default::default()
                },
                |ctx| {
                    view.show(ctx, &mut terminal, "", true, false);
                },
            );
            terminal.process(b"abcdef");
            let metrics = view.metrics.unwrap();
            let a = metrics.cell_rect(0, start, 1).center();
            let b = metrics.cell_rect(0, end, 1).center();
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            for events in [
                vec![egui::Event::PointerMoved(a), button(a, true)],
                vec![egui::Event::PointerMoved(b)],
                vec![button(b, false)],
            ] {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect,
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        view.show(ctx, &mut terminal, "", true, false);
                    },
                );
            }
            assert_eq!(terminal.selected_text(), expected);
        }
    }
}
