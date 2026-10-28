//! Native support for an overlay window drawn on top of the EVE Online client.
//!
//! The overlay is an ordinary window of Telescope (a transparent, borderless
//! egui viewport). Nothing is injected into the game and its memory is never
//! read: the code here only
//!
//! * makes that window sit above the game, ignore the mouse and stay out of
//!   the task switcher ([`OverlayWindow`]), and
//! * finds where the game window is on screen so the overlay can follow it
//!   ([`GameWindowTracker`]).
//!
//! Windows, macOS and Linux (X11, also through XWayland) are supported. On a
//! native Wayland window, [`OverlayWindow::apply`] reports
//! [`OverlayError::Unsupported`]: Wayland has no "always on top" for normal
//! windows and a program cannot see other programs' windows. Run Telescope with
//! X11 there (`WINIT_UNIX_BACKEND=x11`).

use raw_window_handle::HasWindowHandle;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use raw_window_handle::RawWindowHandle;

#[cfg(target_os = "linux")]
mod x11;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "windows")]
use windows::{
    Win32::{
        Foundation::{COLORREF, HWND, LPARAM, POINT, RECT},
        Graphics::Gdi::{
            ClientToScreen, CombineRgn, CreateRectRgn, DeleteObject, RGN_OR, SetWindowRgn,
        },
        UI::Shell::{QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState},
        UI::WindowsAndMessaging::{
            EnumWindows, FindWindowW, GWL_EXSTYLE, GetClientRect, GetForegroundWindow,
            GetWindowLongPtrW, GetWindowTextW, HWND_TOPMOST, IsWindowVisible, LWA_ALPHA,
            SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetLayeredWindowAttributes, SetWindowLongPtrW,
            SetWindowPos, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
        },
    },
    core::{BOOL, HSTRING, PCWSTR},
};

#[cfg(target_os = "macos")]
use objc2_app_kit::{NSApplication, NSColor, NSView, NSWindow, NSWindowCollectionBehavior};
#[cfg(target_os = "macos")]
use objc2_foundation::MainThreadMarker;

/// Why the overlay could not be set up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayError {
    /// This operating system has no implementation.
    Unsupported,
    /// The window handle is not the kind this system uses (or is not
    /// available yet).
    InvalidHandle,
    /// The system refused the call.
    Os(String),
}

impl std::fmt::Display for OverlayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => write!(f, "overlays are not supported on this system"),
            Self::InvalidHandle => write!(f, "the window handle is not usable"),
            Self::Os(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for OverlayError {}

/// A rectangle in screen pixels (the system's own coordinates; the same
/// space the overlay window is positioned in).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScreenRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl ScreenRect {
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// `true` when the rectangle has no area (a minimised window reports
    /// one).
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// Where the game window is, and whether it is the active window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameWindow {
    /// The drawable (client) area of the game window.
    pub rect: ScreenRect,
    /// The game has the keyboard focus.
    pub focused: bool,
}

/// Finds the game window.
///
/// Only the window's position and size are read: no pixels, no process
/// memory.
#[derive(Debug, Clone)]
pub struct GameWindowTracker {
    title_prefix: String,
    process_names: Vec<String>,
}

impl GameWindowTracker {
    /// Title prefix of the EVE client windows (`EVE - <character>`).
    pub const DEFAULT_TITLE_PREFIX: &'static str = "EVE";

