//! Creates an isolated, explicitly named demonstration wardrobe. Never modifies an existing catalogue.
use std::collections::HashSet;
use wardrobe::{db::Database, engine::Ranker, model::*};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    anyhow::ensure!(
        args.next().as_deref() == Some("--data-dir"),
        "Usage: cargo run --example seed_demo -- --data-dir <empty demo folder>"
    );
    let folder = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("Choose an explicit demo folder"))?;
    let mut db = Database::open(&folder)?;
    anyhow::ensure!(
        db.garments()?.is_empty() && db.wear_history()?.is_empty(),
        "Demo seeding requires an empty wardrobe"
    );
    let colors = [
        "navy", "cream", "olive", "black", "white", "brown", "grey", "burgundy", "beige", "blue",
        "pink", "green", "tan", "denim", "red", "yellow",
    ];
    let mut garments = vec![];
    for category in Category::ALL {
        for (n, color) in colors.iter().enumerate() {
            let (name, warmth, formality, fabric) = match category {
                Category::Top => (
                    if n % 3 == 0 {
                        "linen shirt"
                    } else if n % 3 == 1 {
                        "cotton tee"
                    } else {
                        "Oxford shirt"
                    },
                    if n % 3 == 1 { 0.5 } else { 1.0 },
                    if n % 3 == 1 { 1.0 } else { 3.0 },
                    if n % 3 == 0 { "linen" } else { "cotton" },
                ),
                Category::Bottom => (
                    if n % 2 == 0 {
                        "chinos"
                    } else {
                        "straight jeans"
                    },
                    1.0,
                    2.0,
                    if n % 2 == 0 { "cotton" } else { "denim" },
                ),
                Category::OnePiece => ("day dress", 1.5, 3.0, "cotton"),
                Category::Layer => (
                    if n % 2 == 0 {
                        "merino jumper"
                    } else {
                        "cardigan"
                    },
                    2.0,
                    3.0,
                    "wool",
                ),
                Category::Outerwear => (
                    if n % 2 == 0 {
                        "rain shell"
                    } else {
                        "wool coat"
                    },
                    if n % 2 == 0 { 0.5 } else { 3.5 },
                    if n % 2 == 0 { 2.0 } else { 4.0 },
                    if n % 2 == 0 { "nylon" } else { "wool" },
                ),
                Category::Shoes => (
                    if n % 2 == 0 {
                        "leather boots"
                    } else {
                        "trainers"
                    },
                    0.5,
                    if n % 2 == 0 { 3.0 } else { 1.0 },
                    if n % 2 == 0 { "leather" } else { "canvas" },
                ),
                Category::Accessory => {
                    (if n % 2 == 0 { "scarf" } else { "cap" }, 0.0, 2.0, "cotton")
                }
            };
            let g = Garment {
                name: format!("{} {name}", title(color)),
                category: *category,
                color: color.to_string(),
                warmth,
                formality,
                fabric: fabric.into(),
                fit: if n % 3 == 0 { "relaxed" } else { "regular" }.into(),
                pattern: "plain".into(),
                seasons: "spring, autumn".into(),
                styles: "classic, minimal".into(),
                rain: if (*category == Category::Outerwear || *category == Category::Shoes)
                    && n % 2 == 0
                {
                    RainProtection::Waterproof
                } else {
                    RainProtection::None
                },
                windproof: *category == Category::Outerwear && n % 2 == 0,
                walking_comfort: if n % 2 == 0 { 4.0 } else { 5.0 },
                favourite: n == 0 || n == 4,
                availability: if n == 14 {
                    Availability::Laundry
                } else if n == 15 {
                    Availability::Stored
                } else {
                    Availability::Clean
                },
                ..Garment::default()
            };
            db.save_garment(&g, if n == 0 { 4.5 } else { 2.5 })?;
            garments.push(g);
        }
    }
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    db.put_setting("draft",&serde_json::json!({"start_date":date,"start_time":"09:00","end_date":date,"end_time":"17:00","manual":true}))?;
    let plan = Plan {
        start: chrono::NaiveDateTime::parse_from_str(&format!("{date} 09:00"), "%Y-%m-%d %H:%M")?,
        end: chrono::NaiveDateTime::parse_from_str(&format!("{date} 17:00"), "%Y-%m-%d %H:%M")?,
        occasion: Occasion::Casual,
        outdoor_fraction: 0.5,
        walking: false,
        accessories: false,
        weather: WeatherSummary::default(),
        location: "Demo conditions".into(),
        locked: vec![],
        excluded: vec![],
        shuffle: 0,
    };
    let profile = db.profiles()?.last().unwrap().profile.clone();
    let affinities = db.affinities()?;
    let options = Ranker {
        garments: &garments,
        preferences: &Preferences::default(),
        profile: &profile,
        affinities: &affinities,
        wear: &[],
        feedback: &[],
        favourites: &HashSet::new(),
    }
    .generate(&plan, 3)?;
    db.save_outfit(&options[0], true)?;
    db.record_wear(
        &options[1],
        plan.start - chrono::Duration::days(2),
        "Demo wear history",
    )?;
    println!(
        "Created {} demo pieces in {}",
        garments.len(),
        std::path::Path::new(&folder).canonicalize()?.display()
    );
    Ok(())
}
fn title(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().unwrap().to_uppercase().collect::<String>() + chars.as_str()
}
