//! Secret storage in the OS credential store (Windows Credential Manager,
//! macOS Keychain, Linux kernel keyring) via the `keyring` crate.
//!
//! Each secret is stored as its own entry: Windows limits one credential to
//! 2,560 bytes, and Microsoft refresh tokens plus Minecraft tokens together can
//! come close to that.

use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::{anyhow, Result};

const SERVICE: &str = "MioCraft";

/// In-memory store used instead of the OS store in tests.
static MEMORY: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

#[doc(hidden)]
pub fn use_memory_store() {
    let mut m = MEMORY.lock().unwrap();
    if m.is_none() {
        *m = Some(HashMap::new());
    }
}

fn entry(key: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, key).map_err(|e| anyhow!("credential store unavailable: {e}"))
}

pub fn set(key: &str, value: &str) -> Result<()> {
    if let Some(m) = MEMORY.lock().unwrap().as_mut() {
        m.insert(key.into(), value.into());
        return Ok(());
    }
    entry(key)?
        .set_password(value)
        .map_err(|e| anyhow!("couldn't save to the credential store: {e}"))
}

pub fn get(key: &str) -> Result<Option<String>> {
    if let Some(m) = MEMORY.lock().unwrap().as_ref() {
        return Ok(m.get(key).cloned());
    }
    match entry(key)?.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(anyhow!("couldn't read the credential store: {e}")),
    }
}

pub fn delete(key: &str) {
    if let Some(m) = MEMORY.lock().unwrap().as_mut() {
        m.remove(key);
        return;
    }
    if let Ok(e) = entry(key) {
        let _ = e.delete_credential();
    }
}
