//! The user's own settings: the name, the daemon's address, the transport and the theme. They are
//! kept sealed with the state, as [`crate::contacts`] keeps the names given to members, so none of
//! it can be read without the state's key. Until the state is open, the defaults stand.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::backend::Attachments;

/// The file's name under the state directory; and an older version's unsealed file of the same
/// name in the data directory.
pub const FILE: &str = "settings.toml";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub name: String,
    pub daemon: String,
    pub theme: ThemeChoice,
    /// Android: `reticulum` for the mesh, through `replica`; the mixnet otherwise.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub transport: String,
    /// The replica's Reticulum TCP interface, `host:port`; namenlos's bridge when empty.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub replica: String,
    /// The katzenqt1 wire version written and minted, 3 or 4; 4 when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wire_version: Option<u64>,
    /// Android: keep hushphone's screens out of screenshots, screen recordings and the
    /// recent-apps preview. Opt-in: off when unset.
    pub screen_security: bool,
    /// Where an older version kept petnames; moved into the sealed contacts at start and
    /// emptied.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub petnames: BTreeMap<String, String>,
    /// Likewise, the last line read in each group.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub seen: BTreeMap<String, usize>,
    /// Android: whether the Keystore's attestation says it enforces in hardware that a deleted
    /// key stays deleted (rollback resistance); unknown until checked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hardware_key_deletion: Option<bool>,
}

impl Settings {
    /// The settings kept with the state, after an older version's unsealed `settings.toml` in
    /// `data_dir` is moved in and written over and removed; the defaults when there are none yet.
    /// What went wrong, for the user.
    pub fn open(data_dir: &Path, files: &Attachments) -> (Settings, Option<String>) {
        let loaded = Settings::load(files);
        let unread = loaded.is_err();
        let (kept, mut note) = match loaded {
            Ok(s) => (s, None),
            Err(e) => (None, Some(format!("Your settings could not be read, so the defaults are used: {e}"))),
        };
        let old = data_dir.join(FILE);
        let moved = match std::fs::read_to_string(&old) {
            Ok(text) => match toml::from_str::<Settings>(&text) {
                Ok(s) => Some(s),
                Err(e) => {
                    note = Some(format!("The old settings could not be read, so the defaults are used: {e}"));
                    None
                }
            },
            Err(_) => None,
        };
        if let (None, Some(s)) = (&kept, &moved) {
            // Never over a file that could not be read, which may be from a newer hushphone; and
            // the old file stays until its settings are kept sealed.
            let sealed = if unread { Err("the sealed settings are in the way".into()) } else { s.save(files) };
            if let Err(e) = sealed {
                return (s.clone(), Some(note.unwrap_or_else(|| format!("Couldn't seal your settings: {e}"))));
            }
        }
        for f in [old.clone(), old.with_extension("toml.broken")] {
            if let Err(e) = shred(&f) {
                note = Some(format!("Couldn't remove the old settings at {}: {e}", f.display()));
            }
        }
        (kept.or(moved).unwrap_or_default(), note)
    }

    /// The settings kept with the state, or none yet.
    pub fn load(files: &Attachments) -> Result<Option<Settings>, String> {
        match files.read(FILE) {
            Ok(bytes) => toml::from_str(std::str::from_utf8(&bytes).map_err(|e| e.to_string())?).map(Some).map_err(|e| e.to_string()),
            Err(e) if e.not_found => Ok(None),
            Err(e) => Err(e.what),
        }
    }

    pub fn replica(&self) -> &str {
        if self.replica.is_empty() { crate::mesh::BRIDGE } else { &self.replica }
    }

    pub fn save(&self, files: &Attachments) -> Result<(), String> {
        let text = zeroize::Zeroizing::new(toml::to_string(self).map_err(|e| e.to_string())?);
        files.write(FILE, text.as_bytes())
    }
}

/// Write over the file at `path` and remove it; nothing to do if it is not there. Flash storage may
/// still keep the old blocks elsewhere: only what was sealed is safe from that.
pub fn shred(path: &Path) -> std::io::Result<()> {
    let len = match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_file() => m.len(),
        Ok(_) => return std::fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let mut f = std::fs::OpenOptions::new().write(true).open(path)?;
    f.write_all(&vec![0; len as usize])?;
    f.sync_all()?;
    drop(f);
    std::fs::remove_file(path)
}

