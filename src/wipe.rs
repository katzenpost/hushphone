//! The emergency wipe, crypto-erase: the key that seals the state is destroyed first, then the
//! wrapped copy of it, then the sealed state as ordinary files. Without the key, whatever flash
//! storage still holds of the state cannot be read. The bridge blanks the screen before, and
//! drops what it holds and ends the process after. A small marker, with nothing secret in it, is
//! left so that the next start tells the user what was destroyed.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::keys::Keeper;
use crate::settings::shred;

/// The marker's name in the data directory.
pub const MARKER: &str = "wiped.toml";

/// What hushphone keeps in its data directory, all of it removed. Nothing else there is touched:
/// on Linux the directory can be one the user named.
const KEPT: &[&str] = &[
    "state",
    "state.key",
    "state.key.tmp",
    "settings.toml",
    "settings.toml.broken",
    "settings.toml.tmp",
    "reticulum",
    "kpclientd.sock",
    "cache",
];

/// What the wipe did, step by step: "" for done, or why not. Kept in the marker.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Wiped {
    /// Deleting the key that sealed the state, in the Keystore or the keyring.
    pub key: String,
    /// Whether the Keystore's attestation said that deletion is enforced in hardware.
    pub hardware_key_deletion: bool,
    /// Whether there was a `state.key` to write over and delete (Android).
    pub key_file: bool,
    /// Writing over and deleting `state.key`.
    pub key_file_error: String,
    /// Deleting the state and everything else hushphone kept.
    pub files: String,
}

/// Destroy the key the state in `data_dir/state` is sealed with, kept by `k`, then the files.
/// Each step is tried even if one before it failed; what became of each is in the result, which
/// is also left in the marker.
pub async fn wipe(k: &impl Keeper, data_dir: &Path, cache_dir: &Path, hardware_key_deletion: bool) -> Wiped {
    let state = data_dir.join("state");
    let mut w = Wiped { hardware_key_deletion, ..Wiped::default() };
    w.key = k.forget(&state).await.err().unwrap_or_default();
    let key_file = state.with_extension("key");
    w.key_file = key_file.exists();
    w.key_file_error = shred(&key_file).err().map(|e| e.to_string()).unwrap_or_default();
    let mut failed = Vec::new();
    for name in KEPT {
        let p = data_dir.join(name);
        let r = match std::fs::symlink_metadata(&p) {
            Ok(m) if m.is_dir() => std::fs::remove_dir_all(&p),
            Ok(_) => std::fs::remove_file(&p),
            Err(_) => Ok(()),
        };
        if let Err(e) = r {
            failed.push(format!("{}: {e}", p.display()));
        }
    }
    if let Err(e) = std::fs::remove_dir_all(cache_dir)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        failed.push(format!("{}: {e}", cache_dir.display()));
    }
    w.files = failed.join("; ");
    if let Ok(text) = toml::to_string(&w) {
        let _ = std::fs::create_dir_all(data_dir);
        let _ = std::fs::write(data_dir.join(MARKER), text);
    }
    w
}

/// The marker a wipe left in `data_dir`, if there is one.
pub fn marker(data_dir: &Path) -> Option<Wiped> {
    let text = std::fs::read_to_string(data_dir.join(MARKER)).ok()?;
    Some(toml::from_str(&text).unwrap_or_default())
}

/// The marker read and shown: gone.
pub fn forget_marker(data_dir: &Path) {
    let _ = std::fs::remove_file(data_dir.join(MARKER));
}

/// What was destroyed, and how, for the screen after a wipe.
pub fn report(w: &Wiped) -> String {
    let done = |e: &str, what: &str| if e.is_empty() { what.to_string() } else { format!("NOT done: {e}.") };
    let (store, key_file) = if cfg!(target_os = "android") {
        ("the Android Keystore", "state.key, the copy of that key wrapped by the Keystore's: ")
    } else {
        ("the keyring (Secret Service)", "")
    };
    let mut lines = vec![format!(
        "• The key that sealed your chats, in {store}: {} Hardware-enforced key deletion: {}.",
        done(&w.key, "deleted."),
        if w.hardware_key_deletion { "yes" } else { "no" }
    )];
    if w.key_file || !w.key_file_error.is_empty() {
        lines.push(format!("• {key_file}{}", done(&w.key_file_error, "written over and deleted.")));
    }
    lines.push(format!(
        "• Your groups, messages, contacts, settings and attachments, sealed with XChaCha20-Poly1305 under that key: {} Without the key, what flash storage may still hold of them can't be decrypted.",
        done(&w.files, "deleted.")
    ));
    lines.push("• Memory: hushphone let go of its keys and messages and the process ended; the system clears its memory before reusing it.".into());
    lines.push(format!(
        "Not covered: {}hushphone itself, which stays installed, so the phone shows it was used; and files you saved or shared out of hushphone.",
        if cfg!(target_os = "android") { "kpclientd-android and what it keeps; " } else { "kpclientd and what it keeps; " }
    ));
    lines.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::tests::{Fake, block};

    fn tree(dir: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            out.push(e.path().display().to_string());
            if e.path().is_dir() {
                tree(&e.path(), out);
            }
        }
    }

    #[test]
    fn wipe_destroys_the_key_then_the_files_and_leaves_a_marker() {
        let base = crate::test_dir();
        let data = base.path().join("data");
        let state = data.join("state");
        let cache = base.path().join("cache");
        let k = Fake::default();
        let key = block(crate::keys::custody(&k, &state)).unwrap();
        let files = crate::backend::Attachments::new(state.clone(), Some(key));
        let s = crate::settings::Settings { name: "alice".into(), ..Default::default() };
        s.save(&files).unwrap();
        files.write("contacts.toml", b"[petnames]\n0a0b = \"Bob\"\n").unwrap();
        std::fs::write(state.with_extension("key"), b"HPKEY\x02wrapped").unwrap();
        std::fs::create_dir_all(cache.join("voice")).unwrap();
        std::fs::write(cache.join("voice/clip"), b"words").unwrap();
        // Not hushphone's: left alone.
        std::fs::write(data.join("notes.txt"), b"mine").unwrap();
        assert!(!k.keys.lock().unwrap().is_empty());

        let w = block(wipe(&k, &data, &cache, true));
        assert_eq!(w, Wiped { hardware_key_deletion: true, key_file: true, ..Wiped::default() });
        assert!(k.keys.lock().unwrap().is_empty(), "the key is gone");
        let mut left = Vec::new();
        tree(&data, &mut left);
        left.sort();
        assert_eq!(left, [data.join("notes.txt").display().to_string(), data.join(MARKER).display().to_string()]);
        assert!(!cache.exists());
        let m = marker(&data).unwrap();
        assert_eq!(m, w);
        let text = std::fs::read_to_string(data.join(MARKER)).unwrap();
        assert!(!text.contains("alice") && !text.contains("Bob"));
        let r = report(&m);
        assert!(r.contains("deleted") && r.contains("Hardware-enforced key deletion: yes") && r.contains("Not covered"));
        forget_marker(&data);
        assert!(marker(&data).is_none());
    }

    #[test]
    fn a_step_that_fails_is_reported_and_the_rest_still_done() {
        let base = crate::test_dir();
        let data = base.path().join("data");
        std::fs::create_dir_all(data.join("state")).unwrap();
        let k = Fake { broken: true, ..Fake::default() };
        let w = block(wipe(&k, &data, &base.path().join("cache"), false));
        assert!(!w.key.is_empty());
        assert!(!data.join("state").exists());
        assert!(report(&w).contains("NOT done"));
    }
}