    /// A tracker for windows whose title starts with `title_prefix`.
    /// Names of the game's process on macOS, where window titles are not
    /// readable without a permission. Change them with
    /// [`Self::with_process_names`] if the client is called something else.
    pub const DEFAULT_PROCESS_NAMES: [&'static str; 1] = ["EVE Online"];

    pub fn new(title_prefix: impl Into<String>) -> Self {
        Self {
            title_prefix: title_prefix.into(),
            process_names: Self::DEFAULT_PROCESS_NAMES
                .iter()
                .map(|name| name.to_string())
                .collect(),
        }
    }

    /// The names the game's process can have (macOS).
    pub fn with_process_names(mut self, names: impl IntoIterator<Item = String>) -> Self {
        self.process_names = names.into_iter().collect();
        self
    }

    /// `true` when `name` is the name of the game's process (macOS).
    pub fn matches_process(&self, name: &str) -> bool {
        self.process_names.iter().any(|candidate| candidate == name)
    }

    /// `true` when `title` belongs to a game window: the prefix alone, or
    /// followed by `" - "` (`EVE - Character`). `EVE Launcher`, a browser tab
    /// called `EVEmap`... do not match.
    pub fn matches_title(&self, title: &str) -> bool {
        match title.strip_prefix(self.title_prefix.as_str()) {
            Some("") => true,
            Some(rest) => rest.starts_with(" - "),
            None => false,
        }
    }

    /// The first visible, non-empty game window, or `None` when the game is
    /// not running (or not visible, or the system is not supported).
    #[allow(unsafe_code)]
    #[cfg(target_os = "windows")]
    pub fn locate(&self) -> Option<GameWindow> {
        let mut search = WindowSearch {
            tracker: self,
            found: None,
        };
        // SAFETY: `search` outlives the call and `visit` is the only reader
        // of the pointer; `EnumWindows` is synchronous.
        unsafe {
            let _ = EnumWindows(
                Some(WindowSearch::visit),
                LPARAM(&mut search as *mut WindowSearch as isize),
            );
        }
        search.found
    }

    /// The first visible game window. It connects to the X server on every
    /// call: ask a few times a second, not every frame.
    #[cfg(target_os = "linux")]
    pub fn locate(&self) -> Option<GameWindow> {
        x11::X11Session::connect().ok()?.locate(self).ok().flatten()
    }

    /// The game's frontmost window and whether the game is the frontmost
    /// application. No permission is needed (see `macos.rs`).
    #[cfg(target_os = "macos")]
    pub fn locate(&self) -> Option<GameWindow> {
        macos::MacWindowList::locate(self)
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    pub fn locate(&self) -> Option<GameWindow> {
        None
    }
}

impl Default for GameWindowTracker {
    fn default() -> Self {
        Self::new(Self::DEFAULT_TITLE_PREFIX)
    }
}

/// State of one `EnumWindows` pass. `visit` is an associated function (not a
/// free one) because the callback has to be an `extern "system"` pointer, which
/// a closure cannot be.
#[cfg(target_os = "windows")]
struct WindowSearch<'a> {
    tracker: &'a GameWindowTracker,
    found: Option<GameWindow>,
}

#[cfg(target_os = "windows")]
impl WindowSearch<'_> {
    #[allow(unsafe_code)]
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the `&mut WindowSearch` passed by `locate`.
        let search = unsafe { &mut *(lparam.0 as *mut WindowSearch) };
        if search.inspect(hwnd) {
            // keep enumerating
            BOOL(1)
        } else {
            BOOL(0)
        }
    }

    /// Looks at one window. Returns `false` once the game window is found.
    #[allow(unsafe_code)]
    fn inspect(&mut self, hwnd: HWND) -> bool {
        // SAFETY: plain Win32 queries on a handle `EnumWindows` just gave.
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() {
                return true;
            }
            let mut buffer = [0u16; 256];
            let length = GetWindowTextW(hwnd, &mut buffer);
            if length <= 0 {
                return true;
            }
            let title = String::from_utf16_lossy(&buffer[..length as usize]);
            if !self.tracker.matches_title(&title) {
                return true;
            }
            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client).is_err() {
                return true;
            }
            // the client area, in screen coordinates (no title bar / borders)
            let mut origin = POINT { x: 0, y: 0 };
            if !ClientToScreen(hwnd, &mut origin).as_bool() {
                return true;
            }
            let rect = ScreenRect::new(
                origin.x,
                origin.y,
                (client.right - client.left).max(0) as u32,
                (client.bottom - client.top).max(0) as u32,
            );
            if rect.is_empty() {
                // minimised
                return true;
            }
            self.found = Some(GameWindow {
                rect,
                focused: GetForegroundWindow() == hwnd,
            });
            false
        }
    }
}

/// The corner (or the centre) of the game window that an overlay element keeps
/// its distance from. EVE's own interface docks to the window's edges, so an
/// indicator placed near the right edge has to stay near the right edge when
/// the game window is resized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Anchor {
    #[default]
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Center,
}

impl Anchor {
    fn is_right(self) -> bool {
        matches!(self, Self::TopRight | Self::BottomRight)
    }

    fn is_bottom(self) -> bool {
        matches!(self, Self::BottomLeft | Self::BottomRight)
    }

    fn is_center(self) -> bool {
        self == Self::Center
    }

    /// The corner of `game` closest to the centre of `rect` (screen
    /// coordinates).
    pub fn nearest(game: &ScreenRect, rect: &ScreenRect) -> Self {
        let center_x = i64::from(rect.x) + i64::from(rect.width) / 2 - i64::from(game.x);
        let center_y = i64::from(rect.y) + i64::from(rect.height) / 2 - i64::from(game.y);
        let right = center_x * 2 > i64::from(game.width);
        let bottom = center_y * 2 > i64::from(game.height);
        match (right, bottom) {
            (false, false) => Self::TopLeft,
            (true, false) => Self::TopRight,
            (false, true) => Self::BottomLeft,
            (true, true) => Self::BottomRight,
        }
    }
}

/// Where an overlay element sits in the game window: its distance from an
/// [`Anchor`], so it keeps its place when the game is moved or resized.
///
/// The element is kept inside the game window. The caller stores the values
/// (see [`Self::parts`] and [`Self::reference_height`]) and gives them back to
/// [`Self::new`].
///
/// # Different screens
///
/// By default the distances and the size are real pixels, which is how EVE's
/// own interface behaves (a bigger resolution gives more room, not bigger
/// panels). After [`Self::calibrate`] they are *reference* pixels, measured on
/// the game window as it was at that moment, and are scaled by
/// `game height / reference height`: an indicator keeps the same proportion of
/// the window on a 720p laptop and on a 4K monitor. There is no fixed
/// reference resolution: each placement remembers the window it was made on
/// (call `calibrate` when the user places it, and save
/// [`Self::reference_height`] with the rest).
///
/// The rectangles are in the units of the system's window coordinates: pixels
/// on Windows and Linux, points on macOS. The UI converts them to its own
/// units (egui's `pixels_per_point`) before positioning the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayPlacement {
    anchor: Anchor,
    offset_x: i32,
    offset_y: i32,
    width: u32,
    height: u32,
    reference_height: Option<u32>,
}

