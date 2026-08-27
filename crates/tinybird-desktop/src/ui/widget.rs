//! A small immediate-mode widget set.
//!
//! Immediate mode is the right fit here: the app already redraws every frame,
//! and retained widgets would mean a second source of truth living alongside
//! `App`'s fields. Each widget draws itself and returns what the user did.
//!
//! **Coordinate rule:** widgets take rectangles in *window* coordinates and
//! expect to be drawn on the root canvas, because [`UiInput`] reports the
//! pointer in window coordinates. [`super::canvas::Canvas::sub`] is for layout
//! code that does not hit-test (panels, the dashboard); mixing the two would
//! silently offset every click.
//!
//! Geometry is factored into pure functions (`slider_fraction_at`,
//! `row_index_at`, `tab_rects`) so the arithmetic can be tested without a
//! frame buffer.

use super::canvas::{text_width, Align, Canvas, Rect};
use super::input::{MouseButton, UiInput};
use super::theme::Palette;

/// Vertical padding inside a standard control.
pub const CONTROL_PAD_Y: i32 = 4;
/// Horizontal padding inside a standard control.
pub const CONTROL_PAD_X: i32 = 8;
/// Height of one row in a list.
pub const ROW_HEIGHT: i32 = 16;
/// Default text scale for chrome and dialogs.
pub const TEXT_SCALE: i32 = 1;

/// Everything a widget needs, bundled so signatures stay short.
pub struct Ui<'a, 'buf> {
    pub canvas: &'a mut Canvas<'buf>,
    pub input: &'a UiInput,
    pub palette: &'a Palette,
}

impl<'a, 'buf> Ui<'a, 'buf> {
    pub fn new(
        canvas: &'a mut Canvas<'buf>,
        input: &'a UiInput,
        palette: &'a Palette,
    ) -> Self {
        Self {
            canvas,
            input,
            palette,
        }
    }

    fn hovered(&self, rect: Rect, enabled: bool) -> bool {
        enabled && self.input.hovering(rect)
    }

    fn held(&self, rect: Rect, enabled: bool) -> bool {
        enabled && self.input.dragging_from(rect, MouseButton::Left)
    }
}

/// Visual state a control can be in, used to pick colors consistently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControlState {
    Disabled,
    Normal,
    Hover,
    Active,
}

impl ControlState {
    fn resolve(hovered: bool, held: bool, enabled: bool) -> Self {
        if !enabled {
            ControlState::Disabled
        } else if held {
            ControlState::Active
        } else if hovered {
            ControlState::Hover
        } else {
            ControlState::Normal
        }
    }

    fn surface(self, palette: &Palette) -> u32 {
        match self {
            ControlState::Disabled => palette.surface_sunken,
            ControlState::Normal => palette.surface_raised,
            ControlState::Hover => palette.chrome_hover,
            ControlState::Active => palette.chrome_active,
        }
    }

    fn ink(self, palette: &Palette) -> u32 {
        match self {
            ControlState::Disabled => palette.ink_disabled,
            _ => palette.ink,
        }
    }
}

/// A push button. Returns `true` on the frame it is clicked.
pub fn button(ui: &mut Ui<'_, '_>, rect: Rect, label: &str, enabled: bool) -> bool {
    button_styled(ui, rect, label, enabled, false)
}

/// A push button that can be marked as the dialog's default action.
pub fn button_styled(
    ui: &mut Ui<'_, '_>,
    rect: Rect,
    label: &str,
    enabled: bool,
    is_default: bool,
) -> bool {
    let hovered = ui.hovered(rect, enabled);
    let held = ui.held(rect, enabled);
    let state = ControlState::resolve(hovered, held, enabled);

    ui.canvas.fill_rect(rect, state.surface(ui.palette));
    let border = if is_default && enabled {
        ui.palette.accent
    } else {
        ui.palette.chrome_border
    };
    ui.canvas.stroke_rect(rect, 1, border);
    ui.canvas
        .text_in(rect, Align::Center, label, TEXT_SCALE, state.ink(ui.palette));

    enabled && ui.input.clicked(rect)
}

/// A labelled checkbox. Returns `true` when toggled; the caller owns the value.
pub fn checkbox(ui: &mut Ui<'_, '_>, rect: Rect, label: &str, checked: bool, enabled: bool) -> bool {
    let hovered = ui.hovered(rect, enabled);
    let state = ControlState::resolve(hovered, false, enabled);

    if hovered {
        ui.canvas.fill_rect(rect, ui.palette.chrome_hover);
    }

    let box_size = 10;
    let box_rect = Rect::new(
        rect.x + CONTROL_PAD_X / 2,
        rect.y + (rect.h - box_size) / 2,
        box_size,
        box_size,
    );
    ui.canvas.fill_rect(box_rect, ui.palette.surface_sunken);
    ui.canvas.stroke_rect(box_rect, 1, ui.palette.chrome_border);
    if checked {
        ui.canvas.fill_rect(
            box_rect.inset(3),
            if enabled {
                ui.palette.accent
            } else {
                ui.palette.ink_disabled
            },
        );
    }

    let text_rect = Rect::new(
        box_rect.right() + CONTROL_PAD_X,
        rect.y,
        rect.right() - box_rect.right() - CONTROL_PAD_X,
        rect.h,
    );
    ui.canvas.text_in(
        text_rect,
        Align::Left,
        label,
        TEXT_SCALE,
        state.ink(ui.palette),
    );

    enabled && ui.input.clicked(rect)
}

