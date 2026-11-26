use std::{fs::OpenOptions, path::Path, time::Instant};

use anyhow::Result;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

use crate::model::{Device, Event};

/// Commit after this many events, or after `COMMIT_INTERVAL`, whichever comes first.
/// These bound how much recorded input a crash can lose.
const COMMIT_EVENTS: usize = 512;
const COMMIT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

pub(crate) struct Store {
    connection: Connection,
    last_commit: Instant,
    pending: usize,
}

impl Store {
    pub(crate) fn create(path: &Path, metadata: &Value, devices: &[Device]) -> Result<Self> {
        OpenOptions::new().write(true).create_new(true).open(path)?;
        let connection = Connection::open(path)?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             CREATE TABLE metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE devices(id TEXT PRIMARY KEY, json TEXT NOT NULL);
             CREATE TABLE events(sequence INTEGER PRIMARY KEY, elapsed_us INTEGER NOT NULL,
                device_id TEXT NOT NULL, connection INTEGER NOT NULL, kind TEXT NOT NULL,
                fields TEXT NOT NULL, raw BLOB NOT NULL);
             CREATE INDEX events_device_time ON events(device_id, elapsed_us);
             CREATE TABLE markers(id INTEGER PRIMARY KEY, elapsed_us INTEGER NOT NULL, text TEXT NOT NULL);
             CREATE TABLE diagnostics(id INTEGER PRIMARY KEY, elapsed_us INTEGER NOT NULL, text TEXT NOT NULL);
             BEGIN IMMEDIATE;",
        )?;
        let mut store = Self {
            connection,
            last_commit: Instant::now(),
            pending: 0,
        };
        store.metadata("session", metadata)?;
        store.metadata("status", &json!("recording"))?;
        for device in devices {
            store.device(device)?;
        }
        store.flush()?;
        Ok(store)
    }

    pub(crate) fn metadata(&self, key: &str, value: &Value) -> Result<()> {
        self.connection.execute(
            "INSERT OR REPLACE INTO metadata VALUES (?1, ?2)",
            params![key, value.to_string()],
        )?;
        Ok(())
    }

    pub(crate) fn device(&self, device: &Device) -> Result<()> {
        self.connection.execute(
            "INSERT OR REPLACE INTO devices VALUES (?1, ?2)",
            params![device.id, serde_json::to_string(device)?],
        )?;
        Ok(())
    }

    pub(crate) fn event(&mut self, event: &Event) -> Result<()> {
        self.connection.execute(
            "INSERT INTO events VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                i64::try_from(event.sequence)?,
                i64::try_from(event.elapsed_us)?,
                event.device_id,
                i64::try_from(event.connection)?,
                event.kind,
                event.fields.to_string(),
                event.raw
            ],
        )?;
        self.pending += 1;
        self.flush_if_due()
    }

    pub(crate) fn marker(&self, elapsed_us: u64, text: &str) -> Result<()> {
        self.connection.execute(
            "INSERT INTO markers(elapsed_us, text) VALUES (?1, ?2)",
            params![i64::try_from(elapsed_us)?, text],
        )?;
        Ok(())
    }

    pub(crate) fn diagnostic(&self, elapsed_us: u64, text: &str) -> Result<()> {
        self.connection.execute(
            "INSERT INTO diagnostics(elapsed_us, text) VALUES (?1, ?2)",
            params![i64::try_from(elapsed_us)?, text],
        )?;
        Ok(())
    }

    pub(crate) fn flush_if_due(&mut self) -> Result<()> {
        if self.pending >= COMMIT_EVENTS || self.last_commit.elapsed() >= COMMIT_INTERVAL {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        self.connection.execute_batch("COMMIT; BEGIN IMMEDIATE;")?;
        self.pending = 0;
        self.last_commit = Instant::now();
        Ok(())
    }

    pub(crate) fn finish(self, known_loss: u64, diagnostics: u64) -> Result<()> {
        self.metadata("known_loss", &json!(known_loss))?;
        self.metadata("diagnostic_count", &json!(diagnostics))?;
        self.metadata(
            "status",
            &json!(if known_loss == 0 {
                "stopped"
            } else {
                "incomplete"
            }),
        )?;
        self.connection
            .execute_batch("COMMIT; PRAGMA wal_checkpoint(TRUNCATE);")?;
        self.connection.close().map_err(|(_, error)| error)?;
        Ok(())
    }
}
