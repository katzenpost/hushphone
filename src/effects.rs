//! Every effect that uses the user's authority outside the app: the clipboard, files they pick,
//! handing a file to another program. Each takes a [`UserIntent`], which only the UI's callbacks
//! can make, so nothing that arrives from the network can cause one.

use std::path::{Path, PathBuf};

use crate::bridge::UserIntent;

/// A file the user picked to send.
pub struct Picked {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// The most a file sent through a group can hold.
pub const FILE_MAX: u64 = 16 * 1024 * 1024;

#[cfg(not(target_os = "android"))]
mod imp {
    use super::*;

    pub fn copy_text(_: &UserIntent, text: &str) -> Result<(), String> {
        let mut c = arboard::Clipboard::new().map_err(|e| e.to_string())?;
        c.set_text(text.to_string()).map_err(|e| e.to_string())
    }

    pub fn paste_text(_: &UserIntent) -> Result<String, String> {
        let mut c = arboard::Clipboard::new().map_err(|e| e.to_string())?;
        c.get_text().map_err(|e| e.to_string())
    }

    /// Ask the desktop portal for one file, and read it.
    pub async fn pick_file(_: UserIntent) -> Result<Option<Picked>, String> {
        let Some(f) = rfd::AsyncFileDialog::new().set_title("Send a file").pick_file().await else {
            return Ok(None);
        };
        let size = std::fs::metadata(f.path()).map(|m| m.len()).unwrap_or(0);
        if size > FILE_MAX {
            return Err(format!("{} is too large to send: 16 MiB at most.", f.file_name()));
        }
        Ok(Some(Picked { name: f.file_name(), bytes: f.read().await }))
    }

    /// Ask the desktop portal where to save a received file, and copy it there.
    pub async fn save_file(_: UserIntent, from: PathBuf, name: String) -> Result<bool, String> {
        let Some(to) = rfd::AsyncFileDialog::new().set_title("Save file").set_file_name(&name).save_file().await else {
            return Ok(false);
        };
        std::fs::copy(&from, to.path()).map_err(|e| e.to_string())?;
        Ok(true)
    }

    /// Hand a received file to the program the desktop chooses, through the OpenURI portal,
    /// which asks the user which program to use.
    pub async fn open_with(_: UserIntent, path: PathBuf) -> Result<(), String> {
        let f = std::fs::File::open(&path).map_err(|e| e.to_string())?;
        ashpd::desktop::open_uri::OpenFileRequest::default()
            .ask(true)
            .send_file(&f)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(target_os = "android")]
mod imp {
    use super::*;

    pub fn copy_text(_: &UserIntent, text: &str) -> Result<(), String> {
        crate::android::copy_text(text, true)
    }

    pub fn paste_text(_: &UserIntent) -> Result<String, String> {
        crate::android::paste_text()
    }

    pub async fn pick_file(_: UserIntent) -> Result<Option<Picked>, String> {
        Err("Sending files from Android is not ready yet.".into())
    }

    pub async fn save_file(_: UserIntent, _: PathBuf, _: String) -> Result<bool, String> {
        Err("Saving files on Android is not ready yet.".into())
    }

    pub async fn open_with(_: UserIntent, _: PathBuf) -> Result<(), String> {
        Err("Opening files on Android is not ready yet.".into())
    }
}

pub use imp::*;

/// What a file's bytes say it is, never what its name or sender claims.
pub fn sniff(path: &Path) -> String {
    let mut head = [0u8; 512];
    let n = std::fs::File::open(path)
        .and_then(|mut f| std::io::Read::read(&mut f, &mut head))
        .unwrap_or(0);
    match infer::get(&head[..n]) {
        Some(t) if t.mime_type() == "audio/ogg" || t.mime_type() == "audio/opus" => "Ogg audio".into(),
        Some(t) => describe(t.mime_type(), t.extension()),
        None if n > 0 && std::str::from_utf8(&head[..n]).is_ok() => "text".into(),
        None => "file".into(),
    }
}

fn describe(mime: &str, ext: &str) -> String {
    match mime {
        "application/pdf" => "PDF document".into(),
        m if m.starts_with("image/") => format!("{} image", ext.to_uppercase()),
        m if m.starts_with("audio/") => format!("{} audio", ext.to_uppercase()),
        m if m.starts_with("video/") => format!("{} video", ext.to_uppercase()),
        "application/zip" => "ZIP archive".into(),
        "application/x-executable" | "application/vnd.microsoft.portable-executable" | "application/x-mach-binary" => {
            "program".into()
        }
        _ => format!("{} file", ext.to_uppercase()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_come_from_the_bytes() {
        let dir = crate::test_dir();
        let exe = dir.path().join("invoice.pdf");
        std::fs::write(&exe, b"MZ\x90\x00\x03\x00\x00\x00\x04\x00\x00\x00\xff\xff\x00\x00").unwrap();
        assert_eq!(sniff(&exe), "program");
        let pdf = dir.path().join("x.bin");
        std::fs::write(&pdf, b"%PDF-1.7\n").unwrap();
        assert_eq!(sniff(&pdf), "PDF document");
        let txt = dir.path().join("a.png");
        std::fs::write(&txt, b"hello").unwrap();
        assert_eq!(sniff(&txt), "text");
        let opus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/tone.opus");
        assert_eq!(sniff(&opus), "Ogg audio");
    }
}
