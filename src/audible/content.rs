use super::{auth::Auth, client::Client};
use aes::cipher::{block_padding::NoPadding, BlockModeDecrypt, KeyIvInit};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    pub title: String,
    pub start_ms: i64,
    pub length_ms: i64,
}

/// Licencia de descarga concedida para un libro.
pub struct License {
    pub acr: String,
    pub offline_url: String,
    pub content_format: String,
    /// Clave e IV AAXC en hex, listos para `ffmpeg -audible_key/-audible_iv`.
    pub key: String,
    pub iv: String,
    pub chapters: Vec<Chapter>,
}

/// Pide la licencia en máxima calidad: `High` concede el mejor de `available_codecs`.
pub async fn license(client: &Client, asin: &str) -> Result<License> {
    let body = json!({
        "consumption_type": "Download",
        "supported_drm_types": ["Adrm", "Mpeg"],
        "quality": "High",
        "chapter_titles_type": "Tree",
        "response_groups": "content_reference,chapter_info",
    });
    let res = client
        .request(Method::POST, &format!("1.0/content/{asin}/licenserequest"), &[], Some(&body))
        .await?;
    let lic = &res["content_license"];
    if lic["status_code"].as_str() != Some("Granted") {
        bail!("licencia denegada: {}", lic["message"]);
    }
    let meta = &lic["content_metadata"];
    let (key, iv) = decrypt_voucher(
        client.auth(),
        asin,
        lic["license_response"].as_str().context("sin license_response")?,
    )?;
    Ok(License {
        acr: lic["acr"].as_str().context("sin acr")?.to_owned(),
        offline_url: meta["content_url"]["offline_url"].as_str().context("sin offline_url")?.to_owned(),
        content_format: meta["content_reference"]["content_format"].as_str().unwrap_or_default().to_owned(),
        key,
        iv,
        chapters: flatten(&meta["chapter_info"]["chapters"], ""),
    })
}

/// Aplana el árbol de capítulos: las partes con subcapítulos se convierten en
/// hojas "Parte – Capítulo". El tramo propio de una parte (antes de su primer
/// subcapítulo) se conserva como capítulo aparte para no perder audio.
fn flatten(chapters: &Value, prefix: &str) -> Vec<Chapter> {
    let mut out = Vec::new();
    for ch in chapters.as_array().into_iter().flatten() {
        let title = ch["title"].as_str().unwrap_or_default();
        let title = if prefix.is_empty() { title.to_owned() } else { format!("{prefix} – {title}") };
        let start_ms = ch["start_offset_ms"].as_i64().unwrap_or(0);
        let length_ms = ch["length_ms"].as_i64().unwrap_or(0);
        match ch.get("chapters") {
            Some(children) if children.as_array().is_some_and(|c| !c.is_empty()) => {
                let kids = flatten(children, &title);
                let own = kids[0].start_ms - start_ms;
                if own > 0 {
                    out.push(Chapter { title: title.clone(), start_ms, length_ms: own });
                }
                out.extend(kids);
            }
            _ => out.push(Chapter { title, start_ms, length_ms }),
        }
    }
    out
}

/// Descifra el voucher AAXC. Clave e IV del AES-128-CBC = SHA-256 de
/// `device_type + serial + customer_id + asin` (bytes 0..16 y 16..32).
pub fn decrypt_voucher(auth: &Auth, asin: &str, voucher_b64: &str) -> Result<(String, String)> {
    let digest = voucher_digest(auth, asin);
    let mut buf = STANDARD.decode(voucher_b64)?;
    buf.truncate(buf.len() / 16 * 16);
    let plain = cbc::Decryptor::<aes::Aes128>::new_from_slices(&digest[..16], &digest[16..])
        .map_err(|_| anyhow::anyhow!("clave inválida"))?
        .decrypt_padded::<NoPadding>(&mut buf)
        .map_err(|_| anyhow::anyhow!("voucher corrupto"))?;
    // El JSON va seguido de relleno: se corta en la última llave.
    let end = plain.iter().rposition(|&b| b == b'}').context("voucher sin JSON")? + 1;
    let v: Value = serde_json::from_slice(&plain[..end]).context("voucher ilegible")?;
    Ok((
        v["key"].as_str().context("voucher sin key")?.to_owned(),
        v["iv"].as_str().context("voucher sin iv")?.to_owned(),
    ))
}

fn voucher_digest(auth: &Auth, asin: &str) -> Vec<u8> {
    let d = &auth.device_info;
    Sha256::digest(format!("{}{}{}{asin}", d.device_type, d.device_serial_number, auth.customer_info.user_id)).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audible::auth::{CustomerInfo, DeviceInfo};
    use aes::cipher::BlockModeEncrypt;

    fn auth() -> Auth {
        Auth {
            access_token: String::new(),
            refresh_token: String::new(),
            adp_token: String::new(),
            device_private_key: String::new(),
            expires: 0.0,
            locale_code: "us".into(),
            device_info: DeviceInfo {
                device_serial_number: "SERIAL".into(),
                device_type: "A2CZJZGLK2JJVM".into(),
                device_name: String::new(),
            },
            customer_info: CustomerInfo { user_id: "amzn1.account.X".into(), name: String::new() },
        }
    }

    #[test]
    fn voucher_roundtrip() {
        let digest = voucher_digest(&auth(), "B000TEST");
        let json = br#"{"key":"0123456789abcdef0123456789abcdef","iv":"fedcba9876543210fedcba9876543210","rules":[]}"#;
        let len = json.len().div_ceil(16) * 16;
        let mut buf = vec![0u8; len];
        buf[..json.len()].copy_from_slice(json);
        cbc::Encryptor::<aes::Aes128>::new_from_slices(&digest[..16], &digest[16..])
            .unwrap()
            .encrypt_padded::<NoPadding>(&mut buf, len)
            .unwrap();
        let (key, iv) = decrypt_voucher(&auth(), "B000TEST", &STANDARD.encode(&buf)).unwrap();
        assert_eq!(key, "0123456789abcdef0123456789abcdef");
        assert_eq!(iv, "fedcba9876543210fedcba9876543210");
    }

    #[test]
    fn flatten_tree() {
        let tree = json!([
            {"title": "Créditos", "start_offset_ms": 0, "length_ms": 1000},
            {"title": "Parte 1", "start_offset_ms": 1000, "length_ms": 9000, "chapters": [
                {"title": "Cap 1", "start_offset_ms": 1500, "length_ms": 4000},
                {"title": "Cap 2", "start_offset_ms": 5500, "length_ms": 4500}
            ]}
        ]);
        let titles: Vec<_> = flatten(&tree, "").into_iter().map(|c| (c.title, c.start_ms, c.length_ms)).collect();
        assert_eq!(
            titles,
            [
                ("Créditos".into(), 0, 1000),
                ("Parte 1".into(), 1000, 500),
                ("Parte 1 – Cap 1".into(), 1500, 4000),
                ("Parte 1 – Cap 2".into(), 5500, 4500),
            ]
        );
    }
}
