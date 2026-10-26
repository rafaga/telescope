# Changelog

All notable changes to Telescope are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the versions are
the git tags of the repository.

The entries up to *1.0.0-rc01* were reconstructed from the git history, grouped
by topic and dated by the period they happened in. The entries of the later
versions were written for the tag that shipped them.

## [Unreleased]

### Added

* Overlay: a panel over the EVE window that lists the events near the linked
  characters, no longer behind a cargo feature. Every intel report or killmail
  in a system within the alarm radius shows as a row (jumps, system, summary,
  age), newest first, colored by distance; with nothing to show the locked
  overlay paints nothing. The panel is semi-transparent: Turn it on and set its
  opacity in Settings -> Alerts.

## [1.0.6] - 2026-10-23

### Changed

* Player database schema 4: the default intel rules changed after schema 3
  shipped (1.0.5), so databases at version 3 still held the old ones. The
  migration replaces the stored rules with the current default rules, without
  keeping a copy of the old ones, and drops the `node_v2_backup` and
  `edge_v2_backup` tables left by schema 3. A database created from scratch is
  left alone. Rules edited by hand in a schema 3 database are replaced by the
  defaults and cannot be recovered after migrating (October 2026).
* Upgraded the GUI stack to egui 0.37: `eframe`, `egui_extras`, `egui_tiles`
  0.18 and `egui-map` 0.15. `egui-panels` follows the new API: widget ids use
  `Id::unique` and `ui.scope_id()`, accessibility roles use `Role` (navigation
  entries are tabs, segmented options radio buttons, switches, tiles and
  toggles check boxes), the path picker aligns its text with
  `TextEdit::align`, and `SettingsLayout::new` accepts any `Hash + Debug`
  salt (October 2026).
* `egui-snarl` is temporarily taken from the `telescope` branch of
  `rafaga/egui-snarl` (egui 0.37 support) until the upstream release is
  published.

### Fixed

* Emojis are drawn again. egui 0.37 no longer bundles its emoji fonts by
  default, so the settings page icons, the node tooltip icons and the message
  icons were missing. The `monochrome_emoji_fonts` feature of egui is enabled
  to bundle them (October 2026).

## [1.0.5] - 2026-10-18

### Changed

* The rule engine is independent of the format of its input. It evaluates an
  `IntelEvent` (`webb::event`, which replaces `LineContext`): the text of the
  event, which the text detections read, and the `Facts` its source knows by
  itself (solar systems, ships, pilot count). Each source decodes its own format
  through the `EventSource` trait (`ChatLogDecoder` for chat lines,
  `ZKillboardDecoder` for killmails, which are no longer flattened to a sentence
  and searched again). *System report*, *Ship names* and *Pilot count* read the
  facts when the source gave them (`DetectionType::fact`) and the text when it
  did not: a killmail now reports a pilot count over 999, every ship that took
  part (not only the first eight attackers') and a ship named like a system,
  which the sentence could not carry. A fact the source gave is final, even when
  empty. Chat lines have no facts and are read exactly as before (October 2026).
* Ships are identified by their SDE type id. `Facts.ships` and `Data::Ships`
  carry `ShipCount { type_id, name, times }` instead of a name and a count: a
  killmail fills the id, and the ships of a chat line get theirs from the new
  `ShipResolver` (`SystemResolver::ship_resolver`), backed by the published
  ship types of the SDE (`SdeShips`, reloaded when the database file changes).
  The same ship has the same id whatever the source, and ships are merged by
  id when both sides have one and by name otherwise. A name the SDE does not
  know (a nickname, a typo) stays known by its name only. The map tooltips and
  the log text are unchanged. `sync_zkillboard` is now `sync_sde_path` (October
  2026).
* An `IntelEvent` records when it happened (`IntelEvent::at()`): the date of
  the line in the chat log or the time of the killmail, and where it comes from
  (`TimeOrigin::Reported` or `Assumed`). EVE's default settings can leave the
  date out of the chat logs: such a line used to be dropped without a word, and
  now it is read like any other (`IntelLine::parse_or_assume`), taking the moment
  it was read, and Telescope warns once per log in the status log. A killmail
  whose time can't be read is treated the same way, with a warning (October
  2026).
