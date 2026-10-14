use super::client::Client;
use anyhow::Result;
use serde_json::Value;

pub struct LibraryItem {
    pub asin: String,
    pub title: String,
    pub authors: String,
    pub narrators: String,
    pub series: String,
    pub runtime_ms: i64,
    pub percent_complete: f64,
    pub cover_url: Option<String>,
}

const PAGE: usize = 500;

/// The whole library (books only; podcasts are left out).
pub async fn fetch(client: &Client) -> Result<Vec<LibraryItem>> {
    let mut out = Vec::new();
    for page in 1.. {
        let page = page.to_string();
        let res = client
            .get(
                "1.0/library",
                &[
                    ("num_results", &PAGE.to_string()),
                    ("page", &page),
                    ("response_groups", "product_attrs,contributors,series,media,percent_complete,product_desc"),
                    ("image_sizes", "500"),
                ],
            )
            .await?;
        let items = res["items"].as_array().cloned().unwrap_or_default();
        out.extend(items.iter().filter_map(parse));
        // The API does not return total_results: page until a page comes back short.
        if items.len() < PAGE {
            break;
        }
    }
    Ok(out)
}

fn parse(item: &Value) -> Option<LibraryItem> {
    let kind = item["content_delivery_type"].as_str().unwrap_or_default();
    if !matches!(kind, "SinglePartBook" | "MultiPartBook") {
        return None;
    }
    let series = &item["series"][0];
    Some(LibraryItem {
        asin: item["asin"].as_str()?.to_owned(),
        title: item["title"].as_str()?.to_owned(),
        authors: names(&item["authors"]),
        narrators: names(&item["narrators"]),
        series: match (series["title"].as_str(), series["sequence"].as_str()) {
            (Some(t), Some(n)) => format!("{t} {n}"),
            (Some(t), None) => t.to_owned(),
            _ => String::new(),
        },
        runtime_ms: item["runtime_length_min"].as_i64().unwrap_or(0) * 60_000,
        percent_complete: item["percent_complete"].as_f64().unwrap_or(0.0),
        cover_url: item["product_images"]["500"].as_str().map(str::to_owned),
    })
}

/// Joins the names, dropping secondary roles ("Someone - translator").
fn names(list: &Value) -> String {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["name"].as_str())
        .filter(|n| !n.contains(" - "))
        .collect::<Vec<_>>()
        .join(", ")
}
