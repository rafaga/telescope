use std::path::PathBuf;

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
use objc2_app_kit::{NSModalResponseOK, NSOpenPanel};
#[cfg(target_os = "macos")]
use objc2_foundation::{MainThreadMarker,ns_string};
#[cfg(target_os = "macos")]
use raw_window_handle::RawWindowHandle;

#[derive(PartialEq, Copy, Clone)]
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
        }
    }

    pub fn set_directory(&mut self, _path: PathBuf) {
        // Implementar si es necesario para recordar la última carpeta abierta
        self.directory_path = Some(_path);
    }

    pub fn get_directory(&self) -> Option<&PathBuf> {
        self.directory_path.as_ref()
    }

    #[cfg(target_os = "windows")]
    #[allow(unsafe_code)]
    fn open_dialog_windows(&self) -> DialogResult<PathBuf> {
        unsafe {
            if CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok() {
                let dialog: IFileOpenDialog =
                    CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
                        .expect("Failed to create FileOpenDialog instance");

                if let Ok(mut flags) = dialog.GetOptions() {
                    // FOS_PICKFOLDERS: mostrar solo carpetas
                    // FOS_FORCEFILESYSTEM: solo rutas reales del sistema de archivos
                    flags |= match self.dialog_type {
                        DialogType::Directory => FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM,
                        DialogType::File => FOS_FORCEFILESYSTEM,
                    };
                    if dialog.SetOptions(flags).is_ok() {
                        // Con FOS_PICKFOLDERS los filtros de archivo no aplican,
                        // así que omitimos SetFileTypes / SetFileTypeIndex / SetDefaultExtension

                        if self.dialog_type == DialogType::File {
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
                            dialog.SetFileTypes(&file_types).unwrap();
                            let _ = dialog.SetFileTypeIndex(1);
                            let _ = dialog.SetDefaultExtension(w!("rs"));
                        }

                        match dialog.Show(None) {
                            Ok(()) => {
                                if let Ok(result) = dialog.GetResult() {
                                    if let Ok(path) = result.GetDisplayName(SIGDN_FILESYSPATH) {
                                        if let Ok(path_str) = path.to_string() {
                                            println!("Carpeta seleccionada: {}", path_str);
                                            CoTaskMemFree(Some(path.as_ptr() as _));
                                            CoUninitialize();
                                            DialogResult::Ok(PathBuf::from(path_str))
                                        } else {
                                            CoUninitialize();
                                            DialogResult::Err(String::from(
                                                "Failed to convert path to string",
                                            ))
                                        }
                                    } else {
                                        CoUninitialize();
                                        DialogResult::Err(String::from(
                                            "Failed to get selected path",
                                        ))
                                    }
                                } else {
                                    CoUninitialize();
                                    DialogResult::Err(String::from("Failed to get dialog result"))
                                }
                            }
                            Err(e) if e.code() == HRESULT::from_win32(0x4C7) => {
                                // ERROR_CANCELLED — el usuario cerró el diálogo sin elegir
                                DialogResult::Cancelled
                            }
                            Err(e) => {
                                CoUninitialize();
                                DialogResult::Err(e.to_string())
                            }
                        }
                    } else {
                        CoUninitialize();
                        DialogResult::Err(String::from("Failed to set dialog options"))
                    }
                } else {
                    CoUninitialize();
                    DialogResult::Err(String::from("Failed to get dialog options"))
                }

                //CoUninitialize();
            } else {
                DialogResult::Err(String::from("Failed to initialize COM library"))
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[allow(unsafe_code)]
    fn open_dialog_macos(
        &mut self,
        parent_window: objc2::rc::Retained<objc2_app_kit::NSWindow>,
        on_result: impl FnOnce(DialogResult<PathBuf>) + Send + 'static,
    ) {
        // Obtener o crear el marker
        let mtm = match self.main_thread_marker
            .or_else(|| MainThreadMarker::new())
        {
            Some(m) => m,
            None => {
                on_result(DialogResult::Err("Error creating Main Thread Marker".into()));
                return;
            }
        };
        self.main_thread_marker = Some(mtm);

        let panel = unsafe { NSOpenPanel::openPanel(mtm) };

        unsafe {
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
        }

        // panel necesita vivir hasta que el closure se ejecute
        // retain() incrementa el refcount de ObjC para evitar que se libere
        let panel_retained = panel.retain();

        let block = RcBlock::new(move |response: NSModalResponse| {
            let result = if response == NSModalResponseOK {
                let urls = unsafe { panel_retained.URLs() };
                urls.first()
                    .and_then(|url| unsafe { url.path() })
                    .map(|p| DialogResult::Ok(PathBuf::from(p.to_string())))
                    .unwrap_or_else(|| DialogResult::Err("No path returned".into()))
            } else {
                DialogResult::Cancelled
            };
            on_result(result);
        });

        unsafe {
            // beginSheetModalForWindow acepta &NSWindow, Retained implementa Deref
            panel.beginSheetModalForWindow_completionHandler(&parent_window, &block);
        }
    }

    #[cfg(target_os = "linux")]
    fn linux_directory_selector(self) -> DialogResult<PathBuf> {
        DialogResult::Err(String::from("pending to implement"))
    }

    pub fn open_dialog(&self) -> DialogResult<PathBuf> {
        #[cfg(target_os = "windows")]
        return self.open_dialog_windows();

        #[cfg(target_os = "macos")]{
            let handle = frame.window_handle().ok()?;

            if let Some(window) = match handle.as_raw() {
                RawWindowHandle::AppKit(appkit_handle) => {
                    let ptr = appkit_handle.ns_window.as_ptr()
                        as *mut objc2_app_kit::NSWindow;
                    // retain() incrementa el refcount — seguro pasar al closure
                    unsafe { ptr.as_ref().map(|w| w.retain()) }
                }
                _ => None,
            }{
                let slot = Arc::clone(&self.dialog_result);
                self.file_dialog.open_dialog_macos(window, move |result| {
                    *slot.lock().unwrap() = Some(result);
                });
            }
        }
        #[cfg(target_os = "linux")]
        return self.linux_directory_selector();
    }
}
