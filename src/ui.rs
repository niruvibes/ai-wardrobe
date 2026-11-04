use chrono::{Local, NaiveDateTime};
use eframe::egui::{self, Align, Color32, FontId, Layout, RichText, Stroke, Vec2};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::mpsc,
};
use wardrobe::{
    db::Database,
    engine::Ranker,
    model::*,
    vision::{Job, VisionEvent, VisionService},
    weather::{self, Forecast, Hour},
};

const INK: Color32 = Color32::from_rgb(37, 51, 44);
const MUTED: Color32 = Color32::from_rgb(107, 118, 108);
const GREEN: Color32 = Color32::from_rgb(49, 91, 66);
const PAPER: Color32 = Color32::from_rgb(247, 246, 241);
const BORDER: Color32 = Color32::from_rgb(221, 225, 215);
const AMBER: Color32 = Color32::from_rgb(146, 88, 28);

pub fn configure(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::light();
    style.visuals.panel_fill = PAPER;
    style.visuals.window_fill = Color32::WHITE;
    style.visuals.override_text_color = Some(INK);
    style.visuals.selection.bg_fill = Color32::from_rgb(209, 227, 212);
    style.visuals.selection.stroke = Stroke::new(1.0_f32, GREEN);
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(239, 241, 234);
    style.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(239, 241, 234);
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(222, 234, 221);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(208, 225, 208);
    style.spacing.item_spacing = Vec2::new(10.0, 9.0);
    style.spacing.button_padding = Vec2::new(12.0, 8.0);
    style.spacing.interact_size.y = 30.0;
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(15.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Heading, FontId::proportional(29.0));
    ctx.set_style(style);
    egui_extras::install_image_loaders(ctx);
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Today,
    Wardrobe,
    Favourites,
    History,
    Preferences,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Draft {
    start_date: String,
    start_time: String,
    end_date: String,
    end_time: String,
    occasion: Occasion,
    outdoors: f32,
    walking: bool,
    accessories: bool,
    manual: bool,
    conditions: WeatherSummary,
    locked: Vec<String>,
    excluded: Vec<String>,
}
impl Default for Draft {
    fn default() -> Self {
        let date = Local::now().format("%Y-%m-%d").to_string();
        Self {
            start_date: date.clone(),
            start_time: "09:00".into(),
            end_date: date,
            end_time: "17:00".into(),
            occasion: Occasion::Casual,
            outdoors: 0.5,
            walking: false,
            accessories: false,
            manual: false,
            conditions: WeatherSummary::default(),
            locked: vec![],
            excluded: vec![],
        }
    }
}

struct Editor {
    item: Garment,
    affinity: f32,
    queue: VecDeque<PathBuf>,
    another: bool,
    error: String,
    photo_task: Option<PhotoTask>,
    photo_progress: String,
    show_original: bool,
    color_reference: Option<ColorReference>,
    pick_reference: bool,
}
struct PhotoTask {
    job: Job,
    before: Garment,
    output_name: String,
}
impl Editor {
    fn new(item: Garment, affinity: f32, queue: VecDeque<PathBuf>) -> Self {
        let color_reference = item
            .photo_analysis
            .as_ref()
            .and_then(|info| info.color_reference);
        Self {
            item,
            affinity,
            queue,
            another: false,
            error: String::new(),
            photo_task: None,
            photo_progress: String::new(),
            show_original: false,
            color_reference,
            pick_reference: false,
        }
    }
    fn cancel_photo(&mut self) {
        if let Some(task) = self.photo_task.take() {
            task.job.cancel();
        }
        self.photo_progress.clear();
    }
}
enum Event {
    Locations(String, Result<Vec<Location>, String>),
    Forecast(Result<Forecast, String>),
    Outfits(u64, Result<Vec<Outfit>, String>),
}

pub struct WardrobeApp {
    db: Database,
    page: Page,
    garments: Vec<Garment>,
    affinities: HashMap<String, f32>,
    saved: Vec<SavedOutfit>,
    wear: Vec<WearEvent>,
    feedback: Vec<Feedback>,
    profile: ScoringProfile,
    preferences: Preferences,
    profiles: Vec<ProfileRevision>,
    draft: Draft,
    location: Option<Location>,
    city_query: String,
    places: Vec<Location>,
    forecast: Option<Forecast>,
    hourly: Vec<Hour>,
    outfits: Vec<Outfit>,
    query: String,
    search_results: Vec<Garment>,
    category_filter: Option<Category>,
    status_filter: Option<Availability>,
    rain_filter: bool,
    favourite_filter: bool,
    show_archived: bool,
    editor: Option<Editor>,
    message: Option<(String, bool)>,
    network_busy: bool,
    generating: bool,
    epoch: u64,
    shuffle: u64,
    tx: mpsc::Sender<Event>,
    rx: mpsc::Receiver<Event>,
    feedback_dialog: Option<(Outfit, String)>,
    wear_dialog: Option<(Outfit, String, String)>,
    restore_pending: Option<PathBuf>,
    history_query: String,
    vision: VisionService,
    auto_analyse: bool,
    photo_backend: PhotoBackend,
    setup_job: Option<Job>,
    setup_progress: String,
}

impl WardrobeApp {
    pub fn open(root: PathBuf) -> anyhow::Result<Self> {
        let db = Database::open(root)?;
        let profiles = db.profiles()?;
        let current = profiles.last().unwrap().clone();
        let garments = db.garments()?;
        let location: Option<Location> = db.setting("location")?;
        let draft = db.setting("draft")?.unwrap_or_default();
        let (tx, rx) = mpsc::channel();
        let vision = VisionService::new(&db.root);
        let auto_analyse = db.setting("auto_analyse")?.unwrap_or(true);
        let photo_backend = db.setting("photo_backend")?.unwrap_or(PhotoBackend::Auto);
        Ok(Self {
            affinities: db.affinities()?,
            saved: db.outfits()?,
            wear: db.wear_history()?,
            feedback: db.feedback_history()?,
            forecast: db.setting("forecast")?,
            db,
            page: Page::Today,
            search_results: garments.clone(),
            garments,
            profile: current.profile,
            preferences: current.preferences,
            profiles,
            draft,
            city_query: location
                .as_ref()
                .map(|l| l.name.clone())
                .unwrap_or_default(),
            location,
            places: vec![],
            hourly: vec![],
            outfits: vec![],
            query: String::new(),
            category_filter: None,
            status_filter: None,
            rain_filter: false,
            favourite_filter: false,
            show_archived: false,
            editor: None,
            message: None,
            network_busy: false,
            generating: false,
            epoch: 0,
            shuffle: 0,
            tx,
            rx,
            feedback_dialog: None,
            wear_dialog: None,
            restore_pending: None,
            history_query: String::new(),
            vision,
            auto_analyse,
            photo_backend,
            setup_job: None,
            setup_progress: String::new(),
        })
    }

    fn notice(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), false));
    }
    fn error(&mut self, error: impl std::fmt::Display) {
        self.message = Some((error.to_string(), true));
    }

    fn reload(&mut self) -> anyhow::Result<()> {
        self.garments = self.db.garments()?;
        self.affinities = self.db.affinities()?;
        self.saved = self.db.outfits()?;
        self.wear = self.db.wear_history()?;
        self.feedback = self.db.feedback_history()?;
        self.search_results = self.db.search(&self.query)?;
        Ok(())
    }

    fn save_item(&mut self, item: &Garment, affinity: f32) -> bool {
        match self
            .db
            .save_garment(item, affinity)
            .and_then(|_| self.reload())
        {
            Ok(()) => {
                self.epoch += 1;
                self.outfits.clear();
                true
            }
            Err(e) => {
                self.error(e);
                false
            }
        }
    }

    fn poll(&mut self) {
        self.poll_vision();
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Locations(query, result) => {
                    self.network_busy = false;
                    match result {
                        Ok(locations) if query == self.city_query => {
                            if locations.is_empty() {
                                self.notice("No places found. Try a nearby city or postal code.");
                            }
                            self.places = locations;
                        }
                        Ok(_) => {}
                        Err(e) => self.error(format!(
                            "Location search failed: {e}. You can use manual conditions."
                        )),
                    }
                }
                Event::Forecast(result) => {
                    self.network_busy = false;
                    match result {
                        Ok(forecast) if self.location.as_ref() == Some(&forecast.location) => {
                            match self.db.put_setting("forecast", &forecast) {
                                Ok(()) => { self.forecast = Some(forecast); self.notice("Hourly forecast updated."); }
                                Err(e) => self.error(e),
                            }
                        }
                        Ok(_) => {}, Err(e) => self.error(format!("Forecast unavailable: {e}. Your wardrobe still works; enter manual conditions to plan an outfit.")),
                    }
                }
                Event::Outfits(epoch, result) => {
                    self.generating = false;
                    if epoch != self.epoch {
                        self.notice("Your wardrobe or preferences changed. Generate again to use the latest details.");
                        continue;
                    }
                    match result {
                        Ok(outfits) => {
                            self.outfits = outfits;
                            self.notice("Outfits are ready. Lock a piece, swap one, or ask for other options.");
                        }
                        Err(e) => self.error(e),
                    }
                }
            }
        }
    }

    fn poll_vision(&mut self) {
        for event in self.vision.poll() {
            self.handle_vision_event(event);
        }
    }

    fn handle_vision_event(&mut self, event: VisionEvent) {
        match event {
            VisionEvent::Progress { id, message } => {
                if self.setup_job.as_ref().is_some_and(|job| job.id == id) {
                    self.setup_progress = message;
                } else if let Some(editor) = self.editor.as_mut()
                    && editor
                        .photo_task
                        .as_ref()
                        .is_some_and(|task| task.job.id == id)
                {
                    editor.photo_progress = message;
                }
            }
            VisionEvent::Analysed { id, result, .. } => {
                let Some(editor) = self.editor.as_mut() else {
                    return;
                };
                if !editor
                    .photo_task
                    .as_ref()
                    .is_some_and(|task| task.job.id == id)
                {
                    return;
                }
                let task = editor.photo_task.take().unwrap();
                match (*result).and_then(|analysis| {
                    let device = format!(
                        "{} · {}. Background removal: {}",
                        analysis.recognition_backend,
                        analysis.device,
                        analysis.segmentation_backend
                    );
                    let fallback = if analysis.fallback_reasons.is_empty() {
                        String::new()
                    } else {
                        format!(" CPU fallback: {}", analysis.fallback_reasons.join("; "))
                    };
                    analysis
                        .apply(&mut editor.item, &task.before, &task.output_name)
                        .map(|applied| (applied, device, fallback))
                        .map_err(|e| e.to_string())
                }) {
                    Ok((applied, device, fallback)) => {
                        editor.photo_progress = format!(
                            "Auto-filled {} fields. {device}.{fallback} Review the estimates before saving.",
                            applied.len()
                        );
                        editor.error.clear();
                        editor.show_original = false;
                    }
                    Err(message) => {
                        editor.error = message;
                        editor.photo_progress.clear();
                    }
                }
            }
            VisionEvent::SetupFinished { id, result } => {
                if !self.setup_job.as_ref().is_some_and(|job| job.id == id) {
                    return;
                }
                self.setup_job = None;
                match result {
                    Ok(()) => {
                        self.setup_progress =
                            "Photo automation is ready; photos are processed locally.".into();
                        if self.auto_analyse
                            && let Some(mut editor) = self.editor.take()
                        {
                            if editor.item.photo.is_some() {
                                self.start_photo_analysis(&mut editor);
                            }
                            self.editor = Some(editor);
                        }
                    }
                    Err(message) => {
                        self.setup_progress = message;
                    }
                }
            }
        }
    }

    fn start_photo_analysis(&self, editor: &mut Editor) {
        editor.cancel_photo();
        editor.error.clear();
        let Some(source) = editor
            .item
            .original_photo
            .as_ref()
            .or(editor.item.photo.as_ref())
        else {
            return;
        };
        let output_name = format!("{}.png", uuid::Uuid::new_v4());
        let paths = self.db.photo_path(source).and_then(|source| {
            self.db
                .photo_path(&output_name)
                .map(|output| (source, output))
        });
        match paths {
            Ok((source, output)) => {
                let before = editor.item.clone();
                let job = self.vision.analyse_configured(
                    source,
                    output,
                    editor.color_reference,
                    self.photo_backend,
                );
                editor.photo_task = Some(PhotoTask {
                    job,
                    before,
                    output_name,
                });
                editor.photo_progress =
                    "Preparing local clothing recognition… The first photo loads the model.".into();
            }
            Err(e) => editor.error = e.to_string(),
        }
    }

    fn photo_settings(&mut self, ui: &mut egui::Ui) {
        section(ui, "PHOTO AUTOMATION");
        let previous = self.photo_backend;
        ui.horizontal(|ui| {
            ui.label("Processing device");
            choice(
                ui,
                "photo-backend",
                &mut self.photo_backend,
                PhotoBackend::ALL,
                |value| value.label(),
            );
        });
        if previous != self.photo_backend
            && let Err(error) = self.db.put_setting("photo_backend", &self.photo_backend)
        {
            self.error(error);
        }
        ui.small("Automatic uses GPU recognition and CPU background removal to keep imports fast. GPU uses both Vulkan and DirectML, which can be slower when GPU memory is busy. Any recognition fallback is reported. Changes apply to the next photo.");
        if ui
            .add_enabled(
                self.setup_job.is_none()
                    && !self
                        .editor
                        .as_ref()
                        .is_some_and(|editor| editor.photo_task.is_some()),
                egui::Button::new("Release photo-processing memory"),
            )
            .clicked()
        {
            self.vision.release_cached_models();
            self.setup_progress =
                "Photo models unloaded. They will reload for the next photo.".into();
        }
        if ui
            .checkbox(
                &mut self.auto_analyse,
                "Automatically recognise clothing and remove backgrounds when adding photos",
            )
            .changed()
            && let Err(e) = self.db.put_setting("auto_analyse", &self.auto_analyse)
        {
            self.error(e);
        }
        ui.small("Generated names, categories, colours, warmth, formality, seasons and descriptions remain editable. Originals are kept. Photos stay on this computer.");
        let ready = self.vision.runtime.as_ref().is_some_and(|r| r.gpu_ready());
        if ready {
            ui.colored_label(
                GREEN,
                "GPU photo models are installed. Each result records the devices actually used.",
            );
        }
        if ui
            .add_enabled(
                self.setup_job.is_none(),
                egui::Button::new(if ready {
                    "Check GPU photo setup"
                } else {
                    "Set up GPU processing"
                }),
            )
            .clicked()
        {
            self.setup_job = Some(self.vision.setup());
            self.setup_progress="Preparing GPU photo processing. First setup downloads the Vulkan runtime and local models (about 4–5 GB).".into();
        }
        if !self.setup_progress.is_empty() {
            ui.label(&self.setup_progress);
        }
        if self.setup_job.is_some() {
            ui.spinner();
            if ui.small_button("Cancel setup").clicked() {
                if let Some(job) = self.setup_job.take() {
                    job.cancel();
                }
                self.setup_progress = "Setup cancelled. Run it again to resume.".into();
            }
        }
    }

    fn start_search(&mut self, ctx: &egui::Context) {
        if self.city_query.trim().chars().count() < 2 {
            self.error("Enter at least two characters for a place.");
            return;
        }
        let query = self.city_query.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.network_busy = true;
        std::thread::spawn(move || {
            let result = weather::search_locations(&query).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Event::Locations(query, result));
            ctx.request_repaint();
        });
    }

    fn start_forecast(&mut self, ctx: &egui::Context) {
        let Some(location) = self.location.clone() else {
            self.error("Search for and choose a location first.");
            return;
        };
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.network_busy = true;
        std::thread::spawn(move || {
            let result = weather::fetch_forecast(&location).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Event::Forecast(result));
            ctx.request_repaint();
        });
    }

    fn plan(&mut self) -> anyhow::Result<Plan> {
        let parse = |date: &str, time: &str| {
            NaiveDateTime::parse_from_str(
                &format!("{}T{}", date.trim(), time.trim()),
                "%Y-%m-%dT%H:%M",
            )
            .map_err(|_| {
                anyhow::anyhow!("Use dates as YYYY-MM-DD and times as HH:MM in 24-hour format.")
            })
        };
        let mut plan = Plan {
            start: parse(&self.draft.start_date, &self.draft.start_time)?,
            end: parse(&self.draft.end_date, &self.draft.end_time)?,
            occasion: self.draft.occasion,
            outdoor_fraction: self.draft.outdoors,
            walking: self.draft.walking,
            accessories: self.draft.accessories,
            weather: if self.draft.manual {
                self.draft.conditions.clone()
            } else {
                WeatherSummary::default()
            },
            location: "Manual conditions".into(),
            locked: self.draft.locked.clone(),
            excluded: self.draft.excluded.clone(),
            shuffle: self.shuffle,
        };
        plan.validate()?;
        if !self.draft.manual {
            let forecast = self.forecast.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "Choose a location and fetch its forecast, or select manual conditions."
                )
            })?;
            anyhow::ensure!(
                self.location.as_ref() == Some(&forecast.location),
                "Fetch a forecast for your selected location."
            );
            self.hourly = forecast.apply_to(&mut plan)?;
        } else {
            self.hourly.clear();
            plan.weather.source = "Manual conditions entered by you".into();
        }
        Ok(plan)
    }

    fn generate(&mut self, ctx: &egui::Context, override_plan: Option<Plan>) {
        let plan = match override_plan.map(Ok).unwrap_or_else(|| self.plan()) {
            Ok(p) => p,
            Err(e) => {
                self.error(e);
                return;
            }
        };
        let garments = self.garments.clone();
        let affinities = self.affinities.clone();
        // Unsaved edits in the preferences screen never leak into a scored snapshot.
        let active = self
            .profiles
            .last()
            .expect("Database always contains a scoring revision");
        let preferences = active.preferences.clone();
        let profile = active.profile.clone();
        let wear = self.wear.clone();
        let feedback = self.feedback.clone();
        let favourites = self
            .saved
            .iter()
            .filter(|o| o.favourite)
            .map(|o| o.outfit.signature.clone())
            .collect::<HashSet<_>>();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let epoch = self.epoch;
        self.generating = true;
        std::thread::spawn(move || {
            let ranker = Ranker {
                garments: &garments,
                preferences: &preferences,
                profile: &profile,
                affinities: &affinities,
                wear: &wear,
                feedback: &feedback,
                favourites: &favourites,
            };
            let result = ranker.generate(&plan, 3).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Event::Outfits(epoch, result));
            ctx.request_repaint();
        });
    }

    fn toggle_lock(&mut self, item: &Garment) {
        if self.draft.locked.contains(&item.id) {
            self.draft.locked.retain(|id| id != &item.id);
            return;
        }
        self.draft.locked.retain(|id| {
            self.garments.iter().find(|g| &g.id == id).is_some_and(|g| {
                g.category != item.category
                    && !(item.category == Category::OnePiece
                        && matches!(g.category, Category::Top | Category::Bottom))
                    && !(matches!(item.category, Category::Top | Category::Bottom)
                        && g.category == Category::OnePiece)
            })
        });
        self.draft.locked.push(item.id.clone());
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("navigation")
            .exact_width(184.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(236, 239, 229))
                    .inner_margin(20),
            )
            .show(ctx, |ui| {
                ui.add_space(13.0);
                ui.label(RichText::new("W / W").size(31.0).color(GREEN).strong());
                ui.add_space(9.0);
                ui.label(RichText::new("WARDROBE").size(17.0).strong());
                ui.label(
                    RichText::new("A little less deciding.")
                        .size(12.0)
                        .color(MUTED),
                );
                ui.add_space(37.0);
                for (page, label) in [
                    (Page::Today, "Plan an outfit"),
                    (Page::Wardrobe, "My wardrobe"),
                    (Page::Favourites, "Favourites"),
                    (Page::History, "Wear history"),
                    (Page::Preferences, "Preferences"),
                ] {
                    let selected = self.page == page;
                    if ui
                        .add_sized(
                            [144.0, 40.0],
                            egui::Button::new(RichText::new(label).color(if selected {
                                GREEN
                            } else {
                                INK
                            }))
                            .selected(selected),
                        )
                        .clicked()
                    {
                        self.page = page;
                    }
                }
                ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                    ui.label(
                        RichText::new("Stored on this computer")
                            .size(11.0)
                            .color(MUTED),
                    );
                    ui.label(
                        RichText::new(format!(
                            "{} pieces · {} favourites",
                            self.garments.iter().filter(|g| !g.archived).count(),
                            self.garments
                                .iter()
                                .filter(|g| g.favourite && !g.archived)
                                .count()
                        ))
                        .size(12.0)
                        .color(MUTED),
                    );
                });
            });
    }

    fn today(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        heading(
            ui,
            "What are you dressing for?",
            "Your plans, the forecast, and clothes you already own.",
        );
        card().show(ui, |ui| {
            ui.columns(2, |columns| {
                let ui = &mut columns[0];
                section(ui, "01   THE PLAN");
                ui.label("Departure and return · local time at your destination");
                ui.horizontal(|ui| {
                    ui.label("Out");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.draft.start_date)
                            .desired_width(102.0)
                            .hint_text("YYYY-MM-DD"),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.draft.start_time)
                            .desired_width(56.0)
                            .hint_text("HH:MM"),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("Back");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.draft.end_date)
                            .desired_width(102.0)
                            .hint_text("YYYY-MM-DD"),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.draft.end_time)
                            .desired_width(56.0)
                            .hint_text("HH:MM"),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("Occasion");
                    choice(
                        ui,
                        "occasion",
                        &mut self.draft.occasion,
                        Occasion::ALL,
                        |v| v.label(),
                    );
                });
                ui.add(
                    egui::Slider::new(&mut self.draft.outdoors, 0.0..=1.0)
                        .text("Time outdoors")
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                );
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.draft.walking, "Lots of walking");
                    ui.checkbox(&mut self.draft.accessories, "Add an accessory");
                });
                ui.small("Warmth accounts for indoor time. Exercise uses lighter layers.");
                let ui = &mut columns[1];
                section(ui, "02   THE CONDITIONS");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.draft.manual, false, "Hourly forecast");
                    ui.selectable_value(&mut self.draft.manual, true, "Enter manually");
                });
                if self.draft.manual {
                    ui.small("Enter the full range expected while you are out, in Celsius.");
                    ui.horizontal(|ui| {
                        ui.label("Actual °C");
                        ui.add(
                            egui::DragValue::new(&mut self.draft.conditions.temperature_min)
                                .speed(0.5),
                        );
                        ui.label("to");
                        ui.add(
                            egui::DragValue::new(&mut self.draft.conditions.temperature_max)
                                .speed(0.5),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Feels like °C");
                        ui.add(
                            egui::DragValue::new(&mut self.draft.conditions.feels_min).speed(0.5),
                        );
                        ui.label("to");
                        ui.add(
                            egui::DragValue::new(&mut self.draft.conditions.feels_max).speed(0.5),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Rain chance %");
                        ui.add(
                            egui::DragValue::new(&mut self.draft.conditions.rain_probability)
                                .range(0.0..=100.0),
                        );
                        ui.label("mm");
                        ui.add(
                            egui::DragValue::new(&mut self.draft.conditions.precipitation_mm)
                                .range(0.0..=1000.0),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Wind km/h");
                        ui.add(
                            egui::DragValue::new(&mut self.draft.conditions.wind_kmh)
                                .range(0.0..=400.0),
                        );
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.city_query)
                                .hint_text("City or postal code")
                                .desired_width(190.0),
                        );
                        if ui
                            .add_enabled(!self.network_busy, egui::Button::new("Search"))
                            .clicked()
                        {
                            self.start_search(ctx);
                        }
                    });
                    let mut selected = None;
                    for place in &self.places {
                        if ui.button(place.label()).clicked() {
                            selected = Some(place.clone());
                        }
                    }
                    if let Some(place) = selected {
                        match self.db.put_setting("location", &place) {
                            Ok(()) => {
                                self.location = Some(place);
                                self.places.clear();
                                self.forecast = None;
                                self.start_forecast(ctx);
                            }
                            Err(e) => self.error(e),
                        }
                    }
                    if let Some(place) = &self.location {
                        ui.label(RichText::new(place.label()).strong());
                        ui.small(format!("Times are in {}", place.timezone));
                        if ui
                            .add_enabled(!self.network_busy, egui::Button::new("Refresh forecast"))
                            .clicked()
                        {
                            self.start_forecast(ctx);
                        }
                    } else {
                        ui.small("Choose a location to fetch its next seven days of weather.");
                    }
                    if self.network_busy {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Getting weather information…");
                        });
                    }
                    if let Some(forecast) = &self.forecast {
                        if !forecast.fresh() {
                            ui.colored_label(
                                AMBER,
                                "Cached forecast is over 3 hours old. Refresh before generating.",
                            );
                        }
                        if let (Ok(start), Ok(end)) = (
                            parse_date(&self.draft.start_date, &self.draft.start_time),
                            parse_date(&self.draft.end_date, &self.draft.end_time),
                        ) {
                            match forecast.window(start, end) {
                                Ok((weather, _)) => {
                                    weather_label(ui, &weather);
                                }
                                Err(e) => {
                                    ui.small(e.to_string());
                                }
                            }
                        }
                    }
                    ui.hyperlink_to("Weather: Open-Meteo · CC BY 4.0", "https://open-meteo.com/");
                }
            });
        });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            let label = if self.generating {
                "Finding combinations…"
            } else {
                "Generate outfits"
            };
            if primary(ui, label, !self.generating).clicked() {
                self.shuffle = 0;
                self.generate(ctx, None);
            }
            if ui
                .add_enabled(
                    !self.generating && !self.outfits.is_empty(),
                    egui::Button::new("Other options"),
                )
                .clicked()
            {
                self.shuffle += 1;
                self.generate(ctx, None);
            }
            if self.generating {
                ui.spinner();
            }
            if !self.draft.locked.is_empty() && ui.button("Clear locks").clicked() {
                self.draft.locked.clear();
            }
            if !self.draft.excluded.is_empty() && ui.button("Clear exclusions").clicked() {
                self.draft.excluded.clear();
            }
            if ui.button("Choose a piece to start with").clicked() {
                self.page = Page::Wardrobe;
            }
        });
        let lock_names = self
            .draft
            .locked
            .iter()
            .filter_map(|id| {
                self.garments
                    .iter()
                    .find(|g| &g.id == id)
                    .map(|g| g.name.clone())
            })
            .collect::<Vec<_>>();
        if !lock_names.is_empty() {
            ui.label(RichText::new(format!("Locked: {}", lock_names.join(" · "))).color(GREEN));
        }
        if !self.draft.excluded.is_empty() {
            ui.small(format!(
                "{} item(s) excluded from these suggestions",
                self.draft.excluded.len()
            ));
        }
        ui.add_space(12.0);
        if self.outfits.is_empty() {
            card().show(ui, |ui| {
                ui.add_space(15.0); section(ui,"LESS GUESSWORK. MORE GOOD COMBINATIONS.");
                ui.label(if self.garments.is_empty() { "Start with a few tops, bottoms and shoes. Photos are optional; you can add details as you go." } else { "Set your outing above, then generate up to three combinations. Every result includes its reasoning and any comfort gaps." });
                ui.add_space(8.0);
                if ui.button("+ Add clothes").clicked() { self.page = Page::Wardrobe; self.new_editor(); }
                ui.add_space(15.0);
            });
        } else {
            let outfits = self.outfits.clone();
            if let Some(first) = outfits.first() {
                ui.small(format!(
                    "Suggestions for {} → {} · {} · {}",
                    first.plan.start.format("%d %b %H:%M"),
                    first.plan.end.format("%d %b %H:%M"),
                    first.plan.occasion,
                    first.plan.location
                ));
            }
            ui.columns(outfits.len(), |columns| {
                for (i, outfit) in outfits.iter().enumerate() {
                    self.outfit_card(&mut columns[i], ctx, outfit, Some(i + 1));
                }
            });
            if !self.hourly.is_empty() {
                ui.collapsing("See the hourly conditions used", |ui| {
                    egui::Grid::new("hourly").striped(true).show(ui, |ui| {
                        for title in [
                            "Local time",
                            "Actual",
                            "Feels like",
                            "Rain chance",
                            "Precipitation",
                            "Wind",
                        ] {
                            ui.strong(title);
                        }
                        ui.end_row();
                        for h in &self.hourly {
                            ui.label(h.time.format("%d %b %H:%M").to_string());
                            ui.label(format!("{:.1} °C", h.temperature));
                            ui.label(format!("{:.1} °C", h.feels_like));
                            ui.label(format!("{:.0}%", h.rain_probability));
                            ui.label(format!("{:.1} mm", h.precipitation_mm));
                            ui.label(format!("{:.0} km/h", h.wind_kmh));
                            ui.end_row();
                        }
                    });
                });
            }
        }
    }

    fn outfit_card(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        outfit: &Outfit,
        number: Option<usize>,
    ) {
        ui.push_id(&outfit.id, |ui| {
            card().show(ui, |ui| {
                ui.set_min_width(ui.available_width().min(200.0));
                ui.horizontal(|ui| {
                    ui.label(RichText::new(number.map(|n|format!("OPTION {n:02}")).unwrap_or_else(||outfit.plan.occasion.label().to_uppercase())).size(12.0).color(GREEN).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center),|ui| { ui.label(RichText::new(format!("{:.0} match",outfit.score)).strong()).on_hover_text("A relative ranking from your current rules, not a confidence percentage."); });
                });
                ui.add_space(6.0);
                for item in &outfit.items {
                    ui.push_id(&item.id,|ui| {
                        ui.horizontal(|ui| {
                            self.thumbnail(ui,item,Vec2::splat(51.0));
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&item.name).strong());
                                ui.label(RichText::new(format!("{} · {}",item.category,item.color)).size(12.0).color(MUTED));
                            });
                        });
                        ui.horizontal(|ui| {
                            let locked = self.draft.locked.contains(&item.id);
                            if ui.small_button(if locked { "Unlock" } else { "Lock" }).clicked() { self.toggle_lock(item); }
                            if ui.add_enabled(!self.generating,egui::Button::new("Swap").small()).on_hover_text("Keep the other pieces and find alternatives for this one.").clicked() {
                                let mut plan = outfit.plan.clone(); plan.locked = outfit.items.iter().filter(|g|g.id != item.id).map(|g|g.id.clone()).collect();
                                plan.excluded.push(item.id.clone()); plan.shuffle = self.shuffle+1;
                                self.draft.locked = plan.locked.clone(); self.draft.excluded = plan.excluded.clone();
                                self.generate(ctx,Some(plan)); self.page = Page::Today;
                            }
                            if ui.small_button("Exclude").on_hover_text("Leave this piece out until exclusions are cleared.").clicked() {
                                if !self.draft.excluded.contains(&item.id) { self.draft.excluded.push(item.id.clone()); }
                                self.draft.locked.retain(|id| id != &item.id);
                            }
                        });
                        ui.add_space(3.0);
                    });
                }
                ui.separator();
                for reason in &outfit.reasons { ui.label(RichText::new(reason).size(13.0).color(MUTED)); }
                for warning in &outfit.warnings { ui.label(RichText::new(format!("! {warning}")).size(13.0).color(AMBER)); }
                ui.add_space(5.0);
                let favourite = self.saved.iter().any(|o|o.favourite&&o.outfit.signature==outfit.signature);
                ui.horizontal_wrapped(|ui| {
                    if ui.button(if favourite { "★ Saved" } else { "☆ Favourite" }).clicked() {
                        match self.db.save_outfit(outfit,!favourite).and_then(|_|self.reload()) { Ok(()) => self.notice(if favourite {"Removed from favourites."} else {"Outfit saved to favourites."}), Err(e) => self.error(e) }
                    }
                    if ui.button("Wear this").clicked() {
                        self.wear_dialog = Some((outfit.clone(),Local::now().format("%Y-%m-%d %H:%M").to_string(),String::new()));
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button("Like this").clicked() { self.store_feedback(outfit,1,"Love it"); }
                    if ui.small_button("Give feedback").clicked() { self.feedback_dialog = Some((outfit.clone(),"Not my style".into())); }
                });
                ui.collapsing("Why this score?",|ui| {
                    let s=&outfit.breakdown;
                    for (name,value) in [("Weather",s.weather),("Occasion",s.occasion),("Style",s.style),("Preferences",s.preference),("Variety",s.history)] { ui.label(format!("{name}: {value:.0} / 100")); }
                    ui.small(format!("Rules {} · settings revision {}",outfit.algorithm,outfit.profile.revision));
                    ui.small("Style starts with simple colour and pattern rules, your item ratings, and combination feedback. Guide images are reference material until you translate them into tags and preferences.");
                });
            });
        });
    }

    fn store_feedback(&mut self, outfit: &Outfit, value: i32, reason: &str) {
        match self
            .db
            .feedback(outfit, value, reason)
            .and_then(|_| self.reload())
        {
            Ok(()) => {
                self.epoch += 1;
                self.notice(
                    "Feedback saved. Generate again to use it; your previous feedback is retained.",
                );
            }
            Err(e) => self.error(e),
        }
    }

    fn wardrobe(&mut self, ui: &mut egui::Ui, favourites_only: bool) {
        heading(
            ui,
            if favourites_only {
                "Your favourite pieces"
            } else {
                "Your wardrobe"
            },
            "A searchable home for everything you wear.",
        );
        ui.horizontal_wrapped(|ui| {
            if primary(ui, "+ Add item", true).clicked() {
                self.new_editor();
            }
            if ui
                .button("Add photos in bulk")
                .on_hover_text("Select several photos and review each item before saving.")
                .clicked()
                && let Some(paths) = rfd::FileDialog::new()
                    .add_filter("Clothing photos", &["jpg", "jpeg", "png", "webp"])
                    .pick_files()
            {
                let mut queue: VecDeque<_> = paths.into();
                if let Some(path) = queue.pop_front() {
                    self.editor_from_photo(path, queue);
                }
            }
            let changed = ui
                .add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("Search name, colour, fabric, tags…")
                        .desired_width(300.0),
                )
                .changed();
            if changed {
                match self.db.search(&self.query) {
                    Ok(items) => self.search_results = items,
                    Err(e) => self.error(e),
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("category-filter")
                .selected_text(
                    self.category_filter
                        .map(|v| v.label())
                        .unwrap_or("All categories"),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.category_filter, None, "All categories");
                    for c in Category::ALL {
                        ui.selectable_value(&mut self.category_filter, Some(*c), c.label());
                    }
                });
            egui::ComboBox::from_id_salt("status-filter")
                .selected_text(
                    self.status_filter
                        .map(|v| v.label())
                        .unwrap_or("Any availability"),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.status_filter, None, "Any availability");
                    for c in Availability::ALL {
                        ui.selectable_value(&mut self.status_filter, Some(*c), c.label());
                    }
                });
            ui.checkbox(&mut self.rain_filter, "Waterproof");
            if !favourites_only {
                ui.checkbox(&mut self.favourite_filter, "Favourites");
            }
            ui.checkbox(&mut self.show_archived, "Include archived");
            if ui.small_button("Reset filters").clicked() {
                self.query.clear();
                self.search_results = self.garments.clone();
                self.category_filter = None;
                self.status_filter = None;
                self.rain_filter = false;
                self.favourite_filter = false;
                self.show_archived = false;
            }
        });
        let items: Vec<_> = self
            .search_results
            .iter()
            .filter(|g| {
                (!g.archived || self.show_archived)
                    && self.category_filter.is_none_or(|c| g.category == c)
                    && self.status_filter.is_none_or(|s| g.availability == s)
                    && (!self.rain_filter || g.rain == RainProtection::Waterproof)
                    && (!(favourites_only || self.favourite_filter) || g.favourite)
            })
            .cloned()
            .collect();
        ui.label(
            RichText::new(format!("{} matching pieces", items.len()))
                .size(12.0)
                .color(MUTED),
        );
        ui.add_space(8.0);
        if items.is_empty() {
            card().show(ui,|ui| { ui.label(if self.garments.is_empty() {"Your wardrobe is ready for its first piece. Start with a name and category; fill in the rest when convenient."} else {"No clothes match these filters. Try a broader search."}); });
        }
        let cols = (ui.available_width() / 245.0).floor().clamp(2.0, 4.0) as usize;
        for chunk in items.chunks(cols) {
            ui.columns(cols, |columns| {
                for (i, item) in chunk.iter().enumerate() {
                    columns[i].push_id(&item.id, |ui| {
                        card().show(ui, |ui| {
                            self.thumbnail(ui, item, Vec2::new(ui.available_width(), 135.0));
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&item.name).strong());
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui
                                        .small_button(if item.favourite { "★" } else { "☆" })
                                        .clicked()
                                    {
                                        let mut changed = item.clone();
                                        changed.favourite = !changed.favourite;
                                        self.save_item(
                                            &changed,
                                            self.affinities.get(&item.id).copied().unwrap_or(2.5),
                                        );
                                    }
                                });
                            });
                            ui.label(
                                RichText::new(format!("{} · {}", item.category, item.color))
                                    .size(12.0)
                                    .color(MUTED),
                            );
                            ui.label(
                                RichText::new(format!(
                                    "{}{}",
                                    item.rain,
                                    if item.archived { " · Archived" } else { "" }
                                ))
                                .size(12.0)
                                .color(
                                    if item.rain == RainProtection::Waterproof {
                                        GREEN
                                    } else {
                                        MUTED
                                    },
                                ),
                            );
                            let mut status = item.availability;
                            choice(
                                ui,
                                "quick-availability",
                                &mut status,
                                Availability::ALL,
                                |v| v.label(),
                            );
                            if status != item.availability {
                                let mut changed = item.clone();
                                changed.availability = status;
                                self.save_item(
                                    &changed,
                                    self.affinities.get(&item.id).copied().unwrap_or(2.5),
                                );
                            }
                            ui.horizontal(|ui| {
                                if ui.small_button("Edit").clicked() {
                                    self.editor = Some(Editor::new(
                                        item.clone(),
                                        self.affinities.get(&item.id).copied().unwrap_or(2.5),
                                        VecDeque::new(),
                                    ));
                                }
                                if ui
                                    .add_enabled(
                                        !item.archived && item.availability == Availability::Clean,
                                        egui::Button::new(
                                            if self.draft.locked.contains(&item.id) {
                                                "Unlock"
                                            } else {
                                                "Build around"
                                            },
                                        )
                                        .small(),
                                    )
                                    .clicked()
                                {
                                    self.toggle_lock(item);
                                    self.page = Page::Today;
                                }
                            });
                        });
                    });
                }
            });
            ui.add_space(6.0);
        }
    }

    fn favourites(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let outfits = self
            .saved
            .iter()
            .filter(|o| o.favourite)
            .map(|o| o.outfit.clone())
            .collect::<Vec<_>>();
        heading(
            ui,
            "Outfits worth keeping",
            "Saved combinations retain their original plan and score. Rebuild them for a new outing.",
        );
        if outfits.is_empty() {
            card().show(ui, |ui| {
                ui.label("Favourite a suggestion to keep the whole combination here.");
            });
        }
        for chunk in outfits.chunks(3) {
            ui.columns(3,|columns| {
                for (i,outfit) in chunk.iter().enumerate() {
                    let missing=outfit.items.iter().any(|g|self.garments.iter().find(|i|i.id==g.id).is_none_or(|i|i.archived||i.availability!=Availability::Clean));
                    if missing { columns[i].colored_label(AMBER,"Some pieces are currently unavailable."); }
                    if columns[i].button("Use these pieces for my next outing").clicked() {
                        self.draft.locked=outfit.items.iter().map(|g|g.id.clone()).collect(); self.draft.excluded.clear(); self.page=Page::Today; self.outfits.clear();
                        self.notice("Pieces locked. Update your outing and generate again to check the new weather.");
                    }
                    self.outfit_card(&mut columns[i],ctx,outfit,None);
                }
            });
            ui.add_space(10.0);
        }
        ui.add_space(15.0);
        self.wardrobe(ui, true);
    }

    fn history(&mut self, ui: &mut egui::Ui) {
        heading(
            ui,
            "What you wore",
            "Wear history helps make the next suggestion feel fresh.",
        );
        ui.add(
            egui::TextEdit::singleline(&mut self.history_query)
                .hint_text("Find an item, occasion or note…")
                .desired_width(350.0),
        );
        let needle = self.history_query.to_lowercase();
        let entries = self
            .wear
            .iter()
            .filter(|w| {
                format!(
                    "{} {} {}",
                    w.notes,
                    w.outfit.plan.occasion,
                    w.outfit
                        .items
                        .iter()
                        .map(|g| g.search_text())
                        .collect::<Vec<_>>()
                        .join(" ")
                )
                .to_lowercase()
                .contains(&needle)
            })
            .cloned()
            .collect::<Vec<_>>();
        if entries.is_empty() {
            card().show(ui,|ui| { ui.label("Choose 'Wear this' on an outfit to record a wear. Planning an outfit does not automatically mark it worn."); });
        }
        for entry in entries {
            ui.push_id(&entry.id, |ui| {
                card().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.strong(entry.worn_at.format("%a %d %b %Y · %H:%M").to_string());
                        ui.label(entry.outfit.plan.occasion.label());
                    });
                    ui.horizontal_wrapped(|ui| {
                        for item in &entry.outfit.items {
                            self.thumbnail(ui, item, Vec2::splat(60.0));
                            ui.label(&item.name);
                        }
                    });
                    if !entry.notes.is_empty() {
                        ui.label(&entry.notes);
                    }
                    ui.small(format!(
                        "Settings revision {} · {}",
                        entry.outfit.profile.revision, entry.outfit.algorithm
                    ));
                    if ui.small_button("Build around these pieces").clicked() {
                        self.draft.locked =
                            entry.outfit.items.iter().map(|g| g.id.clone()).collect();
                        self.draft.excluded.clear();
                        self.page = Page::Today;
                        self.outfits.clear();
                    }
                });
                ui.add_space(7.0);
            });
        }
        ui.add_space(20.0);
        ui.collapsing(
            format!("Feedback history · {} entries", self.feedback.len()),
            |ui| {
                for f in self.feedback.iter().rev() {
                    ui.label(format!(
                        "{} · {} · {}",
                        f.created_at,
                        if f.value > 0 {
                            "Liked"
                        } else if f.value < 0 {
                            "Disliked"
                        } else {
                            "Reset"
                        },
                        f.reason
                    ));
                }
            },
        );
    }

    fn preferences(&mut self, ui: &mut egui::Ui) {
        heading(
            ui,
            "Make it feel like you",
            "Start broad. Adjust your preferences as you discover what works.",
        );
        ui.small("Suggestions use saved preferences. Save your edits to make them active.");
        card().show(ui, |ui| self.photo_settings(ui));
        ui.add_space(12.0);
        card().show(ui,|ui| {
            section(ui,"COMFORT & PERSONAL TASTE");
            ui.label("Do you tend to feel cold or hot? Positive values ask for warmer clothes.");
            ui.add(egui::Slider::new(&mut self.preferences.thermal_adjustment,-5.0..=5.0).step_by(0.5).text("°C comfort adjustment"));
            ui.add(egui::Slider::new(&mut self.preferences.repeat_days,0..=60).text("Days before repeating a piece"));
            ui.small("Shoes, coats and accessories receive a smaller repeat penalty. Zero turns repeat avoidance off.");
            egui::Grid::new("taste-fields").num_columns(2).spacing([18.0,9.0]).show(ui,|ui| {
                edit_row(ui,"Preferred colours",&mut self.preferences.preferred_colors,"navy, cream, olive");
                edit_row(ui,"Colours to avoid",&mut self.preferences.avoided_colors,"Optional");
                edit_row(ui,"Preferred fits",&mut self.preferences.preferred_fits,"relaxed, regular");
                edit_row(ui,"Style tags",&mut self.preferences.preferred_styles,"Leave blank while you decide");
            });
            ui.small("Use comma-separated tags. Match the same words on your clothing items.");
        });
        ui.add_space(12.0);
        card().show(ui,|ui| {
            section(ui,"STYLE GUIDE");
            ui.label("Keep reference images and notes here for later. Images and notes are not automatically analysed; use clothing tags, preferences and ratings to influence suggestions.");
            ui.add(egui::TextEdit::multiline(&mut self.preferences.guide_notes).desired_rows(3).desired_width(f32::INFINITY).hint_text("What do you like about these looks? Colours, silhouettes, textures…"));
            if ui.button("+ Add reference images").clicked()
                && let Some(paths)=rfd::FileDialog::new().add_filter("Images",&["png","jpg","jpeg","webp"]).pick_files() {
                    for path in paths { match self.db.import_photo(&path) { Ok(photo)=>self.preferences.guide_images.push(photo),Err(e)=>self.error(e) } }
                }
            let photos=self.preferences.guide_images.clone();
            ui.horizontal_wrapped(|ui| { for file in &photos { ui.vertical(|ui| { self.photo(ui,file,Vec2::splat(110.0)); if ui.small_button("Remove reference").clicked() {self.preferences.guide_images.retain(|f|f!=file);} }); } });
        });
        ui.add_space(12.0);
        card().show(ui,|ui| {
            section(ui,"HOW TO RANK OUTFITS");
            ui.label("Weights are relative. Every save creates a new revision; clothing facts and previous scores stay intact.");
            ui.add(egui::Slider::new(&mut self.profile.weather,0.0..=100.0).text("Weather & comfort"));
            ui.add(egui::Slider::new(&mut self.profile.occasion,0.0..=100.0).text("Occasion"));
            ui.add(egui::Slider::new(&mut self.profile.style,0.0..=100.0).text("Style & feedback"));
            ui.add(egui::Slider::new(&mut self.profile.preference,0.0..=100.0).text("Personal preferences"));
            ui.add(egui::Slider::new(&mut self.profile.history,0.0..=100.0).text("Wear variety"));
            ui.horizontal_wrapped(|ui| {
                if primary(ui,"Save preferences",true).clicked() {
                    match self.db.save_profile(&self.profile,&self.preferences) {
                        Ok(revision)=>{self.profile.revision=revision; self.epoch+=1; self.outfits.clear(); match self.db.profiles() {Ok(p)=>self.profiles=p,Err(e)=>self.error(e)} self.notice(format!("Preferences saved as revision {revision}."));},
                        Err(e)=>self.error(e),
                    }
                }
                if ui.button("Load saved preferences").clicked() && let Some(last)=self.profiles.last() { self.profile=last.profile.clone(); self.preferences=last.preferences.clone(); }
            });
            ui.collapsing(format!("Previous revisions ({})",self.profiles.len()),|ui| {
                let mut restore=None;
                for p in self.profiles.iter().rev() {
                    ui.horizontal(|ui| { ui.label(format!("Revision {} · {}",p.profile.revision,p.created_at)); if ui.small_button("Load").clicked() {restore=Some(p.clone());} });
                }
                if let Some(p)=restore {self.profile=p.profile; self.preferences=p.preferences; self.notice("Revision loaded into the editor. Save preferences to make it the current revision.");}
            });
        });
        ui.add_space(12.0);
        card().show(ui,|ui| {
            section(ui,"YOUR DATA");
            ui.label("Clothes, photos, scoring revisions, favourites and history are stored locally.");
            ui.small(self.db.root.display().to_string());
            ui.horizontal_wrapped(|ui| {
                if ui.button("Export a complete backup").clicked()
                    && let Some(folder)=rfd::FileDialog::new().set_title("Choose a folder for the backup").pick_folder() {
                        match self.db.export_backup(&folder) {Ok(path)=>self.notice(format!("Backup saved to {}",path.display())),Err(e)=>self.error(e)}
                    }
                if ui.button("Restore from backup…").clicked() { self.restore_pending=rfd::FileDialog::new().set_title("Choose a Wardrobe backup folder").pick_folder(); }
            });
            ui.small("Only location searches and weather requests contact Open-Meteo. Photos, style references and wardrobe details stay on this computer.");
        });
    }

    fn new_editor(&mut self) {
        if let Some(editor) = self.editor.as_mut() {
            editor.cancel_photo();
        }
        self.editor = Some(Editor::new(Garment::default(), 2.5, VecDeque::new()));
    }

    fn editor_from_photo(&mut self, path: PathBuf, queue: VecDeque<PathBuf>) {
        match self.db.import_photo(&path) {
            Ok(photo) => {
                let name = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .replace(['_', '-'], " ");
                let mut editor = Editor::new(
                    Garment {
                        name,
                        photo: Some(photo.clone()),
                        original_photo: Some(photo),
                        ..Garment::default()
                    },
                    2.5,
                    queue,
                );
                if self.auto_analyse {
                    self.start_photo_analysis(&mut editor);
                }
                self.editor = Some(editor);
            }
            Err(e) => self.error(e),
        }
    }

    fn editor(&mut self, ctx: &egui::Context) {
        let Some(mut editor) = self.editor.take() else {
            return;
        };
        let mut save = false;
        let mut close = false;
        let mut skip = false;
        egui::Modal::new(egui::Id::new("item-editor")).show(ctx,|ui| {
            ui.set_width(630.0);
            ui.heading(if self.garments.iter().any(|g|g.id==editor.item.id) {"Edit clothing item"} else {"Add clothing item"});
            egui::ScrollArea::vertical().max_height(ctx.content_rect().height()-170.0).show(ui,|ui| {
                ui.horizontal(|ui| {
                    if editor.show_original && let Some(original)=&editor.item.original_photo {self.photo(ui,original,Vec2::new(130.0,110.0));}
                    else {self.thumbnail(ui,&editor.item,Vec2::new(130.0,110.0));}
                    ui.vertical(|ui| {
                        if ui.button("Choose photo…").clicked() && let Some(path)=rfd::FileDialog::new().add_filter("Image",&["png","jpg","jpeg","webp"]).pick_file() {
                            match self.db.import_photo(&path) {
                                Ok(p)=>{editor.cancel_photo();editor.item.photo=Some(p.clone());editor.item.original_photo=Some(p);editor.item.photo_analysis=None;editor.color_reference=None;editor.show_original=false;if self.auto_analyse {self.start_photo_analysis(&mut editor);}},
                                Err(e)=>editor.error=e.to_string()
                            }
                        }
                        if editor.item.photo.is_some()&&ui.small_button("Remove photo").clicked() {editor.cancel_photo();editor.item.photo=None;editor.item.original_photo=None;editor.item.photo_analysis=None;}
                        if editor.item.original_photo.is_some() {ui.checkbox(&mut editor.show_original,"Show original photo");}
                        if editor.item.photo.is_some() && ui.add_enabled(editor.photo_task.is_none(),egui::Button::new("Auto-fill & remove background")).clicked() {self.start_photo_analysis(&mut editor);}
                        if editor.item.photo.is_some() && ui.button("Use a neutral colour reference…").clicked(){editor.pick_reference=true;}
                        if editor.color_reference.is_some() && ui.small_button("Remove colour reference").clicked(){editor.color_reference=None;editor.photo_progress="Reference removed. Analyse again to use original colour balance.".into();}
                        ui.small("Originals are kept. Generated details are editable.");
                    });
                });
                if !editor.photo_progress.is_empty(){ui.label(&editor.photo_progress);}
                if editor.photo_task.is_some(){ui.horizontal(|ui|{ui.spinner();if ui.small_button("Skip automation").clicked(){editor.cancel_photo();}});}
                if !self.vision.runtime.as_ref().is_some_and(|r|r.ready()) || self.setup_job.is_some() {ui.collapsing("Set up local photo automation",|ui|self.photo_settings(ui));}
                egui::Grid::new("item-main-fields").num_columns(2).spacing([20.0,9.0]).show(ui,|ui| {
                    edit_row(ui,"Name *",&mut editor.item.name,"e.g. Navy linen shirt");
                    ui.label("Category"); choice(ui,"edit-category",&mut editor.item.category,Category::ALL,|v|v.label()); ui.end_row();
                    edit_row(ui,"Colour(s)",&mut editor.item.color,"navy, white");
                    ui.label("Availability"); choice(ui,"edit-availability",&mut editor.item.availability,Availability::ALL,|v|v.label()); ui.end_row();
                    ui.label("Rain protection"); choice(ui,"edit-rain",&mut editor.item.rain,RainProtection::ALL,|v|v.label()); ui.end_row();
                    ui.label("Warmth"); ui.add(egui::Slider::new(&mut editor.item.warmth,0.0..=5.0).step_by(0.5).text("0 light → 5 insulating")); ui.end_row();
                    ui.label("Formality"); ui.add(egui::Slider::new(&mut editor.item.formality,1.0..=5.0).step_by(0.5).text("1 casual → 5 formal")); ui.end_row();
                    ui.label("My style rating"); ui.add(egui::Slider::new(&mut editor.affinity,0.0..=5.0).step_by(0.5).text("2.5 undecided")); ui.end_row();
                    ui.label("Favourites"); ui.checkbox(&mut editor.item.favourite,"Favourite this piece"); ui.end_row();
                    ui.label("Wind protection"); ui.checkbox(&mut editor.item.windproof,"Windproof"); ui.end_row();
                    if editor.item.category==Category::Shoes {ui.label("Walking comfort"); ui.add(egui::Slider::new(&mut editor.item.walking_comfort,1.0..=5.0).text("1 poor → 5 excellent")); ui.end_row();}
                });
                ui.label("Brief description for outfit matching");
                ui.add(egui::TextEdit::multiline(&mut editor.item.description).desired_rows(3).desired_width(f32::INFINITY).hint_text("Automatically describes fabric appearance, fit, motifs, embroidery and other visible details."));
                let mut details=editor.item.details.join("\n");
                ui.label("Visible details · one per line");
                if ui.add(egui::TextEdit::multiline(&mut details).desired_rows(2).desired_width(f32::INFINITY)).changed(){editor.item.details=details.lines().map(str::trim).filter(|v|!v.is_empty()).take(32).map(str::to_string).collect();}
                if let Some(info)=&editor.item.photo_analysis {
                    ui.collapsing("Photo analysis notes",|ui|{
                        ui.small(format!("{} · {}",info.model,info.generated_at));
                        if !info.recognition_backend.is_empty(){ui.small(format!("Recognition: {} · {}",info.recognition_backend,info.device));ui.small(format!("Background removal: {} · {}",info.segmentation_backend,info.runtime_version));}
                        for reason in &info.fallback_reasons{ui.colored_label(AMBER,format!("CPU fallback: {reason}"));}
                        ui.small("Warmth, formality, seasons and material appearance are estimates. Check them against the actual item.");
                        for uncertainty in &info.uncertainties{ui.small(uncertainty);}
                    });
                }
                ui.collapsing("More details · all optional",|ui| {
                    egui::Grid::new("item-extra-fields").num_columns(2).spacing([20.0,9.0]).show(ui,|ui| {
                        edit_row(ui,"Brand",&mut editor.item.brand,""); edit_row(ui,"Size",&mut editor.item.size,"");
                        edit_row(ui,"Fabric",&mut editor.item.fabric,"linen, cotton"); edit_row(ui,"Pattern",&mut editor.item.pattern,"plain, striped…");
                        edit_row(ui,"Fit",&mut editor.item.fit,"regular, relaxed…"); edit_row(ui,"Seasons",&mut editor.item.seasons,"summer, autumn…");
                        edit_row(ui,"Style tags",&mut editor.item.styles,"minimal, classic, active…");
                    });
                    ui.label("Notes"); ui.add(egui::TextEdit::multiline(&mut editor.item.notes).desired_rows(2).desired_width(f32::INFINITY));
                    ui.checkbox(&mut editor.item.archived,"Archive this piece (preserve its history)");
                });
                ui.small("Style ratings are stored as revisions, separately from clothing facts. Seasons are searchable labels; warmth controls weather suitability.");
            });
            if !editor.error.is_empty() {ui.colored_label(Color32::DARK_RED,&editor.error);}
            ui.separator();
            ui.horizontal(|ui| {
                if primary(ui,if editor.queue.is_empty() {"Save item"} else {"Save & next photo"},editor.photo_task.is_none()).clicked() {save=true;}
                if editor.queue.is_empty() {ui.checkbox(&mut editor.another,"Add another");}
                else {ui.label(format!("{} photos remaining",editor.queue.len())); if ui.button("Skip photo").clicked() {skip=true;}}
                if ui.button("Cancel").clicked() {close=true;}
            });
        });
        if editor.pick_reference {
            self.reference_picker(ctx, &mut editor);
        }
        if close {
            editor.cancel_photo();
            return;
        }
        if save {
            editor.item.name = editor.item.name.trim().to_string();
            match self
                .db
                .save_garment(&editor.item, editor.affinity)
                .and_then(|_| self.reload())
            {
                Ok(()) => {
                    self.epoch += 1;
                    self.outfits.clear();
                    self.notice(format!("{} saved.", editor.item.name));
                    if let Some(path) = editor.queue.pop_front() {
                        self.editor_from_photo(path, editor.queue);
                    } else if editor.another {
                        let mut item = editor.item.clone();
                        item.id = uuid::Uuid::new_v4().to_string();
                        item.name.clear();
                        item.photo = None;
                        item.original_photo = None;
                        item.photo_analysis = None;
                        item.description.clear();
                        item.details.clear();
                        item.archived = false;
                        let mut next = Editor::new(item, editor.affinity, VecDeque::new());
                        next.another = true;
                        self.editor = Some(next);
                    }
                    return;
                }
                Err(e) => editor.error = e.to_string(),
            }
        }
        if skip {
            editor.cancel_photo();
            if let Some(path) = editor.queue.pop_front() {
                self.editor_from_photo(path, editor.queue);
            }
            return;
        }
        self.editor = Some(editor);
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some((outfit, mut reason)) = self.feedback_dialog.take() {
            let mut done = false;
            egui::Modal::new(egui::Id::new("feedback-dialog")).show(ctx,|ui| {
                ui.set_width(380.0); ui.heading("What would you change?");
                for value in ["Not my style","Colours don't work","Too warm","Too cold","Too formal","Too casual","Uncomfortable"] {ui.radio_value(&mut reason,value.to_string(),value);}
                ui.small("Your latest feedback for a combination guides future suggestions. All previous feedback is retained.");
                ui.horizontal(|ui| {
                    if primary(ui,"Save feedback",true).clicked() {
                        self.store_feedback(&outfit,-1,&reason); done=true;
                    }
                    if ui.button("Cancel").clicked() { done=true; }
                });
            });
            if !done {
                self.feedback_dialog = Some((outfit, reason));
            }
        }
        if let Some((outfit, mut at, mut notes)) = self.wear_dialog.take() {
            let mut done = false;
            egui::Modal::new(egui::Id::new("wear-dialog")).show(ctx,|ui| {
                ui.set_width(420.0); ui.heading("Record a wear");
                ui.label("When did you wear this? Local date and time.");
                ui.add(egui::TextEdit::singleline(&mut at).hint_text("YYYY-MM-DD HH:MM"));
                ui.add(egui::TextEdit::multiline(&mut notes).desired_rows(2).hint_text("Optional notes"));
                ui.small("This records history. Set clothing to 'In the wash' from My wardrobe when needed.");
                ui.horizontal(|ui| {
                    if primary(ui,"Record wear",true).clicked() {
                        match NaiveDateTime::parse_from_str(at.trim(),"%Y-%m-%d %H:%M") {
                            Ok(time) if time <= Local::now().naive_local() => {
                                match self.db.record_wear(&outfit,time,&notes).and_then(|_|self.reload()) {Ok(())=>{self.epoch+=1;self.notice("Wear recorded. Future suggestions will account for it.");done=true;},Err(e)=>self.error(e)}
                            }, Ok(_)=>self.error("Record actual wears in the past or present; future outings can be saved as favourites."),
                            Err(_)=>self.error("Use YYYY-MM-DD HH:MM, for example 2026-11-04 09:00."),
                        }
                    }
                    if ui.button("Cancel").clicked() {done=true;}
                });
            });
            if !done {
                self.wear_dialog = Some((outfit, at, notes));
            }
        }
        if let Some(folder) = self.restore_pending.take() {
            let mut done = false;
            egui::Modal::new(egui::Id::new("restore-dialog")).show(ctx,|ui| {
                ui.set_width(460.0); ui.heading("Restore this wardrobe?");
                ui.label(folder.display().to_string());
                ui.label("This replaces your current clothes, favourites, preferences and history with the backup. A recovery backup of the current wardrobe is saved first.");
                ui.horizontal(|ui| {
                    if ui.button("Restore backup").clicked() {
                        match self.db.restore_backup(&folder).and_then(|_|self.reload()) {
                            Ok(())=>{
                                match self.db.profiles() {Ok(p)=>{self.profiles=p;if let Some(p)=self.profiles.last(){self.profile=p.profile.clone();self.preferences=p.preferences.clone();}},Err(e)=>self.error(e)}
                                self.epoch+=1;self.outfits.clear();self.draft=Draft::default();self.location=None;self.forecast=None;
                                self.notice("Backup restored. A recovery copy of the previous wardrobe is in the recovery folder.");done=true;
                            },Err(e)=>self.error(e),
                        }
                    }
                    if ui.button("Cancel").clicked(){done=true;}
                });
            });
            if !done {
                self.restore_pending = Some(folder);
            }
        }
    }

    fn reference_picker(&self, ctx: &egui::Context, editor: &mut Editor) {
        egui::Modal::new(egui::Id::new("neutral-reference")).show(ctx,|ui|{
            ui.set_width(560.0);ui.heading("Choose a neutral reference");
            ui.label("Click the plain centre of a photographic neutral grey or white-balance card beside the garment, under the same light.");
            ui.small("A4 paper is a rough fallback. Avoid shadows, glare and pure-white overexposed patches. Only select a surface you know is neutral; the app does not guess from the garment.");
            let file=editor.item.original_photo.as_ref().or(editor.item.photo.as_ref());
            if let Some(file)=file && let Ok(path)=self.db.photo_path(file) {
                let uri=format!("file://{}",path.to_string_lossy().replace('\\',"/"));
                let response=ui.add(egui::Image::new(uri).fit_to_exact_size(Vec2::new(530.0,350.0)).sense(egui::Sense::click()));
                if response.clicked() && let Some(point)=response.interact_pointer_pos(){
                    let point=(point-response.rect.min)/response.rect.size();
                    editor.color_reference=Some(ColorReference{x:point.x.clamp(0.0,1.0),y:point.y.clamp(0.0,1.0),radius:0.025});
                }
                if let Some(reference)=editor.color_reference {
                    let center=response.rect.min+response.rect.size()*Vec2::new(reference.x,reference.y);
                    let rect=egui::Rect::from_center_size(center,response.rect.size()*reference.radius*2.0);
                    ui.painter().rect_stroke(rect,0,Stroke::new(2.0_f32,GREEN),egui::StrokeKind::Inside);
                    ui.small("Selected patch will neutralise the colour cast before recognition and background removal.");
                }
            }
            ui.horizontal(|ui|{
                if primary(ui,"Use reference & analyse",editor.color_reference.is_some()).clicked(){editor.pick_reference=false;self.start_photo_analysis(editor);}
                if ui.button("Back").clicked(){editor.pick_reference=false;}
            });
        });
    }

    fn photo(&self, ui: &mut egui::Ui, file: &str, size: Vec2) {
        if let Ok(path) = self.db.photo_path(file) {
            let uri = format!("file://{}", path.to_string_lossy().replace('\\', "/"));
            ui.add(
                egui::Image::new(uri)
                    .fit_to_exact_size(size)
                    .corner_radius(7),
            );
        }
    }
    fn thumbnail(&self, ui: &mut egui::Ui, item: &Garment, size: Vec2) {
        if let Some(file) = &item.photo {
            self.photo(ui, file, size);
            return;
        }
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, 8, Color32::from_rgb(238, 239, 230));
        let c = rect.center();
        let unit = size.x.min(size.y) * 0.3;
        let color = swatch(&item.color);
        let points: Vec<[f32; 2]> = match item.category {
            Category::Top | Category::Layer | Category::Outerwear => vec![
                [-0.4, -0.9],
                [-1.0, -0.5],
                [-0.7, 0.0],
                [-0.5, -0.15],
                [-0.5, 0.9],
                [0.5, 0.9],
                [0.5, -0.15],
                [0.7, 0.0],
                [1.0, -0.5],
                [0.4, -0.9],
                [0.2, -0.65],
                [-0.2, -0.65],
            ],
            Category::Bottom => vec![
                [-0.65, -0.9],
                [0.65, -0.9],
                [0.8, 0.9],
                [0.15, 0.9],
                [0.0, 0.0],
                [-0.15, 0.9],
                [-0.8, 0.9],
            ],
            Category::OnePiece => vec![
                [-0.4, -0.9],
                [0.4, -0.9],
                [0.3, -0.1],
                [0.9, 0.9],
                [-0.9, 0.9],
                [-0.3, -0.1],
            ],
            Category::Shoes => vec![
                [-0.9, -0.2],
                [-0.2, -0.4],
                [0.1, 0.0],
                [0.85, 0.3],
                [0.9, 0.6],
                [-0.9, 0.6],
            ],
            Category::Accessory => vec![[-0.7, -0.7], [0.7, -0.7], [0.7, 0.7], [-0.7, 0.7]],
        };
        let points = points
            .into_iter()
            .map(|p| c + Vec2::new(p[0] * unit, p[1] * unit))
            .collect();
        // Concave silhouettes are outlined rather than filled, so they render reliably.
        ui.painter().add(egui::Shape::closed_line(
            points,
            Stroke::new(2.5_f32, color),
        ));
    }
}

