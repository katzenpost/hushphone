//! Every effect that uses the user's authority outside the app: the clipboard, files they pick,
//! handing a file to another program. Each takes a [`UserIntent`], which only the UI's callbacks
//! can make, so nothing that arrives from the network can cause one.

use std::path::PathBuf;

use zeroize::Zeroizing;

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

    /// Ask the desktop portal where to save a received file, and write it there.
    pub async fn save_file(_: UserIntent, bytes: Zeroizing<Vec<u8>>, name: String) -> Result<bool, String> {
        let Some(to) = rfd::AsyncFileDialog::new().set_title("Save file").set_file_name(&name).save_file().await else {
            return Ok(false);
        };
        std::fs::write(to.path(), &*bytes).map_err(|e| e.to_string())?;
        Ok(true)
    }

    /// Hand a received file to the program the desktop chooses, through the OpenURI portal,
    /// which asks the user which program to use. The other program needs the file itself, so
    /// it is written out unsealed in `dir`, which is emptied each time and when the app starts.
    pub async fn open_with(_: UserIntent, bytes: Zeroizing<Vec<u8>>, name: String, dir: PathBuf) -> Result<(), String> {
        clear_opened(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(file_name(&name));
        crate::settings::open_private(&path)
            .and_then(|mut f| std::io::Write::write_all(&mut f, &bytes))
            .map_err(|e| e.to_string())?;
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

    pub async fn save_file(_: UserIntent, _: Zeroizing<Vec<u8>>, _: String) -> Result<bool, String> {
        Err("Saving files on Android is not ready yet.".into())
    }

    pub async fn open_with(_: UserIntent, _: Zeroizing<Vec<u8>>, _: String, _: PathBuf) -> Result<(), String> {
        Err("Opening files on Android is not ready yet.".into())
    }
}

pub use imp::*;

/// A received file's name as one file in a directory of ours.
#[cfg(not(target_os = "android"))]
fn file_name(name: &str) -> String {
    let n = name.replace(['/', '\\'], "_");
    if n.is_empty() || n.starts_with('.') { format!("file{n}") } else { n }
}

/// Remove the files handed to other programs, written over first.
pub fn clear_opened(dir: &std::path::Path) {
    crate::keys::wipe(dir);
}

/// What a file's bytes say it is, never what its name or sender claims.
pub fn sniff(bytes: &[u8]) -> String {
    let head = &bytes[..bytes.len().min(512)];
    let n = head.len();
    match infer::get(head) {
        Some(t) if t.mime_type() == "audio/ogg" || t.mime_type() == "audio/opus" => "Ogg audio".into(),
        Some(t) => describe(t.mime_type(), t.extension()),
        None if n > 0 && std::str::from_utf8(head).is_ok() => "text".into(),
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
        assert_eq!(sniff(b"MZ\x90\x00\x03\x00\x00\x00\x04\x00\x00\x00\xff\xff\x00\x00"), "program");
        assert_eq!(sniff(b"%PDF-1.7\n"), "PDF document");
        assert_eq!(sniff(b"hello"), "text");
        let opus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/tone.opus");
        assert_eq!(sniff(&std::fs::read(opus).unwrap()), "Ogg audio");
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn opened_files_stay_in_their_directory() {
        assert_eq!(file_name("a.pdf"), "a.pdf");
        assert_eq!(file_name("../../.bashrc"), "file.._.._.bashrc");
        assert_eq!(file_name(".."), "file..");
        assert_eq!(file_name(""), "file");
    }
}
