use std::{path::PathBuf, time::Instant};

use eframe::egui;

pub(crate) struct App {
    started: Instant,
    smoke_test: bool,
    graphics: String,
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
        })
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("Input Recorder");
            ui.label(&self.graphics);
            ui.separator();
            ui.label("Device capture is being connected.");
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
}
