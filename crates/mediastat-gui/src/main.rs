#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod flamegraph;
mod selection;
mod shell_actions;
mod sunburst;
mod treemap;
mod views;

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    // Sync active language from saved settings before creating the native OS window
    let boot_settings = mediastat_core::AppSettings::load_or_default();
    boot_settings.sync_active_language();

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(app::main_window_title())
            .with_inner_size([1460.0, 900.0])
            .with_min_inner_size([1000.0, 640.0]),
        ..Default::default()
    };

    eframe::run_native(
        "MediaStat",
        native_options,
        Box::new(|cc| Ok(Box::new(app::MediaStatApp::new(cc)))),
    )
}
