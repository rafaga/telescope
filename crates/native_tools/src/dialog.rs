//! Native open file / open folder dialogs behind one cross-platform API.
//!
//! A [`Dialog`] is configured with a [`DialogType`] and its
//! `open_file_dialog` returns a [`DialogResult`]. Each OS has its own
//! implementation: `IFileOpenDialog` on Windows, `NSOpenPanel` on macOS and the
//! XDG desktop portal on Linux.

use std::path::{Path, PathBuf};
//use std::sync::{Arc, Mutex};

#[cfg(target_os = "windows")]
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
#[cfg(target_os = "windows")]
use windows::{
    Win32::{System::Com::*, UI::Shell::*},
    core::*,
};

#[cfg(target_os = "macos")]
use block2::RcBlock;
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSApplication, NSModalResponse, NSModalResponseOK, NSOpenPanel};
#[cfg(target_os = "macos")]
use objc2_foundation::{MainThreadMarker, NSString, NSURL};
#[cfg(target_os = "macos")]
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::time::Instant;

#[cfg(target_os = "linux")]
use ashpd::desktop::ResponseError;
#[cfg(target_os = "linux")]
use ashpd::desktop::file_chooser::{FileFilter as PortalFileFilter, SelectedFiles};

// ---------------------------------------------------------------------
// Helper macros that propagate errors without the `Try` trait (only
// available on nightly for custom types such as `DialogResult`). They
// stand in for the `?` operator here.
// ---------------------------------------------------------------------

/// Unwraps a `windows::core::Result<T>` into `T`, or returns early with
/// `DialogResult::Err(..)` when it failed.
#[cfg(target_os = "windows")]
macro_rules! try_win {
    ($expr:expr) => {
        match $expr {
            Ok(v) => v,
            Err(e) => return DialogResult::Err(e.to_string()),
        }
    };
}

/// Propagates a `DialogResult<T>`: unwraps `Ok`, and returns early with the
/// same variant on `Cancelled` or `Err`.
#[cfg(target_os = "windows")]
macro_rules! try_dialog {
    ($expr:expr) => {
        match $expr {
            DialogResult::Ok(v) => v,
            DialogResult::Cancelled => return DialogResult::Cancelled,
            DialogResult::Err(e) => return DialogResult::Err(e),
        }
    };
}

#[derive(Debug, PartialEq, Copy, Clone)]
pub enum DialogType {
    Directory,
    File,
}

#[derive(Debug, Clone)]
pub enum DialogResult<T> {
    Ok(T),
    Cancelled,
    Err(String), // optional: if the dialog itself can fail
}

impl<T> DialogResult<T> {
    pub fn is_ok(&self) -> bool {
        matches!(self, DialogResult::Ok(_))
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(self, DialogResult::Cancelled)
    }

    pub fn is_err(&self) -> bool {
        matches!(self, DialogResult::Err(_))
    }

    pub fn unwrap(self) -> T {
        match self {
            DialogResult::Ok(val) => val,
            DialogResult::Cancelled => panic!("called unwrap on Cancelled"),
            DialogResult::Err(e) => panic!("called unwrap on Err: {e}"),
        }
    }

    pub fn unwrap_or(self, default: T) -> T {
        match self {
            DialogResult::Ok(val) => val,
            _ => default,
        }
    }

    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> DialogResult<U> {
        match self {
            DialogResult::Ok(val) => DialogResult::Ok(f(val)),
            DialogResult::Cancelled => DialogResult::Cancelled,
            DialogResult::Err(e) => DialogResult::Err(e),
        }
    }

    pub fn ok(self) -> Option<T> {
        match self {
            DialogResult::Ok(val) => Some(val),
            _ => None,
        }
    }
}

/// A group of file types offered by a file dialog, such as
/// `FileFilter::new("SQLite database", &["db"])`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileFilter {
    /// What the user sees.
    pub name: String,
    /// The extensions it matches, without the dot.
    pub extensions: Vec<String>,
}

impl FileFilter {
    pub fn new(name: impl Into<String>, extensions: &[&str]) -> Self {
        Self {
            name: name.into(),
            extensions: extensions.iter().map(|ext| (*ext).to_string()).collect(),
        }
    }

