//! The user's own settings, in a small TOML file that is not sealed: the name, the daemon's
//! address and the theme. The names the user gives members, and what they have read, are kept
//! sealed with the state instead, in [`crate::contacts`].

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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
    /// Where an older version kept petnames; moved into the sealed contacts at start and
    /// emptied.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub petnames: BTreeMap<String, String>,
    /// Likewise, the last line read in each group.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub seen: BTreeMap<String, usize>,
}

impl Settings {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("settings.toml")
    }

    /// The settings in `data_dir`, or defaults when there are none yet. A file that cannot be
    /// read is moved aside, and why is returned for the user.
    pub fn load(data_dir: &Path) -> (Settings, Option<String>) {
        let path = Self::path(data_dir);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => return (Settings::default(), None),
        };
        match toml::from_str(&text) {
            Ok(s) => (s, None),
            Err(e) => {
                let aside = path.with_extension("toml.broken");
                let _ = std::fs::rename(&path, &aside);
                (Settings::default(), Some(format!("The settings could not be read and were moved to {}: {e}", aside.display())))
            }
        }
    }

    pub fn save(&self, data_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let path = Self::path(data_dir);
        let tmp = path.with_extension("toml.tmp");
        let text = toml::to_string(self).map_err(std::io::Error::other)?;
        let mut f = open_private(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(tmp, path)
    }
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

    #[test]
    fn round_trip() {
        let dir = crate::test_dir();
        assert_eq!(Settings::load(dir.path()), (Settings::default(), None));
        let s = Settings {
            name: "alice".into(),
            daemon: "@katzenpost".into(),
            theme: ThemeChoice::Dark,
            ..Settings::default()
        };
        s.save(dir.path()).unwrap();
        assert_eq!(Settings::load(dir.path()), (s, None));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(Settings::path(dir.path())).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn a_broken_file_is_moved_aside_and_told() {
        let dir = crate::test_dir();
        std::fs::write(Settings::path(dir.path()), "name = [").unwrap();
        let (s, why) = Settings::load(dir.path());
        assert_eq!(s, Settings::default());
        assert!(why.unwrap().contains("settings.toml.broken"));
        assert!(!Settings::path(dir.path()).exists());
        assert!(dir.path().join("settings.toml.broken").exists());
    }

    #[test]
    fn old_petnames_are_still_read() {
        let dir = crate::test_dir();
        std::fs::write(Settings::path(dir.path()), "name = \"a\"\n[petnames]\n0a0b = \"Bob\"\n[seen]\ng = 3\n").unwrap();
        let (s, _) = Settings::load(dir.path());
        assert_eq!(s.petnames.get("0a0b").map(String::as_str), Some("Bob"));
        assert_eq!(s.seen.get("g"), Some(&3));
        // Emptied, they are not written back.
        let s = Settings { petnames: Default::default(), seen: Default::default(), ..s };
        s.save(dir.path()).unwrap();
        let text = std::fs::read_to_string(Settings::path(dir.path())).unwrap();
        assert!(!text.contains("petnames") && !text.contains("seen"));
    }
}
