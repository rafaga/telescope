pub mod dialog;

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
pub fn get_macos_unique_id() -> Result<String, String> {
    // macOS unique ID
    unsafe {
        // 1. Obtener el entry del IORegistry para la plataforma
        let matching = IOServiceMatching(c"IOPlatformExpertDevice".as_ptr())
            .map(|matching| (&matching).into());
        let entry = IOServiceGetMatchingService(kIOMainPortDefault, matching);

        if entry != 0 {
            // 2. Construir la clave como CFString
            let key = CFString::from_str("IOPlatformSerialNumber");

            // 3. Llamar a IORegistryEntryCreateCFProperty
            let cf_value = IORegistryEntryCreateCFProperty(
                entry,
                Some(&key),
                None::<&CFAllocator>, // usar el allocator por defecto
                0,                    // options = 0
            );

            // 4. Liberar el entry
            IOObjectRelease(entry);

            // 5. Convertir el CFType resultante a CFString y luego a String de Rust
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
fn get_linux_unique_id() -> Result<String, String> {
    // Placeholder implementation for Linux unique ID
    Ok(String::from(FALLBACK_UNIQUE_ID))
}

#[cfg(target_os = "windows")]
pub fn get_windows_unique_id() -> Result<String, String> {
    // this get a unique ID for the user, and its used to generate a unique key
    // for the database encryption
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
