use crate::{
    audible::{client::Client, positions, positions::RemotePos},
    db::{Db, LocalPos},
};
use anyhow::Result;

/// If local and remote differ by more than this and both changed, ask.
const ASK_THRESHOLD_MS: i64 = 2 * 60_000;

#[derive(Debug, PartialEq)]
pub enum Decision {
    Start(i64),
    /// Both changed since the last sync and are far apart: the user chooses.
    Ask {
        local: i64,
        remote: i64,
    },
}

pub fn decide(local: Option<LocalPos>, remote: Option<RemotePos>) -> Decision {
    match (local, remote) {
        (None, None) => Decision::Start(0),
        (Some(l), None) => Decision::Start(l.position_ms),
        (None, Some(r)) => Decision::Start(r.position_ms),
        (Some(l), Some(r)) => {
            let synced = l.synced_ms.unwrap_or(i64::MIN);
            // Audible stores its own write time: if it is not later than our
            // last sync, the remote change is ours.
            let local_changed = l.updated_ms > synced;
            let remote_changed = r.updated_ms > synced + 5_000;
            if local_changed && remote_changed && (l.position_ms - r.position_ms).abs() > ASK_THRESHOLD_MS {
                Decision::Ask { local: l.position_ms, remote: r.position_ms }
            } else if r.updated_ms > l.updated_ms {
                Decision::Start(r.position_ms)
            } else {
                Decision::Start(l.position_ms)
            }
        }
    }
}

/// Decides where to start from the local and the Audible position. Offline,
/// uses the local one.
pub async fn start(client: &Client, db: &Db, asin: &str) -> Result<(Decision, bool)> {
    let remote = positions::last(client, asin).await;
    let online = remote.is_ok();
    Ok((decide(db.position(asin)?, remote.ok().flatten()), online))
}

/// Sends a position; if that fails (offline, etc.) it stays queued.
pub async fn push(client: &Client, db: &Db, asin: &str, acr: &str, position_ms: i64) -> bool {
    match positions::put(client, asin, acr, position_ms).await {
        Ok(()) => {
            tracing::info!("{asin}: position {position_ms} sent");
            db.mark_synced(asin, position_ms, now_ms()).is_ok()
        }
        Err(e) => {
            tracing::warn!("{asin}: could not send {position_ms} ({e:#}); queued");
            let _ = db.enqueue(asin, acr, position_ms);
            false
        }
    }
}

/// Retries the queued positions. Returns how many are left.
pub async fn flush(client: &Client, db: &Db) -> Result<usize> {
    let pending = db.pending()?;
    let mut left = 0;
    for p in &pending {
        if !push(client, db, &p.asin, &p.acr, p.position_ms).await {
            left += 1;
        }
    }
    Ok(left)
}

pub fn now_ms() -> i64 {
    (crate::audible::client::now_secs() * 1000.0) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(pos: i64, upd: i64, synced: Option<i64>) -> Option<LocalPos> {
        Some(LocalPos { position_ms: pos, updated_ms: upd, synced_ms: synced })
    }
    fn r(pos: i64, upd: i64) -> Option<RemotePos> {
        Some(RemotePos { position_ms: pos, updated_ms: upd })
    }

    #[test]
    fn decide_cases() {
        let min = 60_000;
        // Only one source.
        assert_eq!(decide(None, None), Decision::Start(0));
        assert_eq!(decide(l(5, 1, None), None), Decision::Start(5));
        assert_eq!(decide(None, r(7, 1)), Decision::Start(7));
        // Listened on the phone after the last sync: remote wins.
        assert_eq!(decide(l(10 * min, 100, Some(100)), r(30 * min, 500_000)), Decision::Start(30 * min));
        // Listened here without being able to send: local wins.
        assert_eq!(decide(l(30 * min, 500_000, Some(100)), r(10 * min, 100)), Decision::Start(30 * min));
        // Both changed and are far apart: ask.
        assert_eq!(
            decide(l(30 * min, 400_000, Some(100)), r(10 * min, 500_000)),
            Decision::Ask { local: 30 * min, remote: 10 * min }
        );
        // Both changed but are close: the most recent, without asking.
        assert_eq!(decide(l(10 * min, 400_000, Some(100)), r(11 * min, 500_000)), Decision::Start(11 * min));
        // Never synced and far apart: ask.
        assert_eq!(decide(l(30 * min, 2, None), r(0, 1)), Decision::Ask { local: 30 * min, remote: 0 });
    }
}

/// The whole flow against a local HTTP server: offline it queues, online the
/// queue drains and the request is signed.
#[cfg(test)]
mod flow_tests {
    use super::*;
    use crate::audible::auth::{Auth, CustomerInfo, DeviceInfo};
    use rsa::pkcs1::{EncodeRsaPrivateKey, LineEnding};
    use std::sync::{Arc, Mutex};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    fn client(base: &str) -> Client {
        let key = rsa::RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 1024).unwrap();
        let auth = Auth {
            access_token: String::new(),
            refresh_token: String::new(),
            adp_token: "ADP".into(),
            device_private_key: key.to_pkcs1_pem(LineEnding::LF).unwrap().to_string(),
            expires: 0.0,
            locale_code: "us".into(),
            device_info: DeviceInfo {
                device_serial_number: "S".into(),
                device_type: "T".into(),
                device_name: String::new(),
            },
            customer_info: CustomerInfo { user_id: "U".into(), name: String::new() },
        };
        Client::from_auth(auth).unwrap().with_base(base)
    }

    /// Answers an empty 200 to everything and records the requests.
    async fn server() -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                // Headers and body may arrive in separate reads.
                let mut req = Vec::new();
                let mut buf = [0u8; 4096];
                while let Ok(Ok(n @ 1..)) =
                    tokio::time::timeout(std::time::Duration::from_millis(100), sock.read(&mut buf)).await
                {
                    req.extend_from_slice(&buf[..n]);
                }
                log.lock().unwrap().push(String::from_utf8_lossy(&req).into_owned());
                let _ = sock.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
            }
        });
        (base, seen)
    }

    #[tokio::test]
    async fn offline_queues_then_flush_sends_signed() {
        let db = Db::memory().unwrap();
        // Port 1 is closed: same as having no network.
        assert!(!push(&client("http://127.0.0.1:1"), &db, "A", "acr", 42_000).await);
        assert_eq!(db.pending().unwrap().len(), 1);

        let (base, seen) = server().await;
        assert_eq!(flush(&client(&base), &db).await.unwrap(), 0);
        assert!(db.pending().unwrap().is_empty());
        assert_eq!(db.position("A").unwrap().unwrap().position_ms, 42_000);

        let req = seen.lock().unwrap().join("\n").to_lowercase();
        assert!(req.starts_with("put /1.0/lastpositions/a "), "{req}");
        assert!(req.contains("x-adp-token: adp"));
        assert!(req.contains("x-adp-signature: "));
        assert!(req.contains("\"position_ms\":42000"));
    }
}
