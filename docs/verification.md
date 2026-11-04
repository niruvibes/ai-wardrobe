# Verification — 4 November 2026

The current source passed:

- `cargo test --offline --locked`: **25 passing tests**, comprising 21 behaviour/integration tests and 4 app tests.
- `cargo clippy --offline --locked --all-targets -- -D warnings`: success, with no code-quality diagnostics. This environment reports an incremental-cache hard-link warning and copies cache files instead.
- `cargo fmt --all -- --check`: success.
- `cargo build --release --offline --locked`: success for Windows x86-64.

The app tests simulate input through egui itself: adding, editing, searching and favouriting clothing; invalid form handling; asynchronous outfit generation; saved scoring revisions; forecast/manual switching; and rendering each app page with stored outfits and history. They do not automate the Windows desktop.

The behaviour tests verify 150-item persistence/search and ranking, clothing availability and exclusions, lock conflicts, swaps preserving other pieces, complete one-piece outfits, unique alternatives, waterproof protection, cold-weather layers, recent-wear penalties, future-wear handling, per-item affinity, feedback changes, immutable scoring snapshots, precise hourly boundaries, overnight outings, stale/missing/malformed forecasts, ambiguous daylight-saving hours, validation, managed photos, backup round-trip and rejected-backup non-mutation.

A live integration request through the app's Rust weather client returned **168 hourly readings** for Sydney, Australia, with destination timezone Australia/Sydney. An eight-hour window was successfully summarised in Celsius, km/h and millimetres. The geocoding check returned disambiguated locations.

An isolated demonstration database contains **112 clothing items**, an outfit favourite and a wear event. It is kept under `test-data/demo`, separate from the user's real wardrobe. The native debug app started and exposed a Wardrobe window. Desktop input/screenshot access was not approved; visual polish and inspection are deferred at the user's request.

The packaged executable is `dist/Wardrobe.exe`, with `dist/START-HERE.txt` and a copy of the usage guide. It uses the user's local app-data folder by default, or an explicitly selected `--data-dir` folder. Source and migrations remain in the project folder.

## Version 0.2 photo automation verification

- `cargo test --offline --locked`: **34 passing tests** (21 existing behaviour checks, 6 app checks, 7 photo-automation checks).
- `.venv/Scripts/python.exe -m unittest discover -s vision -p test_worker.py -v`: **9 passing tests**.
- `cargo clippy --offline --locked --all-targets -- -D warnings`: success. The environment's incremental-cache copy warning is not a code diagnostic.
- `cargo fmt --all -- --check`: success.
- Windows release build and packaging: `scripts/package.ps1`, producing `dist/Wardrobe-0.2.exe`, `dist/vision/worker.py` and pinned Python dependencies. The older executable is retained to avoid interrupting an open instance.

The real local model was tested against the user's embroidered fleece and wide-leg jeans, using separate output files and an isolated database. The Rust `analyse_photos` example exercised the same persistent worker, generated transparent cutouts, checked opaque foreground and transparent background, confirmed original bytes were unchanged, applied all generated fields and saved searchable items to SQLite. Final prompt checks identified:

- **White fleece pullover with cherry blossom embroidery**: Outerwear; white/red; warmth 4; formality 1; winter; fleece-like texture, blossom embroidery and zip closure. Final CPU inference and removal took about **101 seconds**.
- **Light blue wide-leg jeans**: Bottom; light blue/white; warmth 1; formality 1; summer/spring/autumn; denim, faded wash, wide-leg/baggy tags, pockets and belt loops. Final CPU inference and removal took about **94 seconds**.

These are model estimates, not certified garment facts. Native FP32 CPU inference is retained: experimentally quantising the model degraded recognition, and that configuration was removed. Normal analysis uses the pinned local Qwen revision `89644892e4d85e24eaac8bacfd4f463576704203` and BiRefNet General Lite, with no image upload or model-repository code execution. The local libraries, model weights and lockfile are installed.

Additional tests cover the exact file-loader failure in the reported preview, template-echo rejection, invalid model fields, negative embroidery statements, neutral-card white balance, clipped/dark/nonuniform reference rejection, ICC/EXIF handling before inference, cropped alpha/padding, user edits taking precedence, cancelled/superseded job rejection, descriptions affecting actual outfit rankings, original/cutout backup round-trip, old JSON readability, schema migration and protection against stale older writers.

## Version 0.3 GPU verification

- `cargo test --offline --locked`: **35 passing tests** (21 behaviour, 6 app, 8 photo checks).
- `.venv-gpu/Scripts/python.exe -m unittest discover -s vision -p test_*.py -v`: **17 passing tests**.
- `cargo clippy --offline --locked --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.

Windows release build passed with `cargo build --release --offline --locked`. The packaged `dist/Wardrobe-0.3.exe` passed its `--help` startup check. All seven packaged worker/dependency files match their source SHA-256 hashes. Packaging was performed with the build and copy commands directly because the shell's script authorization policy rejected invoking `scripts/package.ps1`; that policy was left unchanged. The package includes the usage guide and phone guide, and retains the older executables.

The separate Windows GPU environment uses ONNX Runtime DirectML 1.24.4 and llama.cpp Vulkan release b12102. The runtime ZIP and official Qwen F16 model/projector were verified against fixed SHA-256 hashes before installation. The Qwen GGUF revision is `52d6c8ffea26cc873ac5ad116f8631268d7eb503`. Recognition logs confirm **29/29 layers offloaded to Radeon RX 7800 XT**, rather than merely detecting a GPU. Strict GPU mode also verifies DirectML is the active first masking provider.

The actual Rust `analyse_photos` service processed the same fleece and jeans, checked source bytes, transparent and opaque pixels, validated results and stored them in isolated SQLite wardrobes. The fleece retained its blossom embroidery details; the jeans retained denim, faded wash and wide-leg/baggy details.

| Configuration | Fleece | Jeans | Evidence folder |
| --- | ---: | ---: | --- |
| Native CPU recognition + CPU masking, earlier comparison | 104.2 s | 105.8 s | `test-data/gpu-benchmark/comparison` |
| Vulkan recognition + CPU masking (Automatic) | 25.0 s | 17.2 s | `test-data/gpu-service-auto` |
| Vulkan + DirectML, repeated after user closed game | 32.2 s | 27.7 s | `test-data/gpu-service-idle` |

These are measured two-photo samples, with the first request loading models and the second reusing them. The comparison is illustrative, not a controlled hardware benchmark: the CPU comparison preceded the user closing a game. An earlier full-GPU run with that competing workload varied from 34 to 131 seconds; its slowdown cannot be attributed solely to DirectML. The no-game rerun confirmed normal GPU performance. Automatic retains CPU masking because its observed whole-import time was lower. A truly first-time driver/model run reached 105 seconds; warmed driver caches and repeated use matter.

No llama-server process remained after the Rust integration service shut down. A Windows kill-on-close job object owns Python and its native descendants. Idle release is five minutes, with a manual release control. Backend tests cover strict-mode errors, CPU selection, visible Automatic fallback reasons, provider activation, missing/escaping manifest paths, archive traversal, checksum rejection and resumable downloads. Rust checks cover provenance backup round-trip, saved backend preference and stale-writer protection under schema 3.

RAW/DNG decoding remains unsupported. The S26 Ultra guide documents Samsung Expert RAW capture and sRGB JPEG/PNG export; this change does not pretend that RAW or A4 paper provides full colour calibration.
