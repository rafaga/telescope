//! Identificación persistente de hardware en Linux con detección de disponibilidad de D-Bus.
//!
//! Estrategia (de mayor a menor confiabilidad):
//!   1. UUID de DMI/SMBIOS (`/sys/class/dmi/id/product_uuid`) — requiere root.
//!   2. D-Bus system bus -> `org.freedesktop.hostname1` -> `GetProductUUID` (con root vía polkit)
//!      o `org.freedesktop.machine1` para variantes de machine-id.
//!   3. `/etc/machine-id` vía lectura directa de archivo (sin D-Bus, sin root).
//!   4. `/var/lib/dbus/machine-id` (symlink legado, mismo contenido que /etc/machine-id).
//!   5. Serial de placa/chasis (`board_serial`, `product_serial`) como último recurso.

// Garantía a nivel de compilador: este módulo es 100% Rust seguro — sin
// bloques `unsafe` ni bloques `extern "C"` (que en edición 2024 requieren
// `unsafe` de todos modos).
#![forbid(unsafe_code)]

use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;

// ============================================================================
// Jerarquía de errores (implementada a mano, sin thiserror)
// ============================================================================

#[cfg(target_os = "linux")]
#[derive(Debug)]
pub enum HwIdError {
    FileRead {
        path: String,
        source: std::io::Error,
    },

    EmptyOrPlaceholder {
        path: String,
    },

    PermissionDenied {
        path: String,
    },

    DbusUnavailable(DbusError),

    AllSourcesExhausted {
        sources_tried: Vec<String>,
    },
}

#[cfg(target_os = "linux")]
impl fmt::Display for HwIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HwIdError::FileRead { path, source } => {
                write!(
                    f,
                    "no se pudo leer el archivo de identificador: {path}: {source}"
                )
            }
            HwIdError::EmptyOrPlaceholder { path } => {
                write!(
                    f,
                    "el valor leído en {path} está vacío o es un placeholder inválido"
                )
            }
            HwIdError::PermissionDenied { path } => {
                write!(
                    f,
                    "permisos insuficientes para leer {path} (requiere root o CAP_DAC_OVERRIDE)"
                )
            }
            HwIdError::DbusUnavailable(source) => {
                write!(f, "D-Bus no está disponible: {source}")
            }
            HwIdError::AllSourcesExhausted { sources_tried } => {
                write!(
                    f,
                    "ninguna fuente de identificador produjo un valor válido; fuentes intentadas: {sources_tried:?}"
                )
            }
        }
    }
}

#[cfg(target_os = "linux")]
impl Error for HwIdError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            HwIdError::FileRead { source, .. } => Some(source),
            HwIdError::DbusUnavailable(source) => Some(source),
            _ => None,
        }
    }
}

#[cfg(target_os = "linux")]
impl From<DbusError> for HwIdError {
    fn from(source: DbusError) -> Self {
        HwIdError::DbusUnavailable(source)
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
pub enum DbusError {
    SessionBusNotFound,

    SystemBusSocketMissing(String),

    ConnectionFailed(String),

    MethodCallFailed {
        interface: String,
        method: String,
        detail: String,
    },

    PolicyDenied(String),
}

#[cfg(target_os = "linux")]
impl fmt::Display for DbusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbusError::SessionBusNotFound => write!(
                f,
                "variable de entorno DBUS_SESSION_BUS_ADDRESS no definida y socket por defecto ausente"
            ),
            DbusError::SystemBusSocketMissing(path) => {
                write!(f, "socket del system bus no encontrado en {path}")
            }
            DbusError::ConnectionFailed(detail) => {
                write!(f, "fallo al conectar con el bus: {detail}")
            }
            DbusError::MethodCallFailed {
                interface,
                method,
                detail,
            } => {
                write!(
                    f,
                    "el método D-Bus '{method}' en '{interface}' falló: {detail}"
                )
            }
            DbusError::PolicyDenied(method) => {
                write!(
                    f,
                    "acceso denegado por policy de D-Bus/polkit al llamar '{method}'"
                )
            }
        }
    }
}

#[cfg(target_os = "linux")]
impl Error for DbusError {}

