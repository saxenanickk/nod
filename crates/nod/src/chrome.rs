//! Window-chrome helpers shared by the top bars (workspace nav + session
//! headers): the macOS traffic-light inset and a draggable spacer, so the
//! header never collides with the window controls and empty header space moves
//! the window like a native title bar.

use gpui::{MouseButton, Pixels, Window, div, prelude::*, px};

/// Height of the top bars, matching gpui-component's title bar.
pub const BAR_HEIGHT: f32 = 38.0;

/// Left padding a top-of-window bar needs to clear the macOS traffic lights.
/// The lights hide in fullscreen, so only a small gutter is needed there.
pub fn traffic_light_pad(window: &Window) -> Pixels {
    #[cfg(target_os = "macos")]
    {
        if window.is_fullscreen() {
            px(12.)
        } else {
            px(78.)
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        px(12.)
    }
}

/// A flex-filling, draggable region for the empty part of a title bar. Clicking
/// and dragging it moves the window, like a native title bar's blank space.
pub fn drag_region() -> gpui::Stateful<gpui::Div> {
    div()
        .id("titlebar-drag")
        .flex_1()
        .h_full()
        .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
}
