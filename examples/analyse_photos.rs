//! End-to-end local import smoke check, using the same worker as the desktop app.
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use wardrobe::{
    db::Database,
    model::{Garment, PhotoBackend},
    vision::{VisionEvent, VisionService},
};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    anyhow::ensure!(
        args.next().as_deref() == Some("--data-dir"),
        "Usage: cargo run --example analyse_photos -- --data-dir <test folder> <photo> [photo ...]"
    );
    let root = PathBuf::from(
        args.next()
            .ok_or_else(|| anyhow::anyhow!("Choose a test data folder"))?,
    );
    let mut backend = PhotoBackend::Auto;
    let mut photos = Vec::new();
    while let Some(arg) = args.next() {
        if arg == "--backend" {
            backend = match args.next().as_deref() {
                Some("auto") => PhotoBackend::Auto,
                Some("gpu") => PhotoBackend::Gpu,
                Some("cpu") => PhotoBackend::Cpu,
                _ => anyhow::bail!("Choose --backend auto, gpu or cpu"),
            };
        } else {
            photos.push(PathBuf::from(arg));
        }
    }
    anyhow::ensure!(!photos.is_empty(), "Choose at least one photo");
    let mut db = Database::open(&root)?;
    let vision = VisionService::new(&root);
    for path in photos {
        let original = db.import_photo(&path)?;
        let before = Garment {
            photo: Some(original.clone()),
            original_photo: Some(original.clone()),
            ..Garment::default()
        };
        let output_name = format!("{}.png", uuid::Uuid::new_v4());
        let output = db.photo_path(&output_name)?;
        let job =
            vision.analyse_configured(db.photo_path(&original)?, output.clone(), None, backend);
        let deadline = Instant::now() + Duration::from_secs(15 * 60);
        let mut complete = false;
        while !complete && Instant::now() < deadline {
            for event in vision.poll() {
                match event {
                    VisionEvent::Progress { id, message } if id == job.id => println!("{message}"),
                    VisionEvent::Analysed { id, result, .. } if id == job.id => {
                        let analysis = (*result).map_err(anyhow::Error::msg)?;
                        println!(
                            "Recognition: {} · {}; mask: {}; offloaded layers: {}",
                            analysis.recognition_backend,
                            analysis.device,
                            analysis.segmentation_backend,
                            analysis.gpu_layers
                        );
                        let mut garment = before.clone();
                        analysis.apply(&mut garment, &before, &output_name)?;
                        garment.validate()?;
                        let pixels = image::open(&output)?.to_rgba8();
                        anyhow::ensure!(
                            pixels.pixels().any(|p| p[3] == 0)
                                && pixels.pixels().any(|p| p[3] > 240),
                            "Cutout lacks usable transparency or foreground"
                        );
                        anyhow::ensure!(
                            std::fs::read(&path)? == std::fs::read(db.photo_path(&original)?)?,
                            "Original photo changed"
                        );
                        db.save_garment(&garment, 2.5)?;
                        std::fs::write(
                            root.join(format!("{}.analysis.json", garment.id)),
                            serde_json::to_vec_pretty(&analysis)?,
                        )?;
                        println!(
                            "Saved: {} · {} · {} · warmth {} · formality {} · {}",
                            garment.name,
                            garment.category,
                            garment.color,
                            garment.warmth,
                            garment.formality,
                            garment.seasons
                        );
                        println!(
                            "{}\nDetails: {}\nCutout: {}\nElapsed: {:.1}s",
                            garment.description,
                            garment.details.join("; "),
                            output.display(),
                            analysis.elapsed_seconds
                        );
                        complete = true;
                    }
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        anyhow::ensure!(complete, "Photo analysis timed out");
    }
    println!(
        "Verified {} stored items in {}",
        db.garments()?.len(),
        root.display()
    );
    Ok(())
}
