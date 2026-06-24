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
    fn open_dialog_macos(self) -> DialogResult<PathBuf> {
        let result;

        if self.main_thread_marker.is_none() {
            self.main_thread_marker = MainThreadMarker::new();
            if self.main_thread_marker.is_none() {
                return DialogResult::Err(String::from("Error creating Main Thread Marker"));
            }
        }

        let panel = unsafe { NSOpenPanel::openPanel(self.main_thread_marker.unwrap()) };
        unsafe {
            if self.dialog_type == DialogType::File {
                panel.setCanChooseFiles(true);
                panel.setCanChooseDirectories(false);
            }
            if self.dialog_type == DialogType::Directory {
                panel.setCanChooseFiles(false);
                panel.setCanChooseDirectories(true);
            }
            panel.setAllowsMultipleSelection(false);
            panel.setResolvesAliases(true);
            panel.setMessage(Some(ns_string!("Select a folder")));
            //panel.setPrompt(ns_string!("Choose"));
        }

        let block = RcBlock::new(move |response: isize| {
            if response == NSModalResponseOK as isize {
                // URLs() returns the selected items; take the first one
                let urls = panel.URLs();
                let result = urls.first()
                    .and_then(|url| url.path().map(|p| DialogResult::Ok(std::path::PathBuf::from(p.to_string()))));
            } else {
                result = DialogResult::Cancelled
            }
        });

        unsafe {
            panel.beginSheetModalForWindow_completionHandler(&parent_window, &block);
        }
        return result;
    }

    #[cfg(target_os = "linux")]
    fn linux_directory_selector(self) -> DialogResult<PathBuf> {
        DialogResult::Err(String::from("pending to implement"))
    }

    pub fn open_dialog(&self) -> DialogResult<PathBuf> {
        #[cfg(target_os = "windows")]
        return self.open_dialog_windows();

        #[cfg(target_os = "macos")]
        return self.clone().open_dialog_macos();

        #[cfg(target_os = "linux")]
        return self.linux_directory_selector();
    }
}