impl OverlayPlacement {
    /// An element of `width` x `height` pixels, `offset_x`/`offset_y` pixels
    /// from `anchor`: inwards from a corner, or from the centre (then they may
    /// be negative).
    pub fn new(anchor: Anchor, offset_x: i32, offset_y: i32, width: u32, height: u32) -> Self {
        Self {
            anchor,
            offset_x,
            offset_y,
            width,
            height,
            reference_height: None,
        }
    }

    /// Restores a saved reference height (see [`Self::calibrate`]): the values
    /// are *reference* pixels of a game window `height` pixels tall, scaled to
    /// the real window. `None` (or `0`) turns the scaling off.
    pub fn with_reference_height(mut self, height: Option<u32>) -> Self {
        self.reference_height = height.filter(|height| *height > 0);
        self
    }

    /// Takes the current height of the `game` window as the reference: the
    /// element looks exactly as it does now, and from here on it grows or
    /// shrinks with the window's height. Call it when the user places the
    /// element (and again to move it to the window of another screen); it is
    /// harmless to call it more than once.
    pub fn calibrate(&mut self, game: &ScreenRect) {
        if game.height == 0 {
            return;
        }
        // the values in real pixels for this window, then they become the
        // reference ones
        let factor = self.factor(game);
        self.offset_x = Self::scaled(self.offset_x, factor);
        self.offset_y = Self::scaled(self.offset_y, factor);
        self.width = Self::scaled_size(self.width, factor);
        self.height = Self::scaled_size(self.height, factor);
        self.reference_height = Some(game.height);
    }

    /// The reference height, if the placement scales with the window.
    pub fn reference_height(&self) -> Option<u32> {
        self.reference_height
    }

    /// `(anchor, offset_x, offset_y, width, height)`, to be saved.
    pub fn parts(&self) -> (Anchor, i32, i32, u32, u32) {
        (
            self.anchor,
            self.offset_x,
            self.offset_y,
            self.width,
            self.height,
        )
    }

    /// Real pixels per reference pixel in `game` (1 when not scaling).
    fn factor(&self, game: &ScreenRect) -> f64 {
        match self.reference_height {
            Some(reference) if game.height > 0 => f64::from(game.height) / f64::from(reference),
            _ => 1.0,
        }
    }

    fn scaled(value: i32, factor: f64) -> i32 {
        (f64::from(value) * factor).round() as i32
    }

    fn unscaled(value: i32, factor: f64) -> i32 {
        (f64::from(value) / factor).round() as i32
    }

    fn scaled_size(value: u32, factor: f64) -> u32 {
        (f64::from(value) * factor).round().max(1.0) as u32
    }

    /// Position of the element's near edge on one axis, inside the window.
    fn axis_position(from_far: bool, centered: bool, offset: i32, size: u32, bound: u32) -> i32 {
        let room = i64::from(bound) - i64::from(size);
        let position = if centered {
            room / 2 + i64::from(offset)
        } else if from_far {
            room - i64::from(offset)
        } else {
            i64::from(offset)
        };
        i32::try_from(position.clamp(0, room.max(0))).unwrap_or(i32::MAX)
    }

    /// The offset that puts the element's near edge at `position`.
    fn axis_offset(from_far: bool, centered: bool, position: i32, size: u32, bound: u32) -> i32 {
        let room = i64::from(bound) - i64::from(size);
        let position = i64::from(position).clamp(0, room.max(0));
        let offset = if centered {
            position - room / 2
        } else if from_far {
            room - position
        } else {
            position
        };
        i32::try_from(offset).unwrap_or(0)
    }

    /// The element's rectangle on screen for the current `game` rectangle.
    pub fn rect_in(&self, game: &ScreenRect) -> ScreenRect {
        let factor = self.factor(game);
        let width = Self::scaled_size(self.width, factor);
        let height = Self::scaled_size(self.height, factor);
        let x = Self::axis_position(
            self.anchor.is_right(),
            self.anchor.is_center(),
            Self::scaled(self.offset_x, factor),
            width,
            game.width,
        );
        let y = Self::axis_position(
            self.anchor.is_bottom(),
            self.anchor.is_center(),
            Self::scaled(self.offset_y, factor),
            height,
            game.height,
        );
        ScreenRect::new(
            game.x.saturating_add(x),
            game.y.saturating_add(y),
            width,
            height,
        )
    }

    /// Keeps the element where it is on screen but measures it from `anchor`.
    pub fn set_anchor(&mut self, game: &ScreenRect, anchor: Anchor) {
        let current = self.rect_in(game);
        self.anchor = anchor;
        self.store(game, current.x, current.y);
    }

    /// The element was dragged to the screen position `x`, `y`: it is kept
    /// there, anchored to the corner of `game` nearest to it.
    pub fn move_to(&mut self, game: &ScreenRect, x: i32, y: i32) {
        let factor = self.factor(game);
        let rect = ScreenRect::new(
            x,
            y,
            Self::scaled_size(self.width, factor),
            Self::scaled_size(self.height, factor),
        );
        self.anchor = Anchor::nearest(game, &rect);
        self.store(game, x, y);
    }

    /// Stores the offsets that put the element at the screen position `x`, `y`.
    fn store(&mut self, game: &ScreenRect, x: i32, y: i32) {
        let factor = self.factor(game);
        let width = Self::scaled_size(self.width, factor);
        let height = Self::scaled_size(self.height, factor);
        self.offset_x = Self::unscaled(
            Self::axis_offset(
                self.anchor.is_right(),
                self.anchor.is_center(),
                x.saturating_sub(game.x),
                width,
                game.width,
            ),
            factor,
        );
        self.offset_y = Self::unscaled(
            Self::axis_offset(
                self.anchor.is_bottom(),
                self.anchor.is_center(),
                y.saturating_sub(game.y),
                height,
                game.height,
            ),
            factor,
        );
    }

