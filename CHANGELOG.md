# Changelog

All notable changes to Telescope are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

There are no tagged releases yet. Everything so far is under *Unreleased* and
was reconstructed from the git history, grouped by topic and dated by the
period it happened in.

## [Unreleased]

### Fixed

* Adding nodes in the open rules editor could give two of them the same id
  (`PatternsEditor::rule_ids` ignored the canvas), which made closing the
  editor fail with a duplicate node id error (September 2026).

### Changed

* `.cargo/config.toml` is no longer tracked by git; the ESI credentials come
  from the build environment (`BUILD.md`). The values that had been committed
  were rotated (September 2026).
* `TelescopeApp::default()` builds the app through `with_settings`, which also
  serves the tests (no SDE update check, placeholder ESI credentials); the
  behaviour of the normal startup is unchanged (September 2026).
* CI and `check.sh` run clippy with `-D warnings`; the test fixtures that
  tripped it were rewritten (September 2026).

* The Settings screen, the SDE update window and the rule graph editor use
  egui's own look-and-feel (flat sections separated by hairlines, check box
  rows, small radii, no shadows except on floating windows) instead of a card
  based one; the pages fill the available width. *Alerts* no longer lists the
  characters the distance is measured from, and *Maps* picks start-up regions
  with toggle buttons (September 2026).

### Added

* Unit tests for the rule engine (`webb::rules`), the chat log reader, the
  rules editor, the SDE updater (against a local HTTP server), the character
  link helpers and `TelescopeApp` itself (`app_tests.rs`), and component tests
  for `egui-panels`. Line coverage of the workspace went from about 65 % to
  72 % (`cargo llvm-cov`, see `BUILD.md`) (September 2026).
* `egui-panels`, a new workspace crate with the building blocks of a settings
  screen in egui (page, `Section`, `form`, `switch`, `Slider`, `segmented`,
  `tile`, `PathPicker`, `stepper`, `EntityCard`, `SideNav`, `ActionBar`,
  `SettingsLayout`, `Draft`), themed from `egui::Visuals` so light and dark
  both work, with no dependency on Telescope (September 2026).
* Settings: the folder status and each channel's last activity (*Sources*),
  *Browse…* for the SDE and private database files and the SDE status
  (*Application*), *Test the full alert* (*Alerts*), a character glow
  intensity slider with a preview (*Maps*) and the live validity of the rule
  graph (*Rules*) (September 2026).

* Interface translations with `rust-i18n` and TOML files in
  `crates/telescope/locales/` (English and Spanish), a language selector in the
  new *Settings -> General* page and `language` in `[ui]` (`"auto"` follows
  the operating system); tests check every language has the same keys as
  English (September 2026).

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

* CI runs the tests (`cargo test --workspace`) on Windows and macOS, and a
  Linux workflow (check, test, clippy, fmt) replaces the disabled one;
  clippy covers the whole workspace with tests and examples (September 2026).
* `native_tools`: comments and error messages in English (September 2026).

* Settings redesigned with `egui-panels`: the pages follow an intel line
  (*Sources -> Rules -> Alerts*, with a stepper) and then *Maps*,
  *Characters* and *Application*, replacing *General*, *Intelligence*,
  *Patterns* and *Data Sources*. Every change, the interface language
  included, is a draft until *Apply* / *Accept* (the language is previewed and
  *Cancel* restores it); a dot marks the pages with changes and the bar lists
  them. Rule cards have an enable switch, a summary and their outputs, and a
  rule open in the node editor is applied too. Characters are cards with their
  location and an *Unlink* button each (September 2026).

* Noto Sans TC replaced by Noto Sans CJK (`NotoSansCJK-Medium.ttc`), always
  loaded as the fallback font so intel lines in Chinese, Japanese, Korean and
  Russian are drawn whatever the interface language (September 2026).
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

### Security

* The player database (tokens included) is always encrypted: Telescope now
  enables `webb`'s `crypted-db` itself (before, whether it was depended on
  the cargo command used to build). The key is a raw SQLCipher key derived
  from the machine identifier `native_tools` reads (on Linux it was a fixed
  placeholder), so opening a connection no longer runs the passphrase
  derivation. An existing plain database is encrypted and one under the old
  passphrase key is re-keyed on the first start (September 2026).
