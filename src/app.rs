use std::{
    collections::{BTreeSet, VecDeque},
    path::PathBuf,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::Result;
use eframe::egui;

use crate::{
    devices, export,
    model::{Device, Event, hex},
    permissions,
    recording::Recording,
};

enum LogEntry {
    Input(Event),
    Marker { elapsed_us: u64, text: String },
}

impl LogEntry {
    fn elapsed_us(&self) -> u64 {
        match self {
            Self::Input(event) => event.elapsed_us,
            Self::Marker { elapsed_us, .. } => *elapsed_us,
        }
    }
}

fn append_log(recent: &mut VecDeque<LogEntry>, entry: LogEntry) {
    // Input preview can arrive after a marker with an earlier capture timestamp.
    let index = recent.partition_point(|item| item.elapsed_us() <= entry.elapsed_us());
    recent.insert(index, entry);
    if recent.len() > 100 {
        recent.pop_front();
    }
}

pub(crate) struct App {
    directory: PathBuf,
    started: Instant,
    smoke_test: bool,
    graphics: String,
    devices: Vec<Device>,
    selected: BTreeSet<String>,
    recent: VecDeque<LogEntry>,
    recording: Option<Recording>,
    stopping: Option<JoinHandle<Result<PathBuf>>>,
    exporting: Option<JoinHandle<Result<(PathBuf, u64)>>>,
    last_recording: Option<PathBuf>,
    marker: String,
    status: String,
    closing: bool,
    elevated: bool,
    restarting: Option<JoinHandle<Result<()>>>,
}

impl App {
    pub(crate) fn new(
        context: &eframe::CreationContext<'_>,
        directory: PathBuf,
        smoke_test: bool,
    ) -> Result<Self> {
        let renderer = context
            .wgpu_render_state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("wgpu renderer is unavailable"))?;
        let adapter = renderer.adapter.get_info();
        anyhow::ensure!(adapter.backend == wgpu::Backend::Dx12, "DX12 is required");
        let graphics = format!("Direct3D 12 | {} | {}", adapter.name, adapter.driver);
        let mut app = Self {
            directory,
            started: Instant::now(),
            smoke_test,
            graphics,
            devices: Vec::new(),
            selected: BTreeSet::new(),
            recent: VecDeque::new(),
            recording: None,
            stopping: None,
            exporting: None,
            last_recording: None,
            marker: String::new(),
            status: "Select devices, then start recording.".into(),
            closing: false,
            elevated: permissions::is_elevated()?,
            restarting: None,
        };
        app.refresh()?;
        let arguments: Vec<_> = std::env::args().collect();
        if let Some(index) = arguments
            .iter()
            .position(|argument| argument == "--selection")
        {
            app.selected = arguments
                .get(index + 1)
                .ok_or_else(|| anyhow::anyhow!("missing device selection"))?
                .split(',')
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .collect();
        }
        Ok(app)
    }

    pub(crate) fn notice(&mut self, message: String) {
        self.status = message;
    }

    fn restart_elevated(&mut self) {
        let directory = self.directory.clone();
        let selected = self.selected.clone();
        self.status =
            "Waiting for administrator restart. Cancelling keeps this window open.".into();
        self.restarting = Some(thread::spawn(move || {
            permissions::restart(&directory, Some(&selected))
        }));
    }

    fn refresh(&mut self) -> Result<()> {
        self.devices = devices::enumerate()?;
        self.devices.push(Device::unattributed());
        self.selected = self
            .devices
            .iter()
            .map(|device| device.id.clone())
            .collect();
        Ok(())
    }

    fn start(&mut self) {
        let devices = self
            .devices
            .iter()
            .filter(|device| self.selected.contains(&device.id))
            .cloned()
            .collect();
        match Recording::start(
            self.directory.join("recordings"),
            devices,
            self.graphics.clone(),
        ) {
            Ok(recording) => {
                self.recording = Some(recording);
                self.recent.clear();
                self.status = "Recording selected devices.".into();
            }
            Err(error) => self.status = format!("Cannot start: {error:#}"),
        }
    }

    fn stop(&mut self) {
        if let Some(recording) = self.recording.take() {
            self.status = "Stopping capture and flushing queued events...".into();
            self.stopping = Some(thread::spawn(move || recording.finish()));
        }
    }

    fn export(&mut self) {
        let Some(database) = self.last_recording.clone() else {
            return;
        };
        let Some(output) = rfd::FileDialog::new()
            .set_directory(&self.directory)
            .add_filter("Capture bundle", &["zip"])
            .set_file_name("input-capture.zip")
            .save_file()
        else {
            return;
        };
        let selected = self.selected.clone();
        self.status = "Exporting selected devices...".into();
        self.exporting = Some(thread::spawn(move || {
            let count = export::bundle(&database, &selected, &output)?;
            Ok((output, count))
        }));
    }

    fn open(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_directory(self.directory.join("recordings"))
            .add_filter("Recording", &["sqlite"])
            .pick_file()
        {
            match export::load_devices(&path) {
                Ok(devices) => {
                    self.selected = devices.iter().map(|device| device.id.clone()).collect();
                    self.devices = devices;
                    self.status = format!("Opened {}", path.display());
                    self.last_recording = Some(path);
                    self.recent.clear();
                }
                Err(error) => self.status = format!("Cannot open recording: {error:#}"),
            }
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self
            .restarting
            .as_ref()
            .is_some_and(|job| job.is_finished())
        {
            match self.restarting.take().expect("finished job exists").join() {
                Ok(Ok(())) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Ok(Err(error)) => self.status = format!("Continuing without elevation: {error:#}"),
                Err(_) => self.status = "Administrator restart failed.".into(),
            }
        }
        if let Some(recording) = &self.recording {
            for event in recording.preview.try_iter() {
                append_log(&mut self.recent, LogEntry::Input(event));
            }
            let error = recording
                .progress
                .lock()
                .expect("progress lock poisoned")
                .error
                .clone();
            if let Some(error) = error {
                self.stop();
                self.status = error;
            }
        }
        if self.stopping.as_ref().is_some_and(|job| job.is_finished()) {
            let result = self.stopping.take().expect("finished job exists").join();
            match result {
                Ok(Ok(path)) => {
                    self.status = format!("Saved {}", path.display());
                    match export::load_devices(&path) {
                        Ok(devices) => self.devices = devices,
                        Err(error) => {
                            self.status =
                                format!("Saved recording, but cannot reload devices: {error:#}")
                        }
                    }
                    self.last_recording = Some(path);
                }
                Ok(Err(error)) => {
                    self.status = format!("Recording failed: {error:#}");
                    self.closing = false;
                }
                Err(_) => {
                    self.status = "Recording worker panicked.".into();
                    self.closing = false;
                }
            }
        }
        if self.exporting.as_ref().is_some_and(|job| job.is_finished()) {
            match self.exporting.take().expect("finished job exists").join() {
                Ok(Ok((path, count))) => {
                    self.status = format!("Exported {count} events to {}", path.display())
                }
                Ok(Err(error)) => {
                    self.status = format!("Export failed: {error:#}");
                    self.closing = false;
                }
                Err(_) => {
                    self.status = "Export worker panicked.".into();
                    self.closing = false;
                }
            }
        }
        if ctx.input(|input| input.viewport().close_requested())
            && (self.recording.is_some() || self.stopping.is_some() || self.exporting.is_some())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.closing = true;
            self.stop();
        }
        if self.closing
            && self.recording.is_none()
            && self.stopping.is_none()
            && self.exporting.is_none()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let idle = self.recording.is_none()
            && self.stopping.is_none()
            && self.exporting.is_none()
            && self.restarting.is_none();
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("Input Recorder");
            ui.label(&self.graphics);
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Data folder: {}", self.directory.display()));
                if ui.add_enabled(idle, egui::Button::new("Change folder")).clicked()
                    && let Some(directory) = rfd::FileDialog::new().set_directory(&self.directory).pick_folder() {
                        self.directory = directory;
                }
            });
            ui.horizontal(|ui| {
                ui.label(if self.elevated { "Administrator" } else { "Standard permissions" });
                if ui.add_enabled(idle && !self.elevated, egui::Button::new("Restart as administrator"))
                    .on_hover_text("Retry device access with administrator permissions. Some drivers remain inaccessible.").clicked() {
                    self.restart_elevated();
                }
            });
            ui.horizontal(|ui| {
                if ui.add_enabled(idle, egui::Button::new("Refresh devices")).clicked()
                    && let Err(error) = self.refresh() { self.status = format!("Refresh failed: {error:#}"); }
                if ui.add_enabled(idle && !self.selected.is_empty(), egui::Button::new("Start recording")).clicked() { self.start(); }
                if ui.add_enabled(self.recording.is_some(), egui::Button::new("Stop")).clicked() { self.stop(); }
                if ui.add_enabled(idle && self.last_recording.is_some(), egui::Button::new("Export selected")).clicked() { self.export(); }
                if ui.add_enabled(idle, egui::Button::new("Open recording")).clicked() { self.open(); }
            });
            ui.label(&self.status);
            if let Some(recording) = &self.recording {
                let progress = recording.progress.lock().expect("progress lock poisoned");
                ui.label(format!("Recording: {:.1} s | {} written | {} known lost", recording.started.elapsed().as_secs_f64(), progress.written, recording.known_loss()));
            }
            ui.horizontal(|ui| {
                ui.add_enabled(self.recording.is_some(), egui::TextEdit::singleline(&mut self.marker).hint_text("Action label, e.g. Press button A"));
                if ui.add_enabled(self.recording.is_some() && !self.marker.trim().is_empty(), egui::Button::new("Add marker")).clicked()
                    && let Some(recording) = &self.recording {
                        match recording.mark(self.marker.clone()) {
                            Ok(elapsed_us) => {
                                self.status = format!("Marker added at {:.3} s: {}", elapsed_us as f64 / 1_000_000.0, self.marker);
                                append_log(&mut self.recent, LogEntry::Marker { elapsed_us, text: std::mem::take(&mut self.marker) });
                            }
                            Err(error) => self.status = format!("Marker failed: {error:#}"),
                        }
                    }
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.add_enabled(idle, egui::Button::new("Select all")).clicked() { self.selected.extend(self.devices.iter().map(|device| device.id.clone())); }
                if ui.add_enabled(idle, egui::Button::new("Clear selection")).clicked() { self.selected.clear(); }
                ui.label("Selection controls recording and export. Refresh before recording a new device.");
            });
            egui::ScrollArea::vertical().id_salt("devices").max_height(220.0).show(ui, |ui| {
                for device in &self.devices {
                    ui.horizontal(|ui| {
                        let mut selected = self.selected.contains(&device.id);
                        if ui.add_enabled(idle, egui::Checkbox::new(&mut selected, format!("{} [{} {:04x}:{:04x}] {}", device.name, device.kind, device.usage_page, device.usage, &device.id[..device.id.len().min(8)]))).changed() {
                            if selected { self.selected.insert(device.id.clone()); } else { self.selected.remove(&device.id); }
                        }
                        if let Some(recording) = &self.recording {
                            let progress = recording.progress.lock().expect("progress lock poisoned");
                            ui.label(format!("{} events", progress.per_device.get(&device.id).unwrap_or(&0)));
                        }
                        ui.push_id(&device.id, |ui| ui.collapsing("Details", |ui| { ui.monospace(serde_json::to_string_pretty(device).unwrap_or_default()); }));
                    });
                }
            });
            ui.separator();
            ui.label("Recent input and markers (preview may skip input; known recording loss is counted above)");
            egui::ScrollArea::vertical().id_salt("events").stick_to_bottom(true).show(ui, |ui| {
                for entry in &self.recent {
                    match entry {
                        LogEntry::Input(event) if self.selected.contains(&event.device_id) => {
                            ui.monospace(format!("{:>10} us  {}  {}  {}", event.elapsed_us, &event.device_id[..event.device_id.len().min(8)], event.kind, hex(&event.raw[..event.raw.len().min(64)])));
                        }
                        LogEntry::Marker { elapsed_us, text } => {
                            ui.label(egui::RichText::new(format!("{elapsed_us:>10} us  MARKER  {text}")).monospace().strong());
                        }
                        LogEntry::Input(_) => {}
                    }
                }
            });
        });
        if self.smoke_test && self.started.elapsed().as_secs() >= 2 {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
