use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
};

use anyhow::{Context, Result};

/// Secret storage. Keys are flat strings such as `conn:{id}:env:JIRA_TOKEN`.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>>;
    fn set(&self, key: &str, value: &str) -> Result<()>;
    fn delete(&self, key: &str) -> Result<()>;
}

/// Item holding every secret as one JSON object.
const VAULT_KEY: &str = "vault";

/// OS keychain (macOS Keychain, Windows Credential Manager, Secret Service).
///
/// All secrets live in a single keychain item that is read once and cached,
/// because macOS asks for permission per item (and again after every rebuild
/// of an unsigned app). Secrets from older versions, stored one item per key,
/// move into the vault the first time they are read.
pub struct KeychainSecrets {
    service: String,
    vault: Mutex<Option<HashMap<String, String>>>,
    /// Keys already looked up as legacy items this session.
    legacy_checked: Mutex<HashSet<String>>,
}

impl KeychainSecrets {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            vault: Mutex::new(None),
            legacy_checked: Default::default(),
        }
    }

    fn entry(&self, key: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, key).context("open keychain entry")
    }

    /// Runs `f` on the cached vault, loading it first. A failed read is not
    /// cached as empty, so a denied prompt can never overwrite stored secrets.
    fn with_vault<T>(&self, f: impl FnOnce(&mut HashMap<String, String>) -> Result<T>) -> Result<T> {
        let mut vault = self.vault.lock().unwrap();
        if vault.is_none() {
            let loaded = match self.entry(VAULT_KEY)?.get_password() {
                Ok(raw) => serde_json::from_str(&raw).context("decode keychain vault")?,
                Err(keyring::Error::NoEntry) => HashMap::new(),
                Err(e) => return Err(e).context("read keychain"),
            };
            *vault = Some(loaded);
        }
        f(vault.as_mut().expect("loaded"))
    }

    fn save(&self, vault: &HashMap<String, String>) -> Result<()> {
        self.entry(VAULT_KEY)?
            .set_password(&serde_json::to_string(vault)?)
            .context("write keychain")
    }

    /// Reads and removes a pre-vault item for `key`, if there is one.
    fn take_legacy(&self, key: &str) -> Result<Option<String>> {
        if !self.legacy_checked.lock().unwrap().insert(key.to_string()) {
            return Ok(None);
        }
        let entry = self.entry(key)?;
        match entry.get_password() {
            Ok(v) => {
                let _ = entry.delete_credential();
                Ok(Some(v))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e).context("read keychain"),
        }
    }
}

impl SecretStore for KeychainSecrets {
    fn get(&self, key: &str) -> Result<Option<String>> {
        if let Some(v) = self.with_vault(|v| Ok(v.get(key).cloned()))? {
            return Ok(Some(v));
        }
        let Some(value) = self.take_legacy(key)? else {
            return Ok(None);
        };
        self.with_vault(|v| {
            v.insert(key.into(), value.clone());
            self.save(v)
        })?;
        Ok(Some(value))
    }

    fn set(&self, key: &str, value: &str) -> Result<()> {
        self.with_vault(|v| {
            if v.get(key).is_some_and(|old| old == value) {
                return Ok(());
            }
            v.insert(key.into(), value.into());
            self.save(v)
        })
    }

    fn delete(&self, key: &str) -> Result<()> {
        // A legacy item for this key may still exist; drop it too.
        let _ = self.take_legacy(key);
        self.with_vault(|v| {
            if v.remove(key).is_some() {
                self.save(v)?;
            }
            Ok(())
        })
    }
}

/// In-memory store for tests and headless experiments.
#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<String, String>>);

impl SecretStore for MemorySecrets {
    fn get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }

    fn set(&self, key: &str, value: &str) -> Result<()> {
        self.0.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

pub fn connection_key(connection_id: &str, key: &str) -> String {
    format!("conn:{connection_id}:{key}")
}

pub fn profile_token_key(profile_id: &str) -> String {
    format!("profile:{profile_id}:token")
}
