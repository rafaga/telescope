# Changelog

All notable changes to Telescope are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

There are no tagged releases yet. Everything so far is under *Unreleased* and
was reconstructed from the git history, grouped by topic and dated by the
period it happened in.

## [Unreleased]

### Added

* Settings window split into pages (*Intelligence*, *Data Sources*,
  *Characters*), with per-page modules and a page menu driven by
  `SettingsPage::ALL` (September 2026).
* Save flow extracted into `save_settings` / `apply_intel_settings`, with unit
  tests for the channel selection (September 2026).
* `IntelLogName`, a single parser for chat log file names, with unit tests
  (September 2026).
* Notifications module and on-screen error reports for intel directory
  problems (September 2026).
* Rescan of intel files at start-up and when a new log file appears; a new font
  for the universe map (September 2026).
* SDE database updater with a progress window: the database is checked,
  downloaded and built automatically when missing (August 2026).
* Pattern engine driven by `patterns.toml` (regex rules, dictionaries, `notify`
  and `map_alert` actions), replacing the first basic pattern parser (July to
  August 2026).
* Linux support: native dialogs through the XDG portal and machine
  identification through D-Bus / machine-id fallbacks (July 2026).
* Mock ESI calls and unit tests for `sde` and `webb` (July 2026).
* Tracing instrumentation across the application, with optional live profiling
  through Tracy behind the `profile` and `profile-memory` features, replacing
  puffin (August 2026).
* Documentation (September 2026):
  * `ARCHITECTURE.md`, with a crate map, a module map, the threads and
    channels, and the main flows, illustrated by six diagrams written in D2
    (sources and generated SVGs in `docs/architecture`).
  * Module-level documentation (`//!`) in every module that lacked it.
  * An expanded `BUILD.md`: requirements, ESI credentials, running, checks and
    a warning that the `wasm32` target is still under development.
  * This `CHANGELOG.md`.
* `ESI_CLIENT_ID` and `ESI_SECRET_KEY` placeholders in the `[env]` section of
  `.cargo/config.toml` (September 2026).

### Changed

* `app.rs` split from a single file of about 1,900 lines into focused modules
  (`windows`, `intel`, `watchdog`, `database`, `notifications`,
  `persistence`); behaviour is unchanged (September 2026).
* Message handling refactored around `Message` / `MessageSpawner`, with
  diagnostics routed through `tracing` (August 2026).
* ESI client id and secret key are read at compile time instead of runtime
  (2024); the `Settings` struct became fully private with explicit accessors
  (June 2026).
* Dependencies `sde` and `egui-map` updated (September 2026). They are now
  resolved from crates.io: the `[patch.crates-io]` entries that pointed at
  local sibling checkouts are commented out in the workspace `Cargo.toml`.
* `BUILD.md` documents Tracy v0.14.1 as the version verified to work, in line
  with `tracing-tracy` 0.12 and `tracy-client` 0.19 (September 2026).

### Fixed

* Intel watcher on Windows and Linux: content writes and file creation /
  removal are now recognised, and the watch is re-registered idempotently so
  saving the settings several times no longer repeats every log line
  (September 2026).
* File activity detection on macOS (September 2026).
* Chat logs are decoded as UTF-16LE, including the byte-order mark EVE writes
  on every appended line (August 2026).
* A chat log whose channel name contains an underscore was attributed to the
  wrong channel; file names that are not chat logs no longer stop the watcher
  (September 2026).
* Changing the intel directory in Settings left the channel list empty.
* Wrong name for the first point returned by `get_systempoint` in `sde` (July
  2026).
* Crashes when a character has no alliance or when the remove button is used
  without a linked character (September 2025).
* Vulkan errors on Windows with wgpu, by changing the dependency
  configuration (May 2026).

### Removed

* `puffin` profiling, replaced by Tracy (August 2026).
* The separate linked-characters window; its content moved into the Settings
  window (April 2024).

## History before this changelog

Development started in February 2023 with the map widget prototype. The ESI
back end was split into the `webb` crate in March 2023, the multi-tab map
layout (`egui_tiles`) and the regional maps arrived in early 2024, and the
native dialog code (including macOS) in mid-2026.