* A Detection node chooses what it reads (`DetectionNode::source`, the *Reads*
  field of its panel): *facts or text* (the default: the facts of the event when
  its source gave them, the text otherwise), *facts only* (an event without that
  fact has nothing for it) or *text only*. Only *System report*, *Ship names* and
  *Pilot count* offer it, the validator rejects *facts only* on any other, and
  saved graphs without the field load as *facts or text* (October 2026).
* **Breaking: the node graph works differently (engine v2).** A wire no longer
  carries a true / false signal per pin: it carries "the event goes on" and the
  findings gathered by the nodes it passed. Every node has a single output.
  Chaining nodes is an AND; the *Aggregator* is now a *Union* (the event goes on
  when it reaches any input, findings merged without repeats); a Detection has a
  *Mode*, *Requires* (default) or *Excludes* (the event goes on unchanged when
  it finds nothing), which replaces the NOT. A Detection always reads the event
  itself, never what an earlier node found. The Output fires when it is reached,
  with all the findings. The default rules were rewritten without gates (the
  tooltip rule is `union -> not_status (Excludes) -> tooltip`).
  Saved graphs of the previous model cannot be converted: database schema 3
  keeps the old graph as it was in the tables `node_v2_backup` and
  `edge_v2_backup` and replaces it with the default graph, and a `rules.toml`
  exported before this version, with gates or `from_pin`, does not import any
  more (October 2026).

### Removed

* The AND / OR / XOR / NOT gate nodes (`NodeKind::Gate`, `GateKind`), the `F`
  (false) output of the Detections, `Edge::from_pin`, `Pin`, `Signal` and the
  `LINE_TAG` marker. XOR has no replacement (October 2026).

## [1.0.4] - 2026-10-14

### Added

* zKillboard input: an Input node of kind *zKillboard* (the source selector of
  *Settings -> Rules*) feeds the graph with the live killmails of zKillboard's
  R2Z2 feed, the successor of RedisQ. Each killmail becomes a line such as
  `Jita: Rifter destroyed by 3 pilots (Sabre, Sabre, Merlin), 12.3M ISK`, so
  the *System report*, *Ship names* and *Pilot count* detections work on it as
  on a chat line. The feed's files already carry the whole ESI killmail, so
  the only lookups are the system and ship names, from the local SDE. The
  source runs on its own thread (`intel/zkillboard.rs`) only while an enabled
  zKillboard input exists, starts at the end of the feed (history is not
  replayed), announces a killmail once, follows zKillboard's polling rules
  (a 6 second wait at the end of the feed, a `User-Agent`, a long pause after
  a 403/429) and reports an outage once to the status log. The rule engine
  delivers each line only to the Input nodes of its own kind (`LineContext::kind`,
  `InputEvent::kind`), so a killmail never reaches a chat-log input. The
  feed is unfiltered: wire detections after the node to pick what matters
  (October 2026).
* Experimental overlay window above the game (cargo feature `overlay`, Windows,
  macOS and Linux/X11): a test window that follows the EVE client, scaled to
  its height, and is shown only while the game is in front. A positioning mode
  with an opaque title bar lets it be dragged and a padlock button locks it
  (title bar hidden, the mouse goes to the game); both are toggled from the
  Debug window. `native_tools::overlay` provides the game window tracker, the
  anchored placement, the topmost click-through window that stays out of the
  task switcher and over macOS full screen Spaces, exclusive full screen
  detection, and a window region (`SetWindowRgn` / X11 Shape) that shows only
  what is painted, so no transparent swapchain is needed.
  `native_tools::permissions` checks the macOS accessibility permission
  (October 2026).
* A housekeeping thread runs a small schedule. It reads the last login of every
  linked character (`GET /characters/{id}/online`, scope
  `esi-location.read_online.v1`) at start-up, when a character is linked and
  then every hour, and stores it in the character and the player database only
  when it changed (`Message::PlayerLastLogon`, `EsiApi::get_last_login`). It
  also runs the update check at start-up and every 6 hours. Failures are never
  fatal: the thread doesn't refresh tokens (the watchdog does), reloads them
  from the player database and retries after 2 minutes when they aren't valid
  yet, and a failed update check warns once per streak and retries after 30
  minutes (October 2026).

### Changed

* The new version dialog opens only for a release that wasn't announced before
  (`UpdateChecker::announce` replaces `UpdateChecker::spawn`), so the periodic
  check doesn't nag (October 2026).
* The tabs of the maps have a folder-style outline with the close button inside
  the tab (October 2026).