/// Resultado tipado análogo a tu `DialogResult<T>` de Windows: separa el valor
/// exitoso del error Y conserva metadata sobre qué fuente lo produjo.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
pub struct HwIdResult<T> {
    pub value: T,
    pub source: IdSource,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdSource {
    DmiUuidDirect,
    DbusHostname1,
    DbusMachine1,
    EtcMachineId,
    VarLibDbusMachineId,
    DmiBoardSerial,
    DmiProductSerial,
}

#[cfg(target_os = "linux")]
impl fmt::Display for IdSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            IdSource::DmiUuidDirect => "DMI product_uuid (lectura directa)",
            IdSource::DbusHostname1 => "D-Bus org.freedesktop.hostname1",
            IdSource::DbusMachine1 => "D-Bus org.freedesktop.machine1",
            IdSource::EtcMachineId => "/etc/machine-id",
            IdSource::VarLibDbusMachineId => "/var/lib/dbus/machine-id",
            IdSource::DmiBoardSerial => "DMI board_serial",
            IdSource::DmiProductSerial => "DMI product_serial",
        };
        write!(f, "{s}")
    }
}

// ============================================================================
// Detección de disponibilidad de D-Bus
// ============================================================================

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbusAvailability {
    pub session_bus_available: bool,
    pub system_bus_available: bool,
    pub session_bus_address: Option<String>,
    pub system_bus_socket_path: Option<String>,
}

#[cfg(target_os = "linux")]
impl DbusAvailability {
    pub fn any_available(&self) -> bool {
        self.session_bus_available || self.system_bus_available
    }
}

/// Detección *estática* (sin abrir conexión real): revisa variables de entorno
/// y existencia de sockets Unix. Es rápida y no dispara alertas de EDR porque
/// no hace I/O de red ni llamadas D-Bus reales.
#[cfg(target_os = "linux")]
pub fn detect_dbus_static() -> DbusAvailability {
    let session_bus_address = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok();

    let session_bus_available = match &session_bus_address {
        Some(addr) => dbus_address_socket_exists(addr),
        None => {
            // Fallback al path convencional cuando la env var no está seteada
            // (algunos entornos headless/systemd la omiten pero el socket existe).
            let uid = rustix::process::getuid();
            let default_path = format!("/run/user/{uid}/bus");
            Path::new(&default_path).exists()
        }
    };

    let system_bus_socket_path = "/run/dbus/system_bus_socket";
    let system_bus_available = Path::new(system_bus_socket_path).exists();

    DbusAvailability {
        session_bus_available,
        system_bus_available,
        session_bus_address,
        system_bus_socket_path: system_bus_available.then(|| system_bus_socket_path.to_string()),
    }
}

#[cfg(target_os = "linux")]
fn dbus_address_socket_exists(addr: &str) -> bool {
    // Formato típico: "unix:path=/run/user/1000/bus,guid=..."
    addr.split(',')
        .find_map(|part| part.strip_prefix("unix:path="))
        .map(|p| Path::new(p).exists())
        .unwrap_or(false)
}

/// Detección *dinámica*: intenta abrir una conexión real al system bus y
/// hacer una llamada de bajo costo (`Peer.Ping`) para confirmar que no solo
/// existe el socket, sino que hay un daemon respondiendo del otro lado.
/// Esto es lo que realmente quieres antes de intentar `GetProductUUID`.
#[cfg(target_os = "linux")]
pub async fn detect_dbus_live() -> Result<DbusAvailability, DbusError> {
    let static_check = detect_dbus_static();

    let system_bus_available = if static_check.system_bus_available {
        match zbus::Connection::system().await {
            Ok(conn) => ping_peer(&conn, "org.freedesktop.DBus").await.is_ok(),
            Err(_) => false,
        }
    } else {
        false
    };

    let session_bus_available = if static_check.session_bus_available {
        match zbus::Connection::session().await {
            Ok(conn) => ping_peer(&conn, "org.freedesktop.DBus").await.is_ok(),
            Err(_) => false,
        }
    } else {
        false
    };

    Ok(DbusAvailability {
        session_bus_available,
        system_bus_available,
        ..static_check
    })
}

#[cfg(target_os = "linux")]
async fn ping_peer(conn: &zbus::Connection, destination: &str) -> zbus::Result<()> {
    let reply = conn
        .call_method(
            Some(destination),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus.Peer"),
            "Ping",
            &(),
        )
        .await?;
    reply.body().deserialize::<()>()?;
    Ok(())
}

// ============================================================================
// Fuentes individuales de identificador
// ============================================================================
#[cfg(target_os = "linux")]
const DMI_UUID_PATH: &str = "/sys/class/dmi/id/product_uuid";
#[cfg(target_os = "linux")]
const DMI_BOARD_SERIAL_PATH: &str = "/sys/class/dmi/id/board_serial";
#[cfg(target_os = "linux")]
const DMI_PRODUCT_SERIAL_PATH: &str = "/sys/class/dmi/id/product_serial";
#[cfg(target_os = "linux")]
const ETC_MACHINE_ID_PATH: &str = "/etc/machine-id";
#[cfg(target_os = "linux")]
const VAR_LIB_DBUS_MACHINE_ID_PATH: &str = "/var/lib/dbus/machine-id";