/// Lay out `count` equal-width segments across `rect`, separated by `gap`.
pub fn segment_rects(rect: Rect, count: usize, gap: i32) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }
    let count_i = count as i32;
    let total_gap = gap * (count_i - 1).max(0);
    let each = ((rect.w - total_gap) / count_i).max(1);
    (0..count_i)
        .map(|index| Rect::new(rect.x + index * (each + gap), rect.y, each, rect.h))
        .collect()
}

/// A horizontal radio group. Returns the newly chosen index, if any.
pub fn radio_row(
    ui: &mut Ui<'_, '_>,
    rect: Rect,
    options: &[&str],
    selected: usize,
    enabled: bool,
) -> Option<usize> {
    let mut chosen = None;
    for (index, (segment, label)) in segment_rects(rect, options.len(), 2)
        .into_iter()
        .zip(options)
        .enumerate()
    {
        let is_selected = index == selected;
        let hovered = ui.hovered(segment, enabled);
        let state = ControlState::resolve(hovered, is_selected, enabled);

        ui.canvas.fill_rect(segment, state.surface(ui.palette));
        if is_selected {
            ui.canvas.fill_rect(
                Rect::new(segment.x, segment.bottom() - 2, segment.w, 2),
                ui.palette.accent,
            );
        }
        ui.canvas.stroke_rect(segment, 1, ui.palette.chrome_border);
        ui.canvas.text_in(
            segment,
            Align::Center,
            label,
            TEXT_SCALE,
            state.ink(ui.palette),
        );

        if enabled && ui.input.clicked(segment) {
            chosen = Some(index);
        }
    }
    chosen
}

/// Fraction (0.0..=1.0) a slider at `rect` represents for pointer x `px`.
pub fn slider_fraction_at(rect: Rect, px: i32) -> f32 {
    if rect.w <= 1 {
        return 0.0;
    }
    ((px - rect.x) as f32 / (rect.w - 1) as f32).clamp(0.0, 1.0)
}

/// A horizontal slider over 0.0..=1.0. Returns the new value while dragging.
pub fn slider(ui: &mut Ui<'_, '_>, rect: Rect, value: f32, enabled: bool) -> Option<f32> {
    let value = value.clamp(0.0, 1.0);
    let track_h = 4;
    let track = Rect::new(
        rect.x,
        rect.y + (rect.h - track_h) / 2,
        rect.w,
        track_h,
    );

    ui.canvas.fill_rect(track, ui.palette.surface_sunken);
    let filled_w = (track.w as f32 * value).round() as i32;
    ui.canvas.fill_rect(
        Rect::new(track.x, track.y, filled_w, track.h),
        if enabled {
            ui.palette.accent
        } else {
            ui.palette.ink_disabled
        },
    );

    let knob_w = 6;
    let knob_x = rect.x + ((rect.w - knob_w) as f32 * value).round() as i32;
    let knob = Rect::new(knob_x, rect.y, knob_w, rect.h);
    let hovered = ui.hovered(rect, enabled);
    let dragging = ui.held(rect, enabled);
    let state = ControlState::resolve(hovered, dragging, enabled);
    ui.canvas.fill_rect(knob, state.surface(ui.palette));
    ui.canvas.stroke_rect(knob, 1, ui.palette.chrome_border);

    if dragging {
        if let Some((px, _)) = ui.input.pointer() {
            return Some(slider_fraction_at(rect, px));
        }
    }
    None
}

/// Which row index a pointer at `py` falls on, or `None` if outside.
pub fn row_index_at(rect: Rect, row_height: i32, scroll_rows: usize, py: i32) -> Option<usize> {
    if row_height <= 0 || py < rect.y || py >= rect.bottom() {
        return None;
    }
    Some(((py - rect.y) / row_height) as usize + scroll_rows)
}

/// How many rows fit in `rect`.
pub fn visible_rows(rect: Rect, row_height: i32) -> usize {
    if row_height <= 0 {
        return 0;
    }
    (rect.h / row_height).max(0) as usize
}

