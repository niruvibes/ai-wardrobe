//! Supervised local Python worker; photos never leave the machine. The UI keeps
//! originals and only applies a result to its matching, still-open editor job.
use crate::model::{Category, ColorReference, Garment, PhotoAnalysisInfo, PhotoBackend, now};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attributes {
    pub name: String,
    pub category: Category,
    pub colors: Vec<String>,
    pub warmth: f32,
    pub formality: f32,
    pub seasons: Vec<String>,
    pub fabric: String,
    pub pattern: String,
    pub fit: String,
    pub styles: Vec<String>,
    pub details: Vec<String>,
    pub description: String,
    pub uncertainties: Vec<String>,
    pub bbox: [f32; 4],
    pub rotation_clockwise: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Analysis {
    pub attributes: Attributes,
    pub model: String,
    pub revision: String,
    pub pipeline: String,
    pub device: String,
    pub source_sha256: String,
    pub width: u32,
    pub height: u32,
    pub elapsed_seconds: f64,
    #[serde(default)]
    pub color_reference: Option<ColorReference>,
    #[serde(default)]
    pub recognition_backend: String,
    #[serde(default)]
    pub segmentation_backend: String,
    #[serde(default)]
    pub runtime_version: String,
    #[serde(default)]
    pub fallback_reasons: Vec<String>,
    #[serde(default)]
    pub gpu_layers: u32,
    #[serde(default)]
    pub timings: std::collections::HashMap<String, f64>,
}
impl Analysis {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.recognition_backend != "Vulkan" || self.gpu_layers > 0,
            "GPU analysis did not confirm any offloaded model layers."
        );
        if let Some(reference) = self.color_reference {
            reference.validate()?;
        }
        let a = &self.attributes;
        ensure!(
            !a.name.trim().is_empty()
                && a.name.len() <= 200
                && !a.description.trim().is_empty()
                && a.description.len() <= 4000,
            "Photo analysis returned incomplete clothing details."
        );
        ensure!(
            a.warmth.is_finite()
                && (0.0..=5.0).contains(&a.warmth)
                && a.formality.is_finite()
                && (1.0..=5.0).contains(&a.formality),
            "Photo analysis returned invalid clothing estimates."
        );
        ensure!(
            !a.colors.is_empty() && a.colors.len() <= 16 && a.details.len() <= 32,
            "Photo analysis returned invalid detail lists."
        );
        ensure!(
            !a.seasons.is_empty()
                && a.seasons
                    .iter()
                    .all(|s| ["spring", "summer", "autumn", "winter"].contains(&s.as_str())),
            "Photo analysis returned invalid seasons."
        );
        ensure!(
            a.bbox
                .iter()
                .all(|n| n.is_finite() && (0.0..=1000.0).contains(n))
                && a.bbox[2] > a.bbox[0]
                && a.bbox[3] > a.bbox[1],
            "Photo analysis returned invalid item boundaries."
        );
        ensure!(
            [0, 90, 180, 270].contains(&a.rotation_clockwise)
                && self.width > 0
                && self.height > 0
                && self.width <= 1600
                && self.height <= 1600,
            "Photo analysis returned an invalid cutout."
        );
        ensure!(
            self.source_sha256.len() == 64
                && self.source_sha256.chars().all(|c| c.is_ascii_hexdigit()),
            "Photo analysis did not identify the original image."
        );
        Ok(())
    }

    /// Do not overwrite a field the user edited while inference was running.
    pub fn apply(&self, item: &mut Garment, before: &Garment, cutout: &str) -> Result<Vec<String>> {
        self.validate()?;
        crate::model::validate_media_name(cutout)?;
        let a = &self.attributes;
        let mut applied = Vec::new();
        macro_rules! field {
            ($field:ident, $value:expr) => {
                if item.$field == before.$field {
                    item.$field = $value;
                    applied.push(stringify!($field).to_string());
                }
            };
        }
        field!(name, a.name.clone());
        field!(category, a.category);
        field!(color, a.colors.join(", "));
        field!(warmth, a.warmth);
        field!(formality, a.formality);
        field!(seasons, a.seasons.join(", "));
        field!(fabric, a.fabric.clone());
        field!(pattern, a.pattern.clone());
        field!(fit, a.fit.clone());
        field!(styles, a.styles.join(", "));
        field!(details, a.details.clone());
        field!(description, a.description.clone());
        if item.photo == before.photo {
            item.original_photo = before
                .original_photo
                .clone()
                .or_else(|| before.photo.clone());
            item.photo = Some(cutout.to_string());
        }
        // Rain/wind protection, brand, size, affinity and walking comfort require
        // user knowledge. No photo prediction silently certifies these facts.
        item.photo_analysis = Some(PhotoAnalysisInfo {
            model: self.model.clone(),
            revision: self.revision.clone(),
            pipeline: self.pipeline.clone(),
            generated_at: now(),
            source_sha256: self.source_sha256.clone(),
            uncertainties: a.uncertainties.clone(),
            inferred_fields: applied.clone(),
            color_reference: self.color_reference,
            device: self.device.clone(),
            recognition_backend: self.recognition_backend.clone(),
            segmentation_backend: self.segmentation_backend.clone(),
            runtime_version: self.runtime_version.clone(),
            fallback_reasons: self.fallback_reasons.clone(),
        });
        Ok(applied)
    }
}

