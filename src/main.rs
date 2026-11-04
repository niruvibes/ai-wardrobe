#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ui;

fn main() {
    if let Err(error) = launch() {
        eprintln!("Wardrobe could not start: {error:#}");
        rfd::MessageDialog::new()
            .set_title("Wardrobe could not start")
            .set_description(format!("{error:#}"))
            .set_level(rfd::MessageLevel::Error)
            .show();
        std::process::exit(1);
    }
}

fn launch() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut data_dir = std::env::var_os("WARDROBE_DATA_DIR").map(std::path::PathBuf::from);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data-dir" => {
                data_dir = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--data-dir requires a folder"))?
                        .into(),
                )
            }
            "--help" | "-h" => {
                println!("Wardrobe\n  --data-dir <folder>  Use a separate local wardrobe database");
                return Ok(());
            }
            _ => anyhow::bail!("Unknown option: {arg}"),
        }
    }
    let data_dir = data_dir.unwrap_or_else(|| {
        std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("XDG_DATA_HOME"))
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("Wardrobe")
    });
    let app = ui::WardrobeApp::open(data_dir)?;
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1240.0, 840.0])
            .with_min_inner_size([960.0, 680.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "Wardrobe",
        options,
        Box::new(|cc| {
            ui::configure(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
