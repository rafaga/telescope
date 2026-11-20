//! Native date picker behind one cross-platform API.
//!
//! No operating system offers a modal "pick a date" dialog the way it offers
//! one for files, so each one gets the closest native control:
//!
//! * macOS: an `NSDatePicker` (calendar style) inside an `NSPopover` shown
//!   over the app's main window.
//! * Windows: the system month calendar (`MonthCal` control) inside a small
//!   popup window next to the cursor.
//! * Linux: no toolkit-independent control exists, so the desktop's own
//!   dialog tool is run: `kdialog --calendar` on KDE and `zenity --calendar`
//!   elsewhere, whichever is installed.
//!
//! When the system has no native picker ([`DatePicker::is_available`] is
//! false) [`DatePicker::open_date_picker`] answers
//! `DialogResult::Err("unsupported: ...")`, and the caller decides what to
//! use instead; this crate knows nothing about any UI toolkit.

use crate::dialog::DialogResult;

#[cfg(target_os = "linux")]
use std::path::PathBuf;

/// A calendar day, without a time or a time zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    pub year: i32,
    /// 1 to 12.
    pub month: u8,
    /// 1 to the number of days of the month.
    pub day: u8,
}

impl Date {
    /// The day, or `None` when `month` or `day` do not exist that year.
    pub fn new(year: i32, month: u8, day: u8) -> Option<Self> {
        (day >= 1 && day <= Self::days_in_month(year, month)).then_some(Self { year, month, day })
    }

    /// Days of `month` (1 to 12) in `year`; 0 for a month that doesn't exist.
    pub fn days_in_month(year: i32, month: u8) -> u8 {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
            2 => 28,
            _ => 0,
        }
    }

    /// Reads `YYYY-MM-DD`.
    pub fn parse_iso(text: &str) -> Option<Self> {
        let mut parts = text.trim().split('-');
        let year = parts.next()?.parse().ok()?;
        let month = parts.next()?.parse().ok()?;
        let day = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Self::new(year, month, day)
    }

    /// Writes `YYYY-MM-DD`.
    pub fn to_iso(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// Reads the English `Tue Nov 17 2026` that old `kdialog` versions print.
    #[cfg(any(target_os = "linux", test))]
    fn parse_text(text: &str) -> Option<Self> {
        let words: Vec<&str> = text.split_whitespace().collect();
        let [.., month, day, year] = words.as_slice() else {
            return None;
        };
        let month = [
            "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
        ]
        .iter()
        .position(|name| month.to_lowercase().starts_with(name))?;
        Self::new(year.parse().ok()?, (month + 1) as u8, day.parse().ok()?)
    }
}

/// Configuration of a native date picker; see the module documentation.
#[derive(Clone, Debug, Default)]
pub struct DatePicker {
    title: Option<String>,
    initial: Option<Date>,
    owner: Option<isize>,
}

impl DatePicker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Shows `title` where the system's picker has a place for one.
    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = Some(title.into());
    }

    /// The day selected when the picker opens (today when not set).
    pub fn set_initial(&mut self, date: Date) {
        self.initial = Some(date);
    }

    /// Windows: the window (`HWND`) the picker belongs to; the window takes
    /// no input while the picker is open. Ignored elsewhere.
    pub fn set_owner(&mut self, window: isize) {
        self.owner = Some(window);
    }

    pub fn title(&self) -> String {
        self.title
            .clone()
            .unwrap_or_else(|| String::from("Select a date"))
    }

    pub fn initial(&self) -> Option<Date> {
        self.initial
    }

    /// Whether this system has a native picker. Always true on Windows and
    /// macOS; on Linux, whether `kdialog` or `zenity` is installed.
    pub fn is_available() -> bool {
        #[cfg(target_os = "linux")]
        {
            LinuxTool::detect().is_some()
        }
        #[cfg(not(target_os = "linux"))]
        {
            true
        }
    }
}

// ---------------------------------------------------------------------
// Linux: kdialog / zenity
// ---------------------------------------------------------------------

/// The external tool that shows the calendar.
#[cfg(target_os = "linux")]
enum LinuxTool {
    Kdialog(PathBuf),
    Zenity(PathBuf),
}

#[cfg(target_os = "linux")]
impl LinuxTool {
    /// The tool to use: `kdialog` first on KDE, `zenity` first elsewhere.
    fn detect() -> Option<Self> {
        let kde = std::env::var("XDG_CURRENT_DESKTOP")
            .map(|desktop| desktop.to_uppercase().contains("KDE"))
            .unwrap_or(false);
        let kdialog = Self::find("kdialog").map(Self::Kdialog);
        let zenity = Self::find("zenity").map(Self::Zenity);
        if kde {
            kdialog.or(zenity)
        } else {
            zenity.or(kdialog)
        }
    }

