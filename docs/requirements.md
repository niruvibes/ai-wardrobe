# Requirements and acceptance evidence

| Requirement | Implementation | Verification |
| --- | --- | --- |
| Rust computer application | Native egui/eframe Windows executable | Build; native startup; app-level egui control tests |
| Catalogue at least 100 items | SQLite, indexed metadata, FTS5 search | 150-item persistence/search and bounded-ranking tests; isolated 112-item demo |
| Quick add and search | Add-another, photo review queue, full-text search and filters | App add/edit/search/favourite control test; DB search tests |
| Clothing attributes | Photos, category, colour, warmth, formality, availability, brand, size, fabric, pattern, fit, seasons, style, notes | Model validation, persistence, managed photo and restore tests |
| Rainproof tracking | Unknown / none / resistant / waterproof plus wind protection | Wet-weather preference and gap-warning tests |
| Weather for the hours out, Celsius | Exact intersecting hourly windows; actual/feels-like/rain/wind; selected location timezone | Boundary/overnight/missing/stale/provider-array tests; live Sydney forecast check |
| Occasion and preferences | Five occasions, exposure, walking, thermal adjustment, colour/fit/style tags | Ranking and application generation tests |
| History | Explicit actual wears; repeat window; reusable historical outfits | Recent/future history tests; persisted wear snapshots |
| Favourites | Clothes and whole outfits | App favourite control test; outfit and backup persistence tests |
| Control over suggestions | Anchor, locks, swap, exclusions, alternative options | Hard-lock conflict tests, swap invariant tests, unique-output test |
| Evolving style scores | Append-only settings/affinity/feedback; versioned outfit snapshots | Revision preservation test, latest-feedback test, unsaved-settings app test |
| Later style guide | Local reference images/notes; explicit manual tags and ratings | Preferences persistence and backup media handling; no claim of automatic image interpretation |
| Robust storage | Versioned schema, transactions, local managed photos, validated portable backup and recovery | Photo/backup round-trip, invalid-backup non-mutation tests |

Visual polish is deferred at the user's request. Native desktop input automation was not approved, so user-interface verification uses the same egui controls and app handlers in a test context.

## Photo automation — version 0.2

| Requirement | Implementation | Current evidence |
| --- | --- | --- |
| Automatically name and categorise items | Local Qwen3-VL structured prediction on photo selection and bulk import | Real embroidered fleece and wide-leg jeans checks; typed result validation and field application tests |
| Automatically generate colours, warmth, formality and seasons | Validated estimates with a basic garment warmth rubric; editable fields | Real model outputs; invalid-estimate rejection and generated-field tests |
| Describe embroidery, material appearance and fit | Brief description, detail list, fabric, pattern, fit and style tags | Fleece's blossom embroidery and jeans' faded wash/wide legs recognised; stored descriptions verified |
| Make those details useful for outfit selection | Full-text index and preference matching include visual context; algorithm version 2 | Search/persistence tests; ranking preference test for suede/embroidery/baggy details |
| Remove background and crop to item | BiRefNet General Lite masks, detached-noise removal, alpha-bound crop and padding | Two real transparent cutouts; alpha/foreground and crop/pixel-preservation tests |
| Repair local photo preview | egui file and image loaders both enabled | Actual local PNG with spaces in its name decodes in the app's configured egui context |
| Preserve originals and manual edits | Managed originals keep source bytes/metadata; results target job IDs and only untouched fields | Original-byte comparison, manual-edit test, stale/cancelled result test and original/cutout backup round-trip |
| Optional colour reference | User selects a known neutral patch; linear-sRGB channel balance before analysis; ICC/EXIF processing | Cast-correction and rejected-reference tests; embedded-profile/EXIF/reference integration test |
| Use an appropriate CV stack | Rust desktop/storage plus a supervised Python inference worker | Live Rust → Python → model → cutout → SQLite import check |
| Keep existing wardrobe usable | Automatic schema upgrade, backward-compatible JSON defaults, stale-writer metadata guard | Existing-v1 database upgrade and stale-save rejection test |
| Deliver a runnable update | Versioned Windows executable plus worker and pinned dependency definitions | Release build and packaged command-line startup check |

The 0.2 release has 34 passing Rust tests and 9 passing Python tests. Photo predictions remain estimates; material composition, insulation and waterproofing cannot be certified from a photo. CPU inference took around 80–120 seconds on the tested setup. Style-guide image interpretation remains outside this photo-import request.

## GPU processing — version 0.3

| Requirement | Implementation | Evidence |
| --- | --- | --- |
| Use RX 7800 XT | Full F16 official Qwen GGUF, llama.cpp Vulkan, verified layer offload | Both real garments processed with 29 layers offloaded on Radeon RX 7800 XT |
| Accelerate masking optionally | ONNX Runtime DirectML with sequential execution and memory patterns disabled | Both real cutouts produced with DirectML active |
| Sensible defaults and fallback | Automatic uses Vulkan recognition/CPU masking; strict GPU and explicit CPU available | End-to-end speed comparison; mocked startup-failure, strict-mode and CPU-selection checks |
| Keep originals and existing wardrobe | Separate GPU environment; metadata defaults; transactional schema 3 upgrade | Original-byte checks, SQLite/backup provenance and stale-writer tests |
| Keep photos local and free model memory | Authenticated loopback helper; supervised Windows process group; cancellation, idle release, manual unload | Real Rust service shutdown leaves no llama-server process |
| Support user's phone workflow | S26 Ultra capture guide with paper reference and sRGB exports | Samsung official documentation; RAW decoding explicitly unsupported |
