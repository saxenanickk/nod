//! Applies a Zed-flavored One Dark palette over gpui-component's default
//! theme, so prdesk reads like Zed rather than a generic shadcn dark theme.

use gpui::{App, Hsla, Rgba, px, rgb};
use gpui_component::{Theme, ThemeMode};

// One Dark-ish palette (Zed's default dark).
const BG: u32 = 0x282c33;
const SURFACE: u32 = 0x2f343e; // panels, secondary buttons, elevated rows
const ELEVATED: u32 = 0x353b45; // popovers, hovered surfaces
const BORDER: u32 = 0x464b57;
const TEXT: u32 = 0xc8ccd4;
const MUTED: u32 = 0x828997;
const ACCENT_BLUE: u32 = 0x74ade8;
const GREEN: u32 = 0xa1c181;
const RED: u32 = 0xd07277;
const YELLOW: u32 = 0xdfc184;

fn h(color: u32) -> Hsla {
    Rgba::from(rgb(color)).into()
}

/// Call after `gpui_component::init` and `Theme::change(Dark)`.
pub fn apply(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    let theme = Theme::global_mut(cx);

    // Typography and shape: tighter radius, monospace matches the diff.
    theme.radius = px(4.);
    theme.radius_lg = px(6.);
    theme.mono_font_family = "Menlo".into();
    theme.shadow = false;

    let c = &mut theme.colors;
    c.background = h(BG);
    c.foreground = h(TEXT);
    c.border = h(BORDER);
    c.drag_border = h(ACCENT_BLUE);

    // Surfaces.
    c.secondary = h(SURFACE);
    c.secondary_hover = h(ELEVATED);
    c.secondary_active = h(ELEVATED);
    c.secondary_foreground = h(TEXT);
    c.popover = h(SURFACE);
    c.popover_foreground = h(TEXT);
    c.input = h(SURFACE);
    c.accordion = h(SURFACE);
    c.accordion_hover = h(ELEVATED);

    // Accents / interactive.
    c.accent = h(ELEVATED);
    c.accent_foreground = h(TEXT);
    c.primary = h(ACCENT_BLUE);
    c.primary_hover = h(0x81b6ea);
    c.primary_active = h(0x679fdc);
    c.primary_foreground = h(0x14181f);
    c.ring = h(ACCENT_BLUE);
    c.selection = Hsla { a: 0.28, ..h(ACCENT_BLUE) };
    c.link = h(ACCENT_BLUE);
    c.caret = h(ACCENT_BLUE);

    // Muted / secondary text.
    c.muted = h(SURFACE);
    c.muted_foreground = h(MUTED);

    // Title bar: a hair above the background so the top bar reads as chrome.
    c.title_bar = h(0x2c313a);
    c.title_bar_border = h(BORDER);

    // Lists / sidebar.
    c.list = h(BG);
    c.list_hover = h(SURFACE);
    c.list_active = h(ELEVATED);
    c.list_active_border = h(ACCENT_BLUE);
    c.sidebar = h(BG);
    c.sidebar_foreground = h(TEXT);
    c.sidebar_border = h(BORDER);
    c.sidebar_accent = h(ELEVATED);

    // Status colors.
    c.danger = h(RED);
    c.danger_hover = h(0xd97e83);
    c.danger_active = h(0xc76166);
    c.danger_foreground = h(0x14181f);
    c.success = h(GREEN);
    c.success_foreground = h(0x14181f);
    c.warning = h(YELLOW);
    c.warning_foreground = h(0x14181f);
    c.info = h(ACCENT_BLUE);

    // Scrollbar.
    c.scrollbar = h(BG);
    c.scrollbar_thumb = h(BORDER);
    c.scrollbar_thumb_hover = h(MUTED);
}