    fn find(name: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
    }

    fn command(&self, title: &str, initial: Option<Date>) -> std::process::Command {
        match self {
            Self::Kdialog(program) => {
                let mut command = std::process::Command::new(program);
                command
                    .arg("--title")
                    .arg(title)
                    .arg("--dateformat")
                    .arg("yyyy-MM-dd");
                if let Some(initial) = initial {
                    command.arg("--default").arg(initial.to_iso());
                }
                command.arg("--calendar").arg(title);
                command
            }
            Self::Zenity(program) => {
                let mut command = std::process::Command::new(program);
                command
                    .arg("--calendar")
                    .arg("--title")
                    .arg(title)
                    .arg("--text")
                    .arg(title)
                    .arg("--date-format=%Y-%m-%d");
                if let Some(initial) = initial {
                    command
                        .arg(format!("--year={}", initial.year))
                        .arg(format!("--month={}", initial.month))
                        .arg(format!("--day={}", initial.day));
                }
                command
            }
        }
    }

    /// Runs the tool and waits for the user. Both tools exit with 1 when the
    /// dialog is cancelled.
    fn pick(&self, title: &str, initial: Option<Date>) -> DialogResult<Date> {
        let output = match self.command(title, initial).output() {
            Ok(output) => output,
            Err(e) => return DialogResult::Err(format!("Failed to run the date picker: {e}")),
        };
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            // `--dateformat` is older than some kdialog installs: their
            // answer is the locale-independent English text.
            return match Date::parse_iso(&text).or_else(|| Date::parse_text(&text)) {
                Some(date) => DialogResult::Ok(date),
                None => DialogResult::Err(format!("Unrecognized date: {}", text.trim())),
            };
        }
        match output.status.code() {
            Some(1) => DialogResult::Cancelled,
            _ => DialogResult::Err(String::from_utf8_lossy(&output.stderr).trim().to_string()),
        }
    }
}

#[cfg(target_os = "linux")]
impl DatePicker {
    /// Opens the picker; `on_result` is called, on another thread, when the
    /// user picks a day or closes it.
    #[tracing::instrument(skip(self, on_result))]
    pub fn open_date_picker(&self, on_result: impl Fn(DialogResult<Date>) + Send + Sync + 'static) {
        let Some(tool) = LinuxTool::detect() else {
            on_result(DialogResult::Err(String::from(
                "unsupported: neither kdialog nor zenity is installed",
            )));
            return;
        };
        let title = self.title();
        let initial = self.initial;
        std::thread::spawn(move || on_result(tool.pick(&title, initial)));
    }
}

// ---------------------------------------------------------------------
// Windows: the system month calendar in a popup window
// ---------------------------------------------------------------------

#[cfg(target_os = "windows")]
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SYSTEMTIME, WPARAM},
        Graphics::Gdi::{
            COLOR_WINDOW, GetMonitorInfoW, HBRUSH, MONITOR_DEFAULTTONEAREST, MONITORINFO,
            MonitorFromPoint,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::*,
            Input::KeyboardAndMouse::{EnableWindow, SetFocus, VK_ESCAPE},
            WindowsAndMessaging::*,
        },
    },
    core::{PCWSTR, w},
};

/// The popup window and its state while the picker runs.
#[cfg(target_os = "windows")]
struct MonthCalPopup {
    calendar: HWND,
    done: bool,
    result: Option<Date>,
}

#[cfg(target_os = "windows")]
impl MonthCalPopup {
    /// The day the calendar shows as selected.
    #[allow(unsafe_code)]
    fn selected(&self) -> Option<Date> {
        let mut time = SYSTEMTIME::default();
        let answer = unsafe {
            SendMessageW(
                self.calendar,
                MCM_GETCURSEL,
                Some(WPARAM(0)),
                Some(LPARAM(&mut time as *mut SYSTEMTIME as isize)),
            )
        };
        (answer.0 != 0)
            .then(|| Date::new(time.wYear as i32, time.wMonth as u8, time.wDay as u8))
            .flatten()
    }

