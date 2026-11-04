use std::collections::{HashMap, HashSet};
use wardrobe::{db::Database, engine::Ranker, model::*, vision::Analysis};

fn analysis() -> Analysis {
    serde_json::from_str(include_str!("fixtures/photo-analysis.json")).unwrap()
}
fn original() -> Garment {
    Garment {
        name: "IMG_001".into(),
        photo: Some("original.png".into()),
        ..Garment::default()
    }
}

#[test]
fn populates_requested_fields_and_preserves_nonvisual_facts_and_original() {
    let before = Garment {
        rain: RainProtection::Waterproof,
        favourite: true,
        size: "L".into(),
        walking_comfort: 4.0,
        ..original()
    };
    let mut item = before.clone();
    analysis().apply(&mut item, &before, "cutout.png").unwrap();
    assert_eq!(item.id, before.id);
    assert_eq!(item.name, "Cream embroidered fleece pullover");
    assert_eq!(item.category, Category::Outerwear);
    assert_eq!(item.color, "cream, burgundy");
    assert_eq!(item.warmth, 3.5);
    assert_eq!(item.formality, 1.5);
    assert_eq!(item.seasons, "autumn, winter");
    assert!(item.description.contains("embroidery"));
    assert!(item.details.iter().any(|v| v.contains("Quarter zip")));
    assert_eq!(item.fabric, "fleece-like");
    assert_eq!(item.fit, "relaxed");
    assert_eq!(item.rain, RainProtection::Waterproof);
    assert!(item.favourite);
    assert_eq!(item.size, "L");
    assert_eq!(item.original_photo.as_deref(), Some("original.png"));
    assert_eq!(item.photo.as_deref(), Some("cutout.png"));
    assert_eq!(item.photo_analysis.unwrap().inferred_fields.len(), 12);
}

#[test]
fn manual_edits_during_processing_take_precedence_over_generated_fields() {
    let before = original();
    let mut item = before.clone();
    item.name = "My favourite warm top".into();
    item.warmth = 2.5;
    item.description = "My own description".into();
    let applied = analysis().apply(&mut item, &before, "cutout.png").unwrap();
    assert_eq!(item.name, "My favourite warm top");
    assert_eq!(item.warmth, 2.5);
    assert_eq!(item.description, "My own description");
    assert!(!applied.contains(&"name".to_string()));
    assert_eq!(item.color, "cream, burgundy");
}

#[test]
fn corrupt_analysis_is_rejected_before_any_fields_change() {
    let before = original();
    let mut item = before.clone();
    let mut result = analysis();
    result.attributes.warmth = 12.0;
    assert!(result.apply(&mut item, &before, "cutout.png").is_err());
    assert_eq!(item.name, before.name);
    assert_eq!(item.photo, before.photo);
    let mut result = analysis();
    result.recognition_backend = "Vulkan".into();
    assert!(result.validate().is_err());
    result.gpu_layers = 29;
    assert!(result.validate().is_ok());
}

#[test]
fn stale_writer_cannot_drop_gpu_provenance_and_backend_setting_survives_reload() {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(root.path()).unwrap();
    let before = original();
    let mut item = before.clone();
    let mut result = analysis();
    result.recognition_backend = "Vulkan".into();
    result.gpu_layers = 29;
    result.apply(&mut item, &before, "cutout.png").unwrap();
    db.save_garment(&item, 2.5).unwrap();
    db.put_setting("photo_backend", &PhotoBackend::Cpu).unwrap();
    let legacy = rusqlite::Connection::open(root.path().join("wardrobe.sqlite3")).unwrap();
    let mut stale = serde_json::to_value(&item).unwrap();
    stale["photo_analysis"]
        .as_object_mut()
        .unwrap()
        .remove("recognition_backend");
    assert!(
        legacy
            .execute(
                "UPDATE garments SET data=?1 WHERE id=?2",
                rusqlite::params![stale.to_string(), item.id]
            )
            .is_err()
    );
    drop(db);
    let db = Database::open(root.path()).unwrap();
    assert_eq!(
        db.setting::<PhotoBackend>("photo_backend").unwrap(),
        Some(PhotoBackend::Cpu)
    );
    assert_eq!(
        db.garments().unwrap()[0]
            .photo_analysis
            .as_ref()
            .unwrap()
            .recognition_backend,
        "Vulkan"
    );
}

