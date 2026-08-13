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
pub fn get_macos_unique_id() -> Result<String, String> {
    profiling::function_scope!();

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
pub fn get_linux_unique_id() -> Result<String, String> {
    profiling::function_scope!();

    // zbus usa el reactor de Tokio (feature "tokio"), así que la cadena
    // de fallback se conduce dentro de un runtime current-thread creado
    // en un hilo aparte (igual que en dialog.rs). El hilo evita pánico
    // si el llamador ya está dentro de un runtime Tokio existente.
    let outcome = std::thread::spawn(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;

        runtime
            .block_on(zbus::get_persistent_hardware_id())
            .map(|result| result.value)
            .map_err(|e| e.to_string())
    })
    .join();

    match outcome {
        Ok(Ok(id)) => Ok(id),
        // Contrato Windows/macOS: cualquier fallo devuelve el ID de respaldo
        _ => Err(String::from(FALLBACK_UNIQUE_ID)),
    }
}

#[cfg(target_os = "windows")]
pub fn get_windows_unique_id() -> Result<String, String> {
    profiling::function_scope!();

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_unique_id_is_not_empty() {
        assert!(!FALLBACK_UNIQUE_ID.is_empty());
    }

    // Contrato igual que en Windows/macOS: Ok(id no vacío) si alguna
    // fuente funcionó, o Err(FALLBACK_UNIQUE_ID) si todas fallaron.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_unique_id_respects_fallback_contract() {
        match get_linux_unique_id() {
            Ok(id) => assert!(!id.is_empty()),
            Err(id) => assert_eq!(id, FALLBACK_UNIQUE_ID),
        }
    }
}