    /// Changes the size (in the same units as the distances); the distance
    /// from the anchor stays.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }
}

/// Where the mouse pointer is on screen, in the units of the system's window
/// coordinates (Windows only; elsewhere `None`). It lets the overlay react to
/// the pointer while its window is click-through and receives no mouse events.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
pub fn cursor_position() -> Option<(i32, i32)> {
    let mut point = POINT::default();
    // SAFETY: `point` is a valid out pointer for the call.
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut point) }.ok()?;
    Some((point.x, point.y))
}

/// Where the mouse pointer is on screen (Windows only; elsewhere `None`).
#[cfg(not(target_os = "windows"))]
pub fn cursor_position() -> Option<(i32, i32)> {
    None
}

/// Whether a game is in *exclusive* full screen, where no other window can be
/// drawn over it (Windows only; elsewhere the answer is always `false`).
///
/// It is the system's own notion (`SHQueryUserNotificationState`): it covers
/// any Direct3D application, so combine it with [`GameWindow::focused`]. A
/// game in the "fullscreen optimisations" borderless mode that Windows 10/11
/// uses for most DirectX 11 games is *not* exclusive and the overlay is shown.
#[derive(Debug, Clone, Copy, Default)]
pub struct FullScreenState;

impl FullScreenState {
    pub fn new() -> Self {
        Self
    }

    /// A Direct3D application is in exclusive full screen now.
    #[cfg(target_os = "windows")]
    #[allow(unsafe_code)]
    pub fn is_exclusive(&self) -> bool {
        // SAFETY: a plain query with no arguments.
        unsafe { SHQueryUserNotificationState() }
            .is_ok_and(|state| state == QUNS_RUNNING_D3D_FULL_SCREEN)
    }

    /// A Direct3D application is in exclusive full screen now.
    #[cfg(not(target_os = "windows"))]
    pub fn is_exclusive(&self) -> bool {
        false
    }
}

/// Who blends the overlay with what is behind it, which decides how the
/// caller paints it (see [`OverlayWindow::blending`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blending {
    /// The system blends the whole window by the opacity given to
    /// [`OverlayWindow::set_opacity_by_title`]: the content is drawn opaque.
    Window,
    /// The alpha of the pixels drawn is used: the unpainted parts are
    /// see-through and the painted ones as translucent as drawn. The window
    /// has to be created transparent.
    Pixels,
}

/// Native settings of the overlay window: above the game, click-through, out
/// of the task switcher, and (macOS) visible over a full screen app.
///
/// The window itself (transparent, undecorated) is created by the UI toolkit;
/// [`OverlayWindow::apply`] is called once with its handle after it exists.
#[derive(Debug, Clone, Copy)]
pub struct OverlayWindow {
    click_through: bool,
    input_rect: Option<ScreenRect>,
    /// The opacity (0 to 255) last given to the window, so it is not set again
    /// while it does not change.
    opacity_applied: Option<u8>,
}

impl OverlayWindow {
    /// A click-through overlay (the mouse reaches the game).
    pub fn new() -> Self {
        Self {
            click_through: true,
            input_rect: None,
            opacity_applied: None,
        }
    }

    /// The part of a click-through window that still takes the mouse.
    pub fn input_rect(&self) -> Option<ScreenRect> {
        self.input_rect
    }

    /// Keeps a part of the click-through window (pixels, relative to its
    /// top-left corner) clickable, e.g. a button, while the rest lets the
    /// mouse through. Only the X11 backend (Shape input region) honours it;
    /// elsewhere the window is click-through as a whole, and the caller has to
    /// toggle [`Self::set_click_through`] instead. Takes effect on the next
    /// apply.
    pub fn set_input_rect(&mut self, rect: Option<ScreenRect>) {
        self.input_rect = rect;
    }

    /// Whether [`Self::set_input_rect`] is honoured on this platform.
    pub const fn supports_input_rect() -> bool {
        cfg!(target_os = "linux")
    }

    /// Whether the overlay lets the mouse through to the game.
    pub fn click_through(&self) -> bool {
        self.click_through
    }

    /// Changes the click-through setting; takes effect on the next
    /// [`Self::apply`] (used by the "edit widgets" mode, which needs the
    /// mouse).
    pub fn set_click_through(&mut self, click_through: bool) {
        self.click_through = click_through;
    }