* The chat log polling sweep is Windows-only (`cfg(target_os)`); the
  `intel-polling` feature is gone (October 2026).
* macOS: `native_tools` uses the safe `objc2-core-graphics` and
  `objc2-application-services` bindings for the window list, the shielding
  window level and the accessibility trust check, instead of hand-written
  `extern` blocks (October 2026).
* The workspace version is 1.0.4. The dependency requirements of `telescope`,
  `webb` and `native_tools` are declared at minor level (`"1.53"` instead of
  `"1.53.2"`), so patch releases are picked up without editing the manifests.
  `tokio` is 1.53 and `Cargo.lock` was refreshed (`jni` 0.52 -> 0.59, `cesu8`
  1.0.3, `windows_x86_64_msvc` 0.8.4; the `windows-targets` and `windows_*`
  0.42.2 entries are gone) (October 2026).
* Audio uses `rodio` 0.23: stream errors are matched through `rodio::CpalError`
  / `CpalErrorKind`, and `is_device_loss` no longer treats `Xrun` or
  `DeviceChanged` as a lost device (October 2026).

### Removed

* `index.html`, the leftover eframe/trunk template (October 2026).

### Security

* `rustls` 0.23.46 (`Cargo.lock`), which fixes RUSTSEC-2026-0346, -0347,
  -0348 and -0349 reported by `cargo deny check`: client session values
  exposing TLS secrets through `Debug`, application data sent to the outer
  server after an ECH rejection, the Brotli certificate decompressor
  allocating up to 1 GiB, and a ServerHello that could downgrade later
  connections to classical key exchange. It is reached through `reqwest`
  and `ureq` (October 2026).

## [1.0.3] - 2026-10-11

### Added

* `intel-polling` feature: besides handling watcher events, the reader thread
  sweeps the recent logs of the monitored channels every second. A sweep costs
  one `stat` per log with nothing new and runs on the reader thread, so it
  cannot duplicate a line. `build.rs` turns it on for Windows targets and other
  systems opt in with `--features intel-polling` (`BUILD.md`) (October 2026).

### Changed

* The universe and region maps show the minimap and the slider, and lines start
  to be drawn at zoom 0.10 (`line_visible_zoom`). `egui-map` is 0.14 (October
  2026).
* The workspace version and `packager.json` (which still said 0.0.1) are 1.0.3.
  The chat log watcher logs its errors, such as a Windows buffer overflow,
  instead of ignoring them (October 2026).

### Fixed

* Windows: new chat lines were not detected while EVE kept the log open.
  `ReadDirectoryChangesW` can defer or never report changes to a file another
  process keeps open and appends to; the polling sweep covers it (October
  2026).

## [1.0.2] - 2026-10-09

### Changed

* Telescope identifies itself to ESI with a real `User-Agent`,
  `telescope/<version> (+https://github.com/rafaga/telescope)`, instead of
  `telescope/dev`. The version is the crate's, so it follows each release
  (October 2026).
* The EVE SSO login uses PKCE only and Telescope no longer has a client
  secret: `ESI_SECRET_KEY` is gone from the build (`AppData`, `BUILD.md`,
  `release.yml`) and from the executable, the `native-auth-flow` feature of
  `webb` is now the only flow, and `EsiManager::new` takes no secret. Builds
  before this one still carry the old secret, so it should be rotated in the
  developer portal once users have updated (October 2026).
