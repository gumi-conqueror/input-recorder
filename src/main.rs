#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod arguments;
mod capture;
mod devices;
mod export;
mod model;
mod packet;
mod permissions;
mod recording;
mod storage;

use std::path::PathBuf;

fn main() {
    if let Err(error) = run() {
        let message = format!("{error:#}");
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
    if let Some(directory) = arguments::value(arguments::DATA_DIR)? {
        return Ok(std::path::absolute(PathBuf::from(directory))?);
    }
    let executable = std::env::current_exe()?;
    let parent = executable
        .parent()
        .ok_or_else(|| anyhow::anyhow!("executable directory is unavailable"))?;
    Ok(parent.join("data"))
}

fn run() -> anyhow::Result<()> {
    permissions::check_session()?;
    let directory = data_directory()?;
    let mut startup_notice = None;
    if arguments::flag(arguments::ELEVATE) && !permissions::is_elevated()? {
        match permissions::restart(&directory, None) {
            Ok(()) => return Ok(()),
            Err(error) => {
                startup_notice = Some(format!(
                    "Elevation was not completed: {error:#}. Continuing without it."
                ))
            }
        }
    }
    if arguments::flag(arguments::LIST_DEVICES) {
        let devices = devices::enumerate()?;
        std::fs::create_dir_all(&directory)?;
        std::fs::write(
            directory.join("devices.json"),
            serde_json::to_vec_pretty(&devices)?,
        )?;
        println!("Found {} input interfaces", devices.len());
        return Ok(());
    }
    if let Some(seconds) = arguments::text(arguments::RECORD_SECONDS)? {
        let seconds: u64 = seconds.parse()?;
        anyhow::ensure!(
            (1..=3600).contains(&seconds),
            "duration must be between 1 and 3600 seconds"
        );
        let mut devices = devices::enumerate()?;
        devices.push(model::Device::unattributed());
        let recording = recording::Recording::start(
            recording::directory(&directory),
            devices,
            "headless diagnostic".into(),
        )?;
        println!("Recording for {seconds} seconds");
        std::thread::sleep(std::time::Duration::from_secs(seconds));
        let database = recording.finish()?;
        let output = database.with_extension("zip");
        let count = export::bundle(&database, &export::recorded_ids(&database)?, &output)?;
        println!("Saved {count} events to {}", output.display());
        return Ok(());
    }
    let smoke_test = arguments::flag(arguments::SMOKE_TEST);
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
        Box::new(move |context| {
            let mut app = app::App::new(context, directory, smoke_test)?;
            if let Some(notice) = startup_notice {
                app.notice(notice);
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|error| anyhow::anyhow!("Direct3D 12 startup failed: {error}"))
}
