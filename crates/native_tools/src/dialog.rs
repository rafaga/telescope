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
use objc2_foundation::{MainThreadMarker, ns_string};
#[cfg(target_os = "macos")]
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::time::Instant;

#[cfg(target_os = "linux")]
use ashpd::desktop::ResponseError;
#[cfg(target_os = "linux")]
use ashpd::desktop::file_chooser::SelectedFiles;

// ---------------------------------------------------------------------
// Macros helper para propagar errores sin depender del trait Try
// (que solo está disponible en nightly para tipos custom como
// DialogResult). Reemplazan al operador `?` en este contexto.
// ---------------------------------------------------------------------

/// Convierte un `windows::core::Result<T>` en `T`, o hace un `return`
/// anticipado con `DialogResult::Err(..)` si falla.
#[cfg(target_os = "windows")]
macro_rules! try_win {
    ($expr:expr) => {
        match $expr {
            Ok(v) => v,
            Err(e) => return DialogResult::Err(e.to_string()),
        }
    };
}

/// Propaga un `DialogResult<T>`: si es `Ok`, extrae el valor; si es
/// `Cancelled` o `Err`, hace `return` anticipado con esa misma variante.
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

#[derive(Clone)]
pub struct Dialog {
    pub dialog_type: DialogType,
    directory_path: Option<PathBuf>, // para recordar la última carpeta abierta
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
            #[cfg(target_os = "macos")]
            main_thread_marker: None,
            //dialog_result: None,
        }
    }

    pub fn set_directory(&mut self, path: &Path) {
        // Implementar si es necesario para recordar la última carpeta abierta
        self.directory_path = Some(path.to_path_buf());
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

        // Obtener NSWindow desde NSApplication — sin raw-window-handle
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
        panel.setMessage(Some(ns_string!("Select a folder")));

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
        // El portal es inherentemente asíncrono (la respuesta llega por
        // una señal de D-Bus), así que la petición se ejecuta en un hilo
        // aparte y el callback se invoca ahí, igual que en macOS, sin
        // congelar la UI.
        let dialog_type = self.dialog_type;
        let start_dir = self.directory_path.clone();
        std::thread::spawn(move || {
            // zbus usa el reactor de Tokio (feature "tokio"), así que la
            // petición al portal se conduce dentro de un runtime
            // current-thread creado en este hilo.
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
            let outcome = runtime.block_on(open_portal_dialog(dialog_type, start_dir));
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
    // Wrappers "safe": el unsafe queda contenido aquí adentro, y
    // ahora devuelven DialogResult directamente en vez de
    // windows::core::Result, para que todo el módulo hable el
    // mismo "idioma" de error.
    // -------------------------------------------------------------

    #[cfg(target_os = "windows")]
    #[allow(unsafe_code)]
    fn create_open_dialog(&self) -> windows::core::Result<IFileOpenDialog> {
        // Esta se queda en windows::core::Result porque se usa con
        // try_win! justo antes de que exista un "dialog" válido con
        // el que construir un DialogResult con más contexto.
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

        if dialog_type == DialogType::File {
            let file_types = [
                COMDLG_FILTERSPEC {
                    pszName: w!("Rust Source Files"),
                    pszSpec: w!("*.rs"),
                },
                COMDLG_FILTERSPEC {
                    pszName: w!("All Files"),
                    pszSpec: w!("*.*"),
                },
            ];
            try_win!(unsafe { dialog.SetFileTypes(&file_types) });
            unsafe {
                let _ = dialog.SetFileTypeIndex(1);
                let _ = dialog.SetDefaultExtension(w!("rs"));
            }
        }

        DialogResult::Ok(())
    }

    /// Muestra el diálogo y devuelve la ruta elegida, o
    /// `DialogResult::Cancelled` si el usuario cerró el diálogo.
    #[cfg(target_os = "windows")]
    #[allow(unsafe_code)]
    fn show_and_get_path(&self, dialog: &IFileOpenDialog) -> DialogResult<PathBuf> {
        const ERROR_CANCELLED: HRESULT = HRESULT::from_win32(0x4C7);

        match unsafe { dialog.Show(None) } {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_CANCELLED => return DialogResult::Cancelled,
            Err(e) => return DialogResult::Err(e.to_string()),
        }

        let result = try_win!(unsafe { dialog.GetResult() });
        let display_name = try_win!(unsafe { result.GetDisplayName(SIGDN_FILESYSPATH) });

        // Guard local para asegurar CoTaskMemFree pase lo que pase
        // con to_string(), incluyendo el return anticipado de abajo.
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
// Implementación de Linux: XDG Desktop Portal (interfaz D-Bus
// org.freedesktop.portal.FileChooser) a través de ashpd, en Rust puro
// y sin arrastrar GTK. Funciona en GNOME, KDE, Wayland, X11 e incluso
// dentro de Flatpak. Se usa el backend tokio de zbus, así que el hilo
// del diálogo crea su propio runtime Tokio current-thread para
// conducir la petición sin congelar la UI.
// ---------------------------------------------------------------------

/// Ejecuta la petición al portal y devuelve el resultado en el
/// "idioma" de DialogResult.
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
) -> DialogResult<PathBuf> {
    let mut request = SelectedFiles::open_file()
        .title("Select a folder")
        .modal(true)
        .multiple(false)
        .directory(dialog_type == DialogType::Directory);

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

/// Mapea los errores del portal: la cancelación del usuario es
/// `DialogResult::Cancelled` y cualquier otro fallo es `Err` con el
/// mensaje original.
#[cfg(target_os = "linux")]
fn map_portal_error(err: ashpd::Error) -> DialogResult<PathBuf> {
    match err {
        ashpd::Error::Response(ResponseError::Cancelled) => DialogResult::Cancelled,
        e => DialogResult::Err(e.to_string()),
    }
}

/// Convierte una URI `file://` (lo que devuelve el portal) en un
/// `PathBuf`, decodificando los %-escapes. Se implementa a mano para
/// no arrastrar el crate `url` por una conversión tan acotada.
#[cfg(target_os = "linux")]
fn file_uri_to_path(uri: &str) -> DialogResult<PathBuf> {
    let Some(rest) = uri.strip_prefix("file://") else {
        return DialogResult::Err(format!("Unsupported URI scheme: {uri}"));
    };
    // Tras el esquema viene el host: vacío (la ruta empieza con '/')
    // o "localhost". Cualquier otro host no es un archivo local.
    let path = match rest.split_once('/') {
        Some(("", path)) | Some(("localhost", path)) => format!("/{path}"),
        _ => return DialogResult::Err(format!("Unsupported file URI: {uri}")),
    };
    match percent_decode(&path) {
        Ok(decoded) => DialogResult::Ok(PathBuf::from(decoded)),
        Err(()) => DialogResult::Err(format!("Invalid percent-encoding in URI: {uri}")),
    }
}

/// Decodifica los %-escapes de una ruta URI (p. ej. `%20` -> ' ').
/// Los bytes resultantes se interpretan como UTF-8 con reemplazo.
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
// ComGuard: RAII que garantiza CoUninitialize() sin importar el path
// de salida (Ok, Err, panic durante el scope, return anticipado, etc.)
// ---------------------------------------------------------------------
#[cfg(target_os = "windows")]
struct ComGuard;

#[cfg(target_os = "windows")]
impl ComGuard {
    /// Único bloque `unsafe` para inicializar COM en este hilo.
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
        // Único lugar donde se llama CoUninitialize. Se ejecuta
        // automáticamente al salir del scope, sin importar el camino.
        unsafe { CoUninitialize() };
    }
}

// ---------------------------------------------------------------------
// Pruebas unitarias
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    // ---- DialogType ----

    #[test]
    fn dialog_type_equality() {
        // DialogType no implementa Debug, así que se comparan con `==`
        // en lugar de assert_eq!.
        assert!(DialogType::File == DialogType::File);
        assert!(DialogType::Directory == DialogType::Directory);
        assert!(DialogType::File != DialogType::Directory);
    }

    #[test]
    fn dialog_type_is_copy() {
        let original = DialogType::Directory;
        let copied = original;
        // Si DialogType no fuera Copy, `original` quedaría movido
        // y esta comparación no compilaría.
        assert!(original == copied);
    }

    // ---- DialogResult: predicados ----

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

    // ---- Diálogo de Linux (XDG Desktop Portal) ----

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
        match file_uri_to_path("file:///home/usuario/caf%C3%A9") {
            DialogResult::Ok(path) => assert_eq!(path, PathBuf::from("/home/usuario/café")),
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
