//! X11 side of the overlay (Linux).
//!
//! Works on X11 and on Wayland sessions through XWayland (EVE runs under Wine
//! or Proton, which use X11 either way). Everything goes through the X11
//! protocol, in pure Rust (`x11rb`); no X library is needed at run time.

use super::{GameWindow, GameWindowTracker, OverlayError, ScreenRect};
use x11rb::connection::Connection;
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageEvent, ClipOrdering, ConnectionExt as _, EventMask, MapState,
    Rectangle, Window,
};
use x11rb::rust_connection::RustConnection;

/// A connection to the X server and its root window.
pub(super) struct X11Session {
    conn: RustConnection,
    root: Window,
}

impl X11Session {
    /// `_NET_WM_STATE` client message: add the property.
    const STATE_ADD: u32 = 1;
    /// The sender is a normal application (EWMH "source indication").
    const SOURCE_APPLICATION: u32 = 1;

    /// Connects to the server named by `DISPLAY`.
    pub(super) fn connect() -> Result<Self, OverlayError> {
        let (conn, screen) = RustConnection::connect(None).map_err(Self::os_error)?;
        let root = conn.setup().roots[screen].root;
        Ok(Self { conn, root })
    }

    fn os_error(error: impl std::fmt::Display) -> OverlayError {
        OverlayError::Os(error.to_string())
    }

    fn atom(&self, name: &[u8]) -> Result<Atom, OverlayError> {
        Ok(self
            .conn
            .intern_atom(false, name)
            .map_err(Self::os_error)?
            .reply()
            .map_err(Self::os_error)?
            .atom)
    }

    /// The managed windows, in the window manager's list (`_NET_CLIENT_LIST`).
    fn client_list(&self) -> Result<Vec<Window>, OverlayError> {
        let property = self.atom(b"_NET_CLIENT_LIST")?;
        let reply = self
            .conn
            .get_property(false, self.root, property, AtomEnum::WINDOW, 0, u32::MAX)
            .map_err(Self::os_error)?
            .reply()
            .map_err(Self::os_error)?;
        Ok(reply
            .value32()
            .map(|windows| windows.collect())
            .unwrap_or_default())
    }

    /// The window with the keyboard focus (`_NET_ACTIVE_WINDOW`).
    fn active_window(&self) -> Option<Window> {
        let property = self.atom(b"_NET_ACTIVE_WINDOW").ok()?;
        let reply = self
            .conn
            .get_property(false, self.root, property, AtomEnum::WINDOW, 0, 1)
            .ok()?
            .reply()
            .ok()?;
        reply.value32()?.next()
    }

    /// The window's title (`_NET_WM_NAME`, or the old `WM_NAME`).
    fn window_title(&self, window: Window) -> Option<String> {
        let utf8 = self.atom(b"UTF8_STRING").ok()?;
        let net_name = self.atom(b"_NET_WM_NAME").ok()?;
        let reply = self
            .conn
            .get_property(false, window, net_name, utf8, 0, 1024)
            .ok()?
            .reply()
            .ok()?;
        if !reply.value.is_empty() {
            return Some(String::from_utf8_lossy(&reply.value).into_owned());
        }
        let reply = self
            .conn
            .get_property(false, window, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 1024)
            .ok()?
            .reply()
            .ok()?;
        (!reply.value.is_empty()).then(|| String::from_utf8_lossy(&reply.value).into_owned())
    }

    /// The window's drawable area in screen coordinates, if it is on screen.
    fn window_rect(&self, window: Window) -> Option<ScreenRect> {
        let attributes = self.conn.get_window_attributes(window).ok()?.reply().ok()?;
        if attributes.map_state != MapState::VIEWABLE {
            return None;
        }
        let geometry = self.conn.get_geometry(window).ok()?.reply().ok()?;
        // the geometry's x/y are relative to the parent (the decorations the
        // window manager adds); translating (0, 0) gives the screen position
        let origin = self
            .conn
            .translate_coordinates(window, self.root, 0, 0)
            .ok()?
            .reply()
            .ok()?;
        let rect = ScreenRect::new(
            i32::from(origin.dst_x),
            i32::from(origin.dst_y),
            u32::from(geometry.width),
            u32::from(geometry.height),
        );
        (!rect.is_empty()).then_some(rect)
    }

