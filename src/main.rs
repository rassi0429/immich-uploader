#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ansi;
mod app;
mod autostart;
mod cli;
mod config;
mod process;
mod single_instance;
mod tray;

use anyhow::Result;
use eframe::egui;
use std::sync::Arc;
use tokio::runtime::Runtime;
use tracing_subscriber::FmtSubscriber;

fn install_japanese_fonts(ctx: &egui::Context) {
    const CANDIDATES: &[&str] = &[
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\yugothm.ttc",
        r"C:\Windows\Fonts\YuGothR.ttc",
        r"C:\Windows\Fonts\meiryo.ttc",
        r"C:\Windows\Fonts\msgothic.ttc",
    ];
    let Some(bytes) = CANDIDATES.iter().find_map(|p| std::fs::read(p).ok()) else {
        eprintln!("[warn] 日本語フォントが見つかりません。豆腐表示になります");
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "japanese".to_owned(),
        Arc::new(egui::FontData::from_owned(bytes)),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "japanese".to_owned());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push("japanese".to_owned());
    ctx.set_fonts(fonts);
}

fn main() -> Result<()> {
    let _ = FmtSubscriber::builder()
        .with_max_level(tracing::Level::INFO)
        .with_writer(std::io::stderr)
        .try_init();

    let _instance_guard = match single_instance::SingleInstanceGuard::try_acquire() {
        Some(g) => g,
        None => {
            eprintln!("Immich Auto Uploader は既に起動しています");
            return Ok(());
        }
    };

    let load = config::Config::load_or_default();
    let cfg = load.config;
    let warning = load.warning;

    if let Err(e) = autostart::sync(cfg.start_on_boot) {
        eprintln!("[warn] 自動起動設定の同期に失敗: {e}");
    }

    let runtime = Runtime::new()?;
    let rt_handle = runtime.handle().clone();

    let start_minimized = cfg.start_minimized_to_tray;

    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([720.0, 640.0])
        .with_min_inner_size([520.0, 420.0])
        .with_title("Immich Auto Uploader")
        .with_visible(!start_minimized);

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let result = eframe::run_native(
        "Immich Auto Uploader",
        options,
        Box::new(move |cc| {
            install_japanese_fonts(&cc.egui_ctx);
            let tray = match tray::build_tray() {
                Ok(t) => Some(t),
                Err(e) => {
                    eprintln!("[warn] タスクトレイ初期化失敗: {e}");
                    None
                }
            };
            Ok(Box::new(app::App::new(cfg, warning, rt_handle, tray)))
        }),
    );

    drop(runtime);

    match result {
        Ok(()) => Ok(()),
        Err(e) => Err(anyhow::anyhow!("eframe 実行失敗: {e}")),
    }
}