impl WardrobeApp {
    fn draw(&mut self, ctx: &egui::Context) {
        self.poll();
        if self.generating
            || self.network_busy
            || self.setup_job.is_some()
            || self
                .editor
                .as_ref()
                .is_some_and(|editor| editor.photo_task.is_some())
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.sidebar(ctx);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(PAPER).inner_margin(26))
            .show(ctx, |ui| {
                if let Some((message, error)) = self.message.clone() {
                    egui::Frame::new()
                        .fill(if error {
                            Color32::from_rgb(254, 237, 229)
                        } else {
                            Color32::from_rgb(224, 237, 219)
                        })
                        .corner_radius(7)
                        .inner_margin(10)
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(message).color(if error {
                                    Color32::DARK_RED
                                } else {
                                    GREEN
                                }));
                                if ui.small_button("Dismiss").clicked() {
                                    self.message = None;
                                }
                            });
                        });
                    ui.add_space(9.0);
                }
                egui::ScrollArea::vertical()
                    .id_salt("main-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        match self.page {
                            Page::Today => self.today(ui, ctx),
                            Page::Wardrobe => self.wardrobe(ui, false),
                            Page::Favourites => self.favourites(ui, ctx),
                            Page::History => self.history(ui),
                            Page::Preferences => self.preferences(ui),
                        }
                        ui.add_space(20.0);
                    });
            });
        self.editor(ctx);
        self.dialogs(ctx);
    }
}

