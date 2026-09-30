//! Machine identification on Linux, with detection of whether D-Bus is
//! available.
//!
//! [`get_persistent_hardware_id`] tries the hardware first (most specific):
//!   1. The DMI/SMBIOS UUID (`/sys/class/dmi/id/product_uuid`) -- needs root.
//!   2. System bus -> `org.freedesktop.hostname1` -> `GetProductUUID` (root
//!      through polkit, when its policy allows it).
//!   3. `/etc/machine-id`, read directly (no D-Bus, no root).
//!   4. `/var/lib/dbus/machine-id` (legacy symlink, same content).
//!   5. The board / product serial (`board_serial`, `product_serial`).
//!
//! [`get_stable_machine_id`] tries the same sources with the machine-id files
//! first: they are readable by every user, so the answer doesn't depend on
//! whether the program runs as root or on the polkit policy. Use it for
//! anything derived from the identifier that has to stay the same (a key).

// Compiler-checked: this module is 100% safe Rust -- no `unsafe` blocks nor
// `extern "C"` blocks (which need `unsafe` in edition 2024 anyway).
#![forbid(unsafe_code)]

use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;

// ============================================================================
// Error types (by hand, without thiserror)
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
                write!(f, "could not read the identifier file {path}: {source}")
            }
            HwIdError::EmptyOrPlaceholder { path } => {
                write!(f, "the value read from {path} is empty or a placeholder")
            }
            HwIdError::PermissionDenied { path } => {
                write!(
                    f,
                    "not allowed to read {path} (needs root or CAP_DAC_OVERRIDE)"
                )
            }
            HwIdError::DbusUnavailable(source) => {
                write!(f, "D-Bus is not available: {source}")
            }
            HwIdError::AllSourcesExhausted { sources_tried } => {
                write!(
                    f,
                    "no identifier source gave a valid value; sources tried: {sources_tried:?}"
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
                "DBUS_SESSION_BUS_ADDRESS is not set and the default socket is missing"
            ),
            DbusError::SystemBusSocketMissing(path) => {
                write!(f, "system bus socket not found at {path}")
            }
            DbusError::ConnectionFailed(detail) => {
                write!(f, "could not connect to the bus: {detail}")
            }
            DbusError::MethodCallFailed {
                interface,
                method,
                detail,
            } => {
                write!(
                    f,
                    "D-Bus method '{method}' on '{interface}' failed: {detail}"
                )
            }
            DbusError::PolicyDenied(method) => {
                write!(f, "D-Bus/polkit policy denied calling '{method}'")
            }
        }
    }
}

#[cfg(target_os = "linux")]
impl Error for DbusError {}

/// An identifier and the source it came from.
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
            IdSource::DmiUuidDirect => "DMI product_uuid (read directly)",
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
// D-Bus availability
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

