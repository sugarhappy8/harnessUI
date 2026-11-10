//! Which window a pointer event reaches, worked out from a front-to-back window list
//! (macOS, where `CGWindowListCopyWindowInfo` is the only window list and it cannot see
//! which windows let clicks through). Pure logic, compiled and tested on every OS.
//!
//! The window list knows neither `ignoresMouseEvents` nor which transparent pixels let a
//! click through, so the first window under a point is not always the one that receives
//! the click. That matters for one case above all: another application's click-through
//! overlay (a screen recorder's border, a dimmer, a presentation highlighter) lying over
//! this app's window would make every point look like that application while the clicks
//! land on this app, its approval cards included. So when this app's window lies under
//! the point beneath a window that may let clicks through (translucent, or at an overlay
//! level such as the status, help, dragging or screen-saver level), the point counts as
//! this app's and the input is refused. Ordinary windows (layer 0, opaque), floating and
//! modal panels, the Dock and pop-up menus receive their own clicks and are trusted.
//!
//! What remains: a borderless window of another application at layer 0 with a clear
//! background lets clicks through its fully transparent pixels, which no list shows.

use crate::geometry::{GlobalPoint, Rect};

/// `kCGMainMenuWindowLevel`.
pub const MENU_BAR_LAYER: i64 = 24;
/// `kCGStatusWindowLevel`: from here up, levels used by overlays.
const STATUS_LAYER: i64 = 25;
/// `kCGPopUpMenuWindowLevel`: menus receive their own clicks.
const POP_UP_MENU_LAYER: i64 = 101;

/// One on-screen window, as the window list reports it.
#[derive(Debug, Clone)]
pub struct ListedWindow {
    pub number: u32,
    pub pid: u32,
    pub layer: i64,
    pub alpha: f64,
    pub bounds: Rect,
    pub owner: String,
    /// The window's title (read on macOS only).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub name: Option<String>,
}

/// Where a pointer event at a point goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// The window at this index of the list.
    Window(usize),
    /// The menu bar, which shows the menus of the application in front.
    MenuBar,
    /// No window: the desktop.
    Desktop,
    /// This app's window at this index lies under the point, beneath a window that may
    /// let the click through to it.
    SelfBeneath(usize),
}

/// A window of another application that may let a click through to what is beneath it.
fn may_pass_clicks(w: &ListedWindow) -> bool {
    w.alpha < 1.0 || (w.layer >= STATUS_LAYER && w.layer != POP_UP_MENU_LAYER)
}

/// The window a pointer event at `p` reaches. `windows` is front to back; `is_mine` says
/// whether a pid is this app or one of its children; `click_through` lists this app's own
/// windows that ignore the mouse (the overlay), which are looked through.
pub fn hit_test(
    windows: &[ListedWindow],
    p: GlobalPoint,
    is_mine: impl Fn(u32) -> bool,
    click_through: &[u64],
) -> Hit {
    let mut under = windows.iter().enumerate().filter(|(_, w)| {
        w.alpha > 0.0 && !click_through.contains(&u64::from(w.number)) && w.bounds.contains(p)
    });
    let Some((i, top)) = under.next() else {
        return Hit::Desktop;
    };
    if top.layer == MENU_BAR_LAYER && top.owner == "Window Server" {
        return Hit::MenuBar;
    }
    if !is_mine(top.pid)
        && may_pass_clicks(top)
        && let Some((j, _)) = under.find(|(_, w)| is_mine(w.pid))
    {
        return Hit::SelfBeneath(j);
    }
    Hit::Window(i)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: u32 = 10;

    fn w(number: u32, pid: u32, layer: i64, alpha: f64, bounds: Rect) -> ListedWindow {
        ListedWindow {
            number,
            pid,
            layer,
            alpha,
            bounds,
            owner: format!("app{pid}"),
            name: None,
        }
    }

    fn mine(pid: u32) -> bool {
        pid == ME
    }

    fn full() -> Rect {
        Rect::new(0.0, 0.0, 1000.0, 800.0)
    }

    fn p(x: f64, y: f64) -> GlobalPoint {
        GlobalPoint::new(x, y)
    }

    #[test]
    fn a_click_through_overlay_over_this_app_does_not_hide_it() {
        // A screen recorder's full-screen border at the screen-saver level, this app's
        // window beneath it on the left, a browser on the right.
        let list = vec![
            w(1, 77, 1000, 1.0, full()),
            w(2, ME, 0, 1.0, Rect::new(0.0, 0.0, 400.0, 800.0)),
            w(3, 50, 0, 1.0, full()),
        ];
        assert_eq!(
            hit_test(&list, p(100.0, 100.0), mine, &[]),
            Hit::SelfBeneath(1)
        );
        // Where this app's window is not, the overlay is reported (D7 then asks).
        assert_eq!(hit_test(&list, p(700.0, 100.0), mine, &[]), Hit::Window(0));
        // A translucent window at layer 0 over this app counts the same.
        let list = vec![
            w(1, 77, 0, 0.6, full()),
            w(2, ME, 0, 1.0, Rect::new(0.0, 0.0, 400.0, 800.0)),
        ];
        assert_eq!(
            hit_test(&list, p(100.0, 100.0), mine, &[]),
            Hit::SelfBeneath(1)
        );
    }

    #[test]
    fn ordinary_windows_panels_menus_and_the_dock_receive_their_own_clicks() {
        for layer in [0, 3, 8, 20, POP_UP_MENU_LAYER] {
            let list = vec![
                w(1, 50, layer, 1.0, Rect::new(0.0, 0.0, 300.0, 300.0)),
                w(2, ME, 0, 1.0, full()),
            ];
            assert_eq!(
                hit_test(&list, p(100.0, 100.0), mine, &[]),
                Hit::Window(0),
                "layer {layer}"
            );
            // Next to it, this app's own window is hit directly.
            assert_eq!(
                hit_test(&list, p(500.0, 100.0), mine, &[]),
                Hit::Window(1),
                "layer {layer}"
            );
        }
    }

    #[test]
    fn menu_bar_desktop_invisible_and_click_through_windows() {
        let mut bar = w(1, 0, MENU_BAR_LAYER, 1.0, Rect::new(0.0, 0.0, 1000.0, 24.0));
        bar.owner = "Window Server".into();
        let list = vec![
            bar,
            w(2, 77, 0, 0.0, full()),
            w(3, ME, 2000, 1.0, full()),
            w(4, 50, 0, 1.0, Rect::new(0.0, 0.0, 500.0, 500.0)),
        ];
        assert_eq!(hit_test(&list, p(10.0, 10.0), mine, &[]), Hit::MenuBar);
        // The invisible window is skipped; this app's overlay (3) is looked through only
        // when registered as click-through.
        assert_eq!(hit_test(&list, p(100.0, 100.0), mine, &[]), Hit::Window(2));
        assert_eq!(hit_test(&list, p(100.0, 100.0), mine, &[3]), Hit::Window(3));
        assert_eq!(hit_test(&list, p(700.0, 700.0), mine, &[3]), Hit::Desktop);
    }
}
