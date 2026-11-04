use chrono::{Duration, NaiveDateTime};
use std::collections::{HashMap, HashSet};
use wardrobe::{
    db::Database,
    engine::Ranker,
    model::*,
    weather::{Forecast, Hour, HourlyResponse, parse_hours},
};

fn dt(text: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap()
}
fn item(name: &str, category: Category) -> Garment {
    Garment {
        name: name.into(),
        category,
        color: "navy".into(),
        warmth: 1.0,
        formality: 2.0,
        ..Garment::default()
    }
}
fn clothes() -> Vec<Garment> {
    vec![
        item("Tee", Category::Top),
        item("Shirt", Category::Top),
        item("Jeans", Category::Bottom),
        item("Chinos", Category::Bottom),
        item("Sneakers", Category::Shoes),
        item("Boots", Category::Shoes),
    ]
}
fn plan() -> Plan {
    Plan {
        start: dt("2026-11-04 09:00"),
        end: dt("2026-11-04 17:00"),
        occasion: Occasion::Casual,
        outdoor_fraction: 1.0,
        walking: false,
        accessories: false,
        weather: WeatherSummary::default(),
        location: "Manual conditions".into(),
        locked: vec![],
        excluded: vec![],
        shuffle: 0,
    }
}
fn rank(garments: &[Garment], plan: &Plan) -> Vec<Outfit> {
    Ranker {
        garments,
        preferences: &Preferences::default(),
        profile: &ScoringProfile::default(),
        affinities: &HashMap::new(),
        wear: &[],
        feedback: &[],
        favourites: &HashSet::new(),
    }
    .generate(plan, 3)
    .unwrap()
}
fn location() -> Location {
    Location {
        name: "Sydney".into(),
        admin1: "NSW".into(),
        country: "Australia".into(),
        latitude: -33.87,
        longitude: 151.21,
        timezone: "Australia/Sydney".into(),
    }
}
fn forecast() -> Forecast {
    Forecast {
        location: location(),
        fetched_at: now(),
        hours: (0..48)
            .map(|n| Hour {
                time: dt("2026-11-04 00:00") + Duration::hours(n),
                temperature: 20.0,
                feels_like: 19.0,
                rain_probability: 0.0,
                precipitation_mm: 0.0,
                wind_kmh: 10.0,
            })
            .collect(),
    }
}

#[test]
fn never_suggests_laundry_stored_excluded_or_archived_items() {
    let mut garments = clothes();
    let mut dirty = item("Dirty", Category::Top);
    dirty.availability = Availability::Laundry;
    garments.push(dirty);
    let mut stored = item("Stored", Category::Shoes);
    stored.availability = Availability::Stored;
    garments.push(stored);
    let mut archived = item("Archived", Category::Bottom);
    archived.archived = true;
    garments.push(archived);
    let mut p = plan();
    p.excluded.push(garments[0].id.clone());
    for outfit in rank(&garments, &p) {
        for g in outfit.items {
            assert_eq!(g.availability, Availability::Clean);
            assert!(!g.archived);
            assert!(!p.excluded.contains(&g.id));
        }
    }
}

#[test]
fn locks_are_preserved_in_every_option_and_conflicts_are_explicit() {
    let garments = clothes();
    let mut p = plan();
    p.locked.push(garments[0].id.clone());
    for outfit in rank(&garments, &p) {
        assert!(outfit.items.iter().any(|g| g.id == garments[0].id));
    }
    p.locked.push(garments[1].id.clone());
    let r = Ranker {
        garments: &garments,
        preferences: &Preferences::default(),
        profile: &ScoringProfile::default(),
        affinities: &HashMap::new(),
        wear: &[],
        feedback: &[],
        favourites: &HashSet::new(),
    };
    assert!(
        r.generate(&p, 3)
            .unwrap_err()
            .to_string()
            .contains("one item")
    );
    p.locked = vec!["missing".into()];
    assert!(r.generate(&p, 3).is_err());
}

#[test]
fn complete_one_piece_outfits_work_without_tops_and_bottoms() {
    let garments = vec![
        item("Dress", Category::OnePiece),
        item("Shoes", Category::Shoes),
    ];
    let results = rank(&garments, &plan());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].items.len(), 2);
}