    /// Applies the native settings to the window behind `window`.
    #[cfg(target_os = "windows")]
    #[allow(unsafe_code)]
    pub fn apply(&self, window: &impl HasWindowHandle) -> Result<(), OverlayError> {
        let handle = window
            .window_handle()
            .map_err(|_| OverlayError::InvalidHandle)?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err(OverlayError::InvalidHandle);
        };
        self.apply_hwnd(HWND(handle.hwnd.get() as *mut std::ffi::c_void))
    }

    #[cfg(target_os = "windows")]
    #[allow(unsafe_code)]
    fn apply_hwnd(&self, hwnd: HWND) -> Result<(), OverlayError> {
        // SAFETY: `hwnd` belongs to a live window of this process.
        unsafe {
            let mut style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            // not in Alt+Tab / the taskbar
            style |= WS_EX_TOOLWINDOW.0;
            if self.click_through {
                // no focus stealing and the mouse goes to the game
                style |= WS_EX_NOACTIVATE.0 | WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0;
            } else {
                // interactive: it must be able to take the focus to be moved
                style &= !(WS_EX_NOACTIVATE.0 | WS_EX_TRANSPARENT.0);
            }
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style as isize);
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
            .map_err(|error| OverlayError::Os(error.to_string()))
        }
    }

    /// Applies the native settings to the window behind `window`.
    #[cfg(target_os = "macos")]
    #[allow(unsafe_code)]
    pub fn apply(&self, window: &impl HasWindowHandle) -> Result<(), OverlayError> {
        let handle = window
            .window_handle()
            .map_err(|_| OverlayError::InvalidHandle)?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return Err(OverlayError::InvalidHandle);
        };
        // SAFETY: `ns_view` is the live `NSView` of the window, and this runs
        // on the main thread (the UI thread).
        let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let window = view.window().ok_or(OverlayError::InvalidHandle)?;
        self.apply_ns_window(&window);
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn apply_ns_window(&self, window: &NSWindow) {
        window.setLevel(macos::MacWindowList::overlay_level());
        // `FullScreenAuxiliary` + `CanJoinAllSpaces`: the overlay can live in
        // the Space of a full screen app.
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        window.setIgnoresMouseEvents(self.click_through);
    }

    /// Applies the native settings to the window behind `window` (X11 only: a
    /// Wayland window gets [`OverlayError::Unsupported`]).
    #[cfg(target_os = "linux")]
    pub fn apply(&self, window: &impl HasWindowHandle) -> Result<(), OverlayError> {
        let handle = window
            .window_handle()
            .map_err(|_| OverlayError::InvalidHandle)?;
        let id = match handle.as_raw() {
            RawWindowHandle::Xlib(handle) => handle.window as u32,
            RawWindowHandle::Xcb(handle) => handle.window.get(),
            RawWindowHandle::Wayland(_) => return Err(OverlayError::Unsupported),
            _ => return Err(OverlayError::InvalidHandle),
        };
        x11::X11Session::connect()?.apply(id, self.click_through, self.input_rect)
    }

    /// Applies the native settings to the window behind `window`.
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    pub fn apply(&self, _window: &impl HasWindowHandle) -> Result<(), OverlayError> {
        Err(OverlayError::Unsupported)
    }

    /// Applies the native settings to the window of this process whose title
    /// is `title`. The UI toolkit gives no native handle for its secondary
    /// windows, so the overlay is found by its (unique) title. The window may
    /// not exist yet on the first frames: the caller tries again until this
    /// succeeds.
    #[cfg(target_os = "windows")]
    pub fn apply_by_title(&self, title: &str) -> Result<(), OverlayError> {
        // SAFETY: plain lookup by title; the result is only used if it is Ok.
        #[allow(unsafe_code)]
        let hwnd = unsafe { FindWindowW(PCWSTR::null(), &HSTRING::from(title)) }
            .map_err(|_| OverlayError::InvalidHandle)?;
        self.apply_hwnd(hwnd)
    }

    /// Who blends the overlay on this platform: the system, the whole window
    /// (Windows, X11), or the pixels it draws (macOS and anywhere else). The
    /// caller paints accordingly, and creates the window transparent only for
    /// [`Blending::Pixels`].
    pub const fn blending() -> Blending {
        if cfg!(any(target_os = "windows", target_os = "linux")) {
            Blending::Window
        } else {
            Blending::Pixels
        }
    }

    /// Makes the window titled `title` as opaque as `opacity` says, 0
    /// (invisible) to 1 (opaque), however the platform does it (see
    /// [`Self::blending`]). Cheap and idempotent: call it every frame, the
    /// window is only touched when the opacity changes (or, where the toolkit
    /// can undo it behind the scenes, always). The window may not exist yet on
    /// the first frames: [`OverlayError::InvalidHandle`] means try again.
    /// Where the system has no way to do it this is a quiet success, and the
    /// window stays opaque.
    pub fn set_opacity_by_title(&mut self, title: &str, opacity: f32) -> Result<(), OverlayError> {
        let alpha = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
        match Self::blending() {
            Blending::Pixels => self.make_pixels_see_through(title),
            Blending::Window => {
                if self.opacity_applied == Some(alpha) {
                    return Ok(());
                }
                match self.blend_window(title, opacity) {
                    Ok(()) | Err(OverlayError::Unsupported) => {
                        self.opacity_applied = Some(alpha);
                        Ok(())
                    }
                    Err(error) => Err(error),
                }
            }
        }
    }

    /// Forgets what was given to the window: call it when the window is
    /// closed, so the next one gets its opacity again.
    pub fn window_closed(&mut self) {
        self.opacity_applied = None;
    }

    /// Blends the whole window with what is behind it, `opacity` from 0
    /// (invisible) to 1 (opaque). The window is drawn opaque: the system does
    /// the blending, so this does not depend on the window having a swapchain
    /// with an alpha channel (on Windows that needs DirectComposition and often
    /// shows a white background).
    #[cfg(target_os = "windows")]
    #[allow(unsafe_code)]
    fn blend_window(&self, title: &str, opacity: f32) -> Result<(), OverlayError> {
        // SAFETY: plain lookup by title; the result is only used if it is Ok.
        let hwnd = unsafe { FindWindowW(PCWSTR::null(), &HSTRING::from(title)) }
            .map_err(|_| OverlayError::InvalidHandle)?;
        let alpha = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
        // SAFETY: `hwnd` belongs to a live window of this process.
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            if style & WS_EX_LAYERED.0 == 0 {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (style | WS_EX_LAYERED.0) as isize);
            }
            SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA)
                .map_err(|error| OverlayError::Os(error.to_string()))
        }
    }

    /// Gives the window its opacity through the compositor
    /// (`_NET_WM_WINDOW_OPACITY`): the window is drawn opaque and the whole of
    /// it is blended. Without a connection to an X server (a native Wayland
    /// session) it is [`OverlayError::Unsupported`].
    #[cfg(target_os = "linux")]
    fn blend_window(&self, title: &str, opacity: f32) -> Result<(), OverlayError> {
        let session = x11::X11Session::connect().map_err(|_| OverlayError::Unsupported)?;
        session.set_opacity_by_title(title, opacity)
    }

    /// Nothing blends the whole window here.
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    fn blend_window(&self, _title: &str, _opacity: f32) -> Result<(), OverlayError> {
        Err(OverlayError::Unsupported)
    }

    /// Lets the window blend the alpha of the pixels it draws with what is
    /// behind it, so the unpainted parts are see-through and the painted ones
    /// translucent. The window is created transparent by the UI toolkit, but
    /// its surface is configured opaque unless the *main* window asked for
    /// transparency too: this undoes that for this window alone, on its own
    /// Core Animation layer, so the main window stays a normal one.
    #[cfg(target_os = "macos")]
    fn make_pixels_see_through(&self, title: &str) -> Result<(), OverlayError> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| OverlayError::Os(String::from("not on the main thread")))?;
        let application = NSApplication::sharedApplication(mtm);
        for window in application.windows().iter() {
            if window.title().to_string() != title {
                continue;
            }
            if window.isOpaque() {
                window.setOpaque(false);
            }
            window.setBackgroundColor(Some(&NSColor::clearColor()));
            // the shadow of a window is cut from its alpha: with only a panel
            // painted it would draw an outline around the window's rectangle
            window.setHasShadow(false);
            if let Some(layer) = window.contentView().and_then(|view| view.layer()) {
                if layer.isOpaque() {
                    layer.setOpaque(false);
                }
                // SAFETY: on the main thread, and the array is only read
                // while nothing else changes the layer tree.
                #[allow(unsafe_code)]
                if let Some(sublayers) = unsafe { layer.sublayers() } {
                    for sublayer in sublayers.iter() {
                        if sublayer.isOpaque() {
                            sublayer.setOpaque(false);
                        }
                    }
                }
            }
            return Ok(());
        }
        Err(OverlayError::InvalidHandle)
    }

    /// Where nothing needs doing.
    #[cfg(not(target_os = "macos"))]
    fn make_pixels_see_through(&self, _title: &str) -> Result<(), OverlayError> {
        Ok(())
    }

    /// Applies the native settings to the window of this process whose title
    /// is `title` (see the Windows version). Must run on the main thread.
    #[cfg(target_os = "macos")]
    pub fn apply_by_title(&self, title: &str) -> Result<(), OverlayError> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| OverlayError::Os(String::from("not on the main thread")))?;
        let application = NSApplication::sharedApplication(mtm);
        for window in application.windows().iter() {
            if window.title().to_string() == title {
                self.apply_ns_window(&window);
                return Ok(());
            }
        }
        Err(OverlayError::InvalidHandle)
    }

    /// Applies the native settings to the window whose title is `title`
    /// (see the Windows version). X11 only.
    #[cfg(target_os = "linux")]
    pub fn apply_by_title(&self, title: &str) -> Result<(), OverlayError> {
        x11::X11Session::connect()?.apply_by_title(title, self.click_through, self.input_rect)
    }

    /// Applies the native settings to the window whose title is `title`.
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    pub fn apply_by_title(&self, _title: &str) -> Result<(), OverlayError> {
        Err(OverlayError::Unsupported)
    }

    /// Shows only `rects` (pixels, relative to the window's top-left corner)
    /// of the window titled `title`: everything else is cut away, so it is
    /// neither drawn nor clickable (the mouse goes to what is underneath).
    ///
    /// This gives an overlay without background or borders that needs no
    /// transparent swapchain: what is shown is painted opaque. An empty list
    /// hides the whole window.
    #[cfg(target_os = "windows")]
    #[allow(unsafe_code)]
    pub fn set_visible_region_by_title(
        &self,
        title: &str,
        rects: &[ScreenRect],
    ) -> Result<(), OverlayError> {
        // SAFETY: plain lookup by title; the result is only used if it is Ok.
        let hwnd = unsafe { FindWindowW(PCWSTR::null(), &HSTRING::from(title)) }
            .map_err(|_| OverlayError::InvalidHandle)?;
        // SAFETY: the regions are created here; the system owns `region`
        // once `SetWindowRgn` accepts it, the pieces are freed right away.
        unsafe {
            let region = CreateRectRgn(0, 0, 0, 0);
            for rect in rects {
                let part = CreateRectRgn(
                    rect.x,
                    rect.y,
                    rect.x + rect.width as i32,
                    rect.y + rect.height as i32,
                );
                CombineRgn(Some(region), Some(region), Some(part), RGN_OR);
                let _ = DeleteObject(part.into());
            }
            if SetWindowRgn(hwnd, Some(region), true) == 0 {
                let _ = DeleteObject(region.into());
                return Err(OverlayError::Os(String::from("SetWindowRgn failed")));
            }
        }
        Ok(())
    }

    /// The window is already transparent on macOS: nothing to cut away.
    #[cfg(target_os = "macos")]
    pub fn set_visible_region_by_title(
        &self,
        _title: &str,
        _rects: &[ScreenRect],
    ) -> Result<(), OverlayError> {
        Ok(())
    }

    /// See the Windows version (X11 Shape extension).
    #[cfg(target_os = "linux")]
    pub fn set_visible_region_by_title(
        &self,
        title: &str,
        rects: &[ScreenRect],
    ) -> Result<(), OverlayError> {
        x11::X11Session::connect()?.set_visible_region_by_title(title, rects)
    }

    /// See the Windows version.
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    pub fn set_visible_region_by_title(
        &self,
        _title: &str,
        _rects: &[ScreenRect],
    ) -> Result<(), OverlayError> {
        Err(OverlayError::Unsupported)
    }
}

