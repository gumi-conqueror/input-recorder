use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::PathBuf,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use crate::{
    capture::Capture,
    model::{Device, Event, Update, hex},
};
use eframe::egui;

pub(crate) struct App {
    started: Instant,
    smoke_test: bool,
    graphics: String,
    capture: Capture,
    devices: Vec<Device>,
    selected: BTreeSet<String>,
    recent: VecDeque<Event>,
    counts: BTreeMap<String, u64>,
    error: String,
}

impl App {
    pub(crate) fn new(
        context: &eframe::CreationContext<'_>,
        directory: PathBuf,
        smoke_test: bool,
    ) -> anyhow::Result<Self> {
        let renderer = context
            .wgpu_render_state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("wgpu renderer is unavailable"))?;
        let adapter = renderer.adapter.get_info();
        anyhow::ensure!(adapter.backend == wgpu::Backend::Dx12, "DX12 is required");
        let graphics = format!("Direct3D 12 | {} | {}", adapter.name, adapter.driver);
        std::fs::write(directory.join("graphics.txt"), &graphics)?;
        Ok(Self {
            started: Instant::now(),
            smoke_test,
            graphics,
            capture: Capture::start()?,
            devices: Vec::new(),
            selected: BTreeSet::new(),
            recent: VecDeque::new(),
            counts: BTreeMap::new(),
            error: String::new(),
        })
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        for update in self.capture.updates.try_iter() {
            match update {
                Update::Devices(devices) => {
                    if self.devices.is_empty() {
                        self.selected
                            .extend(devices.iter().map(|device| device.id.clone()));
                    }
                    self.devices = devices;
                }
                Update::Diagnostic(error) => self.error = error,
            }
        }
        for event in self.capture.events.try_iter().take(16_384) {
            *self.counts.entry(event.device_id.clone()).or_default() += 1;
            self.recent.push_back(event);
            if self.recent.len() > 100 {
                self.recent.pop_front();
            }
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("Input Recorder");
            ui.label(&self.graphics);
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Select all").clicked() {
                    self.selected
                        .extend(self.devices.iter().map(|device| device.id.clone()));
                }
                if ui.button("Clear selection").clicked() {
                    self.selected.clear();
                }
                ui.label(format!(
                    "{} interfaces | {} dropped",
                    self.devices.len(),
                    self.capture.lost.load(Ordering::Relaxed)
                ));
            });
            egui::ScrollArea::vertical()
                .id_salt("devices")
                .max_height(240.0)
                .show(ui, |ui| {
                    for device in &self.devices {
                        ui.horizontal(|ui| {
                            let mut selected = self.selected.contains(&device.id);
                            if ui
                                .checkbox(
                                    &mut selected,
                                    format!(
                                        "{} [{} {:04x}:{:04x}]",
                                        device.name, device.kind, device.usage_page, device.usage
                                    ),
                                )
                                .changed()
                            {
                                if selected {
                                    self.selected.insert(device.id.clone());
                                } else {
                                    self.selected.remove(&device.id);
                                }
                            }
                            ui.label(format!(
                                "{} events",
                                self.counts.get(&device.id).unwrap_or(&0)
                            ));
                            ui.collapsing("Details", |ui| {
                                ui.monospace(
                                    serde_json::to_string_pretty(device).unwrap_or_default(),
                                );
                            });
                        });
                    }
                });
            if !self.error.is_empty() {
                ui.label(&self.error);
            }
            ui.separator();
            ui.label("Recent input (preview only)");
            egui::ScrollArea::vertical()
                .id_salt("events")
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for event in &self.recent {
                        if self.selected.contains(&event.device_id) {
                            ui.monospace(format!(
                                "{:>10} us  {}  {}  {}",
                                event.elapsed_us,
                                &event.device_id[..event.device_id.len().min(8)],
                                event.kind,
                                hex(&event.raw[..event.raw.len().min(64)])
                            ));
                        }
                    }
                });
        });
        if self.smoke_test {
            if self.started.elapsed().as_secs() >= 2 {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
    }

    fn on_exit(&mut self) {
        if let Err(error) = self.capture.stop() {
            eprintln!("{error:#}");
        }
    }
}
