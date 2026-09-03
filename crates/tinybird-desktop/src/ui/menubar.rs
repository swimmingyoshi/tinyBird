//! The classic menu bar: `File  Emulation  Video  Audio  Addons  Tools  Help`.
//!
//! The menu is **declarative**. `App` builds a `Vec<Menu>` describing what is
//! currently available and what is checked; this module renders it, hit-tests
//! it, drives keyboard navigation, and hands back a [`UiCommand`]. Adding an
//! entry never requires touching the renderer — which is the whole point,
//! because the previous design bound each feature to a function key inside a
//! 170-line `match` in `handle_key`, and `F1`–`F12` ran out.
//!
//! Interaction follows the conventions people already have muscle memory for:
//! click a title to open it, hover to switch between open menus, hover a
//! submenu parent to open it, click an item to activate and dismiss, `Esc` to
//! back out one level.

use super::canvas::{text_width, Align, Rect};
use super::input::{MouseButton, UiKey};
use super::widget::{Ui, TEXT_SCALE};
use super::ChromeMode;

/// Horizontal padding either side of a top-level title.
const TITLE_PAD_X: i32 = 10;
/// Height of a normal dropdown entry.
const ENTRY_HEIGHT: i32 = 18;
/// Height of a separator entry.
const SEPARATOR_HEIGHT: i32 = 5;
/// Padding inside a dropdown, left and right.
const DROPDOWN_PAD_X: i32 = 8;
/// Vertical padding at the top and bottom of a dropdown.
const DROPDOWN_PAD_Y: i32 = 4;
/// Width reserved for the check/radio marker column.
const MARK_COLUMN: i32 = 12;
/// Gap between an entry label and its shortcut text.
const SHORTCUT_GAP: i32 = 24;
/// Width reserved for the submenu arrow.
const ARROW_COLUMN: i32 = 12;

/// Every action the classic shell can ask `App` to perform.
///
/// This is the UI's command vocabulary. Keeping it a plain enum means the menu,
/// keyboard shortcuts, and (later) dialogs all funnel into one `match` in
/// `App`, instead of each input path calling methods directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiCommand {
    // --- File ---
    OpenRom,
    OpenRecent(usize),
    ClearRecentRoms,
    ReloadRom,
    SaveStateSlot(u8),
    LoadStateSlot(u8),
    OpenSaveStateManager,
    OpenLoadStateManager,
    TakeScreenshot,
    OpenDataFolder,
    Exit,

    // --- Emulation ---
    TogglePause,
    Reset,
    SetSpeed(u32),

    // --- Video ---
    ToggleFullscreen,
    SetWindowScale(u32),
    ToggleIntegerScaling,
    ToggleColorCorrection,
    ToggleHud,

    // --- Audio ---
    ToggleMute,
    SetVolumePercent(u32),
    VolumeUp,
    VolumeDown,

    // --- Addons ---
    ToggleDashboard,
    ShowTeamPanel,
    ShowEncountersPanel,
    HideAddonUi,
    SetDashboardLayout(u8),
    SetGameSize(u8),
    SetSidePanelWidth(u8),
    SetTheme(u8),
    ChooseWallpaper,
    ToggleSnapshotExport,

    // --- Tools ---
    OpenSettings,
    ShowRomInfo,
    ShowAddonStatus,

    // --- Help ---
    ShowControls,
    ShowAbout,

    // --- Chrome ---
    SetChromeMode(ChromeMode),
}

/// The marker drawn in an entry's left gutter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    None,
    Check(bool),
    Radio(bool),
}

/// What activating an entry does.
#[derive(Clone, Debug)]
pub enum EntryKind {
    Separator,
    Command(UiCommand),
    Submenu(Vec<MenuEntry>),
}

/// One row in a dropdown.
#[derive(Clone, Debug)]
pub struct MenuEntry {
    pub label: String,
    pub shortcut: Option<String>,
    pub mark: Mark,
    pub enabled: bool,
    pub kind: EntryKind,
}

impl MenuEntry {
    pub fn separator() -> Self {
        Self {
            label: String::new(),
            shortcut: None,
            mark: Mark::None,
            enabled: false,
            kind: EntryKind::Separator,
        }
    }

