//! Registro de tbard como dispositivo de Audible (flujo OAuth con PKCE en el navegador).

use super::{
    auth::{tld, Auth, CustomerInfo, DeviceInfo},
    client::now_secs,
};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;
use url::Url;

const DEVICE_TYPE: &str = "A2CZJZGLK2JJVM";

fn marketplace(locale: &str) -> Result<&'static str> {
    Ok(match locale {
        "us" => "AF2M0KC94RCEA",
        "uk" => "A2I9A3Q2GNFNGQ",
        "de" => "AN7V1F1VY261K",
        "fr" => "A2728XDNODOQ8T",
        "ca" => "A2CQZ5RBY40XE",
        "it" => "A2N7FU2W2BU2ZC",
        "au" => "AN7EY7DTAW63G",
        "in" => "AJO3FBRUE6J4S",
        "jp" => "A1QAP3MOU4173J",
        "es" => "ALMIKO4SZCSAR",
        "br" => "A10J1VAYUDTYRN",
        _ => bail!("país no soportado: {locale}"),
    })
}

pub struct Pending {
    pub url: String,
    locale: String,
    serial: String,
    verifier: String,
}

/// Prepara la URL de inicio de sesión de Amazon para este dispositivo nuevo.
pub fn start(locale: &str) -> Result<Pending> {
    let serial = hex::encode_upper(rand::random::<[u8; 16]>());
    let verifier = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let tld = tld(locale);
    let url = Url::parse_with_params(
        &format!("https://www.amazon.{tld}/ap/signin"),
        &[
            ("openid.oa2.response_type", "code"),
            ("openid.oa2.code_challenge_method", "S256"),
            ("openid.oa2.code_challenge", &challenge),
            ("openid.return_to", &format!("https://www.amazon.{tld}/ap/maplanding")),
            ("openid.assoc_handle", &format!("amzn_audible_ios_{locale}")),
            ("openid.identity", "http://specs.openid.net/auth/2.0/identifier_select"),
            ("pageId", "amzn_audible_ios"),
            ("accountStatusPolicy", "P1"),
            ("openid.claimed_id", "http://specs.openid.net/auth/2.0/identifier_select"),
            ("openid.mode", "checkid_setup"),
            ("openid.ns.oa2", "http://www.amazon.com/ap/ext/oauth/2"),
            ("openid.oa2.client_id", &format!("device:{}", client_id(&serial))),
            ("openid.ns.pape", "http://specs.openid.net/extensions/pape/1.0"),
            ("marketPlaceId", marketplace(locale)?),
            ("openid.oa2.scope", "device_auth_access"),
            ("forceMobileLayout", "true"),
            ("openid.ns", "http://specs.openid.net/auth/2.0"),
            ("openid.pape.max_auth_age", "0"),
        ],
    )?;
    Ok(Pending { url: url.into(), locale: locale.to_owned(), serial, verifier })
}

fn client_id(serial: &str) -> String {
    hex::encode(format!("{serial}#{DEVICE_TYPE}"))
}

/// Saca el código de autorización de la URL final (`…/ap/maplanding?…`).
fn authorization_code(redirect: &str) -> Result<String> {
    Url::parse(redirect.trim())
        .context("no es una URL")?
        .query_pairs()
        .find(|(k, _)| k == "openid.oa2.authorization_code")
        .map(|(_, v)| v.into_owned())
        .context("la URL no trae openid.oa2.authorization_code")
}