    /// The patterns of [`Self::extensions`] (`*.db`).
    pub fn patterns(&self) -> Vec<String> {
        self.extensions
            .iter()
            .map(|ext| format!("*.{ext}"))
            .collect()
    }
}

#[derive(Clone)]
pub struct Dialog {
    pub dialog_type: DialogType,
    /// Folder the dialog opens in.
    directory_path: Option<PathBuf>,
    /// Title (macOS: the panel's message) instead of the default one.
    title: Option<String>,
    /// File types offered, first one selected; an "All files" entry is
    /// added after them. Ignored for folders, and on macOS.
    filters: Vec<FileFilter>,
    /// Windows: the window (`HWND`) the dialog belongs to. Ignored elsewhere
    /// (macOS uses the app's main window).
    owner: Option<isize>,
    #[cfg(target_os = "macos")]
    main_thread_marker: Option<MainThreadMarker>,
}

impl Default for Dialog {
    fn default() -> Self {
        Self::new(DialogType::File)
    }
}

impl Dialog {
    pub fn new(dialog_type: DialogType) -> Self {
        Self {
            dialog_type,
            directory_path: None,
            title: None,
            filters: Vec::new(),
            owner: None,
            #[cfg(target_os = "macos")]
            main_thread_marker: None,
        }
    }

    /// Opens the dialog in `path`.
    pub fn set_directory(&mut self, path: &Path) {
        self.directory_path = Some(path.to_path_buf());
    }

