//! Daemon reload subscription over postgres LISTEN/NOTIFY.
//!
//! The resident Discord daemon subscribes to the `daemon_reload` channel and
//! re-queries its state whenever any of the tables it depends on (`psyops`,
//! `discord_hooks`, `discord_auth`) changes — driven by the statement-level
//! triggers in `schema.sql`. This keeps all `sqlx` / [`PgListener`] use inside
//! the db crate; the CLI only ever sees the opaque [`ReloadListener`].

use sqlx::postgres::PgListener;

use crate::{Db, Error};

/// A live subscription to the `daemon_reload` channel. Holds a dedicated
/// connection for its lifetime; reconnects transparently if the connection
/// drops.
pub struct ReloadListener(PgListener);

impl Db {
    /// Subscribe to the `daemon_reload` channel. The returned listener holds a
    /// dedicated connection from the pool for as long as it's kept.
    pub async fn reload_listener(&self) -> Result<ReloadListener, Error> {
        let mut listener = PgListener::connect_with(&self.pool).await?;
        listener.listen("daemon_reload").await?;
        Ok(ReloadListener(listener))
    }
}

impl ReloadListener {
    /// Block until the daemon should reload: either a NOTIFY arrived on the
    /// channel, or the connection dropped (the next call reconnects and
    /// re-LISTENs). A reconnect is treated as a reload signal too, so any
    /// notifications missed during the blip are caught up by reloading. Errs
    /// only on an unrecoverable failure.
    pub async fn next_reload(&mut self) -> Result<(), Error> {
        // try_recv: Ok(Some) = a notification, Ok(None) = connection lost and
        // will reconnect on the next call. Either way, reload.
        let _ = self.0.try_recv().await?;
        Ok(())
    }
}
