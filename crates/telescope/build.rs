//! Embeds the application icon (`assets/icon.ico`) in the Windows executable,
//! so Explorer, the taskbar and the installer's shortcuts show it. The window
//! icon set at run time (`with_icon` in `main.rs`) only covers the open
//! window, not the `.exe` file itself.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=../../assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../../assets/icon.ico");
        if let Err(error) = resource.compile() {
            println!("cargo::warning=could not embed the Windows icon: {error}");
        }
    }
}
