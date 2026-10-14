//! Audible credentials at rest.
//!
//! The credentials (tokens and the device's private key) are encrypted with
//! AES-256-GCM into `credentials.enc`; the 256-bit key lives in the system
//! keychain (macOS Keychain, Windows Credential Manager, Secret Service/KWallet
//! on Linux). Copying the data folder to another machine or user is therefore
//! useless without the keychain. When no keychain is available (e.g. a
//! headless Linux box), they fall back to `credentials.json`, readable only by
//! the current user, and tbard says so.

use crate::{audible::auth::Auth, config::write_private};
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use std::path::Path;

const ENCRYPTED: &str = "credentials.enc";
const PLAIN: &str = "credentials.json";
/// Plain file written by tbard before credentials were encrypted.
const LEGACY: &str = "auth.json";

/// How the credentials are protected.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Protection {
    /// Encrypted; the key is in the system keychain.
    Keychain,
    /// Unencrypted file readable only by the current user.
    FileOnly,
}

/// Where the encryption key is kept. The OS keychain in practice; an
/// in-memory store in tests.
pub trait KeyStore {
    fn get(&self) -> Result<Option<String>>;
    fn set(&self, key: &str) -> Result<()>;
    fn delete(&self) -> Result<()>;
}

pub struct OsKeychain;

impl OsKeychain {
    fn entry() -> Result<keyring::Entry> {
        Ok(keyring::Entry::new("tbard", "credentials-key")?)
    }
}

