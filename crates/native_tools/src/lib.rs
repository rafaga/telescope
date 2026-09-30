//! Platform-specific helpers for Telescope.
//!
//! * [`dialog`]: native open file / folder dialogs behind one API.
//! * `zbus` (Linux only): stable machine identification via DMI, D-Bus and
//!   machine-id fallbacks.
//! * `get_*_unique_id`: a per-machine identifier for each OS, with a fixed
//!   fallback value when the OS cannot provide one. It must not change
//!   between runs: the player database key is derived from it. Windows:
//!   `SystemIdentification::GetSystemIdForPublisher`; macOS: the platform
//!   serial number; Linux: the machine id (see `zbus::get_stable_machine_id`).

pub mod dialog;
#[cfg(target_os = "linux")]
pub mod zbus;

#[cfg(target_os = "windows")]
use windows::{Storage::Streams::DataReader, System::Profile::SystemIdentification};

#[cfg(target_os = "macos")]
use objc2_core_foundation::{CFAllocator, CFString};

#[cfg(target_os = "macos")]
use objc2_io_kit::{
    IOObjectRelease, IORegistryEntryCreateCFProperty, IOServiceGetMatchingService,
    IOServiceMatching, kIOMainPortDefault,
};

const FALLBACK_UNIQUE_ID: &str = "t313/sc0p3";

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
#[tracing::instrument]
pub fn get_macos_unique_id() -> Result<String, String> {
    // macOS unique ID
    unsafe {
        // 1. The platform's IORegistry entry
        let matching = IOServiceMatching(c"IOPlatformExpertDevice".as_ptr())
            .map(|matching| (&matching).into());
        let entry = IOServiceGetMatchingService(kIOMainPortDefault, matching);

        if entry != 0 {
            // 2. The property name as a CFString
            let key = CFString::from_str("IOPlatformSerialNumber");

            // 3. Read the property
            let cf_value = IORegistryEntryCreateCFProperty(
                entry,
                Some(&key),
                None::<&CFAllocator>, // the default allocator
                0,                    // options = 0
            );

            // 4. Release the entry
            IOObjectRelease(entry);

            // 5. CFType -> CFString -> Rust String
            if let Some(retained) = cf_value
                && let Ok(cf_str) = retained.downcast::<CFString>()
            {
                return Ok(cf_str.to_string());
            }
        }
    }
    Err(String::from(FALLBACK_UNIQUE_ID))
}

#[cfg(target_os = "linux")]
#[tracing::instrument]
pub fn get_linux_unique_id() -> Result<String, String> {
    // zbus uses Tokio's reactor (feature "tokio"), so the chain runs in a
    // current-thread runtime on a thread of its own (as in dialog.rs); the
    // thread also avoids a panic when the caller is already inside a Tokio
    // runtime.
    let outcome = std::thread::spawn(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;

        runtime
            .block_on(zbus::get_stable_machine_id())
            .map(|result| result.value)
            .map_err(|e| e.to_string())
    })
    .join();

    match outcome {
        Ok(Ok(id)) => Ok(id),
        // Same contract as Windows/macOS: any failure returns the fallback.
        _ => Err(String::from(FALLBACK_UNIQUE_ID)),
    }
}

#[cfg(target_os = "windows")]
#[tracing::instrument]
pub fn get_windows_unique_id() -> Result<String, String> {
    // The machine's identifier for this publisher; the player database key
    // is derived from it (webb's `esi::cipher`).
    match SystemIdentification::GetSystemIdForPublisher() {
        Ok(info) => {
            if let Ok(id_buffer) = info.Id()
                && let Ok(reader) = DataReader::FromBuffer(&id_buffer)
            {
                // reading bytes from ID IBuffer
                if let Ok(length) = id_buffer.Length() {
                    let mut bytes = vec![0u8; length as usize];
                    if let Ok(()) = reader.ReadBytes(&mut bytes) {
                        return Ok(String::from_utf8_lossy(&bytes).into_owned());
                    }
                }
            }
            Err(String::from(FALLBACK_UNIQUE_ID))
        }
        Err(_) => Err(String::from(FALLBACK_UNIQUE_ID)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_unique_id_is_not_empty() {
        assert!(!FALLBACK_UNIQUE_ID.is_empty());
    }

    // Same contract as Windows/macOS: Ok(a non-empty id) when a source
    // worked, Err(FALLBACK_UNIQUE_ID) when every one failed.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_unique_id_respects_fallback_contract() {
        match get_linux_unique_id() {
            Ok(id) => assert!(!id.is_empty()),
            Err(id) => assert_eq!(id, FALLBACK_UNIQUE_ID),
        }
    }
}