* EVE SSO login: the `state` of the callback is checked against the one sent
  in the login URL, and a callback with any other value is rejected before
  its code is used. Before, any web page could send the browser to the local
  callback during a login and link a character of its choosing (September
  2026).

### Fixed

* When the GPU device is lost (sleep, driver reset), the app now restarts
  itself instead of dying with a `egui-wgpu` panic (exit code 101). At most
  three restarts in a row, and only if wgpu reported the loss.
* Settings -> Rules: the buttons, switch and fields of an entry's card
  didn't respond (Open, Remove, the on/off switch, renaming): the card sensed
  clicks over its whole area on top of them. The same held for *Unlink* on
  the Characters page (`egui_panels::EntityCard`). The card's own click is
  now registered under its contents (September 2026).
* Settings -> Rules: the id field of an entry lost the focus at every key
  (the card's widgets were keyed by the id being edited). The id is now
  applied when the field is left, and an invalid or taken id is reported
  instead of silently put back (September 2026).

* File dialogs: the Windows open dialog only offered `*.rs` files (a leftover
  sample filter), so *Browse…* for the SDE and the private database showed
  nothing; it ignored the folder to open in; and it had no owner window, so
  the app's window kept taking input while it was open. Dialogs now have a
  title, SQLite filters where they pick a database, open in the current
  folder and, on Windows, are modal to Telescope's window (September 2026).
* Linux: the machine identifier behind the player database key is the
  machine id first (`/etc/machine-id`), not the DMI UUID only root can read:
  running Telescope once as root no longer changes the key. The D-Bus
  fallback never asks polkit for a password (September 2026).
* The private database path can be typed, so it can point to a file that
  doesn't exist yet (the open dialog can't pick one); an invalid path is
  shown as such and not applied (September 2026).

* The maps take their intel alerts every frame, even while their tab is
  hidden or the Settings screen is open: they used to read them only when
  drawn, and the 30-message channel dropped the rest (September 2026).
* The SDE update progress window shows over the Settings screen (September
  2026).
* An SDE update builds the new database next to the old one and replaces it
  only once complete: a failed download or build no longer leaves Telescope
  without `sde.db` (September 2026).
* Chat logs are read from the folder being watched, not from a folder picked
  in Settings but not applied yet; applying a new folder stops watching the
  old one (both used to stay watched), and a cancelled folder change no
  longer skips lines (September 2026).
* A report repeated within 3 seconds doesn't sound the alarm (or center the
  maps) again over the one still playing (September 2026).
* Each chat log write no longer adds a "Changed" line to the on-screen log,
  and the app message channel has room for bursts (40 -> 256) (September
  2026).
* No panics when a region's map was never created, when a portrait download
  breaks, or when the intel folder can't be watched at start-up (it is
  reported instead) (September 2026).
* The player database uses write-ahead logging (`journal_mode` was misspelt,
  so SQLite ignored it), and with `crypted-db` the key is set first, quoted,
  and a missing machine id falls back instead of panicking (September 2026).

* Settings: rescanning the chat log folder no longer unchecked every channel
  (Accept then stopped watching them all), and monitored channels without a
  log stay listed; unchecking every start-up region is saved (September 2026).

* Crash on the first frame when a character is linked and `sde.db` can't be
  loaded yet (first run, or while it is being rebuilt): map panes without
  systems no longer pass markers to egui-map 0.9.1, which panicked drawing
  them (September 2026).
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

* `patterns.toml` and its engine (`PatternEngine`): the intel rules are a node
  graph stored in the player database, seeded from the built-in `rules.toml`.
  The installer no longer ships the file and it is no longer copied to the
  data folder (September 2026).
* `puffin` profiling, replaced by Tracy (August 2026).
* The separate linked-characters window; its content moved into the Settings
  window (April 2024).

## History before this changelog

Development started in February 2023 with the map widget prototype. The ESI
back end was split into the `webb` crate in March 2023, the multi-tab map
layout (`egui_tiles`) and the regional maps arrived in early 2024, and the
native dialog code (including macOS) in mid-2026.