    pub fn action(label: impl Into<String>, command: UiCommand) -> Self {
        Self {
            label: label.into(),
            shortcut: None,
            mark: Mark::None,
            enabled: true,
            kind: EntryKind::Command(command),
        }
    }

    pub fn submenu(label: impl Into<String>, entries: Vec<MenuEntry>) -> Self {
        Self {
            label: label.into(),
            shortcut: None,
            mark: Mark::None,
            enabled: !entries.is_empty(),
            kind: EntryKind::Submenu(entries),
        }
    }

    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.mark = Mark::Check(checked);
        self
    }

    pub fn radio(mut self, selected: bool) -> Self {
        self.mark = Mark::Radio(selected);
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    fn is_separator(&self) -> bool {
        matches!(self.kind, EntryKind::Separator)
    }

    /// Separators and disabled entries are skipped by arrow-key navigation.
    fn is_selectable(&self) -> bool {
        !self.is_separator() && self.enabled
    }

    fn height(&self) -> i32 {
        if self.is_separator() {
            SEPARATOR_HEIGHT
        } else {
            ENTRY_HEIGHT
        }
    }
}

/// One top-level menu.
#[derive(Clone, Debug)]
pub struct Menu {
    pub title: String,
    pub entries: Vec<MenuEntry>,
}

impl Menu {
    pub fn new(title: impl Into<String>, entries: Vec<MenuEntry>) -> Self {
        Self {
            title: title.into(),
            entries,
        }
    }
}

/// Which menu is open and how deep into its submenus the user is.
#[derive(Debug, Default)]
pub struct MenuBarState {
    /// Index of the open top-level menu, if any.
    open: Option<usize>,
    /// Indices into successively nested submenus below the open menu.
    path: Vec<usize>,
    /// Highlighted entry at the deepest open level, for keyboard navigation.
    highlight: Option<usize>,
    /// True once the user navigates with the keyboard, so we keep the highlight
    /// visible even when the pointer is elsewhere.
    keyboard_mode: bool,
}

impl MenuBarState {
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn close(&mut self) {
        self.open = None;
        self.path.clear();
        self.highlight = None;
        self.keyboard_mode = false;
    }

    fn open_menu(&mut self, index: usize, keyboard: bool) {
        self.open = Some(index);
        self.path.clear();
        self.highlight = None;
        self.keyboard_mode = keyboard;
    }

    /// Toggle the first menu open/closed, for `Alt` / `F10`.
    pub fn toggle_from_keyboard(&mut self, menu_count: usize) {
        if self.is_open() || menu_count == 0 {
            self.close();
        } else {
            self.open_menu(0, true);
        }
    }

    /// The entry list currently at the deepest open level.
    fn level_entries<'m>(&self, menus: &'m [Menu]) -> Option<&'m [MenuEntry]> {
        let open = self.open?;
        let mut entries: &[MenuEntry] = &menus.get(open)?.entries;
        for &index in &self.path {
            match entries.get(index).map(|entry| &entry.kind) {
                Some(EntryKind::Submenu(children)) => entries = children,
                _ => return None,
            }
        }
        Some(entries)
    }
}

/// Result of one menu bar pass.
#[derive(Clone, Copy, Debug, Default)]
pub struct MenuBarResponse {
    /// The command the user activated, if any.
    pub command: Option<UiCommand>,
    /// The menu handled the pointer this frame; the content layer must ignore it.
    pub captured_pointer: bool,
}

/// Rectangles of the top-level titles, laid out left to right.
pub fn title_rects(bar: Rect, menus: &[Menu]) -> Vec<Rect> {
    let mut rects = Vec::with_capacity(menus.len());
    let mut x = bar.x + TITLE_PAD_X / 2;
    for menu in menus {
        let w = text_width(&menu.title, TEXT_SCALE) + TITLE_PAD_X * 2;
        rects.push(Rect::new(x, bar.y, w, bar.h));
        x += w;
    }
    rects
}