    /// Shows `title` instead of the default title.
    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = Some(title.into());
    }

    /// Windows: makes the dialog modal to `window` (its `HWND`): the window
    /// takes no input while the dialog is open, and the dialog opens over
    /// it. Without an owner the user can click the app's window while the
    /// dialog runs its own message loop on the UI thread, and the app then
    /// handles those events from inside `open_file_dialog`. Ignored on the
    /// other systems.
    pub fn set_owner(&mut self, window: isize) {
        self.owner = Some(window);
    }

    /// Offers `filter` (see [`Dialog::filters`]).
    pub fn add_filter(&mut self, filter: FileFilter) {
        self.filters.push(filter);
    }

    /// The file types offered, in order.
    pub fn filters(&self) -> &[FileFilter] {
        &self.filters
    }

    /// The title shown: the one set, or "Select a folder" / "Select a file".
    pub fn title(&self) -> String {
        self.title.clone().unwrap_or_else(|| {
            String::from(match self.dialog_type {
                DialogType::Directory => "Select a folder",
                DialogType::File => "Select a file",
            })
        })
    }

    pub fn get_directory(&self) -> Option<PathBuf> {
        self.directory_path.clone()
    }

    #[cfg(target_os = "macos")]
    #[tracing::instrument(skip(self, on_result))]
    pub fn open_file_dialog(
        &mut self,
        on_result: impl Fn(DialogResult<PathBuf>) + Send + Sync + 'static,
    ) {
        let on_result = Arc::new(on_result);
        let mtm = match self.main_thread_marker.or_else(MainThreadMarker::new) {
            Some(m) => m,
            None => {
                on_result(DialogResult::Err(
                    "Error creating Main Thread Marker".into(),
                ));
                return;
            }
        };
        self.main_thread_marker = Some(mtm);

        // The NSWindow from NSApplication, without raw-window-handle.
        let parent_window = NSApplication::sharedApplication(mtm).mainWindow();

        let Some(parent_window) = parent_window else {
            on_result(DialogResult::Err("No main window available".into()));
            return;
        };

        let panel = NSOpenPanel::openPanel(mtm);

        match self.dialog_type {
            DialogType::File => {
                panel.setCanChooseFiles(true);
                panel.setCanChooseDirectories(false);
            }
            DialogType::Directory => {
                panel.setCanChooseFiles(false);
                panel.setCanChooseDirectories(true);
            }
        }
        panel.setAllowsMultipleSelection(false);
        panel.setResolvesAliases(true);
        panel.setMessage(Some(&NSString::from_str(&self.title())));
        if let Some(directory) = &self.directory_path {
            let url = NSURL::fileURLWithPath(&NSString::from_str(&directory.to_string_lossy()));
            panel.setDirectoryURL(Some(&url));
        }

        let panel_retained = panel.clone();

        // `#[tracing::instrument]` on this function only covers the
        // synchronous setup above -- `beginSheetModalForWindow_completionHandler`
        // returns immediately, and `block` doesn't run until AppKit calls it
        // back (on this same main-thread run loop) whenever the user
        // responds, arbitrarily later. A span entered here and exited
        // inside the block would sit on tracing-tracy's per-thread span
        // stack across that whole gap, and since ordinary instrumented
        // calls elsewhere on the main thread (egui's own per-frame spans)
        // enter/exit in between, exiting it late would pop out of order and
        // corrupt the stack for every span after it. Recording the actual
        // wait as an event instead sidesteps that entirely: `start` is
        // captured by value (`Copy`), and the event's `wait_ms` field is
        // computed once the block actually fires.
        let start = Instant::now();
        let block = RcBlock::new(move |response: NSModalResponse| {
            let cancelled = response != NSModalResponseOK;
            tracing::info!(
                wait_ms = start.elapsed().as_millis() as u64,
                cancelled,
                "macOS open-panel responded"
            );
            let result = if response == NSModalResponseOK {
                let urls = panel_retained.URLs();
                urls.firstObject()
                    .and_then(|url| url.path())
                    .map(|p| DialogResult::Ok(PathBuf::from(p.to_string())))
                    .unwrap_or_else(|| DialogResult::Err("No path returned".into()))
            } else {
                DialogResult::Cancelled
            };
            on_result(result);
        });

        panel.beginSheetModalForWindow_completionHandler(&parent_window, &block);
    }

    #[cfg(target_os = "linux")]
    #[tracing::instrument(skip(self, on_result))]
    pub fn open_file_dialog(
        &mut self,
        on_result: impl Fn(DialogResult<PathBuf>) + Send + Sync + 'static,
    ) {
        // The portal is asynchronous by nature (the answer arrives as a
        // D-Bus signal), so the request runs on its own thread and the
        // callback is called there, as on macOS, without freezing the UI.
        let dialog_type = self.dialog_type;
        let start_dir = self.directory_path.clone();
        let title = self.title();
        let filters = self.filters.clone();
        std::thread::spawn(move || {
            // zbus uses Tokio's reactor (feature "tokio"), so the portal
            // request is driven by a current-thread runtime made on this
            // thread.
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(e) => {
                    on_result(DialogResult::Err(format!(
                        "Failed to start Tokio runtime: {e}"
                    )));
                    return;
                }
            };
            let outcome =
                runtime.block_on(open_portal_dialog(dialog_type, start_dir, title, filters));
            on_result(outcome);
        });
    }

    #[cfg(target_os = "windows")]
    #[tracing::instrument(skip(self, on_result))]
    pub fn open_file_dialog(
        &mut self,
        on_result: impl Fn(DialogResult<PathBuf>) + Send + Sync + 'static,
    ) {
        let dialog_type = self.dialog_type;

        let outcome: DialogResult<PathBuf> = (|| {
            let _com = try_win!(ComGuard::new());
            let dialog = try_win!(self.create_open_dialog());
            try_dialog!(self.configure_dialog(&dialog, dialog_type));
            self.show_and_get_path(&dialog)
        })();

        on_result(outcome);
    }

    // -------------------------------------------------------------
    // Safe wrappers: the `unsafe` stays in here, and they return
    // `DialogResult` instead of `windows::core::Result` so the whole
    // module speaks the same error type.
    // -------------------------------------------------------------

    #[cfg(target_os = "windows")]
    #[allow(unsafe_code)]
    fn create_open_dialog(&self) -> windows::core::Result<IFileOpenDialog> {
        // Stays a `windows::core::Result`: it is unwrapped with `try_win!`
        // before there is a dialog to build a richer `DialogResult` from.
        unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) }
    }

    #[cfg(target_os = "windows")]
    #[allow(unsafe_code)]
    fn configure_dialog(
        &self,
        dialog: &IFileOpenDialog,
        dialog_type: DialogType,
    ) -> DialogResult<()> {
        let mut flags = try_win!(unsafe { dialog.GetOptions() });

        flags |= match dialog_type {
            DialogType::Directory => FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM,
            DialogType::File => FOS_FORCEFILESYSTEM,
        };
        try_win!(unsafe { dialog.SetOptions(flags) });

        let title = HSTRING::from(self.title());
        try_win!(unsafe { dialog.SetTitle(&title) });

        if dialog_type == DialogType::File && !self.filters.is_empty() {
            // The strings must outlive `SetFileTypes`, which copies them.
            let names: Vec<HSTRING> = self
                .filters
                .iter()
                .map(|filter| HSTRING::from(filter.name.as_str()))
                .chain(std::iter::once(HSTRING::from("All files")))
                .collect();
            let specs: Vec<HSTRING> = self
                .filters
                .iter()
                .map(|filter| HSTRING::from(filter.patterns().join(";")))
                .chain(std::iter::once(HSTRING::from("*.*")))
                .collect();
            let file_types: Vec<COMDLG_FILTERSPEC> = names
                .iter()
                .zip(&specs)
                .map(|(name, spec)| COMDLG_FILTERSPEC {
                    pszName: PCWSTR(name.as_ptr()),
                    pszSpec: PCWSTR(spec.as_ptr()),
                })
                .collect();
            try_win!(unsafe { dialog.SetFileTypes(&file_types) });
            // 1-based: the first filter set.
            let _ = unsafe { dialog.SetFileTypeIndex(1) };
        }

        // A folder that doesn't exist (any more) just isn't applied.
        if let Some(directory) = self.directory_path.as_deref().filter(|dir| dir.is_dir()) {
            let path = HSTRING::from(directory);
            let folder: windows::core::Result<IShellItem> =
                unsafe { SHCreateItemFromParsingName(&path, None::<&IBindCtx>) };
            if let Ok(folder) = folder {
                let _ = unsafe { dialog.SetFolder(&folder) };
            }
        }

        DialogResult::Ok(())
    }

    /// Shows the dialog and returns the path picked, or
    /// `DialogResult::Cancelled` when the user closed it.
    #[cfg(target_os = "windows")]
    #[allow(unsafe_code)]
    fn show_and_get_path(&self, dialog: &IFileOpenDialog) -> DialogResult<PathBuf> {
        const ERROR_CANCELLED: HRESULT = HRESULT::from_win32(0x4C7);

        let owner = self
            .owner
            .map(|window| windows::Win32::Foundation::HWND(window as *mut core::ffi::c_void));
        match unsafe { dialog.Show(owner) } {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_CANCELLED => return DialogResult::Cancelled,
            Err(e) => return DialogResult::Err(e.to_string()),
        }

        let result = try_win!(unsafe { dialog.GetResult() });
        let display_name = try_win!(unsafe { result.GetDisplayName(SIGDN_FILESYSPATH) });

        // Frees the string with CoTaskMemFree whatever `to_string()` does,
        // the early return below included.
        struct CoMem(windows::core::PWSTR);
        impl Drop for CoMem {
            fn drop(&mut self) {
                unsafe { CoTaskMemFree(Some(self.0.as_ptr() as _)) };
            }
        }
        let _mem_guard = CoMem(display_name);

        let path_str = match unsafe { display_name.to_string() } {
            Ok(s) => s,
            Err(_) => {
                return DialogResult::Err(String::from("Failed to convert path to string"));
            }
        };

        DialogResult::Ok(PathBuf::from(path_str))
    }
}

