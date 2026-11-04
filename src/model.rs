use anyhow::{Result, bail, ensure};
use chrono::{NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! choices {
    ($name:ident { $($variant:ident => $label:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name { $($variant),+ }
        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
            pub fn label(self) -> &'static str { match self { $(Self::$variant => $label),+ } }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.label()) }
        }
    };
}

choices!(Category {
    Top => "Top", Bottom => "Bottom", OnePiece => "Dress / one-piece",
    Layer => "Mid-layer", Outerwear => "Outerwear", Shoes => "Shoes", Accessory => "Accessory"
});
choices!(Availability {
    Clean => "Ready to wear", Laundry => "In the wash", Stored => "Packed away", Unavailable => "Unavailable"
});
choices!(RainProtection {
    Unknown => "Not recorded", None => "Not rainproof", Resistant => "Water resistant", Waterproof => "Waterproof"
});
choices!(Occasion {
    Casual => "Casual", Work => "Work / smart casual", Dinner => "Dinner / going out", Formal => "Formal", Active => "Exercise / active"
});
choices!(PhotoBackend { Auto => "Automatic (GPU preferred)", Gpu => "GPU", Cpu => "CPU" });
impl PhotoBackend {
    pub fn code(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Gpu => "gpu",
            Self::Cpu => "cpu",
        }
    }
}
impl Occasion {
    pub fn formality(self) -> f32 {
        match self {
            Self::Casual => 1.5,
            Self::Work => 3.0,
            Self::Dinner => 3.5,
            Self::Formal => 5.0,
            Self::Active => 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Garment {
    pub id: String,
    pub name: String,
    pub category: Category,
    pub color: String,
    pub warmth: f32,
    pub formality: f32,
    pub rain: RainProtection,
    pub windproof: bool,
    pub walking_comfort: f32,
    pub availability: Availability,
    pub favourite: bool,
    pub archived: bool,
    pub brand: String,
    pub size: String,
    pub fabric: String,
    pub pattern: String,
    pub fit: String,
    pub seasons: String,
    pub styles: String,
    pub notes: String,
    pub description: String,
    pub details: Vec<String>,
    pub original_photo: Option<String>,
    pub photo_analysis: Option<PhotoAnalysisInfo>,
    pub photo: Option<String>,
}
impl Default for Garment {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: String::new(),
            category: Category::Top,
            color: "Unspecified".into(),
            warmth: 1.0,
            formality: 2.0,
            rain: RainProtection::Unknown,
            windproof: false,
            walking_comfort: 3.0,
            availability: Availability::Clean,
            favourite: false,
            archived: false,
            brand: String::new(),
            size: String::new(),
            fabric: String::new(),
            pattern: String::new(),
            fit: String::new(),
            seasons: String::new(),
            styles: String::new(),
            notes: String::new(),
            description: String::new(),
            details: vec![],
            original_photo: None,
            photo_analysis: None,
            photo: None,
        }
    }
}
impl Garment {
    pub fn validate(&self) -> Result<()> {
        Uuid::parse_str(&self.id)?;
        ensure!(!self.name.trim().is_empty(), "Give this item a name.");
        ensure!(
            self.name.len() <= 200,
            "Item names must be under 200 characters."
        );
        for (label, value, min, max) in [
            ("Warmth", self.warmth, 0.0, 5.0),
            ("Formality", self.formality, 1.0, 5.0),
            ("Walking comfort", self.walking_comfort, 1.0, 5.0),
        ] {
            ensure!(
                value.is_finite() && (min..=max).contains(&value),
                "{label} is out of range."
            );
        }
        if let Some(photo) = &self.photo {
            validate_media_name(photo)?;
        }
        if let Some(photo) = &self.original_photo {
            validate_media_name(photo)?;
        }
        if let Some(reference) = self
            .photo_analysis
            .as_ref()
            .and_then(|info| info.color_reference)
        {
            reference.validate()?;
        }
        ensure!(
            self.description.len() <= 4000 && self.details.len() <= 32,
            "Clothing description is too long."
        );
        Ok(())
    }
    pub fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {}",
            self.name,
            self.category,
            self.color,
            self.brand,
            self.size,
            self.fabric,
            self.pattern,
            self.fit,
            self.seasons,
            self.styles,
            self.notes,
            self.availability,
            self.rain,
            if self.favourite {
                "favourite favorite"
            } else {
                ""
            },
            if self.windproof { "windproof" } else { "" },
            self.description,
            self.details.join(" ")
        )
        .to_lowercase()
    }

    pub fn media_files(&self) -> impl Iterator<Item = &String> {
        self.photo.iter().chain(self.original_photo.iter())
    }

    pub fn visual_context(&self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.styles,
            self.fabric,
            self.fit,
            self.pattern,
            self.description,
            self.details.join(" ")
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhotoAnalysisInfo {
    pub model: String,
    pub revision: String,
    pub pipeline: String,
    pub generated_at: String,
    pub source_sha256: String,
    pub uncertainties: Vec<String>,
    pub inferred_fields: Vec<String>,
    #[serde(default)]
    pub color_reference: Option<ColorReference>,
    #[serde(default)]
    pub device: String,
    #[serde(default)]
    pub recognition_backend: String,
    #[serde(default)]
    pub segmentation_backend: String,
    #[serde(default)]
    pub runtime_version: String,
    #[serde(default)]
    pub fallback_reasons: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ColorReference {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
}
impl ColorReference {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            [self.x, self.y, self.radius].iter().all(|n| n.is_finite())
                && (0.0..=1.0).contains(&self.x)
                && (0.0..=1.0).contains(&self.y)
                && (0.005..=0.1).contains(&self.radius),
            "Choose a neutral patch inside the original photo."
        );
        Ok(())
    }
}

pub fn validate_media_name(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty() && !value.contains(['/', '\\', ':']) && value != "." && value != "..",
        "Invalid photo filename."
    );
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub thermal_adjustment: f32,
    pub preferred_colors: String,
    pub avoided_colors: String,
    pub preferred_fits: String,
    pub preferred_styles: String,
    pub repeat_days: u32,
    pub guide_notes: String,
    pub guide_images: Vec<String>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            thermal_adjustment: 0.0,
            preferred_colors: String::new(),
            avoided_colors: String::new(),
            preferred_fits: String::new(),
            preferred_styles: String::new(),
            repeat_days: 4,
            guide_notes: String::new(),
            guide_images: vec![],
        }
    }
}
impl Preferences {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.thermal_adjustment.is_finite() && (-5.0..=5.0).contains(&self.thermal_adjustment),
            "Temperature preference is out of range."
        );
        ensure!(self.repeat_days <= 60, "Repeat window must be 0–60 days.");
        for file in &self.guide_images {
            validate_media_name(file)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScoringProfile {
    pub revision: i64,
    pub weather: f32,
    pub occasion: f32,
    pub style: f32,
    pub preference: f32,
    pub history: f32,
}
impl Default for ScoringProfile {
    fn default() -> Self {
        Self {
            revision: 0,
            weather: 35.0,
            occasion: 25.0,
            style: 15.0,
            preference: 15.0,
            history: 10.0,
        }
    }
}
impl ScoringProfile {
    pub fn validate(&self) -> Result<()> {
        let values = [
            self.weather,
            self.occasion,
            self.style,
            self.preference,
            self.history,
        ];
        ensure!(
            values
                .iter()
                .all(|v| v.is_finite() && (0.0..=100.0).contains(v)),
            "Scoring weights must be 0–100."
        );
        ensure!(
            values.iter().sum::<f32>() > 0.0,
            "At least one scoring weight must be above zero."
        );
        Ok(())
    }
    pub fn total(&self) -> f32 {
        self.weather + self.occasion + self.style + self.preference + self.history
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Location {
    pub name: String,
    #[serde(default)]
    pub admin1: String,
    #[serde(default)]
    pub country: String,
    pub latitude: f64,
    pub longitude: f64,
    pub timezone: String,
}
impl Location {
    pub fn label(&self) -> String {
        [&self.name, &self.admin1, &self.country]
            .into_iter()
            .filter(|s| !s.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeatherSummary {
    pub temperature_min: f32,
    pub temperature_max: f32,
    pub feels_min: f32,
    pub feels_max: f32,
    pub rain_probability: f32,
    pub precipitation_mm: f32,
    pub wind_kmh: f32,
    pub source: String,
}
impl Default for WeatherSummary {
    fn default() -> Self {
        Self {
            temperature_min: 18.0,
            temperature_max: 22.0,
            feels_min: 18.0,
            feels_max: 22.0,
            rain_probability: 0.0,
            precipitation_mm: 0.0,
            wind_kmh: 5.0,
            source: "Manual conditions".into(),
        }
    }
}
impl WeatherSummary {
    pub fn validate(&self) -> Result<()> {
        for temp in [
            self.temperature_min,
            self.temperature_max,
            self.feels_min,
            self.feels_max,
        ] {
            ensure!(
                temp.is_finite() && (-90.0..=65.0).contains(&temp),
                "Temperature must be between −90 and 65 °C."
            );
        }
        ensure!(
            self.temperature_min <= self.temperature_max && self.feels_min <= self.feels_max,
            "Minimum temperature cannot exceed maximum."
        );
        ensure!(
            self.rain_probability.is_finite() && (0.0..=100.0).contains(&self.rain_probability),
            "Rain chance must be 0–100%."
        );
        ensure!(
            self.precipitation_mm.is_finite() && (0.0..=1000.0).contains(&self.precipitation_mm),
            "Invalid precipitation amount."
        );
        ensure!(
            self.wind_kmh.is_finite() && (0.0..=400.0).contains(&self.wind_kmh),
            "Wind speed must be 0–400 km/h."
        );
        Ok(())
    }
    pub fn rainy(&self) -> bool {
        self.rain_probability >= 40.0 || self.precipitation_mm >= 0.2
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub occasion: Occasion,
    pub outdoor_fraction: f32,
    pub walking: bool,
    pub accessories: bool,
    pub weather: WeatherSummary,
    pub location: String,
    pub locked: Vec<String>,
    pub excluded: Vec<String>,
    pub shuffle: u64,
}
impl Plan {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.end > self.start,
            "Return time must be after departure; use the next date for overnight outings."
        );
        ensure!(
            self.end - self.start <= chrono::Duration::hours(48),
            "Plan an outing of up to 48 hours."
        );
        ensure!(
            self.outdoor_fraction.is_finite() && (0.0..=1.0).contains(&self.outdoor_fraction),
            "Invalid time outdoors."
        );
        self.weather.validate()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScoreBreakdown {
    pub weather: f32,
    pub occasion: f32,
    pub style: f32,
    pub preference: f32,
    pub history: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outfit {
    pub id: String,
    pub signature: String,
    pub items: Vec<Garment>,
    pub score: f32,
    pub breakdown: ScoreBreakdown,
    pub reasons: Vec<String>,
    pub warnings: Vec<String>,
    pub plan: Plan,
    pub profile: ScoringProfile,
    pub preferences: Preferences,
    pub algorithm: String,
    pub created_at: String,
}
impl Outfit {
    pub fn signature(items: &[Garment]) -> String {
        let mut ids = items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>();
        ids.sort_unstable();
        ids.join("|")
    }
    pub fn validate(&self) -> Result<()> {
        Uuid::parse_str(&self.id)?;
        ensure!(!self.items.is_empty(), "An outfit cannot be empty.");
        for item in &self.items {
            item.validate()?;
        }
        ensure!(
            self.signature == Self::signature(&self.items),
            "Outfit item signature is inconsistent."
        );
        ensure!(self.score.is_finite(), "Invalid outfit score.");
        self.plan.validate()?;
        self.profile.validate()?;
        self.preferences.validate()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedOutfit {
    pub outfit: Outfit,
    pub favourite: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WearEvent {
    pub id: String,
    pub outfit: Outfit,
    pub worn_at: NaiveDateTime,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feedback {
    pub id: String,
    pub signature: String,
    pub item_ids: Vec<String>,
    pub value: i32,
    pub reason: String,
    pub created_at: String,
    pub outfit: Outfit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffinityRevision {
    pub item_id: String,
    pub value: f32,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileRevision {
    pub profile: ScoringProfile,
    pub preferences: Preferences,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Backup {
    pub schema: u32,
    pub garments: Vec<Garment>,
    pub profiles: Vec<ProfileRevision>,
    pub outfits: Vec<SavedOutfit>,
    pub wear: Vec<WearEvent>,
    pub feedback: Vec<Feedback>,
    pub affinity: Vec<AffinityRevision>,
}
impl Backup {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema == 1, "This backup uses an unsupported version.");
        let mut ids = std::collections::HashSet::new();
        for item in &self.garments {
            item.validate()?;
            ensure!(ids.insert(&item.id), "Duplicate item in backup.");
        }
        ensure!(!self.profiles.is_empty(), "Backup has no scoring settings.");
        for p in &self.profiles {
            p.profile.validate()?;
            p.preferences.validate()?;
        }
        for outfit in &self.outfits {
            outfit.outfit.validate()?;
        }
        for wear in &self.wear {
            Uuid::parse_str(&wear.id)?;
            wear.outfit.validate()?;
        }
        for f in &self.feedback {
            Uuid::parse_str(&f.id)?;
            ensure!((-1..=1).contains(&f.value), "Invalid feedback.");
            for id in &f.item_ids {
                Uuid::parse_str(id)?;
            }
            f.outfit.validate()?;
        }
        for a in &self.affinity {
            Uuid::parse_str(&a.item_id)?;
            if !a.value.is_finite() || !(0.0..=5.0).contains(&a.value) {
                bail!("Invalid style affinity.");
            }
        }
        Ok(())
    }
}

pub fn now() -> String {
    Utc::now().to_rfc3339()
}
pub fn tags(text: &str) -> Vec<String> {
    text.split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}