/// A file only its owner can read.
pub fn open_private(path: &Path) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pigeonhole_protocols::secret::Secret;

    fn files(dir: &Path) -> Attachments {
        Attachments::new(dir.join("state"), Some(Secret::new(&[1; 32])))
    }

    #[test]
    fn sealed_round_trip() {
        let dir = crate::test_dir();
        let f = files(dir.path());
        assert_eq!(Settings::open(dir.path(), &f), (Settings::default(), None));
        let s = Settings {
            name: "alice".into(),
            daemon: "@katzenpost".into(),
            theme: ThemeChoice::Dark,
            ..Settings::default()
        };
        assert_eq!(s.replica(), crate::mesh::BRIDGE);
        s.save(&f).unwrap();
        assert_eq!(Settings::open(dir.path(), &f), (s.clone(), None));
        let s = Settings { transport: "reticulum".into(), replica: "10.0.0.1:4242".into(), hardware_key_deletion: Some(false), ..s };
        s.save(&f).unwrap();
        assert_eq!(Settings::load(&f).unwrap().unwrap().replica(), "10.0.0.1:4242");
        assert_eq!(Settings::load(&f).unwrap(), Some(s.clone()));
        assert!(!s.screen_security, "screenshots are allowed unless the user opts in");
        let s = Settings { screen_security: true, ..s };
        s.save(&f).unwrap();
        assert!(Settings::load(&f).unwrap().unwrap().screen_security);
        // Sealed: nothing of it is readable on disk, nor with another key.
        let on_disk = std::fs::read(dir.path().join("state").join(FILE)).unwrap();
        assert!(!on_disk.windows(5).any(|w| w == b"alice") && !on_disk.windows(4).any(|w| w == b"10.0"));
        assert!(Settings::load(&Attachments::new(dir.path().join("state"), Some(Secret::new(&[2; 32])))).is_err());
        assert!(!dir.path().join(FILE).exists(), "nothing is written beside the state");
    }

    /// An older version's plaintext file moves in, keeping what reaches the daemon, and is gone.
    #[test]
    fn migration_deletes_the_plaintext() {
        let dir = crate::test_dir();
        let old = dir.path().join(FILE);
        std::fs::write(&old, "name = \"alice\"\ndaemon = \"@kp\"\ntransport = \"reticulum\"\nreplica = \"10.0.0.1:4242\"\nwire_version = 3\n[petnames]\n0a0b = \"Bob\"\n").unwrap();
        std::fs::write(dir.path().join("settings.toml.broken"), "name = [").unwrap();
        let f = files(dir.path());
        let (s, note) = Settings::open(dir.path(), &f);
        assert_eq!(note, None);
        assert_eq!((s.name.as_str(), s.daemon.as_str(), s.transport.as_str(), s.replica()), ("alice", "@kp", "reticulum", "10.0.0.1:4242"));
        assert_eq!((s.wire_version, s.petnames.get("0a0b").map(String::as_str)), (Some(3), Some("Bob")));
        assert!(!old.exists() && !dir.path().join("settings.toml.broken").exists());
        assert_eq!(Settings::load(&f).unwrap(), Some(s.clone()));
        // A second start finds them sealed alone.
        assert_eq!(Settings::open(dir.path(), &f), (s, None));
    }

    #[test]
    fn a_broken_old_file_is_removed_and_told() {
        let dir = crate::test_dir();
        std::fs::write(dir.path().join(FILE), "name = [").unwrap();
        let (s, why) = Settings::open(dir.path(), &files(dir.path()));
        assert_eq!(s, Settings::default());
        assert!(why.unwrap().contains("could not be read"));
        assert!(!dir.path().join(FILE).exists());
    }

    /// Where the settings cannot be sealed, the old file stays rather than being lost.
    #[test]
    fn the_old_file_stays_until_sealed() {
        let dir = crate::test_dir();
        let old = dir.path().join(FILE);
        std::fs::write(&old, "name = \"alice\"\n").unwrap();
        // The state directory cannot be made: a file is in its place.
        std::fs::write(dir.path().join("state"), "").unwrap();
        let (s, why) = Settings::open(dir.path(), &files(dir.path()));
        assert_eq!(s.name, "alice");
        assert!(why.is_some());
        assert!(old.exists());
    }

    #[test]
    fn old_petnames_are_still_read_and_not_written_back() {
        let dir = crate::test_dir();
        std::fs::write(dir.path().join(FILE), "name = \"a\"\n[petnames]\n0a0b = \"Bob\"\n[seen]\ng = 3\n").unwrap();
        let f = Attachments::new(dir.path().join("state"), None);
        let (s, _) = Settings::open(dir.path(), &f);
        assert_eq!(s.petnames.get("0a0b").map(String::as_str), Some("Bob"));
        assert_eq!(s.seen.get("g"), Some(&3));
        let s = Settings { petnames: Default::default(), seen: Default::default(), ..s };
        s.save(&f).unwrap();
        let text = std::fs::read_to_string(dir.path().join("state").join(FILE)).unwrap();
        assert!(!text.contains("petnames") && !text.contains("seen"));
    }

    #[test]
    fn newer_sealed_settings_are_told_and_kept() {
        let dir = crate::test_dir();
        let f = files(dir.path());
        std::fs::create_dir_all(dir.path().join("state")).unwrap();
        std::fs::write(dir.path().join("state").join(FILE), b"PHSTATE2 later").unwrap();
        std::fs::write(dir.path().join(FILE), "name = \"alice\"\n").unwrap();
        let (s, why) = Settings::open(dir.path(), &f);
        assert_eq!(s.name, "alice");
        assert!(crate::keys::is_newer(&why.unwrap()));
        assert_eq!(std::fs::read(dir.path().join("state").join(FILE)).unwrap(), b"PHSTATE2 later");
        assert!(dir.path().join(FILE).exists(), "the old file is not lost either");
    }

    #[test]
    fn shred_removes_and_tolerates_absence() {
        let dir = crate::test_dir();
        let p = dir.path().join("x");
        std::fs::write(&p, b"secret").unwrap();
        shred(&p).unwrap();
        assert!(!p.exists());
        shred(&p).unwrap();
    }
}
