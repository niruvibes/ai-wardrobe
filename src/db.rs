use crate::model::*;
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use uuid::Uuid;

pub struct Database {
    conn: Connection,
    pub root: PathBuf,
}

impl Database {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("media"))?;
        let conn = Connection::open(root.join("wardrobe.sqlite3"))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        let mut version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        ensure!(
            version <= 3,
            "This database was created by a newer version of Wardrobe."
        );
        if version == 0 {
            conn.execute_batch(include_str!("../migrations/001_initial.sql"))?;
            version = 1;
        }
        if version == 1 {
            conn.execute_batch(include_str!("../migrations/002_visual_metadata.sql"))?;
            version = 2;
        }
        if version == 2 {
            conn.execute_batch(include_str!("../migrations/003_gpu_metadata.sql"))?;
        }
        let db = Self { conn, root };
        if db.profiles()?.is_empty() {
            db.save_profile(&ScoringProfile::default(), &Preferences::default())?;
        }
        Ok(db)
    }

    pub fn garments(&self) -> Result<Vec<Garment>> {
        self.json_rows("SELECT data FROM garments ORDER BY name COLLATE NOCASE", [])
    }

    pub fn search(&self, query: &str) -> Result<Vec<Garment>> {
        let tokens = query
            .split_whitespace()
            .map(|s| format!("\"{}\"*", s.replace('"', "\"\"")))
            .collect::<Vec<_>>();
        if tokens.is_empty() {
            return self.garments();
        }
        self.json_rows("SELECT g.data FROM garment_search JOIN garments g ON g.id=garment_search.item_id WHERE garment_search MATCH ?1 ORDER BY g.name COLLATE NOCASE", [tokens.join(" AND ")])
    }

    pub fn save_garment(&mut self, garment: &Garment, affinity: f32) -> Result<()> {
        garment.validate()?;
        ensure!(
            affinity.is_finite() && (0.0..=5.0).contains(&affinity),
            "Style affinity must be 0–5."
        );
        let tx = self.conn.transaction()?;
        tx.execute("INSERT INTO garments(id,name,category,availability,favourite,archived,data) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(id) DO UPDATE SET name=excluded.name,category=excluded.category,availability=excluded.availability,favourite=excluded.favourite,archived=excluded.archived,data=excluded.data",
            params![garment.id, garment.name.trim(), garment.category.label(), garment.availability.label(), garment.favourite, garment.archived, serde_json::to_string(garment)?])?;
        tx.execute("DELETE FROM garment_search WHERE item_id=?1", [&garment.id])?;
        tx.execute(
            "INSERT INTO garment_search(item_id,text) VALUES(?1,?2)",
            params![garment.id, garment.search_text()],
        )?;
        let previous: Option<f32> = tx
            .query_row(
                "SELECT value FROM affinity_revisions WHERE item_id=?1 ORDER BY id DESC LIMIT 1",
                [&garment.id],
                |r| r.get(0),
            )
            .optional()?;
        if previous != Some(affinity) {
            tx.execute(
                "INSERT INTO affinity_revisions(item_id,value,created_at) VALUES(?1,?2,?3)",
                params![garment.id, affinity, now()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn affinities(&self) -> Result<HashMap<String, f32>> {
        let mut stmt = self.conn.prepare("SELECT item_id,value FROM affinity_revisions WHERE id IN (SELECT MAX(id) FROM affinity_revisions GROUP BY item_id)")?;
        Ok(stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn affinity_revisions(&self) -> Result<Vec<AffinityRevision>> {
        let mut stmt = self
            .conn
            .prepare("SELECT item_id,value,created_at FROM affinity_revisions ORDER BY id")?;
        Ok(stmt
            .query_map([], |r| {
                Ok(AffinityRevision {
                    item_id: r.get(0)?,
                    value: r.get(1)?,
                    created_at: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn save_profile(&self, profile: &ScoringProfile, preferences: &Preferences) -> Result<i64> {
        profile.validate()?;
        preferences.validate()?;
        self.conn.execute(
            "INSERT INTO profile_revisions(profile,preferences,created_at) VALUES(?1,?2,?3)",
            params![
                serde_json::to_string(profile)?,
                serde_json::to_string(preferences)?,
                now()
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn profiles(&self) -> Result<Vec<ProfileRevision>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,profile,preferences,created_at FROM profile_revisions ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        rows.map(|r| {
            let (id, p, pref, created_at) = r?;
            let mut profile: ScoringProfile = serde_json::from_str(&p)?;
            profile.revision = id;
            Ok(ProfileRevision {
                profile,
                preferences: serde_json::from_str(&pref)?,
                created_at,
            })
        })
        .collect()
    }

    pub fn save_outfit(&self, outfit: &Outfit, favourite: bool) -> Result<()> {
        outfit.validate()?;
        self.conn.execute("INSERT INTO outfits(signature,data,favourite) VALUES(?1,?2,?3) ON CONFLICT(signature) DO UPDATE SET data=excluded.data,favourite=excluded.favourite",
            params![outfit.signature, serde_json::to_string(outfit)?, favourite])?;
        Ok(())
    }

    pub fn outfits(&self) -> Result<Vec<SavedOutfit>> {
        let mut stmt = self
            .conn
            .prepare("SELECT data,favourite FROM outfits ORDER BY rowid DESC")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)))?;
        rows.map(|r| {
            let (data, favourite) = r?;
            Ok(SavedOutfit {
                outfit: serde_json::from_str(&data)?,
                favourite,
            })
        })
        .collect()
    }

    pub fn record_wear(
        &self,
        outfit: &Outfit,
        worn_at: chrono::NaiveDateTime,
        notes: &str,
    ) -> Result<()> {
        outfit.validate()?;
        let entry = WearEvent {
            id: Uuid::new_v4().to_string(),
            outfit: outfit.clone(),
            worn_at,
            notes: notes.to_string(),
        };
        self.conn.execute(
            "INSERT INTO wear_events(id,worn_at,data) VALUES(?1,?2,?3)",
            params![
                entry.id,
                entry.worn_at.to_string(),
                serde_json::to_string(&entry)?
            ],
        )?;
        Ok(())
    }
    pub fn wear_history(&self) -> Result<Vec<WearEvent>> {
        self.json_rows("SELECT data FROM wear_events ORDER BY worn_at DESC", [])
    }

    pub fn feedback(&self, outfit: &Outfit, value: i32, reason: &str) -> Result<()> {
        outfit.validate()?;
        ensure!((-1..=1).contains(&value), "Invalid feedback rating.");
        let feedback = Feedback {
            id: Uuid::new_v4().to_string(),
            signature: outfit.signature.clone(),
            item_ids: outfit.items.iter().map(|i| i.id.clone()).collect(),
            value,
            reason: reason.to_string(),
            created_at: now(),
            outfit: outfit.clone(),
        };
        self.conn.execute(
            "INSERT INTO feedback_events(id,signature,data) VALUES(?1,?2,?3)",
            params![
                feedback.id,
                feedback.signature,
                serde_json::to_string(&feedback)?
            ],
        )?;
        Ok(())
    }
    pub fn feedback_history(&self) -> Result<Vec<Feedback>> {
        self.json_rows("SELECT data FROM feedback_events ORDER BY rowid", [])
    }

    pub fn put_setting<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        self.conn.execute("INSERT INTO settings(key,data) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET data=excluded.data", params![key, serde_json::to_string(value)?])?;
        Ok(())
    }
    pub fn setting<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let data: Option<String> = self
            .conn
            .query_row("SELECT data FROM settings WHERE key=?1", [key], |r| {
                r.get(0)
            })
            .optional()?;
        data.map(|s| serde_json::from_str(&s).map_err(Into::into))
            .transpose()
    }

    /// Decode and re-encode photos into app-managed files; never depend on the source path.
    pub fn import_photo(&self, source: &Path) -> Result<String> {
        ensure!(
            fs::metadata(source)?.len() <= 30 * 1024 * 1024,
            "Please use an image smaller than 30 MB."
        );
        let mut reader = image::ImageReader::open(source)?.with_guessed_format()?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(16000);
        limits.max_image_height = Some(16000);
        limits.max_alloc = Some(256 * 1024 * 1024);
        reader.limits(limits);
        let extension = match reader.format() {
            Some(image::ImageFormat::Png) => "png",
            Some(image::ImageFormat::Jpeg) => "jpg",
            Some(image::ImageFormat::WebP) => "webp",
            _ => anyhow::bail!("Use a PNG, JPEG or WebP image."),
        };
        reader.decode().context("Use a PNG, JPEG or WebP image.")?;
        let file = format!("{}.{}", Uuid::new_v4(), extension);
        // Keep original pixels, EXIF and colour profile. The vision worker
        // normalises orientation/ICC when producing a separate sRGB cutout.
        fs::copy(source, self.root.join("media").join(&file))?;
        Ok(file)
    }

    pub fn photo_path(&self, file: &str) -> Result<PathBuf> {
        validate_media_name(file)?;
        Ok(self.root.join("media").join(file))
    }

    pub fn export_backup(&self, parent: &Path) -> Result<PathBuf> {
        let folder = parent.join(format!(
            "Wardrobe-backup-{}-{}",
            chrono::Local::now().format("%Y%m%d-%H%M%S"),
            &Uuid::new_v4().to_string()[..8]
        ));
        fs::create_dir_all(folder.join("media"))?;
        let backup = Backup {
            schema: 1,
            garments: self.garments()?,
            profiles: self.profiles()?,
            outfits: self.outfits()?,
            wear: self.wear_history()?,
            feedback: self.feedback_history()?,
            affinity: self.affinity_revisions()?,
        };
        backup.validate()?;
        for entry in fs::read_dir(self.root.join("media"))? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                fs::copy(entry.path(), folder.join("media").join(entry.file_name()))?;
            }
        }
        fs::write(
            folder.join("wardrobe.json"),
            serde_json::to_vec_pretty(&backup)?,
        )?;
        Ok(folder)
    }

    pub fn restore_backup(&mut self, folder: &Path) -> Result<()> {
        let backup: Backup = serde_json::from_slice(&fs::read(folder.join("wardrobe.json"))?)?;
        backup.validate()?;
        let mut media = std::collections::HashSet::<String>::new();
        let mut add_outfit = |outfit: &Outfit| {
            for g in &outfit.items {
                media.extend(g.media_files().cloned());
            }
            media.extend(outfit.preferences.guide_images.iter().cloned());
        };
        for o in &backup.outfits {
            add_outfit(&o.outfit);
        }
        for w in &backup.wear {
            add_outfit(&w.outfit);
        }
        for f in &backup.feedback {
            add_outfit(&f.outfit);
        }
        for g in &backup.garments {
            media.extend(g.media_files().cloned());
        }
        for p in &backup.profiles {
            media.extend(p.preferences.guide_images.iter().cloned());
        }
        for file in &media {
            validate_media_name(file)?;
            let source = folder.join("media").join(file);
            ensure!(source.is_file(), "Backup is missing photo {file}.");
            let target = self.photo_path(file)?;
            if target.exists() {
                ensure!(
                    fs::read(&source)? == fs::read(&target)?,
                    "A backup photo conflicts with an existing file."
                );
            }
        }
        // Keep an automatic recovery copy before replacing any database records.
        self.export_backup(&self.root.join("recovery"))?;
        for file in &media {
            let target = self.photo_path(file)?;
            if !target.exists() {
                fs::copy(folder.join("media").join(file), target)?;
            }
        }
        let tx = self.conn.transaction()?;
        tx.execute_batch("DELETE FROM affinity_revisions; DELETE FROM garments; DELETE FROM garment_search; DELETE FROM profile_revisions; DELETE FROM outfits; DELETE FROM wear_events; DELETE FROM feedback_events; DELETE FROM settings;")?;
        for g in &backup.garments {
            tx.execute("INSERT INTO garments(id,name,category,availability,favourite,archived,data) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![g.id, g.name, g.category.label(), g.availability.label(), g.favourite, g.archived, serde_json::to_string(g)?])?;
            tx.execute(
                "INSERT INTO garment_search(item_id,text) VALUES(?1,?2)",
                params![g.id, g.search_text()],
            )?;
        }
        for p in &backup.profiles {
            tx.execute("INSERT INTO profile_revisions(id,profile,preferences,created_at) VALUES(?1,?2,?3,?4)", params![p.profile.revision, serde_json::to_string(&p.profile)?, serde_json::to_string(&p.preferences)?, p.created_at])?;
        }
        for o in &backup.outfits {
            tx.execute(
                "INSERT INTO outfits(signature,data,favourite) VALUES(?1,?2,?3)",
                params![
                    o.outfit.signature,
                    serde_json::to_string(&o.outfit)?,
                    o.favourite
                ],
            )?;
        }
        for w in &backup.wear {
            tx.execute(
                "INSERT INTO wear_events(id,worn_at,data) VALUES(?1,?2,?3)",
                params![w.id, w.worn_at.to_string(), serde_json::to_string(w)?],
            )?;
        }
        for f in &backup.feedback {
            tx.execute(
                "INSERT INTO feedback_events(id,signature,data) VALUES(?1,?2,?3)",
                params![f.id, f.signature, serde_json::to_string(f)?],
            )?;
        }
        for a in &backup.affinity {
            tx.execute(
                "INSERT INTO affinity_revisions(item_id,value,created_at) VALUES(?1,?2,?3)",
                params![a.item_id, a.value, a.created_at],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    fn json_rows<T: DeserializeOwned, P: rusqlite::Params>(
        &self,
        sql: &str,
        params: P,
    ) -> Result<Vec<T>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, |r| r.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }
}