/// Registra el dispositivo con el código obtenido y devuelve las credenciales.
pub async fn finish(http: &reqwest::Client, p: &Pending, redirect: &str) -> Result<Auth> {
    let code = authorization_code(redirect)?;
    let tld = tld(&p.locale);
    let body = json!({
        "requested_token_type": ["bearer", "mac_dms", "website_cookies", "store_authentication_cookie"],
        "cookies": { "website_cookies": [], "domain": format!(".amazon.{tld}") },
        "registration_data": {
            "domain": "Device",
            "app_version": "3.56.2",
            "device_serial": p.serial,
            "device_type": DEVICE_TYPE,
            "device_name": "%FIRST_NAME%%FIRST_NAME_POSSESSIVE_STRING%%DUPE_STRATEGY_1ST%tbard",
            "os_version": "15.0.0",
            "software_version": "35602678",
            "device_model": "iPhone",
            "app_name": "Audible",
        },
        "auth_data": {
            "client_id": client_id(&p.serial),
            "authorization_code": code,
            "code_verifier": p.verifier,
            "code_algorithm": "SHA-256",
            "client_domain": "DeviceLegacy",
        },
        "requested_extensions": ["device_info", "customer_info"],
    });
    let res: Value = http
        .post(format!("https://api.amazon.{tld}/auth/register"))
        .json(&body)
        .send()
        .await?
        .json()
        .await?;
    let ok = &res["response"]["success"];
    if ok.is_null() {
        bail!("registro rechazado: {res}");
    }
    let tokens = &ok["tokens"];
    let ext = &ok["extensions"];
    let str_ = |v: &Value| v.as_str().map(str::to_owned).context("respuesta de registro incompleta");
    let expires_in = match &tokens["bearer"]["expires_in"] {
        Value::String(s) => s.parse().unwrap_or(3600.0),
        v => v.as_f64().unwrap_or(3600.0),
    };
    Ok(Auth {
        access_token: str_(&tokens["bearer"]["access_token"])?,
        refresh_token: str_(&tokens["bearer"]["refresh_token"])?,
        adp_token: str_(&tokens["mac_dms"]["adp_token"])?,
        device_private_key: str_(&tokens["mac_dms"]["device_private_key"])?,
        expires: now_secs() + expires_in,
        locale_code: p.locale.clone(),
        device_info: DeviceInfo {
            device_serial_number: str_(&ext["device_info"]["device_serial_number"])?,
            device_type: str_(&ext["device_info"]["device_type"])?,
            device_name: str_(&ext["device_info"]["device_name"])?,
        },
        customer_info: CustomerInfo {
            user_id: str_(&ext["customer_info"]["user_id"])?,
            name: ext["customer_info"]["name"].as_str().unwrap_or_default().to_owned(),
        },
    })
}

/// Removes this device from the Amazon account (the inverse of `finish`).
pub async fn deregister(http: &reqwest::Client, auth: &Auth) -> Result<()> {
    let tld = auth.tld();
    let token: Value = http
        .post(format!("https://api.amazon.{tld}/auth/token"))
        .form(&[
            ("app_name", "Audible"),
            ("app_version", "3.56.2"),
            ("source_token", auth.refresh_token.as_str()),
            ("requested_token_type", "access_token"),
            ("source_token_type", "refresh_token"),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let access = token["access_token"].as_str().context("no access token in the response")?;
    http.post(format!("https://api.amazon.{tld}/auth/deregister"))
        .bearer_auth(access)
        .json(&json!({ "deregister_all_existing_accounts": false }))
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

/// Solo para desarrollo: reutiliza un auth file de `audible quickstart` (sin cifrar).
pub fn import(from: &Path) -> Result<Auth> {
    Auth::load(from).context("auth file de audible-cli ilegible (¿está cifrado?)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_from_redirect() {
        let url = "https://www.amazon.com/ap/maplanding?openid.oa2.authorization_code=ANabc123&openid.mode=id_res";
        assert_eq!(authorization_code(url).unwrap(), "ANabc123");
        assert!(authorization_code("https://www.amazon.com/ap/maplanding?x=1").is_err());
    }

    #[test]
    fn signin_url_has_pkce() {
        let p = start("es").unwrap();
        assert!(p.url.starts_with("https://www.amazon.es/ap/signin?"));
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(p.verifier.as_bytes()));
        assert!(p.url.contains(&challenge));
        assert!(p.url.contains("ALMIKO4SZCSAR"));
    }
}