    /// The popup's messages: a click on a day answers; losing the focus
    /// cancels.
    #[allow(unsafe_code)]
    unsafe extern "system" fn window_proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_NCCREATE {
            let create = lparam.0 as *const CREATESTRUCTW;
            unsafe {
                SetWindowLongPtrW(window, GWLP_USERDATA, (*create).lpCreateParams as isize);
            }
            return unsafe { DefWindowProcW(window, message, wparam, lparam) };
        }
        let state = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *mut MonthCalPopup;
        if state.is_null() {
            return unsafe { DefWindowProcW(window, message, wparam, lparam) };
        }
        let state = unsafe { &mut *state };
        match message {
            WM_NOTIFY => {
                let header = lparam.0 as *const NMHDR;
                if unsafe { (*header).code } == MCN_SELECT {
                    state.result = state.selected();
                    state.done = true;
                    let _ = unsafe { DestroyWindow(window) };
                }
                LRESULT(0)
            }
            WM_ACTIVATE => {
                if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE {
                    state.done = true;
                    let _ = unsafe { DestroyWindow(window) };
                }
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
        }
    }

    /// Shows the popup and runs its own message loop until it closes.
    #[allow(unsafe_code)]
    fn run(picker: &DatePicker) -> DialogResult<Date> {
        let initial = picker.initial;
        let outcome: windows::core::Result<Option<Date>> = (|| unsafe {
            let controls = INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_DATE_CLASSES,
            };
            let _ = InitCommonControlsEx(&controls);

            let instance = GetModuleHandleW(None)?;
            let class_name = w!("NativeToolsDatePicker");
            let class = WNDCLASSW {
                lpfnWndProc: Some(Self::window_proc),
                hInstance: instance.into(),
                lpszClassName: class_name,
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as usize as *mut core::ffi::c_void),
                ..Default::default()
            };
            // Registered once; a second registration fails and is harmless.
            RegisterClassW(&class);

            let mut state = Box::new(MonthCalPopup {
                calendar: HWND::default(),
                done: false,
                result: None,
            });
            let owner = picker
                .owner
                .map(|window| HWND(window as *mut core::ffi::c_void));
            let style = WS_POPUP | WS_BORDER;
            let popup = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                class_name,
                PCWSTR::null(),
                style,
                0,
                0,
                10,
                10,
                owner,
                None,
                Some(instance.into()),
                Some(&mut *state as *mut MonthCalPopup as *const core::ffi::c_void),
            )?;
            state.calendar = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                MONTHCAL_CLASS,
                PCWSTR::null(),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(MCS_NOTODAY as u32),
                0,
                0,
                0,
                0,
                Some(popup),
                None,
                Some(instance.into()),
                None,
            )?;

            // Without an initial day the calendar opens on today.
            if let Some(initial) = initial {
                let time = SYSTEMTIME {
                    wYear: initial.year as u16,
                    wMonth: initial.month as u16,
                    wDay: initial.day as u16,
                    ..Default::default()
                };
                SendMessageW(
                    state.calendar,
                    MCM_SETCURSEL,
                    Some(WPARAM(0)),
                    Some(LPARAM(&time as *const SYSTEMTIME as isize)),
                );
            }

            // The popup is as big as the calendar needs, next to the cursor.
            let mut needed = RECT::default();
            SendMessageW(
                state.calendar,
                MCM_GETMINREQRECT,
                Some(WPARAM(0)),
                Some(LPARAM(&mut needed as *mut RECT as isize)),
            );
            let width = needed.right - needed.left;
            let height = needed.bottom - needed.top;
            let _ = SetWindowPos(
                state.calendar,
                None,
                0,
                0,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            let mut frame = RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            };
            let _ = AdjustWindowRectEx(&mut frame, style, false, WS_EX_TOOLWINDOW | WS_EX_TOPMOST);
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            let popup_width = frame.right - frame.left;
            let popup_height = frame.bottom - frame.top;
            // Inside the work area of the monitor the cursor is on: moved
            // left when it would pass the right edge, and above the cursor
            // when it would pass the bottom one.
            let mut monitor = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let mut x = cursor.x;
            let mut y = cursor.y;
            if GetMonitorInfoW(
                MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            )
            .as_bool()
            {
                let area = monitor.rcWork;
                if x + popup_width > area.right {
                    x = area.right - popup_width;
                }
                if y + popup_height > area.bottom {
                    y = cursor.y - popup_height;
                }
                x = x.max(area.left);
                y = y.max(area.top);
            }
            let _ = SetWindowPos(popup, None, x, y, popup_width, popup_height, SWP_NOZORDER);

            if let Some(owner) = owner {
                let _ = EnableWindow(owner, false);
            }
            let _ = ShowWindow(popup, SW_SHOW);
            let _ = SetForegroundWindow(popup);
            let _ = SetFocus(Some(state.calendar));

            let mut message = MSG::default();
            while !state.done && GetMessageW(&mut message, None, 0, 0).0 > 0 {
                if message.message == WM_KEYDOWN && message.wParam.0 == VK_ESCAPE.0 as usize {
                    state.done = true;
                    let _ = DestroyWindow(popup);
                    continue;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }

            if let Some(owner) = owner {
                let _ = EnableWindow(owner, true);
                let _ = SetForegroundWindow(owner);
            }
            Ok(state.result)
        })();

        match outcome {
            Ok(Some(date)) => DialogResult::Ok(date),
            Ok(None) => DialogResult::Cancelled,
            Err(e) => DialogResult::Err(e.to_string()),
        }
    }
}

