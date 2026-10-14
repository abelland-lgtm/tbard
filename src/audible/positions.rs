use super::client::Client;
use anyhow::{Context, Result};
use reqwest::Method;
use serde_json::json;
use time::{macros::format_description, PrimitiveDateTime};

/// Posición guardada en Audible.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RemotePos {
    pub position_ms: i64,
    /// `last_updated`, en ms epoch (UTC).
    pub updated_ms: i64,
}

pub async fn last(client: &Client, asin: &str) -> Result<Option<RemotePos>> {
    let res = client.get("1.0/annotations/lastpositions", &[("asins", asin)]).await?;
    let pos = &res["asin_last_position_heard_annots"][0]["last_position_heard"];
    if pos["status"].as_str() != Some("Exists") {
        return Ok(None);
    }
    Ok(Some(RemotePos {
        position_ms: pos["position_ms"].as_i64().context("sin position_ms")?,
        updated_ms: parse_updated(pos["last_updated"].as_str().context("sin last_updated")?)?,
    }))
}

pub async fn put(client: &Client, asin: &str, acr: &str, position_ms: i64) -> Result<()> {
    let body = json!({ "acr": acr, "asin": asin, "position_ms": position_ms });
    client.request(Method::PUT, &format!("1.0/lastpositions/{asin}"), &[], Some(&body)).await?;
    Ok(())
}

/// `"2026-08-14 21:17:37.114"` (UTC) → ms epoch.
fn parse_updated(s: &str) -> Result<i64> {
    let fmt = format_description!("[year]-[month]-[day] [hour]:[minute]:[second].[subsecond]");
    let dt = PrimitiveDateTime::parse(s, fmt).with_context(|| format!("fecha inválida: {s}"))?;
    Ok((dt.assume_utc().unix_timestamp_nanos() / 1_000_000) as i64)
}

#[cfg(test)]
mod tests {
    #[test]
    fn parse_updated() {
        assert_eq!(super::parse_updated("2026-08-14 21:17:37.114").unwrap(), 1_786_742_257_114);
        // Audible recorta los ceros finales de los milisegundos.
        assert_eq!(super::parse_updated("2026-10-14 02:59:17.94").unwrap() % 1000, 940);
    }
}