impl eframe::App for WardrobeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.draw(ctx);
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let _ = self.db.put_setting("draft", &self.draft);
    }
}

fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(Color32::WHITE)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(12)
        .inner_margin(16)
}
fn heading(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.heading(title);
    ui.label(RichText::new(subtitle).color(MUTED));
    ui.add_space(17.0);
}
fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).size(12.0).strong().color(GREEN));
    ui.add_space(4.0);
}
fn primary(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(label).color(Color32::WHITE).strong()).fill(GREEN),
    )
}
fn choice<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut T,
    all: &[T],
    label: impl Fn(T) -> &'static str,
) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(label(*value))
        .show_ui(ui, |ui| {
            for v in all {
                ui.selectable_value(value, *v, label(*v));
            }
        });
}
fn edit_row(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str) {
    ui.label(label);
    ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(300.0)
            .hint_text(hint),
    );
    ui.end_row();
}
fn parse_date(date: &str, time: &str) -> Result<NaiveDateTime, chrono::ParseError> {
    NaiveDateTime::parse_from_str(
        &format!("{}T{}", date.trim(), time.trim()),
        "%Y-%m-%dT%H:%M",
    )
}
fn weather_label(ui: &mut egui::Ui, w: &WeatherSummary) {
    ui.label(
        RichText::new(format!(
            "{:.0}–{:.0} °C · feels like {:.0}–{:.0} °C",
            w.temperature_min, w.temperature_max, w.feels_min, w.feels_max
        ))
        .strong(),
    );
    ui.small(format!(
        "Up to {:.0}% rain chance · {:.1} mm · wind up to {:.0} km/h",
        w.rain_probability, w.precipitation_mm, w.wind_kmh
    ));
}
fn swatch(color: &str) -> Color32 {
    match tags(color).first().map(String::as_str).unwrap_or("") {
        "black" => Color32::from_rgb(51, 55, 53),
        "white" | "cream" => Color32::from_rgb(174, 169, 149),
        "navy" | "blue" | "denim" => Color32::from_rgb(65, 89, 123),
        "green" | "olive" => Color32::from_rgb(99, 122, 84),
        "red" | "burgundy" => Color32::from_rgb(151, 78, 73),
        "pink" => Color32::from_rgb(175, 114, 137),
        "brown" | "tan" | "beige" => Color32::from_rgb(147, 118, 83),
        "yellow" | "orange" => Color32::from_rgb(179, 144, 63),
        _ => Color32::from_rgb(115, 128, 117),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Harness {
        ctx: egui::Context,
        app: WardrobeApp,
        output: egui::FullOutput,
        _directory: tempfile::TempDir,
        time: f64,
    }
    impl Harness {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let ctx = egui::Context::default();
            configure(&ctx);
            let app = WardrobeApp::open(directory.path().into()).unwrap();
            let mut h = Self {
                ctx,
                app,
                output: egui::FullOutput::default(),
                _directory: directory,
                time: 0.0,
            };
            h.frame(vec![]);
            h.frame(vec![]);
            h
        }
        fn frame(&mut self, events: Vec<egui::Event>) {
            self.time += 0.02;
            self.output = self.ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(1240.0, 840.0),
                    )),
                    time: Some(self.time),
                    events,
                    ..Default::default()
                },
                |ctx| self.app.draw(ctx),
            );
        }
        fn find(&self, label: &str) -> egui::Pos2 {
            fn find_shape(shape: &egui::Shape, label: &str) -> Option<egui::Pos2> {
                match shape {
                    egui::Shape::Text(t) if t.galley.text() == label => {
                        Some(t.pos + t.galley.rect.center().to_vec2())
                    }
                    egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| find_shape(s, label)),
                    _ => None,
                }
            }
            self.output
                .shapes
                .iter()
                .find_map(|s| find_shape(&s.shape, label))
                .unwrap_or_else(|| panic!("Control not found: {label}"))
        }
        fn click(&mut self, label: &str) {
            let pos = self.find(label);
            self.frame(vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ]);
            self.frame(vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ]);
            self.frame(vec![]);
        }
    }

    #[test]
    fn add_edit_search_and_favourite_work_through_the_app_controls() {
        let mut h = Harness::new();
        h.click("My wardrobe");
        h.click("+ Add item");
        h.click("e.g. Navy linen shirt");
        h.frame(vec![egui::Event::Text("QA navy shirt".into())]);
        h.click("Save item");
        assert!(h.app.editor.is_none());
        assert_eq!(h.app.db.garments().unwrap()[0].name, "QA navy shirt");
        h.click("Search name, colour, fabric, tags…");
        h.frame(vec![egui::Event::Text("navy".into())]);
        assert_eq!(h.app.search_results.len(), 1);
        h.click("☆");
        assert!(h.app.db.garments().unwrap()[0].favourite);
        h.click("Edit");
        assert_eq!(h.app.editor.as_ref().unwrap().item.name, "QA navy shirt");
        h.click("Cancel");
        assert!(h.app.editor.is_none());
        h.click("Favourites");
        assert!(h.output.shapes.len() > 10);
    }

    #[test]
    fn manual_generation_uses_saved_revision_and_can_save_feedback_and_history() {
        let mut h = Harness::new();
        for (name, category) in [
            ("Shirt", Category::Top),
            ("Trousers", Category::Bottom),
            ("Shoes", Category::Shoes),
        ] {
            let g = Garment {
                name: name.into(),
                category,
                ..Garment::default()
            };
            h.app.db.save_garment(&g, 2.5).unwrap();
        }
        h.app.reload().unwrap();
        h.app.draft.manual = true;
        // Editing weights is not the same as saving a new revision.
        h.app.profile.style = 99.0;
        h.frame(vec![]);
        h.frame(vec![]);
        h.click("Generate outfits");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while h.app.generating && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
            h.frame(vec![]);
        }
        assert!(!h.app.generating);
        assert_eq!(h.app.outfits.len(), 1);
        assert_eq!(h.app.outfits[0].profile.revision, 1);
        assert_eq!(h.app.outfits[0].profile.style, 15.0);
        // These same handlers are invoked by the suggestion card controls.
        let outfit = h.app.outfits[0].clone();
        h.app.store_feedback(&outfit, 1, "Love it");
        assert_eq!(h.app.db.feedback_history().unwrap().len(), 1);
        h.app.db.save_outfit(&outfit, true).unwrap();
        h.app
            .db
            .record_wear(&outfit, Local::now().naive_local(), "QA wear")
            .unwrap();
        h.app.reload().unwrap();
        for page in [
            Page::Today,
            Page::Wardrobe,
            Page::Favourites,
            Page::History,
            Page::Preferences,
        ] {
            h.app.page = page;
            h.frame(vec![]);
            h.frame(vec![]);
            assert!(!h.output.shapes.is_empty());
        }
        assert_eq!(h.app.saved.len(), 1);
        assert_eq!(h.app.wear.len(), 1);
    }

    #[test]
    fn invalid_clothing_stays_in_editor_without_writing_to_the_database() {
        let mut h = Harness::new();
        h.click("My wardrobe");
        h.click("+ Add item");
        h.click("Save item");
        assert!(h.app.db.garments().unwrap().is_empty());
        assert!(h.app.editor.as_ref().unwrap().error.contains("name"));
    }

    #[test]
    fn forecast_mode_ignores_invalid_fields_left_in_manual_mode() {
        let mut h = Harness::new();
        h.app.draft.manual = false;
        h.app.draft.conditions.feels_min = 100.0;
        let location = Location {
            name: "Sydney".into(),
            admin1: "NSW".into(),
            country: "Australia".into(),
            latitude: -33.87,
            longitude: 151.21,
            timezone: "Australia/Sydney".into(),
        };
        h.app.location = Some(location.clone());
        let start = parse_date(&h.app.draft.start_date, "00:00").unwrap();
        h.app.forecast = Some(Forecast {
            location,
            fetched_at: now(),
            hours: (0..24)
                .map(|n| Hour {
                    time: start + chrono::Duration::hours(n),
                    temperature: 20.0,
                    feels_like: 19.0,
                    rain_probability: 0.0,
                    precipitation_mm: 0.0,
                    wind_kmh: 5.0,
                })
                .collect(),
        });
        let plan = h.app.plan().unwrap();
        assert_eq!(plan.weather.feels_min, 19.0);
        assert_eq!(h.app.hourly.len(), 8);
    }

    #[test]
    fn local_photo_preview_has_a_working_file_and_image_loader() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("photo with spaces.png");
        image::RgbaImage::from_pixel(32, 24, image::Rgba([120, 80, 50, 255]))
            .save(&path)
            .unwrap();
        let ctx = egui::Context::default();
        configure(&ctx);
        let uri = format!("file://{}", path.to_string_lossy().replace('\\', "/"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            match ctx
                .try_load_image(&uri, egui::load::SizeHint::Width(100))
                .unwrap()
            {
                egui::load::ImagePoll::Ready { image } => {
                    assert_eq!(image.size, [32, 24]);
                    break;
                }
                egui::load::ImagePoll::Pending { .. } => {
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
        }
    }

    #[test]
    fn cancelled_or_superseded_photo_results_cannot_change_the_editor() {
        let mut h = Harness::new();
        h.app.new_editor();
        let result: wardrobe::vision::Analysis =
            serde_json::from_str(include_str!("../tests/fixtures/photo-analysis.json")).unwrap();
        let job = Job::new();
        let before = h.app.editor.as_ref().unwrap().item.clone();
        h.app.editor.as_mut().unwrap().photo_task = Some(PhotoTask {
            job: job.clone(),
            before,
            output_name: "cutout.png".into(),
        });
        h.app.handle_vision_event(VisionEvent::Analysed {
            id: "older job".into(),
            result: Box::new(Ok(result.clone())),
            output: "ignored.png".into(),
        });
        assert!(h.app.editor.as_ref().unwrap().item.name.is_empty());
        h.app.editor.as_mut().unwrap().cancel_photo();
        h.app.handle_vision_event(VisionEvent::Analysed {
            id: job.id,
            result: Box::new(Ok(result)),
            output: "ignored.png".into(),
        });
        assert!(h.app.editor.as_ref().unwrap().item.name.is_empty());
        assert!(h.app.db.garments().unwrap().is_empty());
    }
}
