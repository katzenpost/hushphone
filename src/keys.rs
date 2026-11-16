//! The key that seals the state at rest. The system keeps it, not us: on Linux the Secret Service
//! keyring that the session unlocks, on Android a key the Keystore will not let out wraps it. No
//! passphrase is asked for, so the state is as safe at rest as the session or the device lock.

use std::path::Path;

use pigeonhole_protocols::protocols::katzenqt1::client::store::{Options, Store, StoreError};
use pigeonhole_protocols::secret::Secret;
use rand::RngCore;

use crate::backend::Keys;

/// Where a key is kept: by the system, or by a test.
pub trait Keeper {
    /// The key kept for the state in `dir`, if there is one.
    fn get(&self, dir: &Path) -> impl Future<Output = Result<Option<Secret>, String>> + Send;
    fn put(&self, dir: &Path, key: &Secret) -> impl Future<Output = Result<(), String>> + Send;
    /// Destroy the key kept for the state in `dir`; nothing to do if there is none.
    fn forget(&self, dir: &Path) -> impl Future<Output = Result<(), String>> + Send;
}

/// The key for the state in `dir`, made and kept on first use; or why there is none, for the
/// user. The state is never kept unsealed: without a key it is not opened. A new key is never
/// made for a state that is already sealed: its key is lost, and the user is told.
pub async fn custody(k: &impl Keeper, dir: &Path) -> Result<Secret, String> {
    // A format from a newer hushphone is neither a lost key nor a new state: it is left alone.
    sealed_format(dir)?;
    match k.get(dir).await {
        Ok(Some(key)) => Ok(key),
        Err(e) if is_newer(&e) => Err(e),
        Ok(None) if sealed(dir) => Err(lost(dir, "the keyring has no key for it")),
        Ok(None) => {
            let mut b = zeroize::Zeroizing::new([0u8; 32]);
            rand::rngs::OsRng.fill_bytes(&mut *b);
            let key = Secret::new(&*b);
            k.put(dir, &key).await.map_err(|e| format!("hushphone keeps your chats only encrypted, and no key can be kept for them: {e}."))?;
            Ok(key)
        }
        Err(e) if sealed(dir) => Err(lost(dir, &e)),
        Err(e) => Err(format!("hushphone keeps your chats only encrypted, and their key can't be had: {e}.")),
    }
}

/// The key the state in `dir` is sealed with, as `keys` says it is kept; `None` only for tests.
pub async fn open(keys: &Keys, dir: &Path) -> Result<Option<Secret>, String> {
    match keys {
        Keys::None => Ok(None),
        Keys::Given(k) => sealed_format(dir).map(|()| Some(k.clone())),
        Keys::System => custody(&System, dir).await.map(Some),
    }
}

/// Why a file in a format from a newer hushphone is not opened.
pub fn newer(what: &str) -> String {
    format!("{what} was written by a newer hushphone; install that version or a later one to open it. Nothing was changed")
}

pub fn is_newer(why: &str) -> bool {
    why.contains("written by a newer hushphone")
}

/// How pigeonhole_protocols' sealed files start: this, then their format's version, and both are
/// authenticated as associated data. Version 1 is the one known.
const SEALED_PREFIX: &[u8] = b"PHSTATE";

/// Whether hushphone knows the format of a sealed file that starts with `kept`; one not sealed
/// at all is from before the state was sealed, and is sealed in place by [`seal_existing`].
pub fn check_sealed(kept: &[u8], what: &str) -> Result<(), String> {
    match kept.strip_prefix(SEALED_PREFIX) {
        Some(rest) if !rest.starts_with(b"1") => Err(newer(what)),
        _ => Ok(()),
    }
}

/// The format of the state's database, `state.sealed`, before any key is asked for.
fn sealed_format(dir: &Path) -> Result<(), String> {
    use std::io::Read;
    let mut head = [0u8; 8];
    let n = match std::fs::File::open(dir.join("state.sealed")) {
        Ok(f) => f.take(8).read(&mut head).map_err(|e| e.to_string())?,
        Err(_) => 0,
    };
    check_sealed(&head[..n], "Your chats' state")
}

/// How `state.key` starts: this and [`KEY_VERSION`], authenticated as associated data when the
/// key is wrapped; then the IV's length, the IV and the wrapped key.
pub const KEY_MAGIC: &[u8; 5] = b"HPKEY";
/// 2: the header above. 1, unversioned, began with the IV's length and had no associated data.
pub const KEY_VERSION: u8 = 2;