/// Clamp a scroll offset so the last page is never scrolled past.
pub fn clamp_scroll(scroll: usize, total: usize, visible: usize) -> usize {
    scroll.min(total.saturating_sub(visible))
}

/// What a [`list`] interaction produced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListResponse {
    /// A row was clicked once.
    pub clicked: Option<usize>,
    /// The scroll offset after applying this frame's wheel input.
    pub scroll: usize,
}

/// A scrollable single-selection list.
pub fn list(
    ui: &mut Ui<'_, '_>,
    rect: Rect,
    items: &[String],
    selected: Option<usize>,
    scroll: usize,
    enabled: bool,
) -> ListResponse {
    ui.canvas.fill_rect(rect, ui.palette.surface_sunken);
    ui.canvas.stroke_rect(rect, 1, ui.palette.chrome_border);

    let inner = rect.inset(2);
    let visible = visible_rows(inner, ROW_HEIGHT);

    let mut scroll = clamp_scroll(scroll, items.len(), visible);
    if enabled && ui.input.hovering(rect) {
        let wheel = ui.input.wheel();
        if wheel > 0.0 {
            scroll = scroll.saturating_sub(wheel.ceil() as usize);
        } else if wheel < 0.0 {
            scroll = clamp_scroll(
                scroll + (-wheel).ceil() as usize,
                items.len(),
                visible,
            );
        }
    }

    let mut clicked = None;
    for offset in 0..visible {
        let index = scroll + offset;
        let Some(item) = items.get(index) else {
            break;
        };
        let row = Rect::new(
            inner.x,
            inner.y + offset as i32 * ROW_HEIGHT,
            inner.w,
            ROW_HEIGHT,
        );
        let is_selected = selected == Some(index);
        let hovered = ui.hovered(row, enabled);

        if is_selected {
            ui.canvas.fill_rect(row, ui.palette.selection);
        } else if hovered {
            ui.canvas.fill_rect(row, ui.palette.chrome_hover);
        }

        let ink = if !enabled {
            ui.palette.ink_disabled
        } else if is_selected {
            ui.palette.ink
        } else {
            ui.palette.ink_muted
        };
        let text_rect = Rect::new(row.x + CONTROL_PAD_X, row.y, row.w - CONTROL_PAD_X, row.h);
        ui.canvas
            .text_in(text_rect, Align::Left, item, TEXT_SCALE, ink);

        if enabled && ui.input.clicked(row) {
            clicked = Some(index);
        }
    }

    // Scrollbar, drawn only when it means something.
    if items.len() > visible && visible > 0 {
        let bar = Rect::new(rect.right() - 4, inner.y, 3, inner.h);
        ui.canvas.fill_rect(bar, ui.palette.surface);
        let thumb_h = ((visible * bar.h as usize) / items.len()).max(6) as i32;
        let travel = (bar.h - thumb_h).max(0);
        let max_scroll = items.len().saturating_sub(visible).max(1);
        let thumb_y = bar.y + (travel * scroll as i32) / max_scroll as i32;
        ui.canvas.fill_rect(
            Rect::new(bar.x, thumb_y, bar.w, thumb_h),
            ui.palette.chrome_border,
        );
    }

    ListResponse { clicked, scroll }
}

/// Lay out a tab strip, sizing each tab to its label.
pub fn tab_rects(rect: Rect, labels: &[&str], scale: i32) -> Vec<Rect> {
    let mut rects = Vec::with_capacity(labels.len());
    let mut x = rect.x;
    for label in labels {
        let w = text_width(label, scale) + CONTROL_PAD_X * 2;
        rects.push(Rect::new(x, rect.y, w, rect.h));
        x += w;
    }
    rects
}

/// A horizontal tab strip. Returns the newly selected tab, if any.
pub fn tab_strip(ui: &mut Ui<'_, '_>, rect: Rect, labels: &[&str], selected: usize) -> Option<usize> {
    ui.canvas.fill_rect(rect, ui.palette.surface);
    ui.canvas.hline(
        rect.x,
        rect.bottom() - 1,
        rect.w,
        ui.palette.chrome_border,
    );

    let mut chosen = None;
    for (index, tab) in tab_rects(rect, labels, TEXT_SCALE).into_iter().enumerate() {
        let is_selected = index == selected;
        let hovered = ui.hovered(tab, true);

        if is_selected {
            ui.canvas.fill_rect(tab, ui.palette.surface_raised);
            ui.canvas
                .fill_rect(Rect::new(tab.x, tab.y, tab.w, 2), ui.palette.accent);
        } else if hovered {
            ui.canvas.fill_rect(tab, ui.palette.chrome_hover);
        }

        let ink = if is_selected {
            ui.palette.ink
        } else {
            ui.palette.ink_muted
        };
        ui.canvas
            .text_in(tab, Align::Center, labels[index], TEXT_SCALE, ink);

        if ui.input.clicked(tab) {
            chosen = Some(index);
        }
    }
    chosen
}