impl Default for OverlayWindow {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_matches_the_client_and_nothing_else() {
        let tracker = GameWindowTracker::default();
        assert!(tracker.matches_title("EVE"));
        assert!(tracker.matches_title("EVE - Some Pilot"));
        assert!(!tracker.matches_title("EVE Launcher"));
        assert!(!tracker.matches_title("EVEmap - Chrome"));
        assert!(!tracker.matches_title("Telescope"));
        assert!(!tracker.matches_title(""));
    }

    #[test]
    fn the_process_is_recognised_by_its_name() {
        let tracker = GameWindowTracker::default();
        assert!(tracker.matches_process("EVE Online"));
        assert!(!tracker.matches_process("EVE Launcher"));
        assert!(!tracker.matches_process("Safari"));
        let tracker = tracker.with_process_names([String::from("exefile")]);
        assert!(tracker.matches_process("exefile"));
        assert!(!tracker.matches_process("EVE Online"));
    }

    #[test]
    fn a_custom_prefix_is_used() {
        let tracker = GameWindowTracker::new("Test");
        assert!(tracker.matches_title("Test - one"));
        assert!(!tracker.matches_title("EVE - one"));
    }

    #[test]
    fn an_empty_rect_is_detected() {
        assert!(ScreenRect::default().is_empty());
        assert!(ScreenRect::new(0, 0, 10, 0).is_empty());
        assert!(!ScreenRect::new(-5, 3, 10, 20).is_empty());
    }