/// What `state.key` holds, by its format's version.
#[derive(Debug, PartialEq, Eq)]
pub enum KeptKey<'a> {
    /// The current format: the header to authenticate, the IV and the wrapped key.
    Current { header: &'a [u8], iv: &'a [u8], wrapped: &'a [u8] },
    /// Version 1, to be wrapped again in the current format once unwrapped.
    V1 { iv: &'a [u8], wrapped: &'a [u8] },
}

/// The header `state.key` starts with now.
pub fn key_header() -> Vec<u8> {
    let mut h = KEY_MAGIC.to_vec();
    h.push(KEY_VERSION);
    h
}

/// The IV, after its length, and the wrapped key after it.
fn split_iv(b: &[u8]) -> Result<(&[u8], &[u8]), String> {
    let (&n, rest) = b.split_first().ok_or("the kept key is empty")?;
    rest.split_at_checked(n as usize).ok_or_else(|| "the kept key is cut short".to_string())
}

pub fn read_kept_key(kept: &[u8]) -> Result<KeptKey<'_>, String> {
    let Some(rest) = kept.strip_prefix(KEY_MAGIC.as_slice()) else {
        let (iv, wrapped) = split_iv(kept)?;
        return Ok(KeptKey::V1 { iv, wrapped });
    };
    match rest.first() {
        Some(&KEY_VERSION) => {
            let (iv, wrapped) = split_iv(&rest[1..])?;
            Ok(KeptKey::Current { header: &kept[..KEY_MAGIC.len() + 1], iv, wrapped })
        }
        Some(&v) if v > KEY_VERSION => Err(newer("The state's key")),
        _ => Err("the kept key is in a format hushphone does not know".into()),
    }
}

/// `state.key` in the current format, for `iv` and `wrapped`.
pub fn write_kept_key(iv: &[u8], wrapped: &[u8]) -> Vec<u8> {
    let mut out = key_header();
    out.push(iv.len() as u8);
    out.extend(iv);
    out.extend(wrapped);
    out
}

/// Whether an Android key attestation extension, `ext` as `X509Certificate.getExtensionValue`
/// gives it (an OCTET STRING holding the KeyDescription), lists rollbackResistance ([703]) among
/// the hardware-enforced tags: the eighth element of the KeyDescription SEQUENCE.
pub fn rollback_resistant(ext: &[u8]) -> bool {
    let rollback = || -> Option<bool> {
        let (_, description, _) = der(ext)?;
        let (_, mut fields, _) = der(description)?;
        for _ in 0..7 {
            fields = der(fields)?.2;
        }
        let (_, mut hardware, _) = der(fields)?;
        while !hardware.is_empty() {
            let (tag, _, rest) = der(hardware)?;
            if tag == (0xA0, 703) {
                return Some(true);
            }
            hardware = rest;
        }
        Some(false)
    };
    rollback().unwrap_or(false)
}

/// A DER tag: its class and constructed bits, and its number.
type Tag = (u8, u32);

/// One DER element at the start of `b`: its tag, its contents, and what follows it.
fn der(b: &[u8]) -> Option<(Tag, &[u8], &[u8])> {
    let (&first, mut rest) = b.split_first()?;
    let mut number = u32::from(first & 0x1f);
    if number == 0x1f {
        number = 0;
        loop {
            let (&c, r) = rest.split_first()?;
            rest = r;
            number = number.checked_mul(128)? | u32::from(c & 0x7f);
            if c & 0x80 == 0 {
                break;
            }
        }
    }
    let (&l, r) = rest.split_first()?;
    rest = r;
    let len = if l < 0x80 {
        usize::from(l)
    } else {
        let (bytes, r) = rest.split_at_checked(usize::from(l & 0x7f)).filter(|(b, _)| (1..=4).contains(&b.len()))?;
        rest = r;
        bytes.iter().fold(0usize, |a, &x| a << 8 | usize::from(x))
    };
    let (contents, rest) = rest.split_at_checked(len)?;
    Some(((first & 0xe0, number), contents, rest))
}

fn lost(dir: &Path, why: &str) -> String {
    let fix = if cfg!(target_os = "android") {
        "To start over without them, clear hushphone's storage in Android's settings.".to_string()
    } else {
        format!("If it is gone for good, move {} aside to start over without them.", dir.display())
    };
    format!("Your chats are sealed, but their key can't be had: {why}. {fix}")
}