#[test]
fn descriptions_details_and_both_photo_versions_survive_search_and_backup() {
    let root = tempfile::tempdir().unwrap();
    let exports = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let mut db = Database::open(root.path()).unwrap();
    for name in ["original.png", "cutout.png"] {
        image::RgbaImage::from_pixel(20, 20, image::Rgba([50, 80, 100, 255]))
            .save(db.photo_path(name).unwrap())
            .unwrap();
    }
    let before = original();
    let mut item = before.clone();
    let mut result = analysis();
    result.color_reference = Some(ColorReference {
        x: 0.1,
        y: 0.2,
        radius: 0.025,
    });
    result.recognition_backend = "Vulkan".into();
    result.segmentation_backend = "DirectML".into();
    result.device = "Vulkan0 · AMD Radeon RX 7800 XT".into();
    result.runtime_version = "b12102".into();
    result.gpu_layers = 29;
    result.apply(&mut item, &before, "cutout.png").unwrap();
    db.save_garment(&item, 4.5).unwrap();
    assert_eq!(db.search("cherry blossom").unwrap().len(), 1);
    assert_eq!(db.search("relaxed fleece").unwrap().len(), 1);
    let folder = db.export_backup(exports.path()).unwrap();
    let mut restored = Database::open(target.path()).unwrap();
    restored.restore_backup(&folder).unwrap();
    let item = restored.garments().unwrap().remove(0);
    assert!(
        restored
            .photo_path(item.original_photo.as_ref().unwrap())
            .unwrap()
            .exists()
    );
    assert!(
        restored
            .photo_path(item.photo.as_ref().unwrap())
            .unwrap()
            .exists()
    );
    assert!(item.description.contains("burgundy"));
    let info = item.photo_analysis.unwrap();
    assert_eq!(info.color_reference.unwrap().x, 0.1);
    assert_eq!(info.recognition_backend, "Vulkan");
    assert_eq!(info.segmentation_backend, "DirectML");
    assert_eq!(info.runtime_version, "b12102");
}

#[test]
fn old_clothing_records_remain_readable_without_new_vision_fields() {
    let old = serde_json::json!({"id":uuid::Uuid::new_v4().to_string(),"name":"Old shirt","category":"Top","color":"navy"});
    let item: Garment = serde_json::from_value(old).unwrap();
    item.validate().unwrap();
    assert!(item.description.is_empty());
    assert!(item.original_photo.is_none());
}

#[test]
fn existing_database_upgrades_without_losing_items_and_stale_apps_cannot_drop_metadata() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("wardrobe.sqlite3");
    let legacy = rusqlite::Connection::open(&path).unwrap();
    legacy
        .execute_batch(include_str!("../migrations/001_initial.sql"))
        .unwrap();
    let item = original();
    let mut old = serde_json::to_value(&item).unwrap();
    for key in ["description", "details", "original_photo", "photo_analysis"] {
        old.as_object_mut().unwrap().remove(key);
    }
    legacy.execute("INSERT INTO garments(id,name,category,availability,favourite,archived,data) VALUES(?1,?2,'Top','Ready to wear',0,0,?3)",rusqlite::params![item.id,item.name,serde_json::to_string(&old).unwrap()]).unwrap();
    let mut db = Database::open(root.path()).unwrap();
    assert_eq!(db.garments().unwrap().len(), 1);
    let mut updated = db.garments().unwrap().remove(0);
    updated.description = "A useful new description".into();
    db.save_garment(&updated, 2.5).unwrap();
    let stale = legacy.execute(
        "UPDATE garments SET data=?1 WHERE id=?2",
        rusqlite::params![serde_json::to_string(&old).unwrap(), item.id],
    );
    assert!(stale.is_err());
    assert_eq!(
        db.garments().unwrap()[0].description,
        "A useful new description"
    );
    let version: i64 = legacy
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 3);
}

#[test]
fn outfit_preferences_actually_consume_detected_material_fit_and_description() {
    let top = Garment {
        name: "Plain shirt".into(),
        category: Category::Top,
        ..Garment::default()
    };
    let embroidered = Garment {
        name: "Decorated shirt".into(),
        description: "A suede-like shirt with embroidery.".into(),
        fit: "very baggy".into(),
        ..top.clone()
    };
    let embroidered = Garment {
        id: uuid::Uuid::new_v4().to_string(),
        ..embroidered
    };
    let garments = vec![
        top,
        embroidered.clone(),
        Garment {
            name: "Trousers".into(),
            category: Category::Bottom,
            ..Garment::default()
        },
        Garment {
            name: "Shoes".into(),
            category: Category::Shoes,
            ..Garment::default()
        },
    ];
    let preferences = Preferences {
        preferred_styles: "suede, embroidery".into(),
        preferred_fits: "baggy".into(),
        ..Preferences::default()
    };
    let profile = ScoringProfile {
        weather: 0.0,
        occasion: 0.0,
        style: 0.0,
        preference: 100.0,
        history: 0.0,
        ..ScoringProfile::default()
    };
    let start =
        chrono::NaiveDateTime::parse_from_str("2026-11-04 09:00", "%Y-%m-%d %H:%M").unwrap();
    let plan = Plan {
        start,
        end: start + chrono::Duration::hours(8),
        occasion: Occasion::Casual,
        outdoor_fraction: 0.5,
        walking: false,
        accessories: false,
        weather: WeatherSummary::default(),
        location: "Manual".into(),
        locked: vec![],
        excluded: vec![],
        shuffle: 0,
    };
    let results = Ranker {
        garments: &garments,
        preferences: &preferences,
        profile: &profile,
        affinities: &HashMap::new(),
        wear: &[],
        feedback: &[],
        favourites: &HashSet::new(),
    }
    .generate(&plan, 2)
    .unwrap();
    assert!(results[0].items.iter().any(|g| g.id == embroidered.id));
    assert!(
        results[0]
            .reasons
            .iter()
            .any(|r| r.contains("suede, embroidery"))
    );
}