#[derive(Clone, Debug)]
pub struct Runtime {
    pub root: PathBuf,
}
impl Runtime {
    pub fn discover() -> Option<Self> {
        let mut roots = Vec::new();
        if let Some(path) = std::env::var_os("WARDROBE_VISION_ROOT") {
            roots.push(PathBuf::from(path));
        }
        if let Ok(exe) = std::env::current_exe() {
            roots.extend(exe.ancestors().skip(1).take(4).map(Path::to_path_buf));
        }
        roots.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        let roots = roots
            .into_iter()
            .filter(|p| p.join("vision/worker.py").is_file())
            .collect::<Vec<_>>();
        roots
            .iter()
            .find(|p| python_path(p).is_file())
            .or_else(|| roots.first())
            .map(|root| Self { root: root.clone() })
    }
    pub fn ready(&self) -> bool {
        python_path(&self.root).is_file()
            && (self.gpu_ready()
                || (!gpu_python_path(&self.root).is_file()
                    && self.models().join("vision-model.json").is_file()))
            && self
                .models()
                .join("rembg/birefnet-general-lite.onnx")
                .is_file()
    }
    pub fn gpu_ready(&self) -> bool {
        gpu_python_path(&self.root).is_file() && self.models().join("gpu-model.json").is_file()
    }
    pub fn models(&self) -> PathBuf {
        std::env::var_os("WARDROBE_MODELS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.root.join("models"))
    }
    fn command(&self) -> Command {
        let mut command = Command::new(python_path(&self.root));
        command
            .arg("-u")
            .arg(self.root.join("vision/worker.py"))
            .current_dir(&self.root)
            .env("WARDROBE_MODELS_DIR", self.models())
            .env("PYTHONIOENCODING", "utf-8");
        hide(&mut command);
        command
    }
}
fn python_path(root: &Path) -> PathBuf {
    if gpu_python_path(root).is_file() {
        return gpu_python_path(root);
    }
    if cfg!(windows) {
        root.join(".venv/Scripts/python.exe")
    } else {
        root.join(".venv/bin/python")
    }
}
fn gpu_python_path(root: &Path) -> PathBuf {
    if cfg!(windows) {
        root.join(".venv-gpu/Scripts/python.exe")
    } else {
        root.join(".venv/bin/python")
    }
}
fn hide(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let _ = command;
}

#[derive(Clone)]
pub struct Job {
    pub id: String,
    cancel: Arc<AtomicBool>,
}
impl Job {
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Default for Job {
    fn default() -> Self {
        Self::new()
    }
}
pub enum VisionEvent {
    Progress {
        id: String,
        message: String,
    },
    Analysed {
        id: String,
        result: Box<Result<Analysis, String>>,
        output: PathBuf,
    },
    SetupFinished {
        id: String,
        result: Result<(), String>,
    },
}
enum Request {
    Analyse {
        job: Job,
        source: PathBuf,
        output: PathBuf,
        color_reference: Option<ColorReference>,
        backend: PhotoBackend,
    },
    Setup(Job),
    Release,
    Shutdown,
}

pub struct VisionService {
    pub runtime: Option<Runtime>,
    tx: mpsc::Sender<Request>,
    rx: mpsc::Receiver<VisionEvent>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl VisionService {
    pub fn new(log_dir: &Path) -> Self {
        let runtime = Runtime::discover();
        let worker_runtime = runtime.clone();
        let log_dir = log_dir.to_path_buf();
        let (tx, requests) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = thread::spawn(move || {
            let mut worker: Option<Worker> = None;
            while !stopping.load(Ordering::Relaxed) {
                let request = match requests.recv_timeout(Duration::from_secs(300)) {
                    Ok(request) => request,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        worker = None;
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                match request {
                    Request::Shutdown => break,
                    Request::Release => worker = None,
                    Request::Analyse {
                        job,
                        source,
                        output,
                        color_reference,
                        backend,
                    } => {
                        let result = (|| -> Result<Analysis> {
                            let runtime=worker_runtime.as_ref().context("Photo automation files are missing. Keep the vision folder next to the app.")?;
                            ensure!(
                                runtime.ready(),
                                "Photo automation needs its one-time setup. Choose 'Set up photo automation'."
                            );
                            check_cancel(&job, &stopping)?;
                            if worker.is_none() {
                                worker = Some(Worker::spawn(runtime.command(), &log_dir)?);
                            }
                            let process = worker.as_mut().unwrap();
                            process.send(
                                &serde_json::json!({"id":job.id,"source":source,"output":output,"color_reference":color_reference,"backend":backend.code()}),
                            )?;
                            let value = process.receive(
                                &job,
                                &stopping,
                                &events,
                                Duration::from_secs(15 * 60),
                            )?;
                            let analysis: Analysis = serde_json::from_value(value)?;
                            analysis.validate()?;
                            ensure!(
                                output.is_file(),
                                "The background-removal worker did not save a cutout."
                            );
                            Ok(analysis)
                        })();
                        if result.is_err() {
                            worker = None;
                        }
                        let _ = events.send(VisionEvent::Analysed {
                            id: job.id,
                            result: Box::new(result.map_err(|e| format!("{e:#}"))),
                            output,
                        });
                    }
                    Request::Setup(job) => {
                        worker = None;
                        let result = (|| -> Result<()> {
                            let runtime = worker_runtime.as_ref().context(
                                "Photo automation files are missing from this installation.",
                            )?;
                            let log = log_dir.join("vision-worker.log");
                            if !gpu_python_path(&runtime.root).is_file() {
                                progress(&events, &job, "Creating the local photo environment…");
                                let mut cmd = Command::new("python");
                                cmd.arg("-m").arg("venv").arg(
                                    runtime.root.join(if cfg!(windows) {
                                        ".venv-gpu"
                                    } else {
                                        ".venv"
                                    }),
                                );
                                run_simple(cmd, &job, &stopping, &log)?;
                            }
                            progress(&events, &job, "Checking the local vision libraries…");
                            let mut cmd = Command::new(gpu_python_path(&runtime.root));
                            let requirements = if cfg!(windows) {
                                "requirements.gpu"
                            } else {
                                "requirements"
                            };
                            let locked =
                                runtime.root.join(format!("vision/{requirements}.lock.txt"));
                            cmd.args(["-m", "pip", "install", "-r"])
                                .arg(if locked.is_file() {
                                    locked
                                } else {
                                    runtime.root.join(format!("vision/{requirements}.txt"))
                                });
                            run_simple(cmd, &job, &stopping, &log)?;
                            let mut cmd = runtime.command();
                            cmd.arg("--setup");
                            let mut setup = Worker::spawn(cmd, &log_dir)?;
                            // The setup command uses a fixed protocol id.
                            let setup_job = Job {
                                id: "setup".into(),
                                cancel: job.cancel.clone(),
                            };
                            let (setup_events, setup_rx) = mpsc::channel();
                            let forwarding = events.clone();
                            let id = job.id.clone();
                            let forward = thread::spawn(move || {
                                for event in setup_rx {
                                    if let VisionEvent::Progress { message, .. } = event {
                                        let _ = forwarding.send(VisionEvent::Progress {
                                            id: id.clone(),
                                            message,
                                        });
                                    }
                                }
                            });
                            let result = setup.receive(
                                &setup_job,
                                &stopping,
                                &setup_events,
                                Duration::from_secs(60 * 60),
                            );
                            drop(setup_events);
                            let _ = forward.join();
                            result?;
                            ensure!(
                                runtime.ready(),
                                "Model setup did not finish. Check your connection and try setup again."
                            );
                            Ok(())
                        })();
                        let _ = events.send(VisionEvent::SetupFinished {
                            id: job.id,
                            result: result.map_err(|e| format!("{e:#}")),
                        });
                    }
                }
            }
        });
        Self {
            runtime,
            tx,
            rx,
            stop,
            thread: Some(thread),
        }
    }
    pub fn analyse(&self, source: PathBuf, output: PathBuf) -> Job {
        self.analyse_with_reference(source, output, None)
    }
    pub fn analyse_with_reference(
        &self,
        source: PathBuf,
        output: PathBuf,
        color_reference: Option<ColorReference>,
    ) -> Job {
        self.analyse_configured(source, output, color_reference, PhotoBackend::Auto)
    }
    pub fn analyse_configured(
        &self,
        source: PathBuf,
        output: PathBuf,
        color_reference: Option<ColorReference>,
        backend: PhotoBackend,
    ) -> Job {
        let job = Job {
            id: Uuid::new_v4().to_string(),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let _ = self.tx.send(Request::Analyse {
            job: job.clone(),
            source,
            output,
            color_reference,
            backend,
        });
        job
    }
    pub fn setup(&self) -> Job {
        let job = Job {
            id: Uuid::new_v4().to_string(),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let _ = self.tx.send(Request::Setup(job.clone()));
        job
    }
    pub fn release_cached_models(&self) {
        let _ = self.tx.send(Request::Release);
    }
    pub fn poll(&self) -> Vec<VisionEvent> {
        self.rx.try_iter().collect()
    }
}
impl Drop for VisionService {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.tx.send(Request::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
fn check_cancel(job: &Job, stop: &AtomicBool) -> Result<()> {
    ensure!(
        !job.cancel.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed),
        "Photo processing cancelled."
    );
    Ok(())
}

#[cfg(windows)]
fn own_process_tree(child: &Child) -> Result<std::os::windows::io::OwnedHandle> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    // Closing this handle also terminates the inference server spawned by the
    // Python worker, including on cancellation or an unexpected app exit.
    unsafe {
        let raw = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        ensure!(
            !raw.is_null(),
            "Could not create a managed photo-process group: {}",
            std::io::Error::last_os_error()
        );
        let job = OwnedHandle::from_raw_handle(raw);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        ensure!(
            SetInformationJobObject(
                raw,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32
            ) != 0,
            "Could not manage photo-process lifetime: {}",
            std::io::Error::last_os_error()
        );
        ensure!(
            AssignProcessToJobObject(raw, child.as_raw_handle()) != 0,
            "Could not supervise the photo worker: {}",
            std::io::Error::last_os_error()
        );
        Ok(job)
    }
}
fn progress(events: &mpsc::Sender<VisionEvent>, job: &Job, message: &str) {
    let _ = events.send(VisionEvent::Progress {
        id: job.id.clone(),
        message: message.into(),
    });
}

struct Worker {
    child: Child,
    input: ChildStdin,
    lines: mpsc::Receiver<String>,
    #[cfg(windows)]
    _process_job: std::os::windows::io::OwnedHandle,
}
impl Worker {
    fn spawn(mut command: Command, log_dir: &Path) -> Result<Self> {
        command.env("WARDROBE_LOG_DIR", log_dir);
        fs::create_dir_all(log_dir)?;
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("vision-worker.log"))?;
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(log)
            .spawn()
            .context("Could not start the local photo worker. Run photo automation setup.")?;
        #[cfg(windows)]
        let process_job = match own_process_tree(&child) {
            Ok(job) => job,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        let input = child
            .stdin
            .take()
            .context("Could not open photo-worker input")?;
        let output = child
            .stdout
            .take()
            .context("Could not open photo-worker output")?;
        let (tx, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else {
                    break;
                };
                if line.len() > 1024 * 1024 || tx.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            input,
            lines,
            #[cfg(windows)]
            _process_job: process_job,
        })
    }
    fn send(&mut self, value: &serde_json::Value) -> Result<()> {
        serde_json::to_writer(&mut self.input, value)?;
        self.input.write_all(b"\n")?;
        self.input.flush()?;
        Ok(())
    }
    fn receive(
        &mut self,
        job: &Job,
        stop: &AtomicBool,
        events: &mpsc::Sender<VisionEvent>,
        timeout: Duration,
    ) -> Result<serde_json::Value> {
        let start = Instant::now();
        loop {
            check_cancel(job, stop)?;
            ensure!(
                start.elapsed() < timeout,
                "Local photo processing timed out. Try a smaller photo or restart the photo worker."
            );
            match self.lines.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    let value: serde_json::Value = serde_json::from_str(&line)
                        .context("The local photo worker returned an invalid message")?;
                    ensure!(
                        value["id"].as_str() == Some(job.id.as_str()),
                        "Received a photo result for a different request."
                    );
                    match value["type"].as_str() {
                        Some("progress") => progress(
                            events,
                            job,
                            value["message"].as_str().unwrap_or("Processing photo…"),
                        ),
                        Some("result" | "ready") => return Ok(value["result"].clone()),
                        Some("error") => bail!(
                            "{}",
                            value["message"].as_str().unwrap_or("Photo analysis failed")
                        ),
                        _ => bail!("The local photo worker returned an unknown response"),
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => bail!(
                    "The local photo worker stopped unexpectedly. Try again; details are in vision-worker.log."
                ),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn run_simple(mut command: Command, job: &Job, stop: &AtomicBool, log: &Path) -> Result<()> {
    hide(&mut command);
    let log = OpenOptions::new().create(true).append(true).open(log)?;
    command
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    let mut child = command
        .spawn()
        .context("Could not start Python. Install Python 3.13 and try setup again.")?;
    let start = Instant::now();
    loop {
        if check_cancel(job, stop).is_err() || start.elapsed() > Duration::from_secs(30 * 60) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Photo automation setup was cancelled or timed out.");
        }
        if let Some(status) = child.try_wait()? {
            ensure!(
                status.success(),
                "Installing photo libraries failed. Check vision-worker.log for details."
            );
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
}