// ---------------------------------------------------------------------
// Linux: the XDG Desktop Portal (D-Bus interface
// org.freedesktop.portal.FileChooser) through ashpd, pure Rust and without
// GTK. Works on GNOME, KDE, Wayland, X11 and inside Flatpak. zbus runs on
// its tokio backend, so the dialog's thread makes its own current-thread
// Tokio runtime to drive the request without freezing the UI.
// ---------------------------------------------------------------------

/// Sends the request to the portal and returns its answer as a
/// `DialogResult`.
///
/// The instrumented span's duration IS the real wait for the user's
/// response: this runs to completion on the dedicated thread
/// `open_file_dialog` (Linux) spawns for it, driven by that thread's own
/// `current_thread` runtime, so unlike a span held across an opaque
/// callback (see the equivalent macOS case), there's no other tracing
/// activity sharing this thread's span stack to get out of order with.
#[cfg(target_os = "linux")]
#[tracing::instrument]
async fn open_portal_dialog(
    dialog_type: DialogType,
    start_dir: Option<PathBuf>,
    title: String,
    filters: Vec<FileFilter>,
) -> DialogResult<PathBuf> {
    let mut request = SelectedFiles::open_file()
        .title(title.as_str())
        .modal(true)
        .multiple(false)
        .directory(dialog_type == DialogType::Directory);
    if dialog_type == DialogType::File && !filters.is_empty() {
        let portal_filters = filters
            .iter()
            .map(|filter| {
                filter
                    .patterns()
                    .iter()
                    .fold(PortalFileFilter::new(&filter.name), |portal, pattern| {
                        portal.glob(pattern)
                    })
            })
            .chain(std::iter::once(
                PortalFileFilter::new("All files").glob("*"),
            ));
        request = request.filters(portal_filters);
    }

    if let Some(dir) = start_dir {
        request = match request.current_folder(dir) {
            Ok(request) => request,
            Err(e) => return DialogResult::Err(e.to_string()),
        };
    }

    let files = match request.send().await {
        Ok(request) => match request.response() {
            Ok(files) => files,
            Err(e) => return map_portal_error(e),
        },
        Err(e) => return map_portal_error(e),
    };

    match files.uris().first() {
        Some(uri) => file_uri_to_path(uri.as_str()),
        None => DialogResult::Err(String::from("No path returned")),
    }
}

