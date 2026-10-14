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
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use time::{macros::format_description, OffsetDateTime};

/// Cliente de la API interna de Audible (`api.audible.<tld>`). Cada petición
/// va firmada con la clave del dispositivo (firma ADP): Audible rechaza el
/// Bearer con `client-id: 0` para dispositivos registrados como iPhone.
pub struct Client {
    pub http: reqwest::Client,
    auth: Auth,
    key: SigningKey<rsa::sha2::Sha256>,
}

impl Client {
    pub fn new(auth_path: PathBuf) -> Result<Self> {
        let auth = Auth::load(&auth_path)?;
        let key = RsaPrivateKey::from_pkcs1_pem(&auth.device_private_key).context("clave del dispositivo inválida")?;
        let http = reqwest::Client::builder().user_agent("Audible/3.56.2 iOS/15.0.0").build()?;
        Ok(Self { http, auth, key: SigningKey::new(key) })
    }

    pub fn auth(&self) -> &Auth {
        &self.auth
    }

    pub async fn request(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Result<Value> {
        let mut req = self
            .http
            .request(method.clone(), format!("https://api.audible.{}/{path}", self.auth.tld()))
            .query(query);
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
        let status = res.status();
        let text = res.text().await?;
        if !status.is_success() {
            bail!("{method} {path}: {status}: {text}");
        }
        // El PUT de posiciones responde 200 con cuerpo vacío.
        Ok(if text.trim().is_empty() { Value::Null } else { serde_json::from_str(&text)? })
    }

    pub async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value> {
        self.request(Method::GET, path, query, None).await
    }
}

pub fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64()
}