#[test]
fn wet_weather_prefers_verified_waterproof_shell_and_shoes() {
    let mut garments = clothes();
    let mut shell = item("Rain shell", Category::Outerwear);
    shell.rain = RainProtection::Waterproof;
    shell.warmth = 0.5;
    let dry = item("Dry coat", Category::Outerwear);
    garments.push(shell.clone());
    garments.push(dry);
    garments[5].rain = RainProtection::Waterproof;
    let mut p = plan();
    p.weather.rain_probability = 85.0;
    p.weather.precipitation_mm = 5.0;
    let best = &rank(&garments, &p)[0];
    assert!(best.items.iter().any(|g| g.id == shell.id));
    assert!(best.items.iter().any(|g| g.id == garments[5].id));
    assert!(
        !best
            .warnings
            .iter()
            .any(|w| w.contains("No verified waterproof"))
    );
}

#[test]
fn gaps_in_weather_protection_are_disclosed() {
    let mut p = plan();
    p.weather.rain_probability = 90.0;
    assert!(
        rank(&clothes(), &p)[0]
            .warnings
            .iter()
            .any(|w| w.contains("No verified waterproof"))
    );
}

#[test]
fn cold_weather_adds_warm_removable_layers() {
    let mut garments = clothes();
    let mut coat = item("Coat", Category::Outerwear);
    coat.warmth = 4.0;
    let mut jumper = item("Jumper", Category::Layer);
    jumper.warmth = 2.0;
    garments.extend([coat.clone(), jumper]);
    let mut p = plan();
    p.weather.feels_min = 0.0;
    p.weather.feels_max = 6.0;
    p.weather.temperature_min = 2.0;
    p.weather.temperature_max = 8.0;
    let best = &rank(&garments, &p)[0];
    assert!(best.items.iter().any(|g| g.id == coat.id));
}

#[test]
fn style_affinity_changes_ranking_without_mutating_clothing() {
    let garments = clothes();
    let mut scores = HashMap::new();
    scores.insert(garments[0].id.clone(), 0.0);
    scores.insert(garments[1].id.clone(), 5.0);
    let profile = ScoringProfile {
        weather: 0.0,
        occasion: 0.0,
        style: 100.0,
        preference: 0.0,
        history: 0.0,
        ..ScoringProfile::default()
    };
    let results = Ranker {
        garments: &garments,
        preferences: &Preferences::default(),
        profile: &profile,
        affinities: &scores,
        wear: &[],
        feedback: &[],
        favourites: &HashSet::new(),
    }
    .generate(&plan(), 3)
    .unwrap();
    assert!(results[0].items.iter().any(|g| g.id == garments[1].id));
    assert_eq!(garments[0].color, "navy");
}

#[test]
fn history_penalises_recent_pieces_but_ignores_future_wears() {
    let garments = clothes();
    let p = plan();
    let outfit = rank(&garments, &p).remove(0);
    let event = WearEvent {
        id: uuid::Uuid::new_v4().to_string(),
        outfit: outfit.clone(),
        worn_at: p.start - Duration::hours(1),
        notes: String::new(),
    };
    let profile = ScoringProfile {
        weather: 0.0,
        occasion: 0.0,
        style: 0.0,
        preference: 0.0,
        history: 100.0,
        ..ScoringProfile::default()
    };
    let prefs = Preferences::default();
    let empty = HashMap::new();
    let fav = HashSet::new();
    let history = [event.clone()];
    let r = Ranker {
        garments: &garments,
        preferences: &prefs,
        profile: &profile,
        affinities: &empty,
        wear: &history,
        feedback: &[],
        favourites: &fav,
    };
    let result = r.generate(&p, 3).unwrap();
    assert_ne!(result[0].signature, outfit.signature);
    assert_eq!(result[0].breakdown.history, 100.0);
    let future = [WearEvent {
        worn_at: p.start + Duration::days(1),
        ..event
    }];
    let r = Ranker { wear: &future, ..r };
    assert!(
        r.generate(&p, 3)
            .unwrap()
            .iter()
            .all(|o| o.breakdown.history == 100.0)
    );
}

#[test]
fn combination_feedback_changes_style_and_latest_feedback_wins() {
    let garments = clothes();
    let p = plan();
    let outfit = rank(&garments, &p).remove(0);
    let feedback = Feedback {
        id: uuid::Uuid::new_v4().to_string(),
        signature: outfit.signature.clone(),
        item_ids: outfit.items.iter().map(|g| g.id.clone()).collect(),
        value: -1,
        reason: "Not my style".into(),
        created_at: now(),
        outfit: outfit.clone(),
    };
    let prefs = Preferences::default();
    let profile = ScoringProfile::default();
    let scores = HashMap::new();
    let fav = HashSet::new();
    let events = [feedback.clone()];
    let r = Ranker {
        garments: &garments,
        preferences: &prefs,
        profile: &profile,
        affinities: &scores,
        wear: &[],
        feedback: &events,
        favourites: &fav,
    };
    let disliked = r
        .generate(&p, 20)
        .unwrap()
        .into_iter()
        .find(|o| o.signature == outfit.signature)
        .unwrap();
    assert!(disliked.breakdown.style < outfit.breakdown.style);
    let events = [
        feedback.clone(),
        Feedback {
            value: 1,
            reason: "Love it".into(),
            ..feedback
        },
    ];
    let r = Ranker {
        feedback: &events,
        ..r
    };
    let liked = r
        .generate(&p, 20)
        .unwrap()
        .into_iter()
        .find(|o| o.signature == outfit.signature)
        .unwrap();
    assert!(liked.breakdown.style > outfit.breakdown.style);
}