/// Width and height a dropdown needs to show `entries`.
pub fn dropdown_size(entries: &[MenuEntry]) -> (i32, i32) {
    let mut content_w = 0;
    let mut height = DROPDOWN_PAD_Y * 2;

    for entry in entries {
        height += entry.height();
        if entry.is_separator() {
            continue;
        }

        let mut width = text_width(&entry.label, TEXT_SCALE);
        if let Some(shortcut) = &entry.shortcut {
            width += SHORTCUT_GAP + text_width(shortcut, TEXT_SCALE);
        }
        if matches!(entry.kind, EntryKind::Submenu(_)) {
            width += ARROW_COLUMN;
        }
        content_w = content_w.max(width);
    }

    (
        content_w + MARK_COLUMN + DROPDOWN_PAD_X * 2,
        height.max(ENTRY_HEIGHT),
    )
}

/// Place a dropdown of `size` at `preferred`, nudged to stay inside `bounds`.
pub fn fit_dropdown(preferred: (i32, i32), size: (i32, i32), bounds: Rect) -> Rect {
    let (w, h) = size;
    let x = preferred.0.min(bounds.right() - w).max(bounds.x);
    let y = preferred.1.min(bounds.bottom() - h).max(bounds.y);
    Rect::new(x, y, w, h)
}

/// Rectangle of each entry within a dropdown placed at `frame`.
fn entry_rects(frame: Rect, entries: &[MenuEntry]) -> Vec<Rect> {
    let mut rects = Vec::with_capacity(entries.len());
    let mut y = frame.y + DROPDOWN_PAD_Y;
    for entry in entries {
        let h = entry.height();
        rects.push(Rect::new(frame.x, y, frame.w, h));
        y += h;
    }
    rects
}

/// Step the highlight to the next selectable entry in `direction` (+1 / -1),
/// wrapping and skipping separators and disabled rows.
pub fn next_selectable(
    entries: &[MenuEntry],
    from: Option<usize>,
    direction: i32,
) -> Option<usize> {
    if entries.is_empty() {
        return None;
    }
    let len = entries.len() as i32;
    let start = match from {
        Some(index) => index as i32,
        None if direction > 0 => -1,
        None => len,
    };

    for step in 1..=len {
        let candidate = (start + direction * step).rem_euclid(len) as usize;
        if entries[candidate].is_selectable() {
            return Some(candidate);
        }
    }
    None
}

/// Draw the bar and any open dropdown, and report what the user did.
///
/// Call this last in the frame so dropdowns paint over the content beneath.
pub fn menu_bar(
    ui: &mut Ui<'_, '_>,
    bar: Rect,
    menus: &[Menu],
    state: &mut MenuBarState,
) -> MenuBarResponse {
    let mut response = MenuBarResponse::default();

    // --- the bar itself ---
    ui.canvas.fill_rect(bar, ui.palette.chrome);
    ui.canvas
        .hline(bar.x, bar.bottom() - 1, bar.w, ui.palette.chrome_border);

    let titles = title_rects(bar, menus);
    for (index, title) in titles.iter().enumerate() {
        let is_open = state.open == Some(index);
        let hovered = ui.input.hovering(*title);

        if is_open {
            ui.canvas.fill_rect(*title, ui.palette.chrome_active);
        } else if hovered {
            ui.canvas.fill_rect(*title, ui.palette.chrome_hover);
        }
        ui.canvas.text_in(
            *title,
            Align::Center,
            &menus[index].title,
            TEXT_SCALE,
            if is_open || hovered {
                ui.palette.ink
            } else {
                ui.palette.ink_muted
            },
        );

        if ui.input.pressed_in(*title, MouseButton::Left) {
            response.captured_pointer = true;
            if is_open {
                state.close();
            } else {
                state.open_menu(index, false);
            }
        } else if hovered && state.is_open() && !is_open {
            // Hovering a different title while a menu is open switches to it,
            // without needing a second click.
            state.open_menu(index, false);
        }
    }

    if ui.input.hovering(bar) {
        response.captured_pointer = true;
    }

    let Some(open_index) = state.open else {
        return response;
    };
    let Some(menu) = menus.get(open_index) else {
        state.close();
        return response;
    };

    // --- dropdowns, one level at a time ---
    let bounds = ui.canvas.bounds();
    let mut entries: &[MenuEntry] = &menu.entries;
    let mut anchor = (titles[open_index].x, bar.bottom());
    let mut depth = 0usize;

    loop {
        let frame = fit_dropdown(anchor, dropdown_size(entries), bounds);
        let rects = entry_rects(frame, entries);
        let is_deepest = depth == state.path.len();

        draw_dropdown(ui, frame, entries, &rects, state, depth, is_deepest);

        if ui.input.hovering(frame) {
            response.captured_pointer = true;
        }

        // Pointer interaction only applies to the level under the cursor.
        for (index, rect) in rects.iter().enumerate() {
            let entry = &entries[index];
            if !entry.is_selectable() || !ui.input.hovering(*rect) {
                continue;
            }

            match &entry.kind {
                EntryKind::Submenu(_) => {
                    // Hovering a parent opens its submenu and drops any deeper
                    // level that is no longer reachable.
                    state.path.truncate(depth);
                    state.path.push(index);
                    state.highlight = None;
                    state.keyboard_mode = false;
                }
                EntryKind::Command(command) => {
                    state.path.truncate(depth);
                    state.highlight = Some(index);
                    state.keyboard_mode = false;
                    if ui.input.clicked(*rect) {
                        response.command = Some(*command);
                        response.captured_pointer = true;
                        state.close();
                        return response;
                    }
                }
                EntryKind::Separator => {}
            }
        }

        // Descend if a submenu is open below this level.
        let Some(&child_index) = state.path.get(depth) else {
            break;
        };
        let Some(EntryKind::Submenu(children)) = entries.get(child_index).map(|e| &e.kind) else {
            state.path.truncate(depth);
            break;
        };
        anchor = (
            frame.right() - 2,
            rects.get(child_index).map_or(frame.y, |r| r.y),
        );
        entries = children;
        depth += 1;
    }

    // A press anywhere outside the bar and its dropdowns dismisses the menu.
    if ui.input.just_pressed(MouseButton::Left) && !response.captured_pointer {
        state.close();
    }

    response
}

