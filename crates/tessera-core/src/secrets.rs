// SPDX-License-Identifier: GPL-3.0-or-later

//! API key storage. Keys live in the OS keychain, never in `connections.json`.

use std::collections::HashMap;
use std::sync::Mutex;

use secrecy::{ExposeSecret, SecretString};
use uuid::Uuid;

/// Keychain service name under which all keys are stored.
const SERVICE: &str = "tessera";

/// A keychain failure. Holds only the platform's message, never the key.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct SecretError(String);

impl SecretError {
    /// Builds an error from a platform message; it must not contain the key.
    pub fn new(message: impl Into<String>) -> SecretError {
        SecretError(message.into())
    }
}

/// Storage for API keys, addressed by profile id. Calls block; run them off the render path.
pub trait SecretStore: Send + Sync {
    /// Returns the key of profile `id`, or `None` if none is stored.
    fn get(&self, id: Uuid) -> Result<Option<SecretString>, SecretError>;
    /// Stores or replaces the key of profile `id`.
    fn set(&self, id: Uuid, key: &SecretString) -> Result<(), SecretError>;
    /// Removes the key of profile `id`; a missing key is not an error.
    fn delete(&self, id: Uuid) -> Result<(), SecretError>;
}

/// The OS keychain (Keychain, Credential Manager, Secret Service).
pub struct KeyringStore;

impl KeyringStore {
    fn entry(id: Uuid) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(SERVICE, &id.to_string()).map_err(|e| match e {
            keyring::Error::NoDefaultStore => store_unavailable(keyring::Entry::store_status()),
            other => other.into(),
        })
    }
}

/// Explains a failed credential store setup, e.g. "no Secret Service is running" on Linux.
fn store_unavailable(status: &Result<(), keyring::Error>) -> SecretError {
    match status {
        Err(e) => SecretError(format!("credential store unavailable: {e}")),
        Ok(()) => SecretError(keyring::Error::NoDefaultStore.to_string()),
    }
}

impl From<keyring::Error> for SecretError {
    fn from(e: keyring::Error) -> Self {
        // `Invalid` echoes the rejected value, which may be the key: keep the attribute name only.
        match e {
            keyring::Error::Invalid(attr, _) => SecretError(format!("invalid value for '{attr}'")),
            other => SecretError(other.to_string()),
        }
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, id: Uuid) -> Result<Option<SecretString>, SecretError> {
        match Self::entry(id)?.get_password() {
            Ok(p) => Ok(Some(SecretString::from(p))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn set(&self, id: Uuid, key: &SecretString) -> Result<(), SecretError> {
        Self::entry(id)?.set_password(key.expose_secret())?;
        Ok(())
    }

    fn delete(&self, id: Uuid) -> Result<(), SecretError> {
        match Self::entry(id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// In-memory store for tests and temporary connections, whose keys live in memory only.
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<Uuid, SecretString>>);

impl MemoryStore {
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<Uuid, SecretString>> {
        // A poisoned lock only means another test panicked; the map is still usable.
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, id: Uuid) -> Result<Option<SecretString>, SecretError> {
        Ok(self.map().get(&id).cloned())
    }

    fn set(&self, id: Uuid, key: &SecretString) -> Result<(), SecretError> {
        self.map().insert(id, key.clone());
        Ok(())
    }

    fn delete(&self, id: Uuid) -> Result<(), SecretError> {
        self.map().remove(&id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{profile::Profile, store};

    const KEY: &str = "sk-live-test-1234";

    fn secret() -> SecretString {
        SecretString::from(KEY)
    }

    #[test]
    fn memory_store_round_trip() {
        let s = MemoryStore::default();
        let id = Uuid::new_v4();
        assert!(s.get(id).expect("get").is_none());
        s.set(id, &secret()).expect("set");
        assert_eq!(s.get(id).expect("get").expect("some").expose_secret(), KEY);
        s.delete(id).expect("delete");
        assert!(s.get(id).expect("get").is_none());
    }

    #[test]
    fn memory_store_delete_missing_is_ok() {
        assert!(MemoryStore::default().delete(Uuid::new_v4()).is_ok());
    }

    #[test]
    fn secret_error_never_contains_key() {
        let e: SecretError = keyring::Error::Invalid("password".into(), KEY.into()).into();
        let text = format!("{e} {e:?}");
        assert!(!text.contains(KEY), "{text}");
        assert!(!text.is_empty());

        // A keyring bump that starts printing these bytes must fail here.
        let bytes = KEY.as_bytes().to_vec();
        let platform: keyring::Error = keyring::Error::BadDataFormat(bytes.clone(), "boom".into());
        for err in [keyring::Error::BadEncoding(bytes), platform] {
            let e: SecretError = err.into();
            let text = format!("{e} {e:?}");
            assert!(!text.contains(KEY), "{text}");
            assert!(!text.contains(&format!("{:?}", KEY.as_bytes())), "{text}");
        }
    }

    #[test]
    fn store_unavailable_reports_the_cause() {
        let cause = Err(keyring::Error::BadStoreFormat("no Secret Service".into()));
        assert!(
            store_unavailable(&cause)
                .to_string()
                .contains("no Secret Service")
        );
        assert!(!store_unavailable(&Ok(())).to_string().is_empty());
        assert_eq!(SecretError::new("x").to_string(), "x");
    }

    #[test]
    fn file_never_contains_the_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("connections.json");
        let profile = Profile::new("local", vec![]);
        let mem = MemoryStore::default();
        mem.set(profile.id, &secret()).expect("set");
        let f = store::ConnectionsFile {
            profiles: vec![profile],
            ..Default::default()
        };
        store::save(&p, &f).expect("save");
        assert!(!std::fs::read_to_string(&p).expect("read").contains(KEY));
    }

    #[test]
    #[ignore = "touches the OS keychain; run manually per platform"]
    fn keyring_round_trip() {
        let s = KeyringStore;
        let id = Uuid::new_v4();
        s.set(id, &secret()).expect("set");
        assert_eq!(s.get(id).expect("get").expect("some").expose_secret(), KEY);
        s.delete(id).expect("delete");
        assert!(s.get(id).expect("get").is_none());
    }
}