#[test]
fn swapping_keeps_every_other_piece_and_excludes_the_old_one() {
    let garments = clothes();
    let p = plan();
    let first = rank(&garments, &p).remove(0);
    let old = &first.items[0];
    let mut swap = p;
    swap.locked = first
        .items
        .iter()
        .filter(|g| g.id != old.id)
        .map(|g| g.id.clone())
        .collect();
    swap.excluded = vec![old.id.clone()];
    for option in rank(&garments, &swap) {
        assert!(!option.items.iter().any(|g| g.id == old.id));
        assert!(
            swap.locked
                .iter()
                .all(|id| option.items.iter().any(|g| &g.id == id))
        );
    }
}

#[test]
fn result_options_are_unique() {
    let results = rank(&clothes(), &plan());
    let unique = results.iter().map(|o| &o.signature).collect::<HashSet<_>>();
    assert_eq!(results.len(), 3);
    assert_eq!(unique.len(), 3);
}

#[test]
fn forecast_uses_only_intersecting_hours_including_partial_hours_and_overnight() {
    let mut f = forecast();
    f.hours[17].rain_probability = 99.0;
    let (summary, hours) = f
        .window(dt("2026-11-04 09:30"), dt("2026-11-04 17:00"))
        .unwrap();
    assert_eq!(hours.len(), 8);
    assert_eq!(summary.rain_probability, 0.0);
    let (_, hours) = f
        .window(dt("2026-11-04 23:30"), dt("2026-11-05 01:15"))
        .unwrap();
    assert_eq!(hours.len(), 3);
    let (summary, _) = f
        .window(dt("2026-11-04 16:59"), dt("2026-11-04 17:01"))
        .unwrap();
    assert_eq!(summary.rain_probability, 99.0);
}

#[test]
fn incomplete_and_stale_forecasts_are_rejected() {
    let mut f = forecast();
    f.hours.remove(10);
    assert!(
        f.window(dt("2026-11-04 09:00"), dt("2026-11-04 12:00"))
            .is_err()
    );
    f.fetched_at = (chrono::Utc::now() - Duration::hours(4)).to_rfc3339();
    assert!(!f.fresh());
    assert!(f.apply_to(&mut plan()).is_err());
}

#[test]
fn malformed_provider_arrays_are_not_silently_truncated() {
    let raw = HourlyResponse {
        time: vec!["2026-11-04T09:00".into()],
        temperature_2m: vec![],
        apparent_temperature: vec![],
        precipitation_probability: vec![],
        precipitation: vec![],
        wind_speed_10m: vec![],
    };
    assert!(parse_hours(raw).is_err());
}

#[test]
fn ambiguous_daylight_saving_hours_are_rejected_instead_of_guessing() {
    let mut f = forecast();
    f.hours.push(f.hours[10].clone());
    assert!(
        f.window(dt("2026-11-04 09:00"), dt("2026-11-04 12:00"))
            .unwrap_err()
            .to_string()
            .contains("ambiguous")
    );
}

#[test]
fn rejects_invalid_user_input_and_weights() {
    let mut p = plan();
    p.end = p.start;
    assert!(p.validate().is_err());
    p = plan();
    p.weather.feels_min = 30.0;
    assert!(p.validate().is_err());
    let bad = ScoringProfile {
        weather: 0.0,
        occasion: 0.0,
        style: 0.0,
        preference: 0.0,
        history: 0.0,
        revision: 0,
    };
    assert!(bad.validate().is_err());
    let mut g = item("", Category::Top);
    assert!(g.validate().is_err());
    g.name = "Test".into();
    g.photo = Some("../escape.png".into());
    assert!(g.validate().is_err());
}