* The ESI client is `esi-openapi` 0.2.0 (the fork of `rfesi`), and the unused
  `esi-openapi` 0.1.0 dependency of the `telescope` crate is gone. ESI defines
  every id as `int64`, so characters, corporations, alliances and solar systems
  are `i64` throughout (`Character`, `EsiApi`, the player database, the
  watchdog, the maps' markers) instead of `i32`. A character whose id does not
  fit in 32 bits can now be linked. The database columns were already 64-bit
  integers, so the schema version stays at 2 and nothing is migrated
  (October 2026).

## [1.0.1] - 2026-10-07

### Added

* Update checker: at start-up Telescope asks GitHub for a newer release and,
  when there is one, opens a modal with a button to the release page and a
  *Cancel* button. Nothing is written to the log (October 2026).
* The Debug window has a tab per tool (*Search*, *Intel*, *Alerts & log*,
  *Animations*, *Characters*, *State*) and the *Intel* tab has four preset chat
  lines (a clear report, hostiles with ships, a status query, unmatched
  chatter). *Send* now pushes the line into the input of the detection stage,
  like the log reader does, so alerts, sounds, tooltips and log entries fire;
  the new *Dry run* box keeps the old rules-only evaluation (October 2026).
* Settings -> Application: a switch to hide debug messages from the on-screen
  log. Only the printing is skipped; the messages are still received,
  deduplicated and handled. It is saved in a new `[log]` table of
  `telescope.toml` and defaults to the build type: shown in debug builds,
  hidden in release ones (October 2026).

### Changed

* The ESI client is `esi-openapi` instead of `rfesi`, with the ESI spec
  embedded because CCP removed `/latest/swagger.json`. The embedded spec is
  keyed by the current OpenAPI operation ids the endpoint groups look up; the
  legacy snake_case ids made every ESI call fail with "Could not resolve
  operationId" (October 2026).
* The *Cancel* and *Accept* texts of Settings, the SDE update window and the
  new version modal come from a common locale section (`common.cancel`,
  `common.accept`). The `README.md` links to the Discord server and shows a
  macOS screenshot (October 2026).

## [1.0.0] - 2026-10-04

### Changed

* The universe map's intel alert pulses once instead of repeating for the
  alert's duration; the region maps keep the repeating pulse, and the hint in
  Settings says which map does what (October 2026).

### Removed

* The context menus of the universe and the region maps (`ContextMenu` and its
  locale strings `map.set_beacon` and `map.settings`): their entries only
  closed the menu (October 2026).

## [1.0.0-rc03] - 2026-10-04

### Added

* Tests of the intel pipeline with lines the way players write them (about
  forty variants of system names), through the default rules and the real
  resolver: what is read, what is not, what is left over as unrecognised text,
  and the limits of eight reported systems per line and a hundred matches per
  detection (September 2026).
* The SDE database is updated from `sde-deltas`: `DatabaseUpdater` first asks
  `sde::builder::update::prepare`, which brings the mirror of the SDE kept by
  the previous full build up to date with the build-to-build deltas. `sde.db`
  is then left alone, has only its recorded build moved, or is rebuilt from the
  mirror without downloading CCP's export. When the deltas can't be used the
  full download runs as before and leaves a mirror for the next update. A
  schema change in fields the parser reads is reported as a warning, and the
  update window shows the deltas applied and why a full download was needed
  (October 2026).
* A link to the Ko-fi page in the About window (October 2026).

### Changed

* Systems and ships are detected whatever their case. `h-5gui`, `JITA` and
  `old man star` now raise the same alert as `H-5GUI`, `Jita` and `Old Man
  Star`. The code-like names (`J105443`, `1DQ1-A`, `AD001`) are matched by a
  case-insensitive pattern; the names made of words are looked up: every run
  of up to three words is tried against the universe, longest first, so
  `hostile in jita` finds `Jita`. Ship dictionaries compare with Unicode case
  folding, so `CÁPSULA` matches `Cápsula` (before, only ASCII letters changed
  case). The limit of eight systems per line now counts the systems the line
  reports, not the words that were tried. The case-insensitive switch of the
  system detection node no longer applies and is hidden in the rule editor
  (September 2026).
* `sde` 0.7.1 and `egui-map` 0.13, both from crates.io. `sde` keeps the mirror
  of the SDE as a single zip and unpacks it only to apply deltas or rebuild;
  the updater releases the unpacked copy afterwards (also when the rebuild
  fails or is cancelled) and removes the downloaded export after a full build,
  so what stays on disk is the database, the mirror's zip and dotlan's maps
  (October 2026).
* Settings -> Application: the SDE database path is typed by hand like the
  player database's, keeping what is typed while it isn't valid, and it can
  point at a file that doesn't exist yet in an existing folder, which the
  updater then builds. A folder is refused, and an existing file must be empty
  or a SQLite database, so a file typed by mistake is never replaced (October
  2026).
* A chat log folder that isn't there is shown in *Sources* as a warning that
  asks for a manual pick, not as an error: Telescope looks for EVE's folder on
  its own, so not finding it only asks the user to choose one (October 2026).
* Release packaging: the macOS app is signed ad-hoc (`macos.signingIdentity` in
  `packager.json`, because `cargo-packager` ignores `APPLE_SIGNING_IDENTITY`),
  the job checks the signature of the dmg and of the app inside it and fails if
  it is unsigned, and the WiX installer is built only for en-US (October 2026).

### Fixed

* `release.yml` attached nothing to the release: every job listed both `*.msi`
  and `*.dmg` with `fail_on_unmatched_files`, so each one failed on the pattern
  of the other operating system. Each job now uploads only the installer its OS
  produces (October 2026).

## [1.0.0-rc02] - 2026-09-30

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

### Changed

* Documentation brought up to date with the code (September 2026):
  `ARCHITECTURE.md` (module map, the intel pipeline on its own threads, the
  channels, platform services and diagnostics, tests, how to regenerate the
  diagrams), the six D2 diagrams and their SVGs (now rendered with the `elk`
  layout), a `README.md` with working CI badges and the current features, a
  rewritten `crates/webb/README.md`, a new `crates/native_tools/README.md`, an
  *Installers* section in `BUILD.md`, the stale names of pages, functions
  and messages in comments and in `assets/telescope.default.toml`, and new
  screenshots of the current interface (`docs/screenshots`, English, taken on
  Windows 11) in the `README.md`.
* CI rebuilt around one `ci.yml`: a single job per operating system runs
  clippy, the all-features check and the tests on a shared build, `fmt` runs
  once, and there is a `ci-ok` summary check, run cancellation, least
  privilege permissions and timeouts. The build cache is written only by
  pushes to `master`. Pushes to `dev` are no longer built twice (the pull
  request already covers them), and `master` gets a coverage report
  (`cargo llvm-cov`) (September 2026).
* `Cargo.lock` is versioned and CI and the release build with `--locked`
  (September 2026).
* `release.yml` fails early without the ESI secrets, caches `cargo-packager`,
  publishes SHA-256 checksums, and can be run by hand to try the packaging
  without publishing (September 2026).
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

### Removed

* `Trunk.toml`: the experimental web build is run with
  `trunk build --filehash false` (`BUILD.md`) (September 2026).
* `patterns.toml` and its engine (`PatternEngine`): the intel rules are a node
  graph stored in the player database, seeded from the built-in `rules.toml`.
  The installer no longer ships the file and it is no longer copied to the
  data folder (September 2026).

### Fixed

* Adding nodes in the open rules editor could give two of them the same id
  (`PatternsEditor::rule_ids` ignored the canvas), which made closing the
  editor fail with a duplicate node id error (September 2026).
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

### Security

* `security.yml` runs `cargo deny check` (advisories, licenses, bans, sources)
  on manifest changes and weekly; `deny.toml` was adapted to the actual
  dependency tree, with the two advisories that have no fix documented, and
  the yanked `yoke-derive` was updated (September 2026).
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

## [1.0.0-rc01] - 2026-09-25

### Added

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

### Removed

* `puffin` profiling, replaced by Tracy (August 2026).
* The separate linked-characters window; its content moved into the Settings
  window (April 2024).

### Fixed

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

## History before this changelog

Development started in February 2023 with the map widget prototype. The ESI
back end was split into the `webb` crate in March 2023, the multi-tab map
layout (`egui_tiles`) and the regional maps arrived in early 2024, and the
native dialog code (including macOS) in mid-2026.

[Unreleased]: https://github.com/rafaga/telescope/compare/1.0.6...HEAD
[1.0.6]: https://github.com/rafaga/telescope/compare/1.0.5...1.0.6
[1.0.5]: https://github.com/rafaga/telescope/compare/1.0.4...1.0.5
[1.0.4]: https://github.com/rafaga/telescope/compare/1.0.3...1.0.4
[1.0.3]: https://github.com/rafaga/telescope/compare/1.0.2...1.0.3
[1.0.2]: https://github.com/rafaga/telescope/compare/1.0.1...1.0.2
[1.0.1]: https://github.com/rafaga/telescope/compare/1.0.0...1.0.1
[1.0.0]: https://github.com/rafaga/telescope/compare/1.0.0-rc03...1.0.0
[1.0.0-rc03]: https://github.com/rafaga/telescope/compare/1.0.0-rc02...1.0.0-rc03
[1.0.0-rc02]: https://github.com/rafaga/telescope/compare/1.0.0-rc01...1.0.0-rc02
[1.0.0-rc01]: https://github.com/rafaga/telescope/releases/tag/1.0.0-rc01