    #[test]
    fn the_placement_follows_the_game_window() {
        let placement = OverlayPlacement::new(Anchor::TopLeft, 100, 50, 200, 100);
        let game = ScreenRect::new(10, 20, 1920, 1080);
        assert_eq!(placement.rect_in(&game), ScreenRect::new(110, 70, 200, 100));
        // the game moved and shrank: same offset from its corner
        let game = ScreenRect::new(500, 0, 800, 600);
        assert_eq!(placement.rect_in(&game), ScreenRect::new(600, 50, 200, 100));
    }

    #[test]
    fn a_right_anchored_element_keeps_its_distance_from_the_right_edge() {
        let placement = OverlayPlacement::new(Anchor::TopRight, 20, 10, 200, 100);
        let game = ScreenRect::new(0, 0, 1000, 500);
        assert_eq!(placement.rect_in(&game), ScreenRect::new(780, 10, 200, 100));
        let game = ScreenRect::new(0, 0, 800, 500);
        assert_eq!(placement.rect_in(&game), ScreenRect::new(580, 10, 200, 100));
    }

    #[test]
    fn dragging_anchors_to_the_nearest_corner() {
        let game = ScreenRect::new(100, 100, 1000, 500);
        let mut placement = OverlayPlacement::new(Anchor::TopLeft, 0, 0, 200, 100);
        placement.move_to(&game, 900, 480);
        assert_eq!(placement.parts(), (Anchor::BottomRight, 0, 20, 200, 100));
        // the game grew: the element stayed at the same distance from the corner
        let game = ScreenRect::new(0, 0, 2000, 1000);
        assert_eq!(
            placement.rect_in(&game),
            ScreenRect::new(1800, 880, 200, 100)
        );
    }

    #[test]
    fn dragging_past_the_edges_is_held_inside_the_game() {
        let game = ScreenRect::new(100, 100, 1000, 500);
        let mut placement = OverlayPlacement::new(Anchor::TopLeft, 0, 0, 200, 100);
        placement.move_to(&game, 5000, 5000);
        assert_eq!(placement.parts(), (Anchor::BottomRight, 0, 0, 200, 100));
        placement.move_to(&game, -50, -50);
        assert_eq!(placement.parts(), (Anchor::TopLeft, 0, 0, 200, 100));
    }

    #[test]
    fn a_placement_bigger_than_the_game_sits_at_its_corner() {
        let game = ScreenRect::new(0, 0, 100, 100);
        let placement = OverlayPlacement::new(Anchor::TopLeft, 40, 40, 300, 300);
        assert_eq!(placement.rect_in(&game), ScreenRect::new(0, 0, 300, 300));
    }

    #[test]
    fn a_centered_element_is_measured_from_the_middle() {
        let placement = OverlayPlacement::new(Anchor::Center, 10, -10, 100, 50);
        let game = ScreenRect::new(0, 0, 1000, 500);
        assert_eq!(placement.rect_in(&game), ScreenRect::new(460, 215, 100, 50));
    }

