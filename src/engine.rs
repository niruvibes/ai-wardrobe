//! Explainable, versioned ranking. Availability and user locks are hard constraints;
//! taste, comfort and repetition are soft preferences with visible warnings.
use crate::model::*;
use anyhow::{Result, ensure};
use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
};
use uuid::Uuid;

pub const ALGORITHM_VERSION: &str = "wardrobe-rules/2";
const BEAM_WIDTH: usize = 192;

pub struct Ranker<'a> {
    pub garments: &'a [Garment],
    pub preferences: &'a Preferences,
    pub profile: &'a ScoringProfile,
    pub affinities: &'a HashMap<String, f32>,
    pub wear: &'a [WearEvent],
    pub feedback: &'a [Feedback],
    pub favourites: &'a HashSet<String>,
}

impl Ranker<'_> {
    pub fn generate(&self, plan: &Plan, count: usize) -> Result<Vec<Outfit>> {
        plan.validate()?;
        self.profile.validate()?;
        self.preferences.validate()?;
        let eligible: Vec<&Garment> = self
            .garments
            .iter()
            .filter(|g| {
                !g.archived
                    && g.availability == Availability::Clean
                    && !plan.excluded.contains(&g.id)
            })
            .collect();
        let mut lock_categories = HashSet::new();
        for id in &plan.locked {
            let g = eligible.iter().find(|g| &g.id == id);
            ensure!(
                g.is_some(),
                "A locked item is unavailable, archived or excluded. Unlock it or mark it ready to wear."
            );
            ensure!(
                lock_categories.insert(g.unwrap().category),
                "Lock just one item per clothing category."
            );
        }
        ensure!(
            !(lock_categories.contains(&Category::OnePiece)
                && (lock_categories.contains(&Category::Top)
                    || lock_categories.contains(&Category::Bottom))),
            "Choose a one-piece or a top and bottom, rather than locking both."
        );
        let mut patterns = vec![
            vec![Category::Top, Category::Bottom, Category::Shoes],
            vec![Category::OnePiece, Category::Shoes],
        ];
        patterns.retain(|p| {
            !lock_categories.iter().any(|c| {
                matches!(c, Category::Top | Category::Bottom | Category::OnePiece) && !p.contains(c)
            })
        });
        let mut all = Vec::<Outfit>::new();
        for mut pattern in patterns {
            pattern.extend([Category::Layer, Category::Outerwear]);
            if plan.accessories || lock_categories.contains(&Category::Accessory) {
                pattern.push(Category::Accessory);
            }
            let mut beam: Vec<Vec<Garment>> = vec![vec![]];
            for category in pattern {
                let optional = matches!(
                    category,
                    Category::Layer | Category::Outerwear | Category::Accessory
                );
                let locked = eligible
                    .iter()
                    .find(|g| g.category == category && plan.locked.contains(&g.id));
                let mut choices: Vec<Option<&Garment>> = if let Some(g) = locked {
                    vec![Some(*g)]
                } else {
                    let mut candidates = eligible
                        .iter()
                        .copied()
                        .filter(|g| g.category == category)
                        .collect::<Vec<_>>();
                    candidates.sort_by(|a, b| {
                        self.item_priority(b, plan)
                            .total_cmp(&self.item_priority(a, plan))
                    });
                    // Cap each slot while retaining warmth and waterproof extremes.
                    let mut shortlisted = candidates.iter().take(22).copied().collect::<Vec<_>>();
                    for special in [
                        candidates
                            .iter()
                            .max_by(|a, b| a.warmth.total_cmp(&b.warmth)),
                        candidates
                            .iter()
                            .min_by(|a, b| a.warmth.total_cmp(&b.warmth)),
                        candidates
                            .iter()
                            .find(|g| g.rain == RainProtection::Waterproof),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        if !shortlisted.iter().any(|g| g.id == special.id) {
                            shortlisted.push(special);
                        }
                    }
                    shortlisted.into_iter().map(Some).collect()
                };
                if optional && locked.is_none() {
                    choices.push(None);
                }
                if choices.is_empty() {
                    beam.clear();
                    break;
                }
                let mut expanded = vec![];
                for base in &beam {
                    for choice in &choices {
                        let mut items = base.clone();
                        if let Some(g) = choice {
                            items.push((*g).clone());
                        }
                        expanded.push(items);
                    }
                }
                let remaining_optional = matches!(
                    category,
                    Category::Top | Category::Bottom | Category::OnePiece | Category::Shoes
                );
                expanded.sort_by_cached_key(|items| {
                    let score = if remaining_optional {
                        items
                            .iter()
                            .map(|g| self.item_priority(g, plan))
                            .sum::<f32>()
                            / items.len().max(1) as f32
                    } else {
                        self.score(items, plan).0
                    };
                    std::cmp::Reverse((score * 1000.0) as i64)
                });
                expanded.truncate(BEAM_WIDTH);
                beam = expanded;
            }
            for items in beam {
                if !plan
                    .locked
                    .iter()
                    .all(|id| items.iter().any(|g| &g.id == id))
                {
                    continue;
                }
                let (score, breakdown, reasons, warnings) = self.score(&items, plan);
                all.push(Outfit {
                    id: Uuid::new_v4().to_string(),
                    signature: Outfit::signature(&items),
                    items,
                    score,
                    breakdown,
                    reasons,
                    warnings,
                    plan: plan.clone(),
                    profile: self.profile.clone(),
                    preferences: self.preferences.clone(),
                    algorithm: ALGORITHM_VERSION.into(),
                    created_at: now(),
                });
            }
        }
        ensure!(
            !all.is_empty(),
            "No complete outfit is available. Add ready-to-wear shoes and either a top + bottom or a dress / one-piece, and check your locks and exclusions."
        );
        let mut chosen: Vec<Outfit> = vec![];
        while !all.is_empty() && chosen.len() < count {
            let best = all
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| {
                    let diversity = |o: &Outfit| {
                        chosen
                            .iter()
                            .map(|prior| overlap(&o.items, &prior.items))
                            .fold(0.0_f32, f32::max)
                            * 9.0
                    };
                    (a.score + jitter(&a.signature, plan.shuffle) - diversity(a))
                        .total_cmp(&(b.score + jitter(&b.signature, plan.shuffle) - diversity(b)))
                })
                .map(|(i, _)| i)
                .unwrap();
            let next = all.swap_remove(best);
            all.retain(|o| o.signature != next.signature);
            chosen.push(next);
        }
        Ok(chosen)
    }

    fn item_priority(&self, g: &Garment, plan: &Plan) -> f32 {
        let occasion =
            (1.0 - (g.formality - plan.occasion.formality()).abs() / 4.0).clamp(0.0, 1.0);
        let taste = self.affinities.get(&g.id).copied().unwrap_or(2.5) / 5.0;
        occasion * self.profile.occasion
            + taste * self.profile.style
            + self.item_preference(g) * self.profile.preference
            + self.item_freshness(g, plan) * self.profile.history
            + jitter(&g.id, plan.shuffle)
    }

    fn item_preference(&self, g: &Garment) -> f32 {
        let p = self.preferences;
        let mut score: f32 = 0.65;
        if tags(&p.preferred_colors)
            .iter()
            .any(|c| matches_phrase(&g.color, c))
        {
            score += 0.18;
        }
        if tags(&p.avoided_colors)
            .iter()
            .any(|c| matches_phrase(&g.color, c))
        {
            score -= 0.6;
        }
        if tags(&p.preferred_fits)
            .iter()
            .any(|s| matches_phrase(&format!("{} {}", g.fit, g.styles), s))
        {
            score += 0.1;
        }
        if tags(&p.preferred_styles)
            .iter()
            .any(|s| matches_phrase(&g.visual_context(), s))
        {
            score += 0.15;
        }
        if g.favourite {
            score += 0.1;
        }
        score.clamp(0.0, 1.0)
    }

    fn item_freshness(&self, g: &Garment, plan: &Plan) -> f32 {
        let days = self.preferences.repeat_days;
        if days == 0 {
            return 1.0;
        }
        let most_recent = self
            .wear
            .iter()
            .filter(|w| w.worn_at <= plan.start && w.outfit.items.iter().any(|i| i.id == g.id))
            .map(|w| w.worn_at)
            .max();
        if let Some(at) = most_recent {
            let elapsed = (plan.start - at).num_seconds() as f32 / 86400.0;
            let freshness = (elapsed / days as f32).clamp(0.0, 1.0);
            // Repeating shoes and coats is normal; allow more repetition than tops.
            if matches!(
                g.category,
                Category::Shoes | Category::Outerwear | Category::Accessory
            ) {
                0.5 + freshness * 0.5
            } else {
                freshness
            }
        } else {
            1.0
        }
    }

    fn latest_feedback(&self) -> Vec<&Feedback> {
        let mut latest = HashMap::new();
        for f in self.feedback {
            latest.insert(f.signature.as_str(), f);
        }
        latest.into_values().collect()
    }

    fn score(
        &self,
        items: &[Garment],
        plan: &Plan,
    ) -> (f32, ScoreBreakdown, Vec<String>, Vec<String>) {
        let mut reasons = Vec::new();
        let mut warnings = Vec::new();
        let effective_cold =
            plan.weather.feels_min * plan.outdoor_fraction + 21.0 * (1.0 - plan.outdoor_fraction);
        let mut target = ((24.0 - effective_cold + self.preferences.thermal_adjustment) / 3.0
            + 1.0)
            .clamp(0.8, 12.0);
        if plan.occasion == Occasion::Active {
            target = (target - 1.0).max(0.8);
        }
        let latest = self.latest_feedback();
        let thermal = latest
            .iter()
            .filter(|f| {
                (f.outfit.plan.weather.feels_min - plan.weather.feels_min).abs() <= 5.0
                    && (f.outfit.plan.outdoor_fraction - plan.outdoor_fraction).abs() <= 0.3
            })
            .map(|f| match f.reason.as_str() {
                "Too warm" => -0.5,
                "Too cold" => 0.5,
                _ => 0.0,
            })
            .sum::<f32>()
            .clamp(-1.5, 1.5);
        target = (target + thermal).max(0.8);
        let clothing = items
            .iter()
            .filter(|g| !matches!(g.category, Category::Shoes | Category::Accessory))
            .collect::<Vec<_>>();
        let warmth: f32 = clothing.iter().map(|g| g.warmth).sum();
        let base: f32 = clothing
            .iter()
            .filter(|g| !matches!(g.category, Category::Layer | Category::Outerwear))
            .map(|g| g.warmth)
            .sum();
        let warm_target = ((24.0
            - (plan.weather.feels_max * plan.outdoor_fraction
                + 21.0 * (1.0 - plan.outdoor_fraction))
            + self.preferences.thermal_adjustment)
            / 3.0
            + 1.0)
            .clamp(0.8, 12.0);
        let mismatch = (warmth - target).abs() / target.max(3.0);
        let mut weather = (1.0 - mismatch * 0.65).clamp(0.0, 1.0);
        if base > warm_target + 2.0 {
            weather -= 0.18;
            warnings.push("The base clothes may feel warm during the hottest hours.".into());
        }
        if warmth < target - 2.0 {
            warnings.push("You may need more warmth than these clothes provide.".into());
        }
        if warmth > target + 2.5 {
            warnings.push("This outfit may feel too warm; remove a layer if possible.".into());
        }
        let outer = items.iter().find(|g| g.category == Category::Outerwear);
        let shoes = items.iter().find(|g| g.category == Category::Shoes);
        if plan.outdoor_fraction > 0.05 && plan.weather.rainy() {
            match outer.map(|g| g.rain) {
                Some(RainProtection::Waterproof) => {
                    reasons.push("Waterproof outerwear for the rain risk.".into())
                }
                Some(RainProtection::Resistant) => {
                    weather -= 0.12;
                    warnings.push(
                        "Outerwear is water resistant, not waterproof. Consider an umbrella."
                            .into(),
                    );
                }
                _ => {
                    weather -= 0.35;
                    warnings.push(
                        "No verified waterproof outerwear; take an umbrella or change your plan."
                            .into(),
                    );
                }
            }
            match shoes.map(|g| g.rain) {
                Some(RainProtection::Waterproof) => {
                    reasons.push("Waterproof shoes for wet ground.".into())
                }
                Some(RainProtection::Resistant) => weather -= 0.05,
                _ => {
                    weather -= 0.13;
                    warnings.push(
                        "The shoes are not recorded as water resistant or waterproof.".into(),
                    );
                }
            }
        }
        if plan.outdoor_fraction > 0.2 && plan.weather.wind_kmh >= 25.0 && effective_cold < 20.0 {
            if outer.is_some_and(|g| g.windproof) {
                reasons.push("A windproof outer layer helps in the wind.".into());
            } else {
                weather -= 0.1;
                warnings.push("Wind protection is not recorded for this outfit.".into());
            }
        }
        let required: Vec<_> = items
            .iter()
            .filter(|g| {
                matches!(
                    g.category,
                    Category::Top | Category::Bottom | Category::OnePiece | Category::Shoes
                )
            })
            .collect();
        let mut occasion = required
            .iter()
            .map(|g| (1.0 - (g.formality - plan.occasion.formality()).abs() / 4.0).clamp(0.0, 1.0))
            .sum::<f32>()
            / required.len().max(1) as f32;
        if plan.occasion == Occasion::Formal && required.iter().any(|g| g.formality < 3.5) {
            warnings.push("Some pieces may be too casual for a formal dress code.".into());
            occasion -= 0.2;
        }
        if plan.occasion == Occasion::Active
            && required.iter().any(|g| {
                !tags(&g.styles)
                    .iter()
                    .any(|t| ["active", "sport", "athletic", "sportswear"].contains(&t.as_str()))
            })
        {
            warnings.push(
                "Check suitability for exercise; mark sportswear with an 'active' style tag."
                    .into(),
            );
            occasion -= 0.2;
        }
        if plan.walking
            && let Some(shoe) = shoes
        {
            occasion *= 0.65 + (shoe.walking_comfort / 5.0) * 0.35;
            if shoe.walking_comfort < 3.0 {
                warnings.push("These shoes may be uncomfortable for a lot of walking.".into());
            }
        }
        let preference =
            items.iter().map(|g| self.item_preference(g)).sum::<f32>() / items.len().max(1) as f32;
        let history = items
            .iter()
            .map(|g| self.item_freshness(g, plan))
            .sum::<f32>()
            / items.len().max(1) as f32;
        let affinity = items
            .iter()
            .map(|g| self.affinities.get(&g.id).copied().unwrap_or(2.5) / 5.0)
            .sum::<f32>()
            / items.len().max(1) as f32;
        let harmony = color_harmony(items);
        let signature = Outfit::signature(items);
        let mut learned: f32 = 0.0;
        for f in latest {
            if [
                "Too warm",
                "Too cold",
                "Too formal",
                "Too casual",
                "Uncomfortable",
            ]
            .contains(&f.reason.as_str())
            {
                continue;
            }
            let common = items.iter().filter(|g| f.item_ids.contains(&g.id)).count();
            // Feedback affects combinations, without condemning every individual item.
            if common >= 2 {
                learned += f.value as f32
                    * (common as f32 / f.item_ids.len().max(items.len()).max(1) as f32)
                    * 0.2;
            }
        }
        for f in self.latest_feedback() {
            if f.outfit.plan.occasion != plan.occasion {
                continue;
            }
            let common = items.iter().filter(|g| f.item_ids.contains(&g.id)).count();
            if common >= 2
                && ((f.reason == "Too formal")
                    || (f.reason == "Too casual")
                    || f.reason == "Uncomfortable")
            {
                occasion -= 0.12 * common as f32 / items.len().max(1) as f32;
            }
        }
        let style = (affinity * 0.65
            + harmony * 0.35
            + learned.clamp(-0.35, 0.35)
            + if self.favourites.contains(&signature) {
                0.1
            } else {
                0.0
            })
        .clamp(0.0, 1.0);
        if history > 0.9 {
            reasons.push("Adds variety within your repeat window.".into());
        } else {
            reasons.push(
                "Includes a recently worn piece; alternatives are limited or rank lower.".into(),
            );
        }
        if items.iter().any(|g| g.favourite) {
            reasons.push("Includes a favourite from your wardrobe.".into());
        }
        let matched = tags(&self.preferences.preferred_styles)
            .into_iter()
            .filter(|s| items.iter().any(|g| matches_phrase(&g.visual_context(), s)))
            .collect::<Vec<_>>();
        if !matched.is_empty() {
            reasons.push(format!(
                "Matches your style details: {}.",
                matched.join(", ")
            ));
        }
        if items
            .iter()
            .any(|g| matches!(g.category, Category::Layer | Category::Outerwear))
            && plan.weather.feels_max - plan.weather.feels_min >= 5.0
        {
            reasons.push("Removable layers help across the temperature change.".into());
        }
        reasons.push(format!(
            "Ranked for {} and {:.0}–{:.0} °C feels-like conditions.",
            plan.occasion.label().to_lowercase(),
            plan.weather.feels_min,
            plan.weather.feels_max
        ));
        let breakdown = ScoreBreakdown {
            weather: weather.clamp(0.0, 1.0) * 100.0,
            occasion: occasion.clamp(0.0, 1.0) * 100.0,
            style: style * 100.0,
            preference: preference * 100.0,
            history: history * 100.0,
        };
        let p = self.profile;
        let score = (breakdown.weather * p.weather
            + breakdown.occasion * p.occasion
            + breakdown.style * p.style
            + breakdown.preference * p.preference
            + breakdown.history * p.history)
            / p.total();
        (score, breakdown, reasons, warnings)
    }
}

