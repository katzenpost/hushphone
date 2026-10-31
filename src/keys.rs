//! The key that seals the state at rest. The system keeps it, not us: on Linux the Secret Service
//! keyring that the session unlocks, on Android a key the Keystore will not let out wraps it. No
//! passphrase is asked for, so the state is as safe at rest as the session or the device lock.

use std::path::Path;

use pigeonhole_protocols::protocols::katzenqt1::client::store::{Options, Store, StoreError};
use pigeonhole_protocols::secret::Secret;
use rand::RngCore;

/// What became of the key.
pub enum Custody {
    Sealed(Secret),
    /// No key can be kept, so the state stays unsealed; why, for the user.
    Unsealed(String),
    /// The state is sealed, but its key cannot be had; why, for the user.
    Lost(String),
}

/// Where a key is kept: by the system, or by a test.
pub trait Keeper {
    /// The key kept for the state in `dir`, if there is one.
    fn get(&self, dir: &Path) -> impl Future<Output = Result<Option<Secret>, String>> + Send;
    fn put(&self, dir: &Path, key: &Secret) -> impl Future<Output = Result<(), String>> + Send;
}

/// The key for the state in `dir`, made and kept on first use. A new key is never made for a state
/// that is already sealed: its key is lost, and the user is told.
pub async fn custody(k: &impl Keeper, dir: &Path) -> Custody {
    match k.get(dir).await {
        Ok(Some(key)) => Custody::Sealed(key),
        Ok(None) if sealed(dir) => Custody::Lost("the keyring has no key for it".into()),
        Ok(None) => {
            let mut b = zeroize::Zeroizing::new([0u8; 32]);
            rand::rngs::OsRng.fill_bytes(&mut *b);
            let key = Secret::new(&*b);
            match k.put(dir, &key).await {
                Ok(()) => Custody::Sealed(key),
                Err(e) => Custody::Unsealed(e),
            }
        }
        Err(e) if sealed(dir) => Custody::Lost(e),
        Err(e) => Custody::Unsealed(e),
    }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;
    use std::sync::Mutex;

    use pigeonhole_protocols::protocols::katzenqt1::engine::{Changes, Chat, Line, State, StepResult};

    #[derive(Default)]
    struct Fake {
        keys: Mutex<std::collections::HashMap<PathBuf, Vec<u8>>>,
        broken: bool,
    }

    impl Keeper for Fake {
        async fn get(&self, dir: &Path) -> Result<Option<Secret>, String> {
            if self.broken {
                return Err("no keyring".into());
            }
            Ok(self.keys.lock().unwrap().get(dir).map(|k| Secret::new(k)))
        }

        async fn put(&self, dir: &Path, key: &Secret) -> Result<(), String> {
            if self.broken {
                return Err("no keyring".into());
            }
            self.keys.lock().unwrap().insert(dir.into(), key.to_vec());
            Ok(())
        }
    }

    fn block<T>(f: impl Future<Output = T>) -> T {
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
    fn a_key_is_made_once_and_kept() {
        let base = crate::test_dir();
        let dir = base.path().join("state");
        let k = Fake::default();
        let Custody::Sealed(a) = block(custody(&k, &dir)) else { panic!("not sealed") };
        let Custody::Sealed(b) = block(custody(&k, &dir)) else { panic!("not sealed") };
        assert_eq!(a, b);
    }

    #[test]
    fn without_a_keyring_the_state_stays_unsealed() {
        let base = crate::test_dir();
        let k = Fake { broken: true, ..Fake::default() };
        assert!(matches!(block(custody(&k, &base.path().join("state"))), Custody::Unsealed(_)));
    }

    #[test]
    fn a_sealed_state_without_its_key_is_lost_and_no_key_is_made() {
        let base = crate::test_dir();
        let dir = base.path().join("state");
        write_state(&dir, Some(Secret::new(&[1; 32])));
        let k = Fake::default();
        assert!(matches!(block(custody(&k, &dir)), Custody::Lost(_)));
        assert!(k.keys.lock().unwrap().is_empty());
        let broken = Fake { broken: true, ..Fake::default() };
        assert!(matches!(block(custody(&broken, &dir)), Custody::Lost(_)));
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