/// Maps the portal's errors: the user cancelling is
/// `DialogResult::Cancelled` and any other failure is `Err` with the
/// mensaje original.
#[cfg(target_os = "linux")]
fn map_portal_error(err: ashpd::Error) -> DialogResult<PathBuf> {
    match err {
        ashpd::Error::Response(ResponseError::Cancelled) => DialogResult::Cancelled,
        e => DialogResult::Err(e.to_string()),
    }
}

/// Turns a `file://` URI (what the portal returns) into a `PathBuf`,
/// decoding the %-escapes. Written by hand rather than pulling in the `url`
/// crate for such a narrow conversion.
#[cfg(target_os = "linux")]
fn file_uri_to_path(uri: &str) -> DialogResult<PathBuf> {
    let Some(rest) = uri.strip_prefix("file://") else {
        return DialogResult::Err(format!("Unsupported URI scheme: {uri}"));
    };
    // After the scheme comes the host: empty (the path starts with '/') or
    // "localhost". Any other host is not a local file.
    let path = match rest.split_once('/') {
        Some(("", path)) | Some(("localhost", path)) => format!("/{path}"),
        _ => return DialogResult::Err(format!("Unsupported file URI: {uri}")),
    };
    match percent_decode(&path) {
        Ok(decoded) => DialogResult::Ok(PathBuf::from(decoded)),
        Err(()) => DialogResult::Err(format!("Invalid percent-encoding in URI: {uri}")),
    }
}

/// Decodes the %-escapes of a URI path (e.g. `%20` -> ' '). The resulting
/// bytes are read as UTF-8, with replacement characters where invalid.
#[cfg(target_os = "linux")]
fn percent_decode(input: &str) -> Result<String, ()> {
    fn hex_digit(b: u8) -> Result<u8, ()> {
        match b {
            b'0'..=b'9' => Ok(b - b'0'),
            b'a'..=b'f' => Ok(b - b'a' + 10),
            b'A'..=b'F' => Ok(b - b'A' + 10),
            _ => Err(()),
        }
    }

    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(());
            }
            out.push(hex_digit(bytes[i + 1])? * 16 + hex_digit(bytes[i + 2])?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

// ---------------------------------------------------------------------
// ComGuard: RAII that guarantees CoUninitialize() whatever the way out
// (Ok, Err, a panic inside the scope, an early return...).
// ---------------------------------------------------------------------
#[cfg(target_os = "windows")]
struct ComGuard;

#[cfg(target_os = "windows")]
impl ComGuard {
    /// The one `unsafe` block that initializes COM on this thread.
    #[allow(unsafe_code)]
    fn new() -> windows::core::Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
        Ok(Self)
    }
}