    #[test]
    fn changing_the_anchor_keeps_the_element_where_it_is() {
        let game = ScreenRect::new(0, 0, 1000, 500);
        let mut placement = OverlayPlacement::new(Anchor::TopLeft, 100, 50, 200, 100);
        placement.set_anchor(&game, Anchor::BottomRight);
        assert_eq!(placement.parts(), (Anchor::BottomRight, 700, 350, 200, 100));
        assert_eq!(placement.rect_in(&game), ScreenRect::new(100, 50, 200, 100));
    }

    #[test]
    fn a_scaled_placement_keeps_its_proportions_on_any_screen() {
        let placement = OverlayPlacement::new(Anchor::TopLeft, 100, 50, 200, 100)
            .with_reference_height(Some(1080));
        assert_eq!(placement.reference_height(), Some(1080));
        // the reference size: as stored
        let game = ScreenRect::new(0, 0, 1920, 1080);
        assert_eq!(placement.rect_in(&game), ScreenRect::new(100, 50, 200, 100));
        // 4K: twice as big
        let game = ScreenRect::new(0, 0, 3840, 2160);
        assert_eq!(
            placement.rect_in(&game),
            ScreenRect::new(200, 100, 400, 200)
        );
        // half the height: half as big
        let game = ScreenRect::new(0, 0, 960, 540);
        assert_eq!(placement.rect_in(&game), ScreenRect::new(50, 25, 100, 50));
    }

    #[test]
    fn dragging_a_scaled_placement_stores_reference_pixels() {
        let game = ScreenRect::new(0, 0, 3840, 2160);
        let mut placement = OverlayPlacement::new(Anchor::TopLeft, 0, 0, 200, 100)
            .with_reference_height(Some(1080));
        placement.move_to(&game, 200, 100);
        assert_eq!(placement.parts(), (Anchor::TopLeft, 100, 50, 200, 100));
    }

    #[test]
    fn calibrating_takes_the_current_window_as_the_reference() {
        let mut placement = OverlayPlacement::new(Anchor::TopLeft, 100, 50, 200, 100);
        let game = ScreenRect::new(0, 0, 1280, 720);
        placement.calibrate(&game);
        assert_eq!(placement.reference_height(), Some(720));
        assert_eq!(placement.rect_in(&game), ScreenRect::new(100, 50, 200, 100));
        // the same placement on a bigger screen
        let game = ScreenRect::new(0, 0, 2560, 1440);
        assert_eq!(
            placement.rect_in(&game),
            ScreenRect::new(200, 100, 400, 200)
        );
    }

    #[test]
    fn calibrating_again_keeps_the_look_on_the_new_window() {
        let mut placement = OverlayPlacement::new(Anchor::TopLeft, 100, 50, 200, 100)
            .with_reference_height(Some(720));
        let game = ScreenRect::new(0, 0, 2560, 1440);
        let before = placement.rect_in(&game);
        placement.calibrate(&game);
        assert_eq!(placement.reference_height(), Some(1440));
        assert_eq!(placement.rect_in(&game), before);
        assert_eq!(placement.parts(), (Anchor::TopLeft, 200, 100, 400, 200));
    }

    #[test]
    fn a_zero_reference_height_turns_scaling_off() {
        let placement =
            OverlayPlacement::new(Anchor::TopLeft, 10, 10, 50, 50).with_reference_height(Some(0));
        assert_eq!(placement.reference_height(), None);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn exclusive_full_screen_is_only_reported_on_windows() {
        assert!(!FullScreenState::new().is_exclusive());
    }

    #[test]
    fn who_blends_follows_the_platform() {
        let expected = if cfg!(any(target_os = "windows", target_os = "linux")) {
            Blending::Window
        } else {
            Blending::Pixels
        };
        assert_eq!(OverlayWindow::blending(), expected);
    }

    #[test]
    fn a_window_that_is_not_there_is_not_ready_yet() {
        // nothing carries this title: the caller has to try again (on a
        // platform with nothing to do it is a quiet success instead; on macOS
        // a test thread is not the main one, which AppKit requires)
        let mut overlay = OverlayWindow::new();
        let answer = overlay.set_opacity_by_title("no such window", 0.5);
        assert!(
            match &answer {
                Ok(()) | Err(OverlayError::InvalidHandle) => true,
                Err(OverlayError::Os(message)) => message.contains("main thread"),
                Err(_) => false,
            },
            "{answer:?}"
        );
        // closing it forgets what was applied
        overlay.window_closed();
        assert_eq!(overlay.opacity_applied, None);
    }

    #[test]
    fn click_through_defaults_on_and_can_change() {
        let mut overlay = OverlayWindow::new();
        assert!(overlay.click_through());
        overlay.set_click_through(false);
        assert!(!overlay.click_through());
        assert_eq!(overlay.input_rect(), None);
        overlay.set_input_rect(Some(ScreenRect::new(1, 2, 3, 4)));
        assert_eq!(overlay.input_rect(), Some(ScreenRect::new(1, 2, 3, 4)));
    }

    #[test]
    fn errors_describe_themselves() {
        assert!(!OverlayError::Unsupported.to_string().is_empty());
        assert_eq!(OverlayError::Os(String::from("boom")).to_string(), "boom");
    }
}
