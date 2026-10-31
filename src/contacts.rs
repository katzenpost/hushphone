//! What the user keeps about members and groups: the names they gave members, by key, and the
//! last line read in each group. Member keys are a contact list, so this lives with the state,
//! sealed under its key when the state is.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::backend::Attachments;

/// The file's name under the state directory.
pub const FILE: &str = "contacts.toml";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Contacts {
    /// Our own names for members, by the hex of their key.
    pub petnames: BTreeMap<String, String>,
    /// The last line read in each group, so unread counts survive a restart.
    pub seen: BTreeMap<String, usize>,
}

impl Contacts {
    /// The contacts kept with the state, or none yet.
    pub fn load(files: &Attachments) -> Result<Contacts, String> {
        match files.read(FILE) {
            Ok(bytes) => toml::from_str(std::str::from_utf8(&bytes).map_err(|e| e.to_string())?).map_err(|e| e.to_string()),
            Err(e) if e.not_found => Ok(Contacts::default()),
            Err(e) => Err(e.what),
        }
    }

    pub fn save(&self, files: &Attachments) -> Result<(), String> {
        let text = toml::to_string(self).map_err(|e| e.to_string())?;
        files.write(FILE, text.as_bytes())
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

    /// Take over what an older version kept in the settings; true if anything moved.
    pub fn adopt(&mut self, settings: &mut crate::settings::Settings) -> bool {
        if settings.petnames.is_empty() && settings.seen.is_empty() {
            return false;
        }
        for (k, v) in std::mem::take(&mut settings.petnames) {
            self.petnames.entry(k).or_insert(v);
        }
        for (k, v) in std::mem::take(&mut settings.seen) {
            self.seen.entry(k).or_insert(v);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pigeonhole_protocols::secret::Secret;

    fn contacts() -> Contacts {
        let mut c = Contacts::default();
        c.set_petname("0a0b", "Bob (work)");
        c.seen.insert("g".into(), 7);
        c
    }

    #[test]
    fn round_trip_plain_and_sealed() {
        for key in [None, Some(Secret::new(&[1; 32]))] {
            let d = crate::test_dir();
            let files = Attachments::new(d.path().to_path_buf(), key.clone());
            assert_eq!(Contacts::load(&files).unwrap(), Contacts::default());
            let c = contacts();
            c.save(&files).unwrap();
            assert_eq!(Contacts::load(&files).unwrap(), c);
            assert_eq!(c.petname(&[0x0a, 0x0b]), Some("Bob (work)"));
            let on_disk = std::fs::read(d.path().join(FILE)).unwrap();
            let plain = on_disk.windows(3).any(|w| w == b"Bob");
            assert_eq!(plain, key.is_none(), "the file is plain exactly when there is no key");
            if key.is_some() {
                let other = Attachments::new(d.path().to_path_buf(), Some(Secret::new(&[2; 32])));
                assert!(Contacts::load(&other).is_err());
            }
        }
    }

    #[test]
    fn petnames_move_out_of_the_settings_once() {
        let mut s = crate::settings::Settings::default();
        s.petnames.insert("0a0b".into(), "Bob".into());
        s.seen.insert("g".into(), 3);
        let mut c = Contacts::default();
        c.set_petname("0a0b", "Robert");
        assert!(c.adopt(&mut s));
        assert_eq!(c.petname(&[0x0a, 0x0b]), Some("Robert"), "a name given since wins");
        assert_eq!(c.seen.get("g"), Some(&3));
        assert!(s.petnames.is_empty() && s.seen.is_empty());
        assert!(!c.adopt(&mut s));
    }

    #[test]
    fn an_empty_name_removes_the_petname() {
        let mut c = contacts();
        c.set_petname("0a0b", "");
        assert_eq!(c.petname(&[0x0a, 0x0b]), None);
    }
}
