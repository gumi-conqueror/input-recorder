use std::{
    collections::BTreeSet,
    fs::File,
    io::{BufReader, BufWriter, Read, Seek, Write},
    path::Path,
};

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::{NamedTempFile, TempDir};
use zip::{ZipWriter, write::SimpleFileOptions};

use crate::model::hex;

pub(crate) fn bundle(database: &Path, selected: &BTreeSet<String>, output: &Path) -> Result<u64> {
    ensure!(
        !selected.is_empty(),
        "select at least one recorded device to export"
    );
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let connection = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.execute_batch("BEGIN;")?;
    let scratch = TempDir::new_in(parent)?;
    let mut temporary = NamedTempFile::new_in(parent)?;
    let mut archive = ZipWriter::new(temporary.as_file_mut());
    let mut checksums = String::new();
    let mut devices = Vec::new();
    let mut exported = Vec::new();
    let mut total = 0_u64;
    for id in selected {
        ensure!(
            id.len() <= 128
                && !id.is_empty()
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
            "invalid device ID"
        );
        let snapshot: Option<String> = connection
            .query_row("SELECT json FROM devices WHERE id=?1", [id], |row| {
                row.get(0)
            })
            .optional()?;
        let count: i64 = connection.query_row(
            "SELECT count(*) FROM events WHERE device_id=?1",
            [id],
            |row| row.get(0),
        )?;
        if snapshot.is_none() && count == 0 {
            continue;
        }
        let device: Value = match snapshot {
            Some(snapshot) => serde_json::from_str(&snapshot)?,
            None => json!({"id": id, "name": "Unattributed input", "capabilities": null}),
        };
        add_bytes(
            &mut archive,
            &mut checksums,
            &format!("capabilities/{id}.json"),
            &serde_json::to_vec_pretty(&device["capabilities"])?,
        )?;
        devices.push(device);
        exported.push(id.clone());
        let event_path = scratch.path().join("events.jsonl");
        let raw_path = scratch.path().join("reports.bin");
        {
            let mut events = BufWriter::new(File::create(&event_path)?);
            let mut reports = BufWriter::new(File::create(&raw_path)?);
            let mut offset = 0_u64;
            let mut statement = connection.prepare("SELECT sequence, elapsed_us, connection, kind, fields, raw FROM events WHERE device_id=?1 ORDER BY sequence")?;
            let mut rows = statement.query([id])?;
            while let Some(row) = rows.next()? {
                let raw: Vec<u8> = row.get(5)?;
                let fields: String = row.get(4)?;
                let event = json!({
                    "sequence": row.get::<_, i64>(0)?, "elapsed_us": row.get::<_, i64>(1)?,
                    "device_id": id, "connection": row.get::<_, i64>(2)?, "kind": row.get::<_, String>(3)?,
                    "fields": serde_json::from_str::<Value>(&fields)?,
                    "raw_offset": offset, "raw_length": raw.len(),
                });
                serde_json::to_writer(&mut events, &event)?;
                events.write_all(b"\n")?;
                reports.write_all(&raw)?;
                offset += raw.len() as u64;
                total += 1;
            }
            events.flush()?;
            reports.flush()?;
        }
        add_reader(
            &mut archive,
            &mut checksums,
            &format!("events/{id}.jsonl"),
            BufReader::new(File::open(&event_path)?),
        )?;
        add_reader(
            &mut archive,
            &mut checksums,
            &format!("reports/{id}.bin"),
            BufReader::new(File::open(&raw_path)?),
        )?;
    }
    ensure!(
        !exported.is_empty(),
        "none of the selected devices exist in this recording"
    );
    add_bytes(
        &mut archive,
        &mut checksums,
        "devices.json",
        &serde_json::to_vec_pretty(&devices)?,
    )?;
    for table in ["markers", "diagnostics"] {
        let spool = scratch.path().join("annotations.jsonl");
        {
            let mut writer = BufWriter::new(File::create(&spool)?);
            let mut statement =
                connection.prepare(&format!("SELECT elapsed_us, text FROM {table} ORDER BY id"))?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                serde_json::to_writer(
                    &mut writer,
                    &json!({"elapsed_us": row.get::<_, i64>(0)?, "text": row.get::<_, String>(1)?}),
                )?;
                writer.write_all(b"\n")?;
            }
            writer.flush()?;
        }
        add_reader(
            &mut archive,
            &mut checksums,
            &format!("{table}.jsonl"),
            BufReader::new(File::open(spool)?),
        )?;
    }
    let mut metadata = serde_json::Map::new();
    let mut statement = connection.prepare("SELECT key, value FROM metadata")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        metadata.insert(
            row.get(0)?,
            serde_json::from_str(&row.get::<_, String>(1)?)?,
        );
    }
    if let Some(session) = metadata.get_mut("session") {
        session["selected_devices"] = json!(exported);
    }
    let manifest = json!({"format_version": 1, "recording": metadata, "exported_devices": exported,
        "event_count": total, "raw_references": "byte offsets into reports/<device_id>.bin",
        "diagnostic_scope": "capture session; known loss may include unselected devices",
        "upstream_completeness": "not measurable by this recorder"});
    add_bytes(
        &mut archive,
        &mut checksums,
        "manifest.json",
        &serde_json::to_vec_pretty(&manifest)?,
    )?;
    archive.start_file("checksums.sha256", options())?;
    archive.write_all(checksums.as_bytes())?;
    archive.finish()?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(output)
        .map_err(|error| error.error)
        .context("save export bundle")?;
    Ok(total)
}

