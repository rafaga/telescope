//! Embeds the application icon (`assets/icon.ico`) in the Windows executable,
//! so Explorer, the taskbar and the installer's shortcuts show it. The window
//! icon set at run time (`with_icon` in `main.rs`) only covers the open
//! window, not the `.exe` file itself.
//!
//! Also writes `$OUT_DIR/licenses.rs`: every third-party crate that ends up
//! in the build for the target (normal dependencies only, found with `cargo
//! metadata` from `Cargo.lock`), with its license expression and the license
//! files shipped in its package. A package that ships none gets the
//! standard text of each license of its expression, from `licenses/` (the
//! SPDX license list, `<id>.txt`), under its authors. `Help -> Third-party
//! licenses` shows it (see `app/windows/licenses.rs`).

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=../../assets/icon.ico");
    println!("cargo::rerun-if-changed=../../Cargo.lock");
    println!("cargo::rerun-if-changed=Cargo.toml");
    println!("cargo::rerun-if-changed=licenses");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../../assets/icon.ico");
        if let Err(error) = resource.compile() {
            println!("cargo::warning=could not embed the Windows icon: {error}");
        }
    }
    write_licenses();
}

/// A crate of the build and its license.
struct Package {
    name: String,
    version: String,
    /// SPDX expression from its `Cargo.toml` (`MIT OR Apache-2.0`).
    license: String,
    repository: String,
    /// Its license files, one after the other, or the standard texts.
    text: String,
    /// `text` is the standard text of its licenses (it ships no file).
    standard: bool,
}

fn write_licenses() {
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set for build scripts"))
        .join("licenses.rs");
    let packages = match third_party_packages() {
        Ok(packages) => packages,
        Err(error) => {
            // The app still builds, with an empty list.
            println!("cargo::warning=could not list the dependency licenses: {error}");
            Vec::new()
        }
    };

    // The same text (the MIT or Apache license of a family of crates) is
    // stored once.
    let mut texts: Vec<&str> = Vec::new();
    let mut text_index: HashMap<&str, usize> = HashMap::new();
    let mut code = String::from("pub(crate) static PACKAGES: &[LicensedPackage] = &[\n");
    for package in &packages {
        let text = *text_index.entry(package.text.as_str()).or_insert_with(|| {
            texts.push(package.text.as_str());
            texts.len() - 1
        });
        let _ = writeln!(
            code,
            "    LicensedPackage {{ name: {:?}, version: {:?}, license: {:?}, repository: {:?}, text: {text}, standard: {} }},",
            package.name, package.version, package.license, package.repository, package.standard,
        );
    }
    code.push_str("];\n\npub(crate) static TEXTS: &[&str] = &[\n");
    for text in texts {
        let _ = writeln!(code, "    {text:?},");
    }
    code.push_str("];\n");
    if let Err(error) = std::fs::write(&out, code) {
        panic!("could not write {}: {error}", out.display());
    }
}

