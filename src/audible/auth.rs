use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{io::Write, os::unix::fs::OpenOptionsExt, path::Path};

/// Credenciales de un dispositivo registrado en Audible.
#[derive(Serialize, Deserialize, Clone)]
pub struct Auth {
    pub access_token: String,
    pub refresh_token: String,
    pub adp_token: String,
    pub device_private_key: String,
    /// Caducidad del access_token, en segundos epoch.
    pub expires: f64,
    pub locale_code: String,
    pub device_info: DeviceInfo,
    pub customer_info: CustomerInfo,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct DeviceInfo {
    pub device_serial_number: String,
    pub device_type: String,
    pub device_name: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct CustomerInfo {
    pub user_id: String,
    #[serde(default)]
    pub name: String,
}

impl Auth {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read(path)
            .with_context(|| format!("no hay sesión en {} (ejecuta `tbard login`)", path.display()))?;
        serde_json::from_slice(&raw).context("auth.json inválido")
    }

    /// Guarda con permisos 0600: contiene tokens y la clave privada del dispositivo.
    pub fn save(&self, path: &Path) -> Result<()> {
        let tmp = path.with_extension("tmp");
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(self)?)?;
        f.sync_all()?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    /// Dominio de la tienda según el país (`us` → `com`).
    pub fn tld(&self) -> &'static str {
        tld(&self.locale_code)
    }
}

pub fn tld(locale: &str) -> &'static str {
    match locale {
        "uk" => "co.uk",
        "de" => "de",
        "fr" => "fr",
        "ca" => "ca",
        "it" => "it",
        "au" => "com.au",
        "in" => "in",
        "jp" => "co.jp",
        "es" => "es",
        "br" => "com.br",
        _ => "com",
    }
}
