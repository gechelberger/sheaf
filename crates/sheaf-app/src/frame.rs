//! Window frame: a title bar with drag-to-move and minimize / maximize /
//! close controls, plus client-side decorations (shadow, border, rounded
//! corners, resize edges) when the window system leaves that to us — as
//! Wayland compositors such as WSLg's do. Adapted from GPUI's
//! `window_shadow` example and Zed's workspace frame.

use gpui::{
    AnyElement, App, Bounds, CursorStyle, Decorations, HitboxBehavior, Hsla, MouseButton, Pixels,
    Point, ResizeEdge, SharedString, Size, Stateful, Styled, Tiling, Window, WindowControlArea,
    canvas, div, point, prelude::*, px, size, transparent_black,
};
use sheaf_ui::theme;

pub const TITLE_H: f32 = 34.0;
const SHADOW: Pixels = px(10.0);
const ROUNDING: Pixels = px(8.0);
const BORDER: Pixels = px(1.0);

/// The platform draws its own controls on macOS (traffic lights); everywhere
/// else we draw them.
pub fn draws_controls() -> bool {
    !cfg!(target_os = "macos")
}

/// Where there's no system menu bar, we draw one in the title bar.
pub fn draws_menu_bar() -> bool {
    !cfg!(target_os = "macos")
}

/// A non-interactive title-bar region with native caption behavior on Windows.
fn caption_region() -> gpui::Div {
    div()
        .h_full()
        .flex()
        .items_center()
        .when(cfg!(target_os = "windows"), |d| {
            d.window_control_area(WindowControlArea::Drag)
        })
}

/// The title bar. `menu_bar` follows the app name; `title` is shown centered; the whole bar moves the
/// window, double-click toggles maximize, right-click opens the window menu.
pub fn title_bar(
    title: SharedString,
    menu_bar: Option<AnyElement>,
    on_close: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    window: &Window,
    cx: &mut App,
) -> AnyElement {
    let controls = window.window_controls();
    let maximized = window.is_maximized();
    let button = |id: &'static str, label: &'static str, danger: bool| {
        div()
            .id(id)
            .occlude()
            .w(px(46.0))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .text_sm()
            .text_color(theme::TEXT_SUBTLE)
            .cursor_pointer()
            .hover(move |s| {
                if danger {
                    s.bg(theme::CLOSE_HOVER).text_color(theme::TEXT_BRIGHT)
                } else {
                    s.bg(theme::HOVER)
                }
            })
            // Linux's parent mouse handler must not move the window when a
            // button is pressed. Windows needs the event to reach its native
            // window-control handlers instead.
            .when(!cfg!(target_os = "windows"), |button| {
                button.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            })
            .child(label)
    };
    let mut buttons = div().flex().h_full().items_stretch();
    if draws_controls() {
        if controls.minimize {
            buttons = buttons.child(button("win-min", "—", false).map(|button| {
                if cfg!(target_os = "windows") {
                    button.window_control_area(WindowControlArea::Min)
                } else {
                    button.on_click(|_, window, _| window.minimize_window())
                }
            }));
        }
        if controls.maximize {
            buttons = buttons.child(
                button("win-max", if maximized { "❐" } else { "▢" }, false).map(|button| {
                    if cfg!(target_os = "windows") {
                        // Windows' zoom() only maximizes. Native hit testing
                        // handles restore as well as the Snap Layouts menu.
                        button.window_control_area(WindowControlArea::Max)
                    } else {
                        button.on_click(|_, window, _| window.zoom_window())
                    }
                }),
            );
        }
        // Windows sends WM_CLOSE through the existing window_should_close
        // callback, which prompts for unsaved changes without relying on focus.
        // Elsewhere the caller closes directly: dispatching an action would
        // miss the workspace when focus is in the palette or menu bar.
        buttons = buttons.child(button("win-close", "✕", true).map(|button| {
            if cfg!(target_os = "windows") {
                button.window_control_area(WindowControlArea::Close)
            } else {
                button.on_click(on_close)
            }
        }));
    }
    let _ = cx;
    div()
        .id("title-bar")
        .h(px(TITLE_H))
        .flex_shrink_0()
        .flex()
        .items_center()
        .bg(theme::TITLE_BAR)
        .border_b_1()
        .border_color(theme::FRAME_BORDER)
        .when(!cfg!(target_os = "windows"), |bar| {
            bar.on_mouse_down(MouseButton::Left, |e, window, _| {
                if e.click_count == 2 {
                    window.zoom_window();
                } else {
                    window.start_window_move();
                }
            })
        })
        .on_click(|e, window, _| {
            if e.is_right_click()
                && let Some(pos) = e.mouse_position()
            {
                window.show_window_menu(pos);
            }
        })
        // Keep native caption regions separate from menus and buttons. Native
        // caption handling supplies dragging, double-click, and window menus.
        .child(
            caption_region()
                .pl_3()
                .child(crate::brand::mark(px(16.0), window)),
        )
        .child(
            caption_region()
                .pl_2()
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme::TEXT)
                .child("sheaf"),
        )
        .children(menu_bar.map(|m| div().h_full().pl_2().occlude().child(m)))
        .child(
            caption_region()
                .flex_1()
                .justify_center()
                .text_sm()
                .text_color(theme::TEXT_MUTED)
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(title),
        )
        .child(buttons)
        .into_any_element()
}