#[allow(clippy::too_many_arguments)]
fn draw_dropdown(
    ui: &mut Ui<'_, '_>,
    frame: Rect,
    entries: &[MenuEntry],
    rects: &[Rect],
    state: &MenuBarState,
    depth: usize,
    is_deepest: bool,
) {
    // A cheap drop shadow keeps the dropdown legible over a busy dashboard.
    ui.canvas
        .blend_rect(frame.translate(3, 3), 0x0000_0000, 110);
    ui.canvas.fill_rect(frame, ui.palette.surface);
    ui.canvas.stroke_rect(frame, 1, ui.palette.chrome_border);

    for (index, (entry, rect)) in entries.iter().zip(rects).enumerate() {
        if entry.is_separator() {
            ui.canvas.hline(
                rect.x + DROPDOWN_PAD_X,
                rect.y + rect.h / 2,
                rect.w - DROPDOWN_PAD_X * 2,
                ui.palette.chrome_border,
            );
            continue;
        }

        let path_selected = state.path.get(depth) == Some(&index);
        let key_selected = is_deepest && state.keyboard_mode && state.highlight == Some(index);
        let hovered = entry.enabled && ui.input.hovering(*rect);
        let highlighted = hovered || path_selected || key_selected;

        if highlighted && entry.enabled {
            ui.canvas.fill_rect(*rect, ui.palette.selection);
        }

        let ink = if entry.enabled {
            ui.palette.ink
        } else {
            ui.palette.ink_disabled
        };

        // Marker gutter.
        match entry.mark {
            Mark::None => {}
            Mark::Check(true) => {
                ui.canvas.text(
                    rect.x + DROPDOWN_PAD_X,
                    rect.y + (rect.h - 8) / 2,
                    "x",
                    TEXT_SCALE,
                    ui.palette.accent,
                );
            }
            Mark::Radio(true) => {
                ui.canvas.fill_rect(
                    Rect::new(rect.x + DROPDOWN_PAD_X + 2, rect.y + rect.h / 2 - 2, 4, 4),
                    ui.palette.accent,
                );
            }
            Mark::Check(false) | Mark::Radio(false) => {}
        }

        let label_x = rect.x + DROPDOWN_PAD_X + MARK_COLUMN;
        ui.canvas.text_in(
            Rect::new(label_x, rect.y, rect.w, rect.h),
            Align::Left,
            &entry.label,
            TEXT_SCALE,
            ink,
        );

        if let Some(shortcut) = &entry.shortcut {
            ui.canvas.text_in(
                Rect::new(
                    rect.x,
                    rect.y,
                    rect.w - DROPDOWN_PAD_X - ARROW_COLUMN / 2,
                    rect.h,
                ),
                Align::Right,
                shortcut,
                TEXT_SCALE,
                if entry.enabled {
                    ui.palette.ink_muted
                } else {
                    ui.palette.ink_disabled
                },
            );
        }

        if matches!(entry.kind, EntryKind::Submenu(_)) {
            ui.canvas.text_in(
                Rect::new(rect.x, rect.y, rect.w - DROPDOWN_PAD_X, rect.h),
                Align::Right,
                ">",
                TEXT_SCALE,
                ink,
            );
        }
    }
}