#[test]
fn persists_150_items_and_searches_names_tags_and_rain_protection() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Database::open(dir.path()).unwrap();
    for n in 0..150 {
        let mut g = item(
            &format!("Piece {n:03}"),
            Category::ALL[n % Category::ALL.len()],
        );
        g.fabric = "Linen".into();
        g.styles = "relaxed, classic".into();
        g.rain = RainProtection::Waterproof;
        db.save_garment(&g, 2.5).unwrap();
    }
    assert_eq!(db.garments().unwrap().len(), 150);
    assert_eq!(db.search("linen classic").unwrap().len(), 150);
    assert_eq!(db.search("waterproof").unwrap().len(), 150);
    assert_eq!(db.search("Piece 049").unwrap().len(), 1);
    assert!(db.search("\" OR *").is_ok());
    drop(db);
    assert_eq!(
        Database::open(dir.path())
            .unwrap()
            .garments()
            .unwrap()
            .len(),
        150
    );
}

#[test]
fn scoring_revisions_and_affinities_are_append_only_and_snapshots_stay_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Database::open(dir.path()).unwrap();
    let garments = clothes();
    for g in &garments {
        db.save_garment(g, 2.5).unwrap();
    }
    let outfit = rank(&garments, &plan()).remove(0);
    db.save_outfit(&outfit, true).unwrap();
    db.record_wear(&outfit, plan().start, "A good day").unwrap();
    db.feedback(&outfit, 1, "Love it").unwrap();
    let profile = ScoringProfile {
        style: 90.0,
        ..ScoringProfile::default()
    };
    db.save_profile(&profile, &Preferences::default()).unwrap();
    db.save_garment(&garments[0], 4.5).unwrap();
    assert_eq!(db.profiles().unwrap().len(), 2);
    assert_eq!(db.affinities().unwrap()[&garments[0].id], 4.5);
    assert_eq!(db.affinity_revisions().unwrap().len(), 7);
    let saved = &db.outfits().unwrap()[0].outfit;
    assert_eq!(saved.profile.style, 15.0);
    assert_eq!(saved.score, outfit.score);
    assert_eq!(db.wear_history().unwrap().len(), 1);
    assert_eq!(db.feedback_history().unwrap().len(), 1);
}

#[test]
fn photo_is_managed_and_complete_backup_round_trips() {
    let source_dir = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let exports = tempfile::tempdir().unwrap();
    let source = source_dir.path().join("photo.png");
    image::RgbaImage::from_pixel(20, 20, image::Rgba([20, 40, 50, 255]))
        .save(&source)
        .unwrap();
    let mut db = Database::open(root.path()).unwrap();
    let mut garments = clothes();
    let file = db.import_photo(&source).unwrap();
    garments[0].photo = Some(file.clone());
    for g in &garments {
        db.save_garment(g, 3.0).unwrap();
    }
    let outfit = rank(&garments, &plan()).remove(0);
    db.save_outfit(&outfit, true).unwrap();
    db.record_wear(&outfit, plan().start, "Test").unwrap();
    db.feedback(&outfit, -1, "Too cold").unwrap();
    let backup = db.export_backup(exports.path()).unwrap();
    drop(source_dir);
    let mut restored = Database::open(dest.path()).unwrap();
    restored.restore_backup(&backup).unwrap();
    assert_eq!(restored.garments().unwrap().len(), 6);
    assert!(restored.photo_path(&file).unwrap().exists());
    assert_eq!(
        restored.outfits().unwrap()[0].outfit.signature,
        outfit.signature
    );
    assert!(restored.outfits().unwrap()[0].favourite);
    assert_eq!(restored.wear_history().unwrap()[0].notes, "Test");
    assert_eq!(restored.feedback_history().unwrap()[0].reason, "Too cold");
    assert!(dest.path().join("recovery").is_dir());
}

#[test]
fn invalid_backup_is_rejected_without_replacing_existing_data() {
    let root = tempfile::tempdir().unwrap();
    let exports = tempfile::tempdir().unwrap();
    let mut db = Database::open(root.path()).unwrap();
    let g = item("Keep me", Category::Top);
    db.save_garment(&g, 2.5).unwrap();
    let backup = db.export_backup(exports.path()).unwrap();
    let file = backup.join("wardrobe.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    value["garments"][0]["photo"] = "../outside.png".into();
    std::fs::write(&file, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(db.restore_backup(&backup).is_err());
    assert_eq!(db.garments().unwrap()[0].name, "Keep me");
}

#[test]
fn ranks_a_catalogue_larger_than_100_without_full_cartesian_expansion() {
    let garments = (0..150)
        .map(|n| item(&format!("Piece {n}"), Category::ALL[n % 7]))
        .collect::<Vec<_>>();
    let start = std::time::Instant::now();
    let results = rank(&garments, &plan());
    assert_eq!(results.len(), 3);
    // Generous guard against accidentally reverting to an unbounded product search.
    assert!(start.elapsed() < std::time::Duration::from_secs(20));
}
