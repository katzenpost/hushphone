//! Every effect that uses the user's authority outside the app: the clipboard, the file they
//! pick to send, the file they save. Each takes a [`UserIntent`], which only the UI's callbacks
//! can make, so nothing that arrives from the network can cause one.
//!
//! On Linux, files pass through one directory of the user's, `~/hushphone`: a file to send is
//! put in `send/` first and picked from there, and a received file is saved into `received/`.
//! The app reads and writes nowhere else, and its sandbox holds it to that. On Android, the
//! system's document picker hands over one file at a time, to read or to write.

use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

use crate::bridge::UserIntent;

/// A file the user picked to send.
pub struct Picked {
    pub name: String,
    pub bytes: Zeroizing<Vec<u8>>,
}

/// A file in the send directory, for the user to pick from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub size: u64,
}

/// The most a file sent through a group can hold.
pub const FILE_MAX: u64 = 16 * 1024 * 1024;

/// Where files to send are put, under the files directory.
pub const SEND: &str = "send";
/// Where received files are saved, under the files directory.
pub const RECEIVED: &str = "received";

/// A received file's name as one file in a directory of ours: no path separators, not hidden.
pub fn file_name(name: &str) -> String {
    let n = name.replace(['/', '\\'], "_");
    if n.is_empty() || n.starts_with('.') { format!("file{n}") } else { n }
}

/// `name` as a file right under `dir`: one plain component, nothing that leaves the directory.
fn under(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let rel = Path::new(name);
    let mut parts = rel.components();
    match (parts.next(), parts.next()) {
        (Some(std::path::Component::Normal(_)), None) if !name.starts_with('.') => Ok(dir.join(rel)),
        _ => Err(format!("{name:?} is not a file in the directory")),
    }
}

/// `name`, or `name (2)`, `name (3)`… as the first that is not yet in `dir`, created there with
/// mode 0600 and never over another file.
fn create_fresh(dir: &Path, name: &str) -> std::io::Result<(std::fs::File, PathBuf)> {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for n in 1.. {
        let candidate = if n == 1 { name.to_string() } else { format!("{stem} ({n}){ext}") };
        let path = dir.join(&candidate);
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        match o.open(&path) {
            Ok(f) => return Ok((f, path)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

/// Regular files in `dir/send`, by name: no directories, links or hidden files.
pub fn list_files(_: &UserIntent, dir: &Path) -> Result<Vec<Entry>, String> {
    let send = dir.join(SEND);
    std::fs::create_dir_all(&send).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for e in std::fs::read_dir(&send).map_err(|e| e.to_string())? {
        let e = e.map_err(|e| e.to_string())?;
        let Ok(name) = e.file_name().into_string() else { continue };
        if name.starts_with('.') {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
        if !meta.is_file() {
            continue;
        }
        out.push(Entry { name, size: meta.len() });
    }
    out.sort_by_key(|e| e.name.to_lowercase());
    Ok(out)
}

/// Read the file `name` in `dir/send`, which must be a regular file of at most [`FILE_MAX`].
pub fn pick_file(_: &UserIntent, dir: &Path, name: &str) -> Result<Picked, String> {
    use std::io::Read;
    let path = under(&dir.join(SEND), name)?;
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NOFOLLOW);
    }
    let f = o.open(&path).map_err(|e| format!("can't read {name}: {e}"))?;
    let meta = f.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err(format!("{name} is not a regular file"));
    }
    if meta.len() > FILE_MAX {
        return Err(format!("{name} is too large to send: 16 MiB at most."));
    }
    let mut bytes = Zeroizing::new(Vec::with_capacity(meta.len() as usize));
    f.take(FILE_MAX + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > FILE_MAX {
        return Err(format!("{name} is too large to send: 16 MiB at most."));
    }
    Ok(Picked { name: crate::text::identifier(name, 200), bytes })
}

/// Write a received file into `dir/received` under its name, never over another file. Returns
/// where it went.
pub fn save_file(_: &UserIntent, dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    use std::io::Write;
    let received = dir.join(RECEIVED);
    std::fs::create_dir_all(&received).map_err(|e| e.to_string())?;
    let (mut f, path) = create_fresh(&received, &file_name(name)).map_err(|e| e.to_string())?;
    f.write_all(bytes).and_then(|()| f.sync_all()).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(not(target_os = "android"))]
mod imp {
    use super::*;

    pub fn copy_text(_: &UserIntent, text: &str) -> Result<(), String> {
        use arboard::SetExtLinux;
        let mut c = arboard::Clipboard::new().map_err(|e| e.to_string())?;
        // A voucher code is a secret: clipboard managers are asked not to keep it.
        c.set().exclude_from_history().text(text.to_string()).map_err(|e| e.to_string())
    }

    pub fn paste_text(_: &UserIntent) -> Result<String, String> {
        let mut c = arboard::Clipboard::new().map_err(|e| e.to_string())?;
        c.get_text().map_err(|e| e.to_string())
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
}

pub use imp::*;

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

    fn intent() -> UserIntent {
        UserIntent::for_test()
    }

    #[test]
    fn types_come_from_the_bytes() {
        assert_eq!(sniff(b"MZ\x90\x00\x03\x00\x00\x00\x04\x00\x00\x00\xff\xff\x00\x00"), "program");
        assert_eq!(sniff(b"%PDF-1.7\n"), "PDF document");
        assert_eq!(sniff(b"hello"), "text");
        let opus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/tone.opus");
        assert_eq!(sniff(&std::fs::read(opus).unwrap()), "Ogg audio");
    }

    #[test]
    fn saved_files_stay_in_their_directory() {
        assert_eq!(file_name("a.pdf"), "a.pdf");
        assert_eq!(file_name("../../.bashrc"), "file.._.._.bashrc");
        assert_eq!(file_name(".."), "file..");
        assert_eq!(file_name(""), "file");
    }

    #[test]
    fn only_plain_files_in_send_are_listed_and_picked() {
        let d = crate::test_dir();
        let send = d.path().join(SEND);
        std::fs::create_dir_all(&send).unwrap();
        std::fs::write(send.join("report.pdf"), b"%PDF").unwrap();
        std::fs::write(send.join(".hidden"), b"x").unwrap();
        std::fs::create_dir(send.join("sub")).unwrap();
        std::fs::write(d.path().join("outside"), b"secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(d.path().join("outside"), send.join("link")).unwrap();
        let names: Vec<_> = list_files(&intent(), d.path()).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["report.pdf"]);
        assert_eq!(&*pick_file(&intent(), d.path(), "report.pdf").unwrap().bytes, b"%PDF");
        for bad in ["../outside", "sub/x", ".hidden", "link", "/etc/passwd", ""] {
            assert!(pick_file(&intent(), d.path(), bad).is_err(), "{bad:?} was read");
        }
    }

    #[test]
    fn saving_never_overwrites_and_keeps_to_received() {
        let d = crate::test_dir();
        let a = save_file(&intent(), d.path(), "../.bashrc", b"one").unwrap();
        assert_eq!(a, d.path().join(RECEIVED).join("file.._.bashrc"));
        let b = save_file(&intent(), d.path(), "notes.txt", b"one").unwrap();
        let c = save_file(&intent(), d.path(), "notes.txt", b"two").unwrap();
        assert_eq!(b.file_name().unwrap(), "notes.txt");
        assert_eq!(c.file_name().unwrap(), "notes (2).txt");
        assert_eq!(std::fs::read(&b).unwrap(), b"one");
        assert_eq!(std::fs::read(&c).unwrap(), b"two");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&c).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }
}
