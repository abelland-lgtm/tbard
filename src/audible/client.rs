use super::auth::Auth;
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::Method;
use rsa::{
    pkcs1::DecodeRsaPrivateKey,
    pkcs1v15::SigningKey,
    signature::{SignatureEncoding, Signer},
    RsaPrivateKey,
};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};
use time::{macros::format_description, OffsetDateTime};

/// Cliente de la API interna de Audible (`api.audible.<tld>`). Cada petición
/// va firmada con la clave del dispositivo (firma ADP): Audible rechaza el
/// Bearer con `client-id: 0` para dispositivos registrados como iPhone.
pub struct Client {
    pub http: reqwest::Client,
    /// `https://api.audible.<tld>`; los tests lo apuntan a un servidor local.
    base: String,
    auth: Auth,
    key: SigningKey<rsa::sha2::Sha256>,
    /// How the credentials are stored on this machine.
    pub protection: crate::secret::Protection,
}

impl Client {
    /// Loads the saved credentials from the private data directory.
    pub fn open(data_dir: &std::path::Path) -> Result<Self> {
        let (auth, protection) = crate::secret::load(data_dir, &crate::secret::OsKeychain)?;
        Ok(Self { protection, ..Self::from_auth(auth)? })
    }

    pub fn from_auth(auth: Auth) -> Result<Self> {
        let key = RsaPrivateKey::from_pkcs1_pem(&auth.device_private_key).context("clave del dispositivo inválida")?;
        let http = reqwest::Client::builder().user_agent("Audible/3.56.2 iOS/15.0.0").build()?;
        let base = format!("https://api.audible.{}", auth.tld());
        let protection = crate::secret::Protection::Keychain;
        Ok(Self { http, base, auth, key: SigningKey::new(key), protection })
    }

    pub fn auth(&self) -> &Auth {
        &self.auth
    }

    #[cfg(test)]
    pub fn with_base(mut self, base: &str) -> Self {
        self.base = base.to_owned();
        self
    }

    /// Reintenta los 5xx y 429 (dos veces, con espera creciente). Los errores de
    /// red fallan enseguida: la cola de posiciones ya cubre el modo sin conexión.
    pub async fn request(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Result<Value> {
        let mut attempt = 0;
        loop {
            let (status, text) = self.send(method.clone(), path, query, body).await?;
            let transient = status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
            if transient && attempt < 2 {
                attempt += 1;
                tracing::warn!("{method} {path}: {status}, reintento {attempt}");
                tokio::time::sleep(std::time::Duration::from_secs(1 << attempt)).await;
                continue;
            }
            if !status.is_success() {
                bail!("{method} {path}: {status}: {text}");
            }
            // El PUT de posiciones responde 200 con cuerpo vacío.
            return Ok(if text.trim().is_empty() { Value::Null } else { serde_json::from_str(&text)? });
        }
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Result<(reqwest::StatusCode, String)> {
        let mut req = self
            .http
            .request(method.clone(), format!("{}/{path}", self.base))
            .query(query)
            // Sin red, que falle rápido y la posición vaya a la cola.
            .timeout(std::time::Duration::from_secs(15));
        if let Some(body) = body {
            req = req.json(body);
        }
        let mut req = req.build()?;
        let url = req.url();
        let signed_path = match url.query() {
            Some(q) => format!("{}?{q}", url.path()),
            None => url.path().to_owned(),
        };
        let body = req.body().and_then(|b| b.as_bytes()).unwrap_or_default();
        let date = OffsetDateTime::now_utc().format(format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:6]Z"
        ))?;
        let data = format!(
            "{method}\n{signed_path}\n{date}\n{}\n{}",
            String::from_utf8_lossy(body),
            self.auth.adp_token
        );
        let signature = STANDARD.encode(self.key.sign(data.as_bytes()).to_bytes());
        let h = req.headers_mut();
        h.insert("x-adp-token", self.auth.adp_token.parse()?);
        h.insert("x-adp-alg", "SHA256withRSA:1.0".parse()?);
        h.insert("x-adp-signature", format!("{signature}:{date}").parse()?);

        let res = self.http.execute(req).await?;
        Ok((res.status(), res.text().await?))
    }

    pub async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value> {
        self.request(Method::GET, path, query, None).await
    }
}

pub fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64()
}