/// *Static* detection (no connection is opened): checks the environment
/// variables and whether the Unix sockets exist. Fast, and it trips no EDR
/// alert since it does no network I/O nor real D-Bus calls.
#[cfg(target_os = "linux")]
#[tracing::instrument]
pub fn detect_dbus_static() -> DbusAvailability {
    let session_bus_address = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok();

    let session_bus_available = match &session_bus_address {
        Some(addr) => dbus_address_socket_exists(addr),
        None => {
            // The conventional path when the variable isn't set (some
            // headless/systemd environments leave it out, but the socket
            // exists).
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
    // Usually "unix:path=/run/user/1000/bus,guid=..."
    addr.split(',')
        .find_map(|part| part.strip_prefix("unix:path="))
        .map(|p| Path::new(p).exists())
        .unwrap_or(false)
}

/// *Live* detection: opens a real connection to each bus and makes a cheap
/// call (`Peer.Ping`) to confirm not only that the socket exists but that a
/// daemon answers on it. What to check before trying `GetProductUUID`.
#[cfg(target_os = "linux")]
#[tracing::instrument]
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
#[tracing::instrument]
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
// Identifier sources
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

/// Placeholders manufacturers commonly leave in the firmware.
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
#[tracing::instrument]
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

/// Through D-Bus: `org.freedesktop.hostname1.GetProductUUID`, systemd's
/// "official" way to get the product UUID without file permissions: the
/// `systemd-hostnamed` daemon runs as root and hands it out as its polkit
/// policy allows, which may include normal users. Non-interactive: polkit
/// never asks the user for a password here.
#[cfg(target_os = "linux")]
#[tracing::instrument]
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
            &(false,), // interactive = false: no polkit password prompt
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
            detail: format!("could not read the reply: {e}"),
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
// The fallback chains
// ============================================================================

/// Tries each source, hardware first (see the module docs), and returns the
/// first that works; the error lists what was tried and why it failed.
#[cfg(target_os = "linux")]
#[tracing::instrument]
pub async fn get_persistent_hardware_id() -> Result<HwIdResult<String>, HwIdError> {
    let mut attempts_log: Vec<String> = Vec::new();

    // 1. DMI read directly (needs root, but instant and without D-Bus)
    match try_dmi_uuid_direct() {
        Ok(result) => return Ok(result),
        Err(e) => attempts_log.push(format!("{}: {e}", IdSource::DmiUuidDirect)),
    }

    // 2. D-Bus hostname1 (works without root when polkit allows it)
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
            "{}: system bus not available ({:?})",
            IdSource::DbusHostname1,
            dbus_status
        ));
    }

    // 3. /etc/machine-id (no root, no D-Bus: the most portable)
    match try_etc_machine_id() {
        Ok(result) => return Ok(result),
        Err(e) => attempts_log.push(format!("{}: {e}", IdSource::EtcMachineId)),
    }

    // 4. Legacy symlink
    match try_var_lib_dbus_machine_id() {
        Ok(result) => return Ok(result),
        Err(e) => attempts_log.push(format!("{}: {e}", IdSource::VarLibDbusMachineId)),
    }

    // 5. Hardware serials, as a last resort
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

/// Like [`get_persistent_hardware_id`], with the machine-id files first:
/// every user can read them, so the identifier doesn't change with the
/// privileges the program runs with (root can read the DMI UUID, a normal
/// user usually can't) or with the polkit policy. The hardware sources are
/// only tried on a system without a machine id.
#[cfg(target_os = "linux")]
#[tracing::instrument]
pub async fn get_stable_machine_id() -> Result<HwIdResult<String>, HwIdError> {
    let mut attempts_log: Vec<String> = Vec::new();
    for source in [try_etc_machine_id, try_var_lib_dbus_machine_id] {
        match source() {
            Ok(result) => return Ok(result),
            Err(e) => attempts_log.push(e.to_string()),
        }
    }
    get_persistent_hardware_id()
        .await
        .map_err(|error| match error {
            HwIdError::AllSourcesExhausted { mut sources_tried } => {
                attempts_log.append(&mut sources_tried);
                HwIdError::AllSourcesExhausted {
                    sources_tried: attempts_log,
                }
            }
            other => other,
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
        // Only checks that static detection runs, without assuming D-Bus
        // is there on the runner.
        let _ = detect_dbus_static();
    }

    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn the_stable_id_prefers_the_machine_id() {
        if let Ok(machine_id) = try_etc_machine_id() {
            let stable = get_stable_machine_id().await.unwrap();
            assert_eq!(stable.source, IdSource::EtcMachineId);
            assert_eq!(stable.value, machine_id.value);
        }
    }

    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn full_chain_resolves_or_reports_all_failures() {
        // Without root nor D-Bus (CI) this usually ends at /etc/machine-id,
        // which nearly every modern container has.
        let result = get_persistent_hardware_id().await;
        match result {
            Ok(r) => println!("id from {}: {}", r.source, r.value),
            Err(HwIdError::AllSourcesExhausted { sources_tried }) => {
                println!("every source failed:\n{}", sources_tried.join("\n"));
            }
            Err(e) => panic!("unexpected error: {e}"),
        }
    }
}
