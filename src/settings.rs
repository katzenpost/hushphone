//! The user's own settings, and the names they gave members, in a small TOML file.

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
    /// Our own names for members, by the hex of their key.
    pub petnames: BTreeMap<String, String>,
}

impl Settings {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("settings.toml")
    }

    /// The settings in `data_dir`, or defaults when there are none yet.
    pub fn load(data_dir: &Path) -> Settings {
        std::fs::read_to_string(Self::path(data_dir))
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
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

    pub fn petname(&self, key: &[u8]) -> Option<&str> {
        self.petnames.get(&hex::encode(key)).map(String::as_str)
    }

    pub fn set_petname(&mut self, key_hex: &str, name: &str) {
        let name = crate::text::name(name);
        if name == "?" {
            self.petnames.remove(key_hex);
        } else {
            self.petnames.insert(key_hex.to_string(), name);
        }
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
        assert_eq!(Settings::load(dir.path()), Settings::default());
        let mut s = Settings {
            name: "alice".into(),
            daemon: "@katzenpost".into(),
            theme: ThemeChoice::Dark,
            ..Settings::default()
        };
        s.set_petname("0a0b", "Bob (work)");
        s.save(dir.path()).unwrap();
        assert_eq!(Settings::load(dir.path()), s);
        assert_eq!(s.petname(&[0x0a, 0x0b]), Some("Bob (work)"));
        s.set_petname("0a0b", "");
        assert_eq!(s.petname(&[0x0a, 0x0b]), None);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(Settings::path(dir.path())).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