impl KeyStore for OsKeychain {
    fn get(&self) -> Result<Option<String>> {
        match Self::entry()?.get_password() {
            Ok(k) => Ok(Some(k)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn set(&self, key: &str) -> Result<()> {
        Ok(Self::entry()?.set_password(key)?)
    }

    fn delete(&self) -> Result<()> {
        match Self::entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// On-disk format of `credentials.enc`.
#[derive(Serialize, Deserialize)]
struct Envelope {
    v: u8,
    nonce: String,
    data: String,
}

fn cipher(key_b64: &str) -> Result<Aes256Gcm> {
    let key = STANDARD.decode(key_b64).context("corrupt key in the keychain")?;
    Aes256Gcm::new_from_slice(&key).map_err(|_| anyhow::anyhow!("corrupt key in the keychain"))
}

fn encrypt(key_b64: &str, plain: &[u8]) -> Result<Vec<u8>> {
    let nonce: [u8; 12] = rand::random();
    let data = cipher(key_b64)?
        .encrypt(&Nonce::from(nonce), plain)
        .map_err(|_| anyhow::anyhow!("encryption failed"))?;
    let env = Envelope { v: 1, nonce: STANDARD.encode(nonce), data: STANDARD.encode(data) };
    Ok(serde_json::to_vec(&env)?)
}

fn decrypt(key_b64: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    let env: Envelope = serde_json::from_slice(bytes).context("corrupt credentials file")?;
    if env.v != 1 {
        bail!("credentials file from a newer tbard (format {})", env.v);
    }
    let nonce: [u8; 12] =
        STANDARD.decode(&env.nonce)?.try_into().map_err(|_| anyhow::anyhow!("corrupt credentials file"))?;
    cipher(key_b64)?
        .decrypt(&Nonce::from(nonce), STANDARD.decode(&env.data)?.as_slice())
        .map_err(|_| anyhow::anyhow!("credentials cannot be decrypted with the key in the keychain"))
}

/// Saves the credentials in `dir`, encrypted when a keychain is available.
pub fn save(dir: &Path, auth: &Auth, keys: &dyn KeyStore) -> Result<Protection> {
    let plain = serde_json::to_vec_pretty(auth)?;
    let key = keys.get().and_then(|k| match k {
        Some(k) => Ok(k),
        None => {
            let k = STANDARD.encode(rand::random::<[u8; 32]>());
            keys.set(&k)?;
            Ok(k)
        }
    });
    let protection = match key {
        Ok(key) => {
            write_private(&dir.join(ENCRYPTED), &encrypt(&key, &plain)?)?;
            let _ = std::fs::remove_file(dir.join(PLAIN));
            Protection::Keychain
        }
        Err(e) => {
            tracing::warn!("no system keychain ({e:#}); credentials stored unencrypted");
            write_private(&dir.join(PLAIN), &plain)?;
            let _ = std::fs::remove_file(dir.join(ENCRYPTED));
            Protection::FileOnly
        }
    };
    let _ = std::fs::remove_file(dir.join(LEGACY));
    Ok(protection)
}

/// Loads the credentials from `dir`, migrating the old plain `auth.json`.
pub fn load(dir: &Path, keys: &dyn KeyStore) -> Result<(Auth, Protection)> {
    let enc = dir.join(ENCRYPTED);
    if enc.exists() {
        let key = keys
            .get()
            .context("cannot reach the system keychain to decrypt the credentials")?
            .context("the credentials key is missing from the system keychain: run `tbard login` again")?;
        let auth = serde_json::from_slice(&decrypt(&key, &std::fs::read(enc)?)?)?;
        return Ok((auth, Protection::Keychain));
    }
    if dir.join(PLAIN).exists() {
        return Ok((Auth::load(&dir.join(PLAIN))?, Protection::FileOnly));
    }
    if dir.join(LEGACY).exists() {
        let auth = Auth::load(&dir.join(LEGACY))?;
        let protection = save(dir, &auth, keys)?;
        return Ok((auth, protection));
    }
    bail!("not logged in: run `tbard login`")
}

/// Removes the credentials from disk and their key from the keychain.
pub fn delete(dir: &Path, keys: &dyn KeyStore) -> Result<()> {
    for name in [ENCRYPTED, PLAIN, LEGACY] {
        match std::fs::remove_file(dir.join(name)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    keys.delete()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audible::auth::{CustomerInfo, DeviceInfo};
    use std::cell::RefCell;

    #[derive(Default)]
    struct MemKeys(RefCell<Option<String>>);

    impl KeyStore for MemKeys {
        fn get(&self) -> Result<Option<String>> {
            Ok(self.0.borrow().clone())
        }
        fn set(&self, key: &str) -> Result<()> {
            *self.0.borrow_mut() = Some(key.to_owned());
            Ok(())
        }
        fn delete(&self) -> Result<()> {
            *self.0.borrow_mut() = None;
            Ok(())
        }
    }

    struct NoKeychain;

    impl KeyStore for NoKeychain {
        fn get(&self) -> Result<Option<String>> {
            bail!("no keychain")
        }
        fn set(&self, _: &str) -> Result<()> {
            bail!("no keychain")
        }
        fn delete(&self) -> Result<()> {
            Ok(())
        }
    }

    fn auth() -> Auth {
        Auth {
            access_token: "access".into(),
            refresh_token: "refresh-SECRET".into(),
            adp_token: "adp".into(),
            device_private_key: "key".into(),
            expires: 0.0,
            locale_code: "us".into(),
            device_info: DeviceInfo {
                device_serial_number: "S".into(),
                device_type: "T".into(),
                device_name: "N".into(),
            },
            customer_info: CustomerInfo { user_id: "U".into(), name: "Name".into() },
        }
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tbard-secret-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn encrypted_roundtrip_and_wrong_key() {
        let dir = tmp("enc");
        let keys = MemKeys::default();
        assert_eq!(save(&dir, &auth(), &keys).unwrap(), Protection::Keychain);
        let on_disk = std::fs::read_to_string(dir.join(ENCRYPTED)).unwrap();
        assert!(!on_disk.contains("refresh-SECRET"));
        let (loaded, p) = load(&dir, &keys).unwrap();
        assert_eq!((loaded.refresh_token.as_str(), p), ("refresh-SECRET", Protection::Keychain));

        // Another key (another user or machine) cannot decrypt it.
        keys.set(&STANDARD.encode([7u8; 32])).unwrap();
        assert!(load(&dir, &keys).is_err());
        delete(&dir, &keys).unwrap();
        assert!(keys.get().unwrap().is_none());
        assert!(load(&dir, &keys).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn migrates_legacy_plain_file() {
        let dir = tmp("legacy");
        std::fs::write(dir.join(LEGACY), serde_json::to_vec(&auth()).unwrap()).unwrap();
        let keys = MemKeys::default();
        let (loaded, p) = load(&dir, &keys).unwrap();
        assert_eq!((loaded.adp_token.as_str(), p), ("adp", Protection::Keychain));
        assert!(!dir.join(LEGACY).exists());
        assert!(dir.join(ENCRYPTED).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn falls_back_to_private_file_without_keychain() {
        let dir = tmp("plain");
        assert_eq!(save(&dir, &auth(), &NoKeychain).unwrap(), Protection::FileOnly);
        assert!(dir.join(PLAIN).exists());
        let (_, p) = load(&dir, &NoKeychain).unwrap();
        assert_eq!(p, Protection::FileOnly);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