/// Placeholders comunes que fabricantes dejan sin llenar en el firmware.
#[cfg(target_os = "linux")]
const KNOWN_PLACEHOLDERS: &[&str] = &[
    "to be filled by o.e.m.",
    "not specified",
    "default string",
    "system serial number",
    "00000000-0000-0000-0000-000000000000",
    "none",
];

#[cfg(target_os = "linux")]
fn is_placeholder(value: &str) -> bool {
    let normalized = value.trim().to_lowercase();
    normalized.is_empty() || KNOWN_PLACEHOLDERS.contains(&normalized.as_str())
}

#[cfg(target_os = "linux")]
fn read_dmi_field(path: &str) -> Result<String, HwIdError> {
    let content = fs::read_to_string(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            HwIdError::PermissionDenied {
                path: path.to_string(),
            }
        } else {
            HwIdError::FileRead {
                path: path.to_string(),
                source: e,
            }
        }
    })?;

    let trimmed = content.trim().to_string();
    if is_placeholder(&trimmed) {
        return Err(HwIdError::EmptyOrPlaceholder {
            path: path.to_string(),
        });
    }
    Ok(trimmed)
}

#[cfg(target_os = "linux")]
pub fn try_dmi_uuid_direct() -> Result<HwIdResult<String>, HwIdError> {
    read_dmi_field(DMI_UUID_PATH).map(|value| HwIdResult {
        value,
        source: IdSource::DmiUuidDirect,
    })
}

#[cfg(target_os = "linux")]
pub fn try_etc_machine_id() -> Result<HwIdResult<String>, HwIdError> {
    read_dmi_field(ETC_MACHINE_ID_PATH).map(|value| HwIdResult {
        value,
        source: IdSource::EtcMachineId,
    })
}

#[cfg(target_os = "linux")]
pub fn try_var_lib_dbus_machine_id() -> Result<HwIdResult<String>, HwIdError> {
    read_dmi_field(VAR_LIB_DBUS_MACHINE_ID_PATH).map(|value| HwIdResult {
        value,
        source: IdSource::VarLibDbusMachineId,
    })
}

#[cfg(target_os = "linux")]
pub fn try_board_serial() -> Result<HwIdResult<String>, HwIdError> {
    read_dmi_field(DMI_BOARD_SERIAL_PATH).map(|value| HwIdResult {
        value,
        source: IdSource::DmiBoardSerial,
    })
}

#[cfg(target_os = "linux")]
pub fn try_product_serial() -> Result<HwIdResult<String>, HwIdError> {
    read_dmi_field(DMI_PRODUCT_SERIAL_PATH).map(|value| HwIdResult {
        value,
        source: IdSource::DmiProductSerial,
    })
}

/// Vía D-Bus: `org.freedesktop.hostname1.GetProductUUID`. Esta es la ruta
/// "oficial" de systemd para obtener el UUID de producto sin depender de
/// permisos de archivo directos — el daemon `systemd-hostnamed` ya corre
/// como root y expone el dato vía policy de polkit, que puede permitir
/// lectura a usuarios normales dependiendo de la configuración del sistema.
#[cfg(target_os = "linux")]
pub async fn try_dbus_hostname1_uuid() -> Result<HwIdResult<String>, HwIdError> {
    let conn = zbus::Connection::system()
        .await
        .map_err(|e| HwIdError::DbusUnavailable(DbusError::ConnectionFailed(e.to_string())))?;

    let reply = conn
        .call_method(
            Some("org.freedesktop.hostname1"),
            "/org/freedesktop/hostname1",
            Some("org.freedesktop.hostname1"),
            "GetProductUUID",
            &(true,), // interactive=true permite prompt de polkit si aplica
        )
        .await
        .map_err(|e| {
            let msg = e.to_string();
            let dbus_err = if msg.contains("AccessDenied") || msg.contains("NotAuthorized") {
                DbusError::PolicyDenied("hostname1.GetProductUUID".to_string())
            } else {
                DbusError::MethodCallFailed {
                    interface: "org.freedesktop.hostname1".to_string(),
                    method: "GetProductUUID".to_string(),
                    detail: msg,
                }
            };
            HwIdError::DbusUnavailable(dbus_err)
        })?;

    let raw_bytes: Vec<u8> = reply.body().deserialize::<Vec<u8>>().map_err(|e| {
        HwIdError::DbusUnavailable(DbusError::MethodCallFailed {
            interface: "org.freedesktop.hostname1".to_string(),
            method: "GetProductUUID".to_string(),
            detail: format!("deserialización de respuesta falló: {e}"),
        })
    })?;

    let uuid_str = uuid_bytes_to_string(&raw_bytes);
    if is_placeholder(&uuid_str) {
        return Err(HwIdError::EmptyOrPlaceholder {
            path: "dbus:org.freedesktop.hostname1/GetProductUUID".to_string(),
        });
    }

    Ok(HwIdResult {
        value: uuid_str,
        source: IdSource::DbusHostname1,
    })
}