#[cfg(target_os = "windows")]
impl DatePicker {
    /// Opens the picker and calls `on_result` when the user picks a day or
    /// closes it. It runs its own message loop on the calling (UI) thread,
    /// like [`crate::dialog::Dialog::open_file_dialog`] does on Windows; set
    /// an owner with [`DatePicker::set_owner`] so the app takes no input
    /// meanwhile.
    #[tracing::instrument(skip(self, on_result))]
    pub fn open_date_picker(&self, on_result: impl Fn(DialogResult<Date>) + Send + Sync + 'static) {
        on_result(MonthCalPopup::run(self));
    }
}

// ---------------------------------------------------------------------
// macOS: NSDatePicker in an NSPopover
// ---------------------------------------------------------------------

#[cfg(target_os = "macos")]
use std::cell::{Cell, RefCell};

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
#[cfg(target_os = "macos")]
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
#[cfg(target_os = "macos")]
use objc2_app_kit::{
    NSApplication, NSDatePicker, NSDatePickerElementFlags, NSDatePickerStyle, NSPopover,
    NSPopoverBehavior, NSViewController,
};
#[cfg(target_os = "macos")]
use objc2_foundation::{
    MainThreadMarker, NSCalendar, NSCalendarUnit, NSDateComponents, NSNotification,
    NSNotificationCenter, NSObjectProtocol, NSPoint, NSRect, NSRectEdge, NSSize,
};

/// What the picker's target keeps while the popover is open.
#[cfg(target_os = "macos")]
struct TargetState {
    on_result: Box<dyn Fn(DialogResult<Date>)>,
    popover: RefCell<Option<Retained<NSPopover>>>,
    observer: RefCell<Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>>,
    answered: Cell<bool>,
}

#[cfg(target_os = "macos")]
define_class!(
    /// Receives the picker's action (a day was clicked).
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NativeToolsDatePickerTarget"]
    #[ivars = TargetState]
    struct DatePickerTarget;

    unsafe impl NSObjectProtocol for DatePickerTarget {}

    impl DatePickerTarget {
        #[unsafe(method(dateChanged:))]
        fn date_changed(&self, sender: &NSDatePicker) {
            let state = self.ivars();
            let calendar = NSCalendar::currentCalendar();
            let parts = calendar.components_fromDate(
                NSCalendarUnit::Year | NSCalendarUnit::Month | NSCalendarUnit::Day,
                &sender.dateValue(),
            );
            let date = Date::new(parts.year() as i32, parts.month() as u8, parts.day() as u8);
            state.answered.set(true);
            (state.on_result)(match date {
                Some(date) => DialogResult::Ok(date),
                None => DialogResult::Err(String::from("Unrecognized date")),
            });
            if let Some(popover) = state.popover.borrow().as_ref() {
                popover.close();
            }
        }
    }
);

#[cfg(target_os = "macos")]
impl DatePickerTarget {
    #[allow(unsafe_code)]
    fn new(mtm: MainThreadMarker, state: TargetState) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(state);
        unsafe { msg_send![super(this), init] }
    }

    /// The popover closed: answers `Cancelled` when no day was picked, and
    /// lets go of everything that pointed back at this target.
    #[allow(unsafe_code)]
    fn popover_closed(&self) {
        let state = self.ivars();
        if !state.answered.replace(true) {
            (state.on_result)(DialogResult::Cancelled);
        }
        state.popover.borrow_mut().take();
        if let Some(observer) = state.observer.borrow_mut().take() {
            unsafe {
                NSNotificationCenter::defaultCenter().removeObserver(AsRef::<AnyObject>::as_ref(&*observer));
            }
        }
    }
}