    /// The first visible game window.
    pub(super) fn locate(
        &self,
        tracker: &GameWindowTracker,
    ) -> Result<Option<GameWindow>, OverlayError> {
        let active = self.active_window();
        for window in self.client_list()? {
            let Some(title) = self.window_title(window) else {
                continue;
            };
            if !tracker.matches_title(&title) {
                continue;
            }
            if let Some(rect) = self.window_rect(window) {
                return Ok(Some(GameWindow {
                    rect,
                    focused: active == Some(window),
                }));
            }
        }
        Ok(None)
    }

    /// Applies [`Self::apply`] to the managed window titled `title`.
    pub(super) fn apply_by_title(
        &self,
        title: &str,
        click_through: bool,
        input_rect: Option<ScreenRect>,
    ) -> Result<(), OverlayError> {
        for window in self.client_list()? {
            if self.window_title(window).as_deref() == Some(title) {
                return self.apply(window, click_through, input_rect);
            }
        }
        Err(OverlayError::InvalidHandle)
    }

    /// Shows only `rects` (window-relative pixels) of the managed window
    /// titled `title`: the rest is cut away with the Shape extension, so it is
    /// neither drawn nor clickable. This needs no compositor transparency.
    pub(super) fn set_visible_region_by_title(
        &self,
        title: &str,
        rects: &[ScreenRect],
    ) -> Result<(), OverlayError> {
        let rectangles: Vec<x11rb::protocol::xproto::Rectangle> = rects
            .iter()
            .map(|rect| x11rb::protocol::xproto::Rectangle {
                x: rect.x as i16,
                y: rect.y as i16,
                width: rect.width as u16,
                height: rect.height as u16,
            })
            .collect();
        for window in self.client_list()? {
            if self.window_title(window).as_deref() == Some(title) {
                self.conn
                    .shape_rectangles(
                        shape::SO::SET,
                        shape::SK::BOUNDING,
                        ClipOrdering::UNSORTED,
                        window,
                        0,
                        0,
                        &rectangles,
                    )
                    .map_err(Self::os_error)?
                    .check()
                    .map_err(Self::os_error)?;
                self.conn.flush().map_err(Self::os_error)?;
                return Ok(());
            }
        }
        Err(OverlayError::InvalidHandle)
    }

    /// Sends a `_NET_WM_STATE` request to the window manager for `window`.
    fn add_state(
        &self,
        window: Window,
        first: &[u8],
        second: Option<&[u8]>,
    ) -> Result<(), OverlayError> {
        let message_type = self.atom(b"_NET_WM_STATE")?;
        let first = self.atom(first)?;
        let second = match second {
            Some(name) => self.atom(name)?,
            None => 0,
        };
        let event = ClientMessageEvent::new(
            32,
            window,
            message_type,
            [Self::STATE_ADD, first, second, Self::SOURCE_APPLICATION, 0],
        );
        self.conn
            .send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                event,
            )
            .map_err(Self::os_error)?;
        Ok(())
    }

    /// Above the other windows, out of the taskbar and pager, and (if
    /// `click_through`) transparent to the mouse, except for `input_rect`
    /// (window-relative pixels), which keeps taking it.
    pub(super) fn apply(
        &self,
        window: Window,
        click_through: bool,
        input_rect: Option<ScreenRect>,
    ) -> Result<(), OverlayError> {
        self.add_state(
            window,
            b"_NET_WM_STATE_ABOVE",
            Some(b"_NET_WM_STATE_SKIP_TASKBAR"),
        )?;
        self.add_state(window, b"_NET_WM_STATE_SKIP_PAGER", None)?;
        if click_through {
            // an input region with nothing but the button (or empty): every
            // other click goes to the window below
            let button: Vec<Rectangle> = input_rect
                .filter(|rect| !rect.is_empty())
                .map(|rect| Rectangle {
                    x: rect.x as i16,
                    y: rect.y as i16,
                    width: rect.width as u16,
                    height: rect.height as u16,
                })
                .into_iter()
                .collect();
            self.conn
                .shape_rectangles(
                    shape::SO::SET,
                    shape::SK::INPUT,
                    ClipOrdering::UNSORTED,
                    window,
                    0,
                    0,
                    &button,
                )
                .map_err(Self::os_error)?
                .check()
                .map_err(Self::os_error)?;
        } else {
            // no mask: back to the default input region (the whole window)
            self.conn
                .shape_mask(shape::SO::SET, shape::SK::INPUT, window, 0, 0, x11rb::NONE)
                .map_err(Self::os_error)?
                .check()
                .map_err(Self::os_error)?;
        }
        self.conn.flush().map_err(Self::os_error)?;
        Ok(())
    }
}