/// Wrap the window content in client-side decorations when the platform
/// asks for them; a no-op frame otherwise.
pub fn client_side_decorations(
    content: impl IntoElement,
    window: &mut Window,
    _cx: &mut App,
) -> Stateful<gpui::Div> {
    let decorations = window.window_decorations();
    let tiling = match decorations {
        Decorations::Server => Tiling::default(),
        Decorations::Client { tiling } => tiling,
    };
    match decorations {
        Decorations::Client { .. } => window.set_client_inset(SHADOW),
        Decorations::Server => window.set_client_inset(px(0.0)),
    }

    div()
        .id("window-frame")
        .size_full()
        .bg(transparent_black())
        .map(|d| match decorations {
            Decorations::Server => d,
            Decorations::Client { .. } => rounded(d, tiling)
                .when(!tiling.top, |d| d.pt(SHADOW))
                .when(!tiling.bottom, |d| d.pb(SHADOW))
                .when(!tiling.left, |d| d.pl(SHADOW))
                .when(!tiling.right, |d| d.pr(SHADOW))
                // Edge hit-testing: set the resize cursor while hovering the
                // shadow band, and start a resize on press.
                .child(
                    canvas(
                        |_, window, _| {
                            window.insert_hitbox(
                                Bounds::new(
                                    point(px(0.0), px(0.0)),
                                    window.window_bounds().get_bounds().size,
                                ),
                                HitboxBehavior::Normal,
                            )
                        },
                        move |_, hitbox, window, _| {
                            let Some(edge) = resize_edge(
                                window.mouse_position(),
                                window.window_bounds().get_bounds().size,
                                tiling,
                            ) else {
                                return;
                            };
                            window.set_cursor_style(
                                match edge {
                                    ResizeEdge::Top | ResizeEdge::Bottom => {
                                        CursorStyle::ResizeUpDown
                                    }
                                    ResizeEdge::Left | ResizeEdge::Right => {
                                        CursorStyle::ResizeLeftRight
                                    }
                                    ResizeEdge::TopLeft | ResizeEdge::BottomRight => {
                                        CursorStyle::ResizeUpLeftDownRight
                                    }
                                    ResizeEdge::TopRight | ResizeEdge::BottomLeft => {
                                        CursorStyle::ResizeUpRightDownLeft
                                    }
                                },
                                &hitbox,
                            );
                        },
                    )
                    .size_full()
                    .absolute(),
                )
                .on_mouse_move(|_, window, _| window.refresh())
                .on_mouse_down(MouseButton::Left, move |e, window, _| {
                    if let Some(edge) =
                        resize_edge(e.position, window.window_bounds().get_bounds().size, tiling)
                    {
                        window.start_window_resize(edge);
                    }
                }),
        })
        .child(
            div()
                .size_full()
                .overflow_hidden()
                .cursor(CursorStyle::Arrow)
                .map(|d| match decorations {
                    Decorations::Server => d,
                    Decorations::Client { .. } => rounded(d, tiling)
                        .border_color(theme::BORDER)
                        .when(!tiling.top, |d| d.border_t(BORDER))
                        .when(!tiling.bottom, |d| d.border_b(BORDER))
                        .when(!tiling.left, |d| d.border_l(BORDER))
                        .when(!tiling.right, |d| d.border_r(BORDER))
                        .when(!tiling.is_tiled(), |d| {
                            d.shadow(vec![
                                gpui::BoxShadow::new(
                                    px(0.),
                                    px(0.),
                                    Hsla {
                                        h: 0.,
                                        s: 0.,
                                        l: 0.,
                                        a: 0.4,
                                    },
                                )
                                .blur_radius(SHADOW / 2.),
                            ])
                        }),
                })
                // Inner content handles its own hover; don't let moves reach
                // the frame's refresh handler.
                .on_mouse_move(|_, _, cx| cx.stop_propagation())
                .child(content),
        )
}

fn resize_edge(pos: Point<Pixels>, win: Size<Pixels>, tiling: Tiling) -> Option<ResizeEdge> {
    let inner = Bounds::new(Point::default(), win).inset(SHADOW * 1.5);
    if inner.contains(&pos) {
        return None;
    }
    let corner = size(SHADOW * 1.5, SHADOW * 1.5);
    let at = |x: Pixels, y: Pixels| Bounds::new(point(x, y), corner).contains(&pos);
    if !tiling.top && !tiling.left && at(px(0.), px(0.)) {
        return Some(ResizeEdge::TopLeft);
    }
    if !tiling.top && !tiling.right && at(win.width - corner.width, px(0.)) {
        return Some(ResizeEdge::TopRight);
    }
    if !tiling.bottom && !tiling.left && at(px(0.), win.height - corner.height) {
        return Some(ResizeEdge::BottomLeft);
    }
    if !tiling.bottom && !tiling.right && at(win.width - corner.width, win.height - corner.height) {
        return Some(ResizeEdge::BottomRight);
    }
    if !tiling.top && pos.y < SHADOW {
        Some(ResizeEdge::Top)
    } else if !tiling.bottom && pos.y > win.height - SHADOW {
        Some(ResizeEdge::Bottom)
    } else if !tiling.left && pos.x < SHADOW {
        Some(ResizeEdge::Left)
    } else if !tiling.right && pos.x > win.width - SHADOW {
        Some(ResizeEdge::Right)
    } else {
        None
    }
}

/// Round the corners that aren't tiled against a screen edge.
fn rounded<T: Styled>(d: T, tiling: Tiling) -> T {
    let mut d = d;
    if !tiling.top && !tiling.left {
        d = d.rounded_tl(ROUNDING);
    }
    if !tiling.top && !tiling.right {
        d = d.rounded_tr(ROUNDING);
    }
    if !tiling.bottom && !tiling.left {
        d = d.rounded_bl(ROUNDING);
    }
    if !tiling.bottom && !tiling.right {
        d = d.rounded_br(ROUNDING);
    }
    d
}
