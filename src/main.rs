#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;

use std::path::PathBuf;

fn main() {
    if let Err(error) = run() {
        let message = format!("{error:#}");
        if let Ok(directory) = data_directory() {
            let _ = std::fs::write(directory.join("startup-error.txt"), &message);
        }
        eprintln!("{message}");
        rfd::MessageDialog::new()
            .set_title("Input Recorder")
            .set_level(rfd::MessageLevel::Error)
            .set_description(&message)
            .show();
        std::process::exit(1);
    }
}

fn data_directory() -> anyhow::Result<PathBuf> {
    let arguments: Vec<_> = std::env::args_os().collect();
    if let Some(index) = arguments
        .iter()
        .position(|argument| argument == "--data-dir")
    {
        let directory = PathBuf::from(
            arguments
                .get(index + 1)
                .ok_or_else(|| anyhow::anyhow!("--data-dir requires a directory"))?,
        );
        std::fs::create_dir_all(&directory)?;
        return Ok(directory);
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("LOCALAPPDATA is unavailable"))?;
    let directory = base.join("InputRecorder");
    std::fs::create_dir_all(&directory)?;
    Ok(directory)
}

fn run() -> anyhow::Result<()> {
    let directory = data_directory()?;
    let smoke_test = std::env::args().any(|argument| argument == "--smoke-test");
    let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    // Override the environment-derived defaults before any adapter is created.
    setup.instance_descriptor.backends = wgpu::Backends::DX12;
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_inner_size([1000.0, 680.0]),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            wgpu_setup: setup.into(),
            ..Default::default()
        },
        ..Default::default()
    };
    eframe::run_native(
        "Input Recorder",
        options,
        Box::new(move |context| Ok(Box::new(app::App::new(context, directory, smoke_test)?))),
    )
    .map_err(|error| anyhow::anyhow!("Direct3D 12 startup failed: {error}"))
}