/// Draw a titled panel frame, returning the content rect inside it.
pub fn panel(ui: &mut Ui<'_, '_>, rect: Rect, title: Option<&str>) -> Rect {
    ui.canvas.fill_rect(rect, ui.palette.surface);
    ui.canvas.stroke_rect(rect, 1, ui.palette.chrome_border);

    let Some(title) = title else {
        return rect.inset(CONTROL_PAD_X);
    };

    let header_h = ROW_HEIGHT + CONTROL_PAD_Y;
    let (header, body) = rect.split_top(header_h);
    ui.canvas
        .fill_rect(Rect::new(header.x, header.y, header.w, 2), ui.palette.accent);
    ui.canvas.text_in(
        Rect::new(header.x + CONTROL_PAD_X, header.y, header.w, header.h),
        Align::Left,
        title,
        TEXT_SCALE,
        ui.palette.ink,
    );
    ui.canvas
        .hline(body.x, body.y, body.w, ui.palette.chrome_border);
    body.inset(CONTROL_PAD_X)
}

/// A horizontal rule used to group controls.
pub fn separator(ui: &mut Ui<'_, '_>, x: i32, y: i32, w: i32) {
    ui.canvas.hline(x, y, w, ui.palette.chrome_border);
}

/// A plain text label.
pub fn label(ui: &mut Ui<'_, '_>, rect: Rect, align: Align, text: &str, muted: bool) {
    let ink = if muted {
        ui.palette.ink_muted
    } else {
        ui.palette.ink
    };
    ui.canvas.text_in(rect, align, text, TEXT_SCALE, ink);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_rects_partition_without_overlapping() {
        let row = Rect::new(0, 0, 100, 20);
        let segments = segment_rects(row, 4, 2);
        assert_eq!(segments.len(), 4);
        for pair in segments.windows(2) {
            assert!(
                pair[0].right() <= pair[1].x,
                "segments {pair:?} overlap"
            );
        }
        assert!(segments.last().unwrap().right() <= row.right());
    }

    #[test]
    fn segment_rects_handles_zero_and_one() {
        assert!(segment_rects(Rect::new(0, 0, 100, 20), 0, 2).is_empty());
        let single = segment_rects(Rect::new(0, 0, 100, 20), 1, 2);
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].w, 100);
    }

    #[test]
    fn slider_fraction_spans_the_full_track() {
        let track = Rect::new(10, 0, 101, 12);
        assert_eq!(slider_fraction_at(track, 10), 0.0);
        assert_eq!(slider_fraction_at(track, 110), 1.0);
        assert!((slider_fraction_at(track, 60) - 0.5).abs() < 0.01);
    }

    #[test]
    fn slider_fraction_clamps_outside_the_track() {
        let track = Rect::new(10, 0, 101, 12);
        assert_eq!(slider_fraction_at(track, -500), 0.0);
        assert_eq!(slider_fraction_at(track, 5000), 1.0);
    }

    #[test]
    fn slider_fraction_survives_a_degenerate_track() {
        assert_eq!(slider_fraction_at(Rect::new(0, 0, 1, 10), 0), 0.0);
        assert_eq!(slider_fraction_at(Rect::new(0, 0, 0, 10), 50), 0.0);
    }

    #[test]
    fn row_index_accounts_for_scroll() {
        let rect = Rect::new(0, 100, 200, 64);
        assert_eq!(row_index_at(rect, 16, 0, 100), Some(0));
        assert_eq!(row_index_at(rect, 16, 0, 115), Some(0));
        assert_eq!(row_index_at(rect, 16, 0, 116), Some(1));
        assert_eq!(row_index_at(rect, 16, 5, 100), Some(5));
    }

    #[test]
    fn row_index_rejects_points_outside_the_list() {
        let rect = Rect::new(0, 100, 200, 64);
        assert_eq!(row_index_at(rect, 16, 0, 99), None);
        assert_eq!(row_index_at(rect, 16, 0, 164), None);
    }

    #[test]
    fn clamp_scroll_never_scrolls_past_the_last_page() {
        assert_eq!(clamp_scroll(99, 10, 4), 6);
        assert_eq!(clamp_scroll(0, 10, 4), 0);
        assert_eq!(clamp_scroll(5, 3, 4), 0, "list shorter than the viewport");
    }

    #[test]
    fn tabs_are_sized_to_their_labels_and_laid_out_left_to_right() {
        let strip = Rect::new(0, 0, 400, 20);
        let rects = tab_rects(strip, &["A", "Longer"], TEXT_SCALE);
        assert_eq!(rects.len(), 2);
        assert!(rects[1].w > rects[0].w);
        assert_eq!(rects[0].right(), rects[1].x);
    }
}