fn sealed(dir: &Path) -> bool {
    dir.join("state.sealed").exists()
}

/// Seal the state in `dir` with `key` if it is not sealed yet, or not all of it: in place, as
/// one sealed file and each file beside it sealed whole, with the unsealed database wiped.
pub fn seal_existing(dir: &Path, key: &Secret) -> Result<(), String> {
    match Store::open(dir, Options { no_sync: false, key: Some(key.clone()) }) {
        Ok(_) => Ok(()),
        Err(StoreError::Unsealed) => Store::reseal(dir, key.clone()).map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// The Secret Service keyring of the session.
#[cfg(not(target_os = "android"))]
pub struct System;

#[cfg(not(target_os = "android"))]
impl System {
    fn attributes(dir: &Path) -> std::collections::HashMap<&'static str, String> {
        std::collections::HashMap::from([
            ("application", "hushphone".to_string()),
            ("state", dir.to_string_lossy().into_owned()),
        ])
    }

    async fn keyring() -> Result<oo7::Keyring, String> {
        let k = oo7::Keyring::new().await.map_err(|e| {
            eprintln!("hushphone: keyring: {e}");
            "no keyring (Secret Service) is running".to_string()
        })?;
        k.unlock().await.map_err(|e| {
            eprintln!("hushphone: keyring: {e}");
            "the keyring stayed locked".to_string()
        })?;
        Ok(k)
    }
}

#[cfg(not(target_os = "android"))]
impl Keeper for System {
    async fn get(&self, dir: &Path) -> Result<Option<Secret>, String> {
        let k = Self::keyring().await?;
        let items = k.search_items(&Self::attributes(dir)).await.map_err(|e| e.to_string())?;
        let Some(item) = items.first() else { return Ok(None) };
        item.unlock().await.map_err(|e| e.to_string())?;
        let s = item.secret().await.map_err(|e| e.to_string())?;
        if s.len() != 32 {
            return Err("the keyring's key for hushphone is not 32 bytes".into());
        }
        Ok(Some(Secret::new(&s)))
    }

    async fn put(&self, dir: &Path, key: &Secret) -> Result<(), String> {
        let k = Self::keyring().await?;
        k.create_item("hushphone state key", &Self::attributes(dir), oo7::Secret::blob(&**key), true)
            .await
            .map_err(|e| e.to_string())
    }

    async fn forget(&self, dir: &Path) -> Result<(), String> {
        Self::keyring().await?.delete(&Self::attributes(dir)).await.map_err(|e| e.to_string())
    }
}

/// A key the Android Keystore will not let out wraps ours, kept in `state.key` beside the state.
#[cfg(target_os = "android")]
pub struct System;

#[cfg(target_os = "android")]
impl Keeper for System {
    async fn get(&self, dir: &Path) -> Result<Option<Secret>, String> {
        crate::android::unwrap_state_key(&dir.with_extension("key"))
    }

    async fn put(&self, dir: &Path, key: &Secret) -> Result<(), String> {
        crate::android::wrap_state_key(&dir.with_extension("key"), key)
    }

    /// The Keystore's key; `state.key`, which it wrapped, is the wipe's to write over.
    async fn forget(&self, _: &Path) -> Result<(), String> {
        crate::android::delete_state_key()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;
    use std::sync::Mutex;

    use pigeonhole_protocols::protocols::katzenqt1::engine::{Changes, Chat, Line, State, StepResult};

    #[derive(Default)]
    pub(crate) struct Fake {
        pub(crate) keys: Mutex<std::collections::HashMap<PathBuf, Vec<u8>>>,
        pub(crate) broken: bool,
        pub(crate) refuse_put: bool,
        pub(crate) newer: bool,
    }

    impl Keeper for Fake {
        async fn get(&self, dir: &Path) -> Result<Option<Secret>, String> {
            if self.broken {
                return Err("no keyring".into());
            }
            if self.newer {
                return Err(newer("The state's key"));
            }
            Ok(self.keys.lock().unwrap().get(dir).map(|k| Secret::new(k)))
        }

        async fn put(&self, dir: &Path, key: &Secret) -> Result<(), String> {
            if self.broken || self.refuse_put {
                return Err("no keyring".into());
            }
            self.keys.lock().unwrap().insert(dir.into(), key.to_vec());
            Ok(())
        }

        async fn forget(&self, dir: &Path) -> Result<(), String> {
            if self.broken {
                return Err("no keyring".into());
            }
            self.keys.lock().unwrap().remove(dir);
            Ok(())
        }
    }

    pub(crate) fn block<T>(f: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f)
    }

    const WORDS: &str = "the words of a message";

    /// A state with one group and one line, sealed with `key` if given.
    fn write_state(dir: &Path, key: Option<Secret>) {
        let mut st = Store::open(dir, Options { no_sync: true, key }).unwrap();
        let state = State {
            me: "alice".into(),
            chats: vec![Chat {
                id: "g".into(),
                name: "friends".into(),
                log: vec![Line { text: WORDS.into(), ..Line::default() }],
                ..Chat::default()
            }],
        };
        let changes = Changes {
            chats: BTreeSet::from(["g".to_string()]),
            lines: BTreeMap::from([("g".to_string(), BTreeSet::from([0]))]),
            ..Changes::default()
        };
        st.commit(&state, &StepResult { changes, ..StepResult::default() }).unwrap();
    }

    fn holds(dir: &Path, needle: &[u8]) -> bool {
        std::fs::read_dir(dir).unwrap().flatten().any(|e| {
            let p = e.path();
            if p.is_dir() { holds(&p, needle) } else { std::fs::read(&p).unwrap().windows(needle.len()).any(|w| w == needle) }
        })
    }

    #[test]
    fn the_kept_key_is_read_by_its_version() {
        let now = write_kept_key(&[1; 12], b"wrapped");
        assert!(now.starts_with(b"HPKEY\x02\x0c"));
        assert_eq!(read_kept_key(&now), Ok(KeptKey::Current { header: b"HPKEY\x02", iv: &[1; 12], wrapped: b"wrapped" }));
        // Version 1, unversioned, is still read, to be written again in the current format.
        let mut v1 = vec![12];
        v1.extend([1; 12]);
        v1.extend(b"wrapped");
        assert_eq!(read_kept_key(&v1), Ok(KeptKey::V1 { iv: &[1; 12], wrapped: b"wrapped" }));
        // A newer one is said to be so, not taken for a broken or wrong key.
        let mut v3 = now.clone();
        v3[5] = 3;
        assert!(is_newer(&read_kept_key(&v3).unwrap_err()));
        let mut v0 = now.clone();
        v0[5] = 0;
        assert!(!is_newer(&read_kept_key(&v0).unwrap_err()));
        assert!(read_kept_key(&[]).is_err() && read_kept_key(&[12, 1]).is_err() && read_kept_key(b"HPKEY\x02\x0c").is_err());
    }

    #[test]
    fn a_sealed_file_is_known_by_its_version() {
        assert!(check_sealed(b"PHSTATE1...", "x").is_ok());
        assert!(check_sealed(b"plain redb", "x").is_ok(), "unsealed: sealed in place");
        assert!(is_newer(&check_sealed(b"PHSTATE2...", "x").unwrap_err()));
    }

    /// A state from a newer hushphone is not opened, not taken for one whose key is lost, and
    /// nothing in it changes; a newer kept key likewise.
    #[test]
    fn a_newer_state_is_left_alone() {
        let base = crate::test_dir();
        let dir = base.path().join("state");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("state.sealed"), b"PHSTATE2 from the future").unwrap();
        let k = Fake::default();
        let why = block(custody(&k, &dir)).unwrap_err();
        assert!(is_newer(&why) && !why.contains("can't be had"), "{why}");
        assert!(k.keys.lock().unwrap().is_empty(), "no key is made for it");
        assert_eq!(std::fs::read(dir.join("state.sealed")).unwrap(), b"PHSTATE2 from the future");
        assert!(is_newer(&block(open(&Keys::Given(Secret::new(&[1; 32])), &dir)).unwrap_err()));
        let newer_key = Fake { newer: true, ..Fake::default() };
        std::fs::write(dir.join("state.sealed"), b"PHSTATE1").unwrap();
        assert!(is_newer(&block(custody(&newer_key, &dir)).unwrap_err()));
    }

    /// A KeyDescription with `hardware` as its hardware-enforced list, as getExtensionValue gives it.
    fn attestation(hardware: &[u8]) -> Vec<u8> {
        // Versions and security levels, the challenge, no unique ID, and no software-enforced tags.
        let mut fields = vec![0x02, 1, 4, 0x0a, 1, 2, 0x02, 1, 4, 0x0a, 1, 2, 0x04, 2, b'h', b'p', 0x04, 0, 0x30, 0];
        fields.extend([0x30, hardware.len() as u8]);
        fields.extend(hardware);
        let mut description = vec![0x30, 0x81, fields.len() as u8];
        description.extend(fields);
        let mut ext = vec![0x04, description.len() as u8];
        ext.extend(description);
        ext
    }

    #[test]
    fn rollback_resistance_is_read_from_the_hardware_enforced_list() {
        // purpose [1] SET { 2 }, then rollbackResistance [703] NULL.
        let purpose = [0xa1, 5, 0x31, 3, 0x02, 1, 2];
        let rollback = [0xbf, 0x85, 0x3f, 2, 0x05, 0];
        assert!(rollback_resistant(&attestation(&[purpose.as_slice(), &rollback].concat())));
        assert!(!rollback_resistant(&attestation(&purpose)));
        assert!(!rollback_resistant(&attestation(&[])));
        // Cut short anywhere, or not an attestation at all: no.
        let full = attestation(&[purpose.as_slice(), &rollback].concat());
        for n in 0..full.len() {
            assert!(!rollback_resistant(&full[..n]), "cut at {n}");
        }
        assert!(!rollback_resistant(b"not der"));
    }

    #[test]
    fn a_key_is_made_once_and_kept() {
        let base = crate::test_dir();
        let dir = base.path().join("state");
        let k = Fake::default();
        let a = block(custody(&k, &dir)).unwrap();
        let b = block(custody(&k, &dir)).unwrap();
        assert_eq!(a, b);
    }

    /// No unsealed path remains: without a keyring, nothing is opened and nothing is written.
    #[test]
    fn without_a_keyring_the_state_is_not_opened() {
        let base = crate::test_dir();
        let dir = base.path().join("state");
        let k = Fake { broken: true, ..Fake::default() };
        let why = block(custody(&k, &dir)).unwrap_err();
        assert!(why.contains("no keyring") && why.contains("only encrypted"), "{why}");
        assert!(!dir.exists());
        let refuses_to_keep = Fake { refuse_put: true, ..Fake::default() };
        assert!(block(custody(&refuses_to_keep, &dir)).is_err());
        assert!(!dir.exists());
    }

    #[test]
    fn a_sealed_state_without_its_key_is_lost_and_no_key_is_made() {
        let base = crate::test_dir();
        let dir = base.path().join("state");
        write_state(&dir, Some(Secret::new(&[1; 32])));
        let k = Fake::default();
        assert!(block(custody(&k, &dir)).unwrap_err().contains("sealed, but their key"));
        assert!(k.keys.lock().unwrap().is_empty());
        let broken = Fake { broken: true, ..Fake::default() };
        assert!(block(custody(&broken, &dir)).unwrap_err().contains("sealed, but their key"));
    }

    #[test]
    fn an_unsealed_state_is_sealed_in_place() {
        let base = crate::test_dir();
        let dir = base.path().join("state");
        write_state(&dir, None);
        assert!(holds(&dir, WORDS.as_bytes()));
        let key = Secret::new(&[1; 32]);
        seal_existing(&dir, &key).unwrap();
        assert!(!holds(base.path(), WORDS.as_bytes()));
        assert!(dir.join("state.sealed").exists() && !dir.join("state.redb").exists());
        let mut st = Store::open(&dir, Options { no_sync: true, key: Some(key.clone()) }).unwrap();
        assert_eq!(st.load("bob").unwrap().chats[0].log[0].text, WORDS);
        drop(st);
        // Sealing again changes nothing.
        seal_existing(&dir, &key).unwrap();
    }

    /// A sealing cut short is finished at the next start.
    #[test]
    fn a_sealing_cut_short_is_finished() {
        let base = crate::test_dir();
        let dir = base.path().join("state");
        let key = Secret::new(&[1; 32]);
        write_state(&dir, None);
        std::fs::write(dir.join(".sealing"), b"").unwrap();
        seal_existing(&dir, &key).unwrap();
        assert!(!holds(base.path(), WORDS.as_bytes()));
        assert!(!dir.join(".sealing").exists());
        let mut st = Store::open(&dir, Options { no_sync: true, key: Some(key) }).unwrap();
        assert_eq!(st.load("bob").unwrap().chats[0].log[0].text, WORDS);
    }
}
