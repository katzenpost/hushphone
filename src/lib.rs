//! hushphone: group chat over the Katzenpost mixnet, compatible with katzenqt, jam and
//! frank_chat, for Linux and Android.

slint::include_modules!();

pub mod backend;
pub mod bridge;
pub mod effects;
pub mod model;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub mod sandbox;
pub mod settings;
pub mod text;
pub mod voice;

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
