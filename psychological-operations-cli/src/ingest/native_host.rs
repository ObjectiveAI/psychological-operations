//! Chrome native-messaging protocol implementation.
//!
//! Frame format: `[u32 LE length][UTF-8 JSON payload]` on both
//! directions. We loop reading frames from stdin, dispatching to a
//! tiny JSON-tagged protocol, and writing framed JSON replies.
//!
//! The host stays alive for as long as the extension's port is open;
//! Chrome closes stdin when the port disconnects, which we read as
//! EOF and exit cleanly.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::extract::IncomingPostId;
use super::identity::{self, Identity};
use crate::x_app::config::{self as x_app_config, XAppConfig};
use crate::db::Db;

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Inbound {
    Init,
    Ingest { tweets: Vec<IncomingPostId> },
    XAppSave { credentials: IncomingCredentials },
}

#[derive(Debug, Deserialize, Default)]
struct IncomingCredentials {
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    api_key_secret: Option<String>,
    #[serde(default)]
    bearer_token: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Outbound<'a> {
    InitOk { psyop: &'a str, commit: &'a str },
    InitErr { error: String },
    IngestOk { inserted: usize, skipped: usize },
    IngestErr { error: String },
    XAppSaveOk,
    XAppSaveErr { error: String },
}

pub async fn run() -> Result<crate::Output, crate::error::Error> {
    // Resolve identity up front so an early failure is reported as
    // init_err on the first message rather than panicking later.
    let identity_result = identity::resolve();

    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut db: Option<Db> = None;

    loop {
        let mut len_buf = [0u8; 4];
        match stdin.read_exact(&mut len_buf).await {
            Ok(_) => {}
            // Clean EOF — Chrome closed the port.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => {
                return Err(crate::error::Error::Io(e));
            }
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        // Chrome's spec caps inbound messages at 1 MB. Be generous; bail
        // on anything implausible to avoid allocating gigabytes on a
        // garbled stream.
        if len > 16 * 1024 * 1024 {
            return Err(crate::error::Error::Other(format!(
                "native-messaging frame too large: {len} bytes",
            )));
        }
        let mut buf = vec![0u8; len];
        stdin.read_exact(&mut buf).await.map_err(crate::error::Error::Io)?;

        // Parse leniently — bad JSON is reported, not fatal.
        let parsed: Result<Inbound, _> = serde_json::from_slice(&buf);
        let reply: Outbound = match parsed {
            Err(e) => Outbound::IngestErr {
                error: format!("malformed message: {e}"),
            },
            Ok(Inbound::Init) => match &identity_result {
                Ok(id) => Outbound::InitOk { psyop: &id.psyop, commit: &id.commit },
                Err(e) => Outbound::InitErr { error: e.to_string() },
            },
            Ok(Inbound::Ingest { tweets }) => match &identity_result {
                Err(e) => Outbound::IngestErr { error: e.to_string() },
                Ok(id) => match handle_ingest(&mut db, id, tweets) {
                    Ok((inserted, skipped)) => Outbound::IngestOk { inserted, skipped },
                    Err(e) => Outbound::IngestErr { error: e.to_string() },
                },
            },
            Ok(Inbound::XAppSave { credentials }) => match handle_x_app_save(credentials) {
                Ok(()) => Outbound::XAppSaveOk,
                Err(e) => Outbound::XAppSaveErr { error: e.to_string() },
            },
        };

        write_frame(&mut stdout, &reply).await?;
    }
    Ok(crate::Output::Empty)
}

fn handle_ingest(
    db: &mut Option<Db>,
    identity: &Identity,
    tweets: Vec<IncomingPostId>,
) -> Result<(usize, usize), crate::error::Error> {
    if db.is_none() {
        *db = Some(Db::open()?);
    }
    let db = db.as_ref().unwrap();

    let mut inserted = 0;
    let mut skipped = 0;
    for incoming in tweets {
        let id = match incoming.into_id() {
            Ok(s) => s,
            Err(_reason) => {
                skipped += 1;
                continue;
            }
        };
        match db.enqueue_for_you(&id, &identity.psyop, &identity.commit) {
            Ok(true) => inserted += 1,
            Ok(false) => skipped += 1,
            Err(_) => skipped += 1,
        }
    }
    Ok((inserted, skipped))
}

/// Merge incoming credentials into the on-disk x_app.json.
/// Some-wins, None-preserves so a partial paste doesn't clobber
/// previously-captured fields.
fn handle_x_app_save(creds: IncomingCredentials) -> Result<(), crate::error::Error> {
    let existing = x_app_config::load().unwrap_or_default();
    let now = chrono::Utc::now().to_rfc3339();
    let incoming = XAppConfig {
        client_id:      creds.client_id,
        client_secret:  creds.client_secret,
        api_key:        creds.api_key,
        api_key_secret: creds.api_key_secret,
        bearer_token:   creds.bearer_token,
        saved_at:       Some(now),
    };
    let merged = x_app_config::merge(existing, incoming);
    x_app_config::save(&merged)
}

async fn write_frame<W: tokio::io::AsyncWrite + Unpin>(
    w: &mut W,
    reply: &Outbound<'_>,
) -> Result<(), crate::error::Error> {
    let body = serde_json::to_vec(reply)?;
    let len = (body.len() as u32).to_le_bytes();
    w.write_all(&len).await.map_err(crate::error::Error::Io)?;
    w.write_all(&body).await.map_err(crate::error::Error::Io)?;
    w.flush().await.map_err(crate::error::Error::Io)?;
    Ok(())
}

// Quiet `serde_json::Value` import warning if we ever add diagnostic
// JSON inspection later.
#[allow(dead_code)]
fn _v(_: Value) {}