pub(crate) fn recorded_ids(database: &Path) -> Result<BTreeSet<String>> {
    let connection = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut statement =
        connection.prepare("SELECT id FROM devices UNION SELECT device_id FROM events")?;
    Ok(statement
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?)
}

pub(crate) fn load_devices(database: &Path) -> Result<Vec<crate::model::Device>> {
    let connection = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut statement = connection.prepare("SELECT json FROM devices ORDER BY id")?;
    let snapshots = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    snapshots
        .iter()
        .map(|snapshot| serde_json::from_str(snapshot).map_err(Into::into))
        .collect()
}

fn options() -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(true)
}

fn add_bytes<W: Write + Seek>(
    archive: &mut ZipWriter<W>,
    checksums: &mut String,
    name: &str,
    bytes: &[u8],
) -> Result<()> {
    add_reader(archive, checksums, name, bytes)
}

fn add_reader<W: Write + Seek>(
    archive: &mut ZipWriter<W>,
    checksums: &mut String,
    name: &str,
    mut reader: impl Read,
) -> Result<()> {
    archive.start_file(name, options())?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
        archive.write_all(&buffer[..count])?;
    }
    use std::fmt::Write as _;
    writeln!(checksums, "{}  {name}", hex(&digest.finalize()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{Device, Event},
        storage::Store,
    };

    #[test]
    fn exports_only_selected_devices_and_preserves_raw_bytes() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("recording.sqlite");
        let device = |id: &str| Device {
            id: id.into(),
            path: id.into(),
            name: id.into(),
            kind: "hid".into(),
            usage_page: 0xff00,
            usage: 1,
            properties: json!({}),
            capabilities: json!({"input_report_bytes": 3}),
            errors: Vec::new(),
        };
        let mut store = Store::create(
            &database,
            &json!({"selected_devices": ["a", "b"]}),
            &[device("a"), device("b")],
        )?;
        for (sequence, id, raw) in [
            (1, "a", vec![1, 0, 255]),
            (2, "b", vec![99, 99, 99]),
            (3, "a", vec![2, 128, 7]),
        ] {
            store.event(&Event {
                sequence,
                elapsed_us: sequence * 10,
                device_id: id.into(),
                connection: 1,
                kind: "hid".into(),
                fields: json!({}),
                raw,
            })?;
        }
        store.marker(15, "Press A")?;
        store.finish(2, 0)?;
        let output = directory.path().join("export.zip");
        assert_eq!(
            bundle(&database, &BTreeSet::from(["a".into()]), &output)?,
            2
        );
        let mut archive = zip::ZipArchive::new(File::open(output)?)?;
        assert!(!archive.file_names().any(|name| name.contains("/b.")));
        let mut raw = Vec::new();
        archive.by_name("reports/a.bin")?.read_to_end(&mut raw)?;
        assert_eq!(raw, [1, 0, 255, 2, 128, 7]);
        let mut events = String::new();
        archive
            .by_name("events/a.jsonl")?
            .read_to_string(&mut events)?;
        let events: Vec<Value> = events
            .lines()
            .map(serde_json::from_str)
            .collect::<serde_json::Result<_>>()?;
        assert_eq!(events[1]["raw_offset"], 3);
        assert_eq!(events[1]["raw_length"], 3);
        let manifest: Value = serde_json::from_reader(archive.by_name("manifest.json")?)?;
        assert_eq!(manifest["recording"]["status"], "incomplete");
        assert_eq!(
            manifest["recording"]["session"]["selected_devices"],
            json!(["a"])
        );
        let mut checksums = String::new();
        archive
            .by_name("checksums.sha256")?
            .read_to_string(&mut checksums)?;
        for line in checksums.lines() {
            let (expected, name) = line.split_once("  ").expect("checksum separator");
            let mut bytes = Vec::new();
            archive.by_name(name)?.read_to_end(&mut bytes)?;
            assert_eq!(expected, hex(&Sha256::digest(bytes)), "checksum for {name}");
        }
        Ok(())
    }

    #[test]
    fn unfinished_recording_stays_marked_and_export_failure_keeps_destination() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("unfinished.sqlite");
        let store = Store::create(&database, &json!({}), &[])?;
        drop(store);
        let connection = Connection::open(&database)?;
        let status: String =
            connection.query_row("SELECT value FROM metadata WHERE key='status'", [], |row| {
                row.get(0)
            })?;
        assert_eq!(status, "\"recording\"");
        let output = directory.path().join("existing.zip");
        std::fs::write(&output, b"keep this file")?;
        assert!(bundle(&database, &BTreeSet::from(["missing".into()]), &output).is_err());
        assert_eq!(std::fs::read(output)?, b"keep this file");
        Ok(())
    }
}
