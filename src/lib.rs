//! hushphone: group chat over the Katzenpost mixnet, compatible with katzenqt, jam and
//! frank_chat, for Linux and Android.

slint::include_modules!();

pub mod backend;
pub mod bridge;
pub mod contacts;
pub mod effects;
pub mod fskv;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub mod harden;
pub mod keys;
pub mod mesh;
pub mod model;
pub mod ping;
pub mod qr;
pub mod radio;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub mod sandbox;
pub mod settings;
pub mod sound;
pub mod text;
pub mod voice;
// The kernel structures it uses are laid out for 64-bit targets.
#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
pub mod webcam;

#[cfg(target_os = "android")]
mod android;

pub use bridge::Setup;

/// Open the window and run until it closes.
pub fn run(setup: Setup) -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    bridge::start(&ui, setup);
    ui.run()
}

#[cfg(test)]
pub(crate) fn test_dir() -> tempfile::TempDir {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-tmp");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::tempdir_in(base).unwrap()
}