#[cfg(target_os = "linux")]
fn uuid_bytes_to_string(bytes: &[u8]) -> String {
    if bytes.len() != 16 {
        return String::new();
    }
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15],
    )
}

// ============================================================================
// Orquestador: cadena de fallback completa
// ============================================================================

/// Intenta cada fuente en orden de confiabilidad y regresa la primera que
/// funcione, junto con metadata de qué se intentó y por qué falló lo demás.
#[cfg(target_os = "linux")]
pub async fn get_persistent_hardware_id() -> Result<HwIdResult<String>, HwIdError> {
    let mut attempts_log: Vec<String> = Vec::new();

    // 1. Lectura directa de DMI (requiere root, pero es instantánea sin D-Bus)
    match try_dmi_uuid_direct() {
        Ok(result) => return Ok(result),
        Err(e) => attempts_log.push(format!("{}: {e}", IdSource::DmiUuidDirect)),
    }

    // 2. D-Bus hostname1 (funciona sin root si polkit lo permite)
    let dbus_status = detect_dbus_live().await.unwrap_or(DbusAvailability {
        session_bus_available: false,
        system_bus_available: false,
        session_bus_address: None,
        system_bus_socket_path: None,
    });

    if dbus_status.system_bus_available {
        match try_dbus_hostname1_uuid().await {
            Ok(result) => return Ok(result),
            Err(e) => attempts_log.push(format!("{}: {e}", IdSource::DbusHostname1)),
        }
    } else {
        attempts_log.push(format!(
            "{}: system bus no disponible ({:?})",
            IdSource::DbusHostname1,
            dbus_status
        ));
    }

    // 3. /etc/machine-id (sin root, sin D-Bus — el más portable)
    match try_etc_machine_id() {
        Ok(result) => return Ok(result),
        Err(e) => attempts_log.push(format!("{}: {e}", IdSource::EtcMachineId)),
    }

    // 4. Symlink legado
    match try_var_lib_dbus_machine_id() {
        Ok(result) => return Ok(result),
        Err(e) => attempts_log.push(format!("{}: {e}", IdSource::VarLibDbusMachineId)),
    }

    // 5. Seriales de hardware como último recurso
    match try_board_serial() {
        Ok(result) => return Ok(result),
        Err(e) => attempts_log.push(format!("{}: {e}", IdSource::DmiBoardSerial)),
    }

    match try_product_serial() {
        Ok(result) => return Ok(result),
        Err(e) => attempts_log.push(format!("{}: {e}", IdSource::DmiProductSerial)),
    }

    Err(HwIdError::AllSourcesExhausted {
        sources_tried: attempts_log,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn placeholder_detection() {
        assert!(is_placeholder(""));
        assert!(is_placeholder("  "));
        assert!(is_placeholder("To Be Filled By O.E.M."));
        assert!(is_placeholder("00000000-0000-0000-0000-000000000000"));
        assert!(!is_placeholder("4c4c4544-0044-3010-8035-b9c04f435931"));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn dbus_static_detection_does_not_panic() {
        // Solo verifica que la detección estática corre sin error en CI,
        // sin asumir que D-Bus esté presente en el runner.
        let _ = detect_dbus_static();
    }

    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn full_chain_resolves_or_reports_all_failures() {
        // En CI sin permisos root ni D-Bus, esto normalmente cae hasta
        // /etc/machine-id, que casi siempre existe en contenedores modernos.
        let result = get_persistent_hardware_id().await;
        match result {
            Ok(r) => println!("ID obtenido vía {}: {}", r.source, r.value),
            Err(HwIdError::AllSourcesExhausted { sources_tried }) => {
                println!("Todas las fuentes fallaron:\n{}", sources_tried.join("\n"));
            }
            Err(e) => panic!("error inesperado: {e}"),
        }
    }
}