fn overlap(a: &[Garment], b: &[Garment]) -> f32 {
    let common = a.iter().filter(|g| b.iter().any(|i| g.id == i.id)).count();
    common as f32 / (a.len() + b.len() - common).max(1) as f32
}

fn matches_phrase(text: &str, phrase: &str) -> bool {
    let words = |s: &str| {
        s.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
    };
    let haystack = words(text);
    let needle = words(phrase);
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn jitter(key: &str, seed: u64) -> f32 {
    if seed == 0 {
        return 0.0;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    seed.hash(&mut hasher);
    (hasher.finish() % 1000) as f32 / 1000.0 * 5.0
}

fn color_harmony(items: &[Garment]) -> f32 {
    let neutral = [
        "black",
        "white",
        "grey",
        "gray",
        "navy",
        "beige",
        "cream",
        "brown",
        "tan",
        "denim",
        "unspecified",
    ];
    let colors: HashSet<_> = items.iter().flat_map(|g| tags(&g.color)).collect();
    let accents = colors
        .iter()
        .filter(|c| !neutral.contains(&c.as_str()))
        .count();
    let patterns = items
        .iter()
        .filter(|g| {
            !g.pattern.is_empty()
                && !["solid", "plain", "none"].contains(&g.pattern.to_lowercase().as_str())
        })
        .count();
    (0.95 - accents.saturating_sub(2) as f32 * 0.12 - patterns.saturating_sub(1) as f32 * 0.08)
        .clamp(0.2, 1.0)
}