#[cfg(target_os = "macos")]
impl DatePicker {
    /// Opens the picker over the app's main window; `on_result` is called on
    /// the main thread when the user picks a day or dismisses it. Must be
    /// called from the main thread.
    #[tracing::instrument(skip(self, on_result))]
    #[allow(unsafe_code)]
    pub fn open_date_picker(&self, on_result: impl Fn(DialogResult<Date>) + Send + Sync + 'static) {
        let Some(mtm) = MainThreadMarker::new() else {
            on_result(DialogResult::Err(
                "Error creating Main Thread Marker".into(),
            ));
            return;
        };
        let Some(content) = NSApplication::sharedApplication(mtm)
            .mainWindow()
            .and_then(|window| window.contentView())
        else {
            on_result(DialogResult::Err("No main window available".into()));
            return;
        };

        let picker = NSDatePicker::new(mtm);
        picker.setDatePickerStyle(NSDatePickerStyle::ClockAndCalendar);
        picker.setDatePickerElements(NSDatePickerElementFlags::YearMonthDay);
        picker.setBezeled(false);
        picker.setDrawsBackground(false);
        if let Some(initial) = self.initial {
            let parts = NSDateComponents::new();
            parts.setYear(initial.year as isize);
            parts.setMonth(initial.month as isize);
            parts.setDay(initial.day as isize);
            if let Some(date) = NSCalendar::currentCalendar().dateFromComponents(&parts) {
                picker.setDateValue(&date);
            }
        }
        picker.sizeToFit();

        let view_controller = NSViewController::new(mtm);
        view_controller.setView(&picker);
        let popover = NSPopover::new(mtm);
        popover.setBehavior(NSPopoverBehavior::Transient);
        popover.setContentViewController(Some(&view_controller));
        popover.setContentSize(picker.frame().size);

        let target = DatePickerTarget::new(
            mtm,
            TargetState {
                on_result: Box::new(on_result),
                popover: RefCell::new(Some(popover.clone())),
                observer: RefCell::new(None),
                answered: Cell::new(false),
            },
        );
        unsafe {
            picker.setTarget(Some(&target));
            picker.setAction(Some(sel!(dateChanged:)));
        }

        // Dismissing the popover (a click outside, Esc) is a cancel.
        let closing = target.clone();
        let block = block2::RcBlock::new(move |_: std::ptr::NonNull<NSNotification>| {
            closing.popover_closed();
        });
        let observer = unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(objc2_app_kit::NSPopoverDidCloseNotification),
                Some(&*popover as &AnyObject),
                None,
                &block,
            )
        };
        target.ivars().observer.replace(Some(observer));

        // Anchored at the middle of the window's content.
        let bounds = content.bounds();
        let anchor = NSRect::new(
            NSPoint::new(bounds.size.width / 2.0, bounds.size.height / 2.0),
            NSSize::new(1.0, 1.0),
        );
        popover.showRelativeToRect_ofView_preferredEdge(anchor, &content, NSRectEdge::MinY);
    }
}

// ---------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_new_rejects_missing_days() {
        assert!(Date::new(2026, 2, 29).is_none());
        assert!(Date::new(2024, 2, 29).is_some());
        assert!(Date::new(2026, 13, 1).is_none());
        assert!(Date::new(2026, 4, 31).is_none());
        assert!(Date::new(2026, 1, 0).is_none());
    }

    #[test]
    fn leap_years_follow_the_gregorian_rule() {
        assert_eq!(Date::days_in_month(1900, 2), 28);
        assert_eq!(Date::days_in_month(2000, 2), 29);
        assert_eq!(Date::days_in_month(2026, 12), 31);
    }

    #[test]
    fn iso_round_trip() {
        let date = Date::new(2026, 11, 7).unwrap();
        assert_eq!(date.to_iso(), "2026-11-07");
        assert_eq!(Date::parse_iso("2026-11-07\n"), Some(date));
        assert_eq!(Date::parse_iso("2026-11"), None);
        assert_eq!(Date::parse_iso("2026-11-07-1"), None);
        assert_eq!(Date::parse_iso("2026-02-30"), None);
    }

    #[test]
    fn parses_the_english_text_of_old_kdialog() {
        assert_eq!(
            Date::parse_text("Tue Nov 17 2026\n"),
            Date::new(2026, 11, 17)
        );
        assert_eq!(Date::parse_text("Nov 17"), None);
        assert_eq!(Date::parse_text("Di. Foo 30 2010"), None);
    }

    #[test]
    fn picker_defaults_and_setters() {
        let mut picker = DatePicker::new();
        assert_eq!(picker.title(), "Select a date");
        assert_eq!(picker.initial(), None);
        picker.set_title("Due date");
        picker.set_initial(Date::new(2026, 1, 2).unwrap());
        assert_eq!(picker.title(), "Due date");
        assert_eq!(picker.initial(), Date::new(2026, 1, 2));
    }
}