/// The crates the app is built with for the target: normal dependencies,
/// followed from this package, without the workspace's own crates (not
/// build scripts' or tests' dependencies, which don't ship).
fn third_party_packages() -> Result<Vec<Package>, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| String::from("cargo"));
    let target = std::env::var("TARGET").map_err(|error| error.to_string())?;
    let manifest = Path::new(&std::env::var("CARGO_MANIFEST_DIR").map_err(|e| e.to_string())?)
        .join("Cargo.toml");
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--locked", "--offline"])
        .args(["--filter-platform", &target])
        .arg("--manifest-path")
        .arg(&manifest)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    let metadata: Value =
        serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;

    let packages: HashMap<&str, &Value> = metadata["packages"]
        .as_array()
        .ok_or("no packages in cargo metadata")?
        .iter()
        .filter_map(|package| Some((package["id"].as_str()?, package)))
        .collect();
    let nodes: HashMap<&str, &Value> = metadata["resolve"]["nodes"]
        .as_array()
        .ok_or("no dependency graph in cargo metadata")?
        .iter()
        .filter_map(|node| Some((node["id"].as_str()?, node)))
        .collect();
    let own_name = std::env::var("CARGO_PKG_NAME").map_err(|error| error.to_string())?;
    let root = packages
        .iter()
        .find(|(_, package)| package["name"] == own_name.as_str() && package["source"].is_null())
        .map(|(id, _)| *id)
        .ok_or("this package is not in cargo metadata")?;

    // Walks the normal dependencies from this package.
    let mut seen: HashSet<&str> = HashSet::from([root]);
    let mut queue = vec![root];
    while let Some(id) = queue.pop() {
        let Some(deps) = nodes.get(id).and_then(|node| node["deps"].as_array()) else {
            continue;
        };
        for dep in deps {
            let normal = dep["dep_kinds"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind["kind"].is_null()));
            if let Some(pkg) = dep["pkg"].as_str()
                && normal
                && seen.insert(pkg)
            {
                queue.push(pkg);
            }
        }
    }

    let mut result: Vec<Package> = seen
        .into_iter()
        .filter_map(|id| packages.get(id))
        // Crates with no source are this workspace's own.
        .filter(|package| !package["source"].is_null())
        .map(|package| {
            let dir = package["manifest_path"]
                .as_str()
                .map(Path::new)
                .and_then(Path::parent)
                .map(Path::to_path_buf)
                .unwrap_or_default();
            let license_file = package["license_file"].as_str().map(|file| dir.join(file));
            let license = string(package, "license");
            let mut text = license_text(&dir, license_file.as_deref());
            let standard = text.is_empty();
            if standard {
                let authors: Vec<&str> = package["authors"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                text = standard_text(&license, &authors);
            }
            Package {
                name: string(package, "name"),
                version: string(package, "version"),
                license,
                repository: string(package, "repository"),
                text,
                standard,
            }
        })
        .collect();
    result.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    Ok(result)
}

fn string(package: &Value, field: &str) -> String {
    package[field].as_str().unwrap_or_default().to_owned()
}

/// The standard text of every license in the SPDX `expression` (`MIT OR
/// Apache-2.0`, `MIT/Apache-2.0`) found in `licenses/`, after a line naming
/// the package's `authors` as the copyright holders.
fn standard_text(expression: &str, authors: &[&str]) -> String {
    let mut text = String::new();
    if !authors.is_empty() {
        let _ = writeln!(text, "Copyright holders: {}", authors.join(", "));
    }
    let mut seen = HashSet::new();
    let ids = expression
        .split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '/'))
        .filter(|id| !id.is_empty() && !matches!(*id, "OR" | "AND" | "WITH"));
    for id in ids {
        if !seen.insert(id) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(Path::new("licenses").join(format!("{id}.txt")))
        else {
            continue;
        };
        if !text.is_empty() {
            text.push('\n');
        }
        let _ = writeln!(text, "── {id} ──\n");
        text.push_str(content.trim_end());
        text.push('\n');
    }
    text.trim_end().to_owned()
}

/// The license files of the package in `dir` (`LICENSE`, `LICENSE-MIT`,
/// `COPYING`, `NOTICE`...) and its `license-file`, each under its name.
fn license_text(dir: &Path, license_file: Option<&Path>) -> String {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().to_ascii_uppercase())
                .unwrap_or_default();
            path.is_file()
                && ["LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE"]
                    .iter()
                    .any(|prefix| name.starts_with(prefix))
        })
        .collect();
    if let Some(file) = license_file
        && file.is_file()
        && !files.iter().any(|known| known == file)
    {
        files.push(file.to_path_buf());
    }
    files.sort();
    let mut text = String::new();
    for file in files {
        let Ok(content) = std::fs::read_to_string(&file) else {
            continue;
        };
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        if let Some(name) = file.file_name() {
            let _ = writeln!(text, "── {} ──\n", name.to_string_lossy());
        }
        text.push_str(content.trim_end());
    }
    text
}