/// Handle a key press while a menu is open, or `Alt`/`F10` while it is not.
///
/// Returns the activated command, if the key ran one. `handled` reports whether
/// the key belonged to the menu at all, so the caller knows not to forward it
/// to the emulator.
pub struct KeyOutcome {
    pub command: Option<UiCommand>,
    pub handled: bool,
}

pub fn handle_key(state: &mut MenuBarState, menus: &[Menu], key: UiKey) -> KeyOutcome {
    let mut outcome = KeyOutcome {
        command: None,
        handled: false,
    };

    if !state.is_open() {
        if matches!(key, UiKey::Function(10)) {
            state.toggle_from_keyboard(menus.len());
            outcome.handled = true;
        }
        return outcome;
    }

    outcome.handled = true;
    let Some(entries) = state.level_entries(menus).map(<[MenuEntry]>::to_vec) else {
        state.close();
        return outcome;
    };

    match key {
        UiKey::Escape => {
            if state.path.pop().is_some() {
                state.highlight = None;
            } else {
                state.close();
            }
        }
        UiKey::Down => {
            state.keyboard_mode = true;
            state.highlight = next_selectable(&entries, state.highlight, 1);
        }
        UiKey::Up => {
            state.keyboard_mode = true;
            state.highlight = next_selectable(&entries, state.highlight, -1);
        }
        UiKey::Left => {
            state.keyboard_mode = true;
            if state.path.pop().is_some() {
                state.highlight = None;
            } else if let Some(open) = state.open {
                let count = menus.len();
                if count > 0 {
                    let next = (open + count - 1) % count;
                    state.open_menu(next, true);
                }
            }
        }
        UiKey::Right => {
            state.keyboard_mode = true;
            let submenu = state
                .highlight
                .and_then(|index| entries.get(index))
                .filter(|entry| matches!(entry.kind, EntryKind::Submenu(_)) && entry.enabled);
            if submenu.is_some() {
                if let Some(index) = state.highlight {
                    state.path.push(index);
                    state.highlight = None;
                }
            } else if let Some(open) = state.open {
                let count = menus.len();
                if count > 0 {
                    state.open_menu((open + 1) % count, true);
                }
            }
        }
        UiKey::Enter | UiKey::Space => {
            let Some(index) = state.highlight else {
                return outcome;
            };
            match entries.get(index).map(|entry| &entry.kind) {
                Some(EntryKind::Command(command)) => {
                    outcome.command = Some(*command);
                    state.close();
                }
                Some(EntryKind::Submenu(_)) => {
                    state.path.push(index);
                    state.highlight = None;
                }
                _ => {}
            }
        }
        UiKey::Function(10) => state.close(),
        _ => outcome.handled = false,
    }

    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::MENU_BAR_HEIGHT;

    fn sample_entries() -> Vec<MenuEntry> {
        vec![
            MenuEntry::action("Open ROM...", UiCommand::OpenRom).shortcut("Ctrl+O"),
            MenuEntry::separator(),
            MenuEntry::action("Reload ROM", UiCommand::ReloadRom).enabled(false),
            MenuEntry::submenu(
                "Save State",
                vec![MenuEntry::action("Slot 1", UiCommand::SaveStateSlot(1))],
            ),
            MenuEntry::action("Exit", UiCommand::Exit),
        ]
    }

    fn sample_menus() -> Vec<Menu> {
        vec![
            Menu::new("File", sample_entries()),
            Menu::new(
                "Help",
                vec![MenuEntry::action("About", UiCommand::ShowAbout)],
            ),
        ]
    }

    #[test]
    fn titles_are_laid_out_left_to_right_without_gaps() {
        let menus = sample_menus();
        let rects = title_rects(Rect::new(0, 0, 800, MENU_BAR_HEIGHT), &menus);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0].right(), rects[1].x);
        assert!(rects[0].w > text_width("File", TEXT_SCALE));
    }

    #[test]
    fn dropdown_is_wide_enough_for_the_longest_label_and_shortcut() {
        let entries = sample_entries();
        let (w, h) = dropdown_size(&entries);
        let widest =
            text_width("Open ROM...", TEXT_SCALE) + SHORTCUT_GAP + text_width("Ctrl+O", TEXT_SCALE);
        assert!(w >= widest, "dropdown {w} too narrow for {widest}");
        // Four normal entries plus one separator, plus padding.
        assert_eq!(h, ENTRY_HEIGHT * 4 + SEPARATOR_HEIGHT + DROPDOWN_PAD_Y * 2);
    }

    #[test]
    fn dropdowns_are_nudged_back_inside_the_window() {
        let bounds = Rect::new(0, 0, 640, 480);
        let placed = fit_dropdown((600, 460), (200, 100), bounds);
        assert_eq!(placed.right(), 640);
        assert_eq!(placed.bottom(), 480);
    }

    #[test]
    fn a_dropdown_larger_than_the_window_is_pinned_to_the_origin() {
        let bounds = Rect::new(0, 0, 100, 100);
        let placed = fit_dropdown((50, 50), (400, 400), bounds);
        assert_eq!((placed.x, placed.y), (0, 0));
    }

    #[test]
    fn arrow_navigation_skips_separators_and_disabled_entries() {
        let entries = sample_entries();
        // 0 = Open ROM, 1 = separator, 2 = disabled Reload, 3 = Save State.
        assert_eq!(next_selectable(&entries, Some(0), 1), Some(3));
        assert_eq!(next_selectable(&entries, Some(3), -1), Some(0));
    }

    #[test]
    fn arrow_navigation_wraps_at_both_ends() {
        let entries = sample_entries();
        assert_eq!(next_selectable(&entries, None, 1), Some(0));
        assert_eq!(next_selectable(&entries, None, -1), Some(4));
        assert_eq!(next_selectable(&entries, Some(4), 1), Some(0));
        assert_eq!(next_selectable(&entries, Some(0), -1), Some(4));
    }

    #[test]
    fn navigation_reports_nothing_when_no_entry_is_selectable() {
        let entries = vec![
            MenuEntry::separator(),
            MenuEntry::action("Nope", UiCommand::Exit).enabled(false),
        ];
        assert_eq!(next_selectable(&entries, None, 1), None);
    }

    #[test]
    fn f10_opens_and_closes_the_first_menu() {
        let menus = sample_menus();
        let mut state = MenuBarState::default();

        let outcome = handle_key(&mut state, &menus, UiKey::Function(10));
        assert!(outcome.handled);
        assert!(state.is_open());

        handle_key(&mut state, &menus, UiKey::Function(10));
        assert!(!state.is_open());
    }

    #[test]
    fn enter_activates_the_highlighted_command_and_closes_the_menu() {
        let menus = sample_menus();
        let mut state = MenuBarState::default();
        handle_key(&mut state, &menus, UiKey::Function(10));
        handle_key(&mut state, &menus, UiKey::Down);

        let outcome = handle_key(&mut state, &menus, UiKey::Enter);
        assert_eq!(outcome.command, Some(UiCommand::OpenRom));
        assert!(!state.is_open());
    }

    #[test]
    fn right_opens_a_submenu_and_left_backs_out_of_it() {
        let menus = sample_menus();
        let mut state = MenuBarState::default();
        handle_key(&mut state, &menus, UiKey::Function(10));
        // Down twice: Open ROM, then Save State (separator and disabled skipped).
        handle_key(&mut state, &menus, UiKey::Down);
        handle_key(&mut state, &menus, UiKey::Down);
        handle_key(&mut state, &menus, UiKey::Right);
        assert_eq!(state.path, vec![3]);

        handle_key(&mut state, &menus, UiKey::Left);
        assert!(state.path.is_empty());
        assert!(
            state.is_open(),
            "backing out of a submenu keeps the menu open"
        );
    }

    #[test]
    fn right_on_a_plain_entry_moves_to_the_next_top_level_menu() {
        let menus = sample_menus();
        let mut state = MenuBarState::default();
        handle_key(&mut state, &menus, UiKey::Function(10));
        handle_key(&mut state, &menus, UiKey::Down); // Open ROM
        handle_key(&mut state, &menus, UiKey::Right);
        assert_eq!(state.open, Some(1));
    }

    #[test]
    fn escape_closes_one_level_at_a_time() {
        let menus = sample_menus();
        let mut state = MenuBarState::default();
        handle_key(&mut state, &menus, UiKey::Function(10));
        handle_key(&mut state, &menus, UiKey::Down);
        handle_key(&mut state, &menus, UiKey::Down);
        handle_key(&mut state, &menus, UiKey::Right);

        handle_key(&mut state, &menus, UiKey::Escape);
        assert!(state.is_open());
        handle_key(&mut state, &menus, UiKey::Escape);
        assert!(!state.is_open());
    }

    // --- pointer-driven behaviour, exercised through a real draw pass ---

    use crate::ui::canvas::Canvas;
    use crate::ui::input::{MouseButton, UiInput};
    use crate::ui::theme;

    const CANVAS_W: usize = 800;
    const CANVAS_H: usize = 600;
    const BAR: Rect = Rect::new(0, 0, CANVAS_W as i32, MENU_BAR_HEIGHT);

    /// Run one menu bar frame against a scratch buffer.
    fn frame(input: &UiInput, state: &mut MenuBarState, menus: &[Menu]) -> MenuBarResponse {
        let mut buffer = vec![0u32; CANVAS_W * CANVAS_H];
        let mut canvas = Canvas::new(&mut buffer, CANVAS_W, CANVAS_H);
        let palette = *theme::palette(0);
        let mut ui = Ui::new(&mut canvas, input, &palette);
        menu_bar(&mut ui, BAR, menus, state)
    }

    /// Press and release the left button at one point, as two frames.
    fn click_at(input: &mut UiInput, state: &mut MenuBarState, menus: &[Menu], x: i32, y: i32) {
        input.set_pointer(x, y);
        input.set_button(MouseButton::Left, true);
        frame(input, state, menus);
        input.end_frame();

        input.set_button(MouseButton::Left, false);
    }

    #[test]
    fn clicking_a_title_opens_that_menu() {
        let menus = sample_menus();
        let mut state = MenuBarState::default();
        let mut input = UiInput::new();

        let titles = title_rects(BAR, &menus);
        input.set_pointer(titles[0].center_x(), titles[0].center_y());
        input.set_button(MouseButton::Left, true);
        let response = frame(&input, &mut state, &menus);

        assert!(state.is_open());
        assert!(
            response.captured_pointer,
            "the content layer must not also see this click"
        );
    }

    #[test]
    fn clicking_the_open_title_again_closes_the_menu() {
        let menus = sample_menus();
        let mut state = MenuBarState::default();
        let mut input = UiInput::new();
        let titles = title_rects(BAR, &menus);

        click_at(
            &mut input,
            &mut state,
            &menus,
            titles[0].center_x(),
            titles[0].center_y(),
        );
        assert!(state.is_open());
        input.end_frame();

        click_at(
            &mut input,
            &mut state,
            &menus,
            titles[0].center_x(),
            titles[0].center_y(),
        );
        assert!(!state.is_open());
    }

    #[test]
    fn hovering_a_different_title_switches_menus_without_a_second_click() {
        let menus = sample_menus();
        let mut state = MenuBarState::default();
        let mut input = UiInput::new();
        let titles = title_rects(BAR, &menus);

        click_at(
            &mut input,
            &mut state,
            &menus,
            titles[0].center_x(),
            titles[0].center_y(),
        );
        input.end_frame();

        // Move to the next title without pressing anything.
        input.set_pointer(titles[1].center_x(), titles[1].center_y());
        frame(&input, &mut state, &menus);
        assert_eq!(state.open, Some(1));
    }

    #[test]
    fn clicking_an_entry_runs_its_command_and_dismisses_the_menu() {
        let menus = sample_menus();
        let mut state = MenuBarState::default();
        let mut input = UiInput::new();
        let titles = title_rects(BAR, &menus);

        click_at(
            &mut input,
            &mut state,
            &menus,
            titles[0].center_x(),
            titles[0].center_y(),
        );
        input.end_frame();

        // "Open ROM..." is the first entry of the File dropdown.
        let entry_y = BAR.bottom() + DROPDOWN_PAD_Y + ENTRY_HEIGHT / 2;
        input.set_pointer(titles[0].x + 20, entry_y);
        input.set_button(MouseButton::Left, true);
        frame(&input, &mut state, &menus);
        input.end_frame();

        input.set_button(MouseButton::Left, false);
        let response = frame(&input, &mut state, &menus);

        assert_eq!(response.command, Some(UiCommand::OpenRom));
        assert!(!state.is_open(), "activating an entry dismisses the menu");
    }

    #[test]
    fn clicking_a_disabled_entry_does_nothing() {
        let menus = sample_menus();
        let mut state = MenuBarState::default();
        let mut input = UiInput::new();
        let titles = title_rects(BAR, &menus);

        click_at(
            &mut input,
            &mut state,
            &menus,
            titles[0].center_x(),
            titles[0].center_y(),
        );
        input.end_frame();

        // Entry 2 is the disabled "Reload ROM", after entry 0 and a separator.
        let entry_y =
            BAR.bottom() + DROPDOWN_PAD_Y + ENTRY_HEIGHT + SEPARATOR_HEIGHT + ENTRY_HEIGHT / 2;
        input.set_pointer(titles[0].x + 20, entry_y);
        input.set_button(MouseButton::Left, true);
        frame(&input, &mut state, &menus);
        input.end_frame();
        input.set_button(MouseButton::Left, false);
        let response = frame(&input, &mut state, &menus);

        assert_eq!(response.command, None);
        assert!(state.is_open(), "the menu stays open on a dead click");
    }

    #[test]
    fn hovering_a_submenu_parent_opens_it() {
        let menus = sample_menus();
        let mut state = MenuBarState::default();
        let mut input = UiInput::new();
        let titles = title_rects(BAR, &menus);

        click_at(
            &mut input,
            &mut state,
            &menus,
            titles[0].center_x(),
            titles[0].center_y(),
        );
        input.end_frame();

        // Entry 3 is the "Save State" submenu.
        let entry_y =
            BAR.bottom() + DROPDOWN_PAD_Y + ENTRY_HEIGHT * 2 + SEPARATOR_HEIGHT + ENTRY_HEIGHT / 2;
        input.set_pointer(titles[0].x + 20, entry_y);
        frame(&input, &mut state, &menus);

        assert_eq!(state.path, vec![3]);
    }

    #[test]
    fn pressing_outside_the_menu_dismisses_it() {
        let menus = sample_menus();
        let mut state = MenuBarState::default();
        let mut input = UiInput::new();
        let titles = title_rects(BAR, &menus);

        click_at(
            &mut input,
            &mut state,
            &menus,
            titles[0].center_x(),
            titles[0].center_y(),
        );
        input.end_frame();

        input.set_pointer(700, 500);
        input.set_button(MouseButton::Left, true);
        frame(&input, &mut state, &menus);
        assert!(!state.is_open());
    }

    #[test]
    fn a_closed_menu_does_not_swallow_clicks_meant_for_the_game() {
        let menus = sample_menus();
        let mut state = MenuBarState::default();
        let mut input = UiInput::new();

        input.set_pointer(400, 400);
        input.set_button(MouseButton::Left, true);
        let response = frame(&input, &mut state, &menus);

        assert!(!response.captured_pointer);
        assert!(!state.is_open());
    }

    #[test]
    fn keys_are_not_claimed_while_the_menu_is_closed() {
        let menus = sample_menus();
        let mut state = MenuBarState::default();
        // Otherwise every game input would be swallowed by a closed menu.
        for key in [UiKey::Down, UiKey::Enter, UiKey::Escape, UiKey::Char('z')] {
            assert!(!handle_key(&mut state, &menus, key).handled, "{key:?}");
        }
    }
}
