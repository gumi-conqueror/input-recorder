use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Arc, Mutex, atomic::Ordering},
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender, bounded, select, unbounded};
use serde_json::json;

use crate::{
    capture::Capture,
    model::{Device, Event, Update},
    storage::Store,
};

#[derive(Clone, Debug, Default)]
pub(crate) struct Progress {
    pub written: u64,
    pub per_device: BTreeMap<String, u64>,
    pub error: Option<String>,
}

pub(crate) struct Recording {
    capture: Capture,
    writer: JoinHandle<Result<PathBuf>>,
    marker_tx: Sender<(u64, String)>,
    pub preview: Receiver<Event>,
    pub progress: Arc<Mutex<Progress>>,
    pub started: Instant,
}

impl Recording {
    pub(crate) fn start(
        directory: PathBuf,
        devices: Vec<Device>,
        graphics: String,
    ) -> Result<Self> {
        std::fs::create_dir_all(&directory)?;
        let unix_us = SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros();
        let path = directory.join(format!("session-{unix_us}.sqlite"));
        let selected: BTreeSet<_> = devices.iter().map(|device| device.id.clone()).collect();
        anyhow::ensure!(!selected.is_empty(), "select at least one input interface");
        let capture = Capture::start()?;
        let started = capture.started;
        let mut store = Store::create(
            &path,
            &json!({
                "format_version": 1, "application_version": env!("CARGO_PKG_VERSION"),
                "started_unix_us": capture.started_unix_us, "clock": "monotonic_microseconds_since_capture_start",
                "clock_anchor_precision": "adjacent monotonic and wall-clock reads", "graphics": graphics,
                "selected_devices": selected, "backend": "win32_raw_input",
                "upstream_loss_detection": "unavailable",
            }),
            &devices,
        )?;
        let events = capture.events.clone();
        let updates = capture.updates.clone();
        let lost = capture.lost.clone();
        let (preview_tx, preview) = bounded(128);
        let (marker_tx, markers) = unbounded::<(u64, String)>();
        let progress = Arc::new(Mutex::new(Progress::default()));
        let thread_progress = progress.clone();
        let writer = thread::Builder::new().name("recording-writer".into()).spawn(move || {
            let result = (|| -> Result<PathBuf> {
                let mut diagnostics = 0_u64;
                let mut latest_us = 0;
                let mut prior_loss = 0;
                loop {
                    // Metadata and errors are drained before events so new connection snapshots are available.
                    for update in updates.try_iter() {
                        match update {
                            Update::Devices(devices) => for device in devices {
                                if selected.contains(&device.id) { store.device(&device)?; }
                            },
                            Update::Diagnostic(message) => {
                                diagnostics += 1;
                                store.diagnostic(latest_us, &message)?;
                            }
                        }
                    }
                    let current_loss = lost.load(Ordering::Relaxed);
                    if current_loss != prior_loss {
                        store.diagnostic(latest_us, &format!("capture loss increased from {prior_loss} to {current_loss}; counts include unselected inputs"))?;
                        prior_loss = current_loss;
                    }
                    select! {
                        recv(events) -> event => match event {
                            Ok(event) => {
                                latest_us = event.elapsed_us;
                                if selected.contains(&event.device_id) {
                                    store.event(&event)?;
                                    let mut progress = thread_progress.lock().expect("progress lock poisoned");
                                    progress.written += 1;
                                    *progress.per_device.entry(event.device_id.clone()).or_default() += 1;
                                    drop(progress);
                                    let _ = preview_tx.try_send(event);
                                }
                            }
                            Err(_) => break,
                        },
                        recv(markers) -> marker => if let Ok((time, text)) = marker { store.marker(time, &text)?; },
                        default(Duration::from_millis(50)) => {}
                    }
                    store.flush_if_due()?;
                }
                for (time, text) in markers.try_iter() { store.marker(time, &text)?; }
                for update in updates.try_iter() {
                    if let Update::Diagnostic(message) = update {
                        diagnostics += 1;
                        store.diagnostic(latest_us, &message)?;
                    }
                }
                store.finish(lost.load(Ordering::Relaxed), diagnostics)?;
                Ok(path)
            })();
            if let Err(error) = &result {
                thread_progress.lock().expect("progress lock poisoned").error = Some(format!("recording failed: {error:#}"));
            }
            result
        })?;
        Ok(Self {
            capture,
            writer,
            marker_tx,
            preview,
            progress,
            started,
        })
    }

    pub(crate) fn mark(&self, text: String) -> Result<()> {
        anyhow::ensure!(!text.trim().is_empty(), "enter an action label");
        self.marker_tx
            .send((self.started.elapsed().as_micros() as u64, text))?;
        Ok(())
    }

    pub(crate) fn known_loss(&self) -> u64 {
        self.capture.lost.load(Ordering::Relaxed)
    }

    pub(crate) fn finish(mut self) -> Result<PathBuf> {
        self.capture.stop().context("stop capture")?;
        self.writer
            .join()
            .map_err(|_| anyhow::anyhow!("recording writer panicked"))?
    }
}