#[cfg(target_os = "windows")]
impl Drop for ComGuard {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // The one place CoUninitialize is called; it runs when the scope
        // ends, whatever the way out.
        unsafe { CoUninitialize() };
    }
}

// ---------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    // ---- DialogType ----

    #[test]
    fn dialog_type_equality() {
        // Compared with `==` rather than `assert_eq!`.
        assert!(DialogType::File == DialogType::File);
        assert!(DialogType::Directory == DialogType::Directory);
        assert!(DialogType::File != DialogType::Directory);
    }

    #[test]
    fn dialog_type_is_copy() {
        let original = DialogType::Directory;
        let copied = original;
        // If DialogType weren't Copy, `original` would have been moved and
        // this comparison wouldn't compile.
        assert!(original == copied);
    }

    // ---- DialogResult: predicates ----

    #[test]
    fn dialog_result_ok_predicates() {
        let result: DialogResult<i32> = DialogResult::Ok(42);
        assert!(result.is_ok());
        assert!(!result.is_cancelled());
        assert!(!result.is_err());
    }

    #[test]
    fn dialog_result_cancelled_predicates() {
        let result: DialogResult<i32> = DialogResult::Cancelled;
        assert!(!result.is_ok());
        assert!(result.is_cancelled());
        assert!(!result.is_err());
    }

    #[test]
    fn dialog_result_err_predicates() {
        let result: DialogResult<i32> = DialogResult::Err("boom".to_string());
        assert!(!result.is_ok());
        assert!(!result.is_cancelled());
        assert!(result.is_err());
    }

    // ---- DialogResult: unwrap ----

    #[test]
    fn dialog_result_unwrap_returns_value() {
        let result: DialogResult<i32> = DialogResult::Ok(42);
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    #[should_panic(expected = "called unwrap on Cancelled")]
    fn dialog_result_unwrap_panics_on_cancelled() {
        let result: DialogResult<i32> = DialogResult::Cancelled;
        let _ = result.unwrap();
    }

    #[test]
    #[should_panic(expected = "called unwrap on Err: boom")]
    fn dialog_result_unwrap_panics_on_err() {
        let result: DialogResult<i32> = DialogResult::Err("boom".to_string());
        let _ = result.unwrap();
    }

    // ---- DialogResult: unwrap_or ----

    #[test]
    fn dialog_result_unwrap_or_returns_value_on_ok() {
        let result: DialogResult<i32> = DialogResult::Ok(42);
        assert_eq!(result.unwrap_or(7), 42);
    }

    #[test]
    fn dialog_result_unwrap_or_returns_default_on_cancelled() {
        let result: DialogResult<i32> = DialogResult::Cancelled;
        assert_eq!(result.unwrap_or(7), 7);
    }

    #[test]
    fn dialog_result_unwrap_or_returns_default_on_err() {
        let result: DialogResult<i32> = DialogResult::Err("boom".to_string());
        assert_eq!(result.unwrap_or(7), 7);
    }

    // ---- DialogResult: map ----

    #[test]
    fn dialog_result_map_transforms_ok_value() {
        let result: DialogResult<i32> = DialogResult::Ok(2);
        assert_eq!(result.map(|v| v * 10).unwrap(), 20);
    }

    #[test]
    fn dialog_result_map_keeps_cancelled() {
        let result: DialogResult<i32> = DialogResult::Cancelled;
        assert!(result.map(|v| v * 10).is_cancelled());
    }

    #[test]
    fn dialog_result_map_keeps_err() {
        let result: DialogResult<i32> = DialogResult::Err("boom".to_string());
        match result.map(|v| v * 10) {
            DialogResult::Err(e) => assert_eq!(e, "boom"),
            _ => panic!("expected DialogResult::Err"),
        }
    }

    // ---- DialogResult: ok ----

    #[test]
    fn dialog_result_ok_converts_to_some() {
        let result: DialogResult<i32> = DialogResult::Ok(42);
        assert_eq!(result.ok(), Some(42));
    }

    #[test]
    fn dialog_result_ok_converts_cancelled_to_none() {
        let result: DialogResult<i32> = DialogResult::Cancelled;
        assert_eq!(result.ok(), None);
    }

    #[test]
    fn dialog_result_ok_converts_err_to_none() {
        let result: DialogResult<i32> = DialogResult::Err("boom".to_string());
        assert_eq!(result.ok(), None);
    }

    // ---- Dialog ----

    #[test]
    fn dialog_new_has_no_directory() {
        let dialog = Dialog::new(DialogType::File);
        assert!(dialog.dialog_type == DialogType::File);
        assert_eq!(dialog.get_directory(), None);
    }

    #[test]
    fn dialog_default_is_file_type() {
        let dialog = Dialog::default();
        assert!(dialog.dialog_type == DialogType::File);
        assert_eq!(dialog.get_directory(), None);
    }

    #[test]
    fn dialog_set_and_get_directory() {
        let mut dialog = Dialog::new(DialogType::Directory);
        dialog.set_directory(Path::new("/tmp/example"));
        assert_eq!(dialog.get_directory(), Some(PathBuf::from("/tmp/example")));
    }

    #[test]
    fn dialog_set_directory_overwrites_previous_value() {
        let mut dialog = Dialog::new(DialogType::Directory);
        dialog.set_directory(Path::new("/tmp/first"));
        dialog.set_directory(Path::new("/tmp/second"));
        assert_eq!(dialog.get_directory(), Some(PathBuf::from("/tmp/second")));
    }

    // ---- Linux dialog (XDG Desktop Portal) ----

    #[cfg(target_os = "linux")]
    #[test]
    fn file_uri_to_path_basic() {
        match file_uri_to_path("file:///home/user/logs") {
            DialogResult::Ok(path) => assert_eq!(path, PathBuf::from("/home/user/logs")),
            other => panic!("expected DialogResult::Ok, got {other:?}"),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn file_uri_to_path_decodes_percent_escapes() {
        match file_uri_to_path("file:///home/user/EVE%20Logs") {
            DialogResult::Ok(path) => assert_eq!(path, PathBuf::from("/home/user/EVE Logs")),
            other => panic!("expected DialogResult::Ok, got {other:?}"),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn file_uri_to_path_decodes_utf8_multibyte() {
        match file_uri_to_path("file:///home/user/caf%C3%A9") {
            DialogResult::Ok(path) => assert_eq!(path, PathBuf::from("/home/user/café")),
            other => panic!("expected DialogResult::Ok, got {other:?}"),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn file_uri_to_path_accepts_localhost_host() {
        match file_uri_to_path("file://localhost/tmp/x") {
            DialogResult::Ok(path) => assert_eq!(path, PathBuf::from("/tmp/x")),
            other => panic!("expected DialogResult::Ok, got {other:?}"),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn file_uri_to_path_rejects_non_file_scheme() {
        assert!(file_uri_to_path("https://example.com/x").is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn file_uri_to_path_rejects_remote_host() {
        assert!(file_uri_to_path("file://nas/share/x").is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn file_uri_to_path_rejects_empty_uri() {
        assert!(file_uri_to_path("file://").is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn file_uri_to_path_rejects_invalid_hex_escape() {
        assert!(file_uri_to_path("file:///tmp/%zz").is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn file_uri_to_path_rejects_truncated_escape() {
        assert!(file_uri_to_path("file:///tmp/100%").is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn map_portal_error_cancelled_becomes_cancelled() {
        let err = ashpd::Error::Response(ResponseError::Cancelled);
        assert!(map_portal_error(err).is_cancelled());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn map_portal_error_other_response_becomes_err() {
        let err = ashpd::Error::Response(ResponseError::Other);
        assert!(map_portal_error(err).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn map_portal_error_non_response_becomes_err() {
        assert!(map_portal_error(ashpd::Error::NoResponse).is_err());
    }
}
