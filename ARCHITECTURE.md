# Architecture

Telescope is a desktop application (egui/eframe with the wgpu renderer) that
watches the chat logs of EVE Online, evaluates their text against configurable
pattern rules and shows the resulting alerts on interactive maps of New Eden.
Characters linked through EVE SSO are followed through ESI so the maps can show
where they are and warn about nearby threats.

This document is a map of the code: where things live and how they talk to
each other. For how to build and run it see [BUILD.md](BUILD.md).

## Workspace

| Crate | Path | Role |
|-------|------|------|
| `telescope` | `crates/telescope` | The application: a binary plus a small library (`TelescopeApp`). UI, settings, file watching and alerting. |
| `webb` | `crates/webb` | EVE back end with no UI: the local OAuth callback server, the ESI client, the local player database, the intel rules (`graph`, `rules`, `intel`) and the map-tooltip `map_alerts`. |
| `native_tools` | `crates/native_tools` | OS-specific code: native file / folder dialogs and per-machine identification. |
| `egui-panels` | `crates/egui-panels` | Building blocks for settings screens in egui (pages, sections, forms, switches, sliders, tiles, side navigation, action bar, `Draft`), with no dependency on the rest of the workspace so other projects can use it. |

![Crate dependencies: telescope depends on webb and native_tools in this workspace, and on the external sde and egui-map crates](docs/architecture/crates.svg)

Two more crates by the same author, published on crates.io, are used from outside this workspace:

* `sde`: parses CCP's Static Data Export, builds the SDE database and answers
  universe queries (`SdeManager`, `Universe`).
* `egui-map`: the egui widget that draws the maps.

Both are resolved from crates.io. The workspace `Cargo.toml` keeps
commented-out `[patch.crates-io]` entries that point at sibling checkouts
(`../sde` and `../egui-map`): uncomment them to develop those crates together
with Telescope.

### `crates/webb`

| Module | Purpose |
|--------|---------|
| `auth_service` | Tiny `hyper` server that receives the SSO redirect (`/login?code=...&state=...`) and hands both values to the application. |
| `esi` | `EsiManagerCore`: authorization, token refresh, location and portrait queries, and reading / writing characters, corporations and alliances. |
| `esi/player_database` | SQLite schema and queries of the player database (encrypted with SQLCipher under the default `crypted-db` feature). |
| `esi/cipher` | Encryption of the player database (`crypted-db`, enabled by Telescope): a raw SQLCipher key, SHA-256 of the machine identifier from `native_tools`; before the first open it encrypts a plain file or re-keys one under the older passphrase key. |
| `esi/data` | ESI client configuration. |
| `objects` | Domain types: tokens and the `Character`, `Corporation` and `Alliance` entities. |
| `graph` | The intel node graph: typed `Node`s (Input/Detection/Output/Aggregator/Gate/Formatter) and `Edge`s, `RuleGraph` validation, the per-line `Executor` (which propagates a true / false / absent signal per pin: a Detection's T is true when it matched, F when it did not; absent means "not evaluated", and a `not` of it stays absent), the embedded default graph (`default_graph`, from `rules.toml`) and the `SystemResolver`. This is the model persisted in the player database and edited by *Settings -> Rules*. |
| `rules` | The typed building blocks the graph shares: `DetectionRuleKind`/`DetectionType`, `OutputKind`/`OutputType`, `InputKind`, the built-in `Dictionaries`, `IntelLine` and `parse_line`. |
| `intel` | What the intel modules share: `IntelLine`, `IntelCategory`, `PatternError` (rule validation errors), the size limits and text helpers. The old `patterns.toml` engine (and the file) were removed; the graph replaced them. |
| `map_alerts` | `AlertSummary::from_messages` condenses a line's `Mensaje`s into what a map node's tooltip shows; `AlertLog` keeps each system's active alerts, deduplicated and expiring on their own. |

### `crates/native_tools`

| Module | Purpose |
|--------|---------|
| `dialog` | Open file / folder dialogs: `IFileOpenDialog` on Windows, `NSOpenPanel` on macOS, the XDG desktop portal on Linux. |
| `zbus` (Linux only) | Machine identification through DMI, D-Bus and machine-id fallbacks. |
| `lib.rs` | Per-OS `get_*_unique_id` functions. |

## The `telescope` crate

```text
crates/telescope
├── build.rs                 embeds the Windows icon; writes the third-party license list
│                            (from Cargo.lock) that the Licenses window shows
├── locales/                 interface texts, one TOML file per language
└── src
    ├── main.rs              entry point: logging / tracing setup, working directory, opens the window
    ├── lib.rs               exports TelescopeApp; loads the translations
    ├── app_dirs.rs          where the files live: next to the program or in the per-user data folder
    ├── gpu_diagnostics.rs   records why the GPU stopped and relaunches after a lost device
    ├── i18n.rs              interface language: available languages, "auto", switching
    ├── log_bridge.rs        copies WARN / ERROR of dependencies into the status log
    ├── repaint.rs           wakes the UI from other threads (request_repaint)
    ├── app.rs               TelescopeApp: state, construction, per-frame ui(),
    │                        event_manager() and map pane management
    └── app/
        ├── app_tests.rs     tests that build a TelescopeApp on a temporary folder (see "Tests")
        ├── audio.rs         the alarm sound: an audio thread that owns the output device
        ├── character_link.rs  linking / unlinking characters, keeping the watchdog in sync
        ├── data.rs          static ESI application data (scopes, callback URL, keys)
        ├── database.rs      SDE build cache directory, reload after an SDE update, region list
        ├── database_updater.rs  progress window + background SDE check / build
        ├── file.rs          notify event handler for the chat log directory
        ├── intel/           the intel pipeline (see "Chat log -> alert")
        │   ├── mod.rs       TelescopeApp methods: apply_intel_settings(), scan_intel_files(),
        │   │                apply_graph(), sync_alarm_shared() / sync_alarm_jumps()
        │   ├── input.rs     IntelLogName, UTF-16LE decoding, ChatLogSource -> InputEvent,
        │   │                IntelOffsets (where each monitored log was last read)
        │   ├── reader.rs    the reader thread: changed log -> appended lines
        │   ├── detection.rs the detection thread (graph Executor behind an RwLock)
        │   ├── dispatch.rs  the dispatch thread: map messages, sound, status log
        │   └── resolve.rs   UniverseResolver / SharedResolver (system ids) and jump distance
        ├── messages.rs      Message, MapSync, CharacterSync, spawners and send helpers
        ├── notifications.rs the on-screen status log
        ├── persistence.rs   save_settings()
        ├── settings.rs      Settings: paths, map options, channels; persisted to a TOML file
        ├── tiles.rs         egui_tiles panes: UniversePane, RegionPane, TreeBehavior
        ├── watchdog.rs      background location polling of the linked characters
        ├── windows.rs       secondary windows
        └── windows/
            ├── about.rs     About window
            ├── debug.rs     debug window (debug builds only)
            ├── licenses.rs  third-party licenses window (Help menu)
            ├── settings.rs  Settings screen (egui-panels): navigation, pages, Cancel / Apply / Accept,
            │                which pages have changes not applied
            └── settings/
                ├── sources.rs      chat log folder and watched channels, with their activity
                ├── patterns.rs     Rules: input cards + node editor, live graph validity
                ├── alerts.rs       alert distance, map alert, sound, "Test the full alert"
                ├── maps.rs         start-up regions, character glow intensity
                ├── characters.rs   linked characters
                └── application.rs  interface language, SDE and private database paths
```

The interface texts live in `crates/telescope/locales/<code>.toml` (see
[Languages](#languages)).

![Module map of the telescope crate: entry points, app.rs with TelescopeApp, and the user interface, intel pipeline, EVE / ESI and shared core groups](docs/architecture/modules.svg)

`TelescopeApp` is one struct; the modules above only add `impl TelescopeApp`
blocks (or free helpers), so the state stays in a single place and every
submodule can reach it.

## Runtime files

Telescope reads and writes these in its working directory: the folder it runs
from, or the per-user data folder when it is installed (see
[Platform services](#platform-services-and-diagnostics)):

| File | Content |
|------|---------|
| `telescope.toml` | User settings (`Settings`). |
| `rules.toml` | Optional export/import of the whole rule configuration (*Settings -> Rules*). |
| `sde.db` | The SDE database. Built automatically when it does not exist. |
| `sde-build-cache/` | Next to `sde.db`: the downloaded SDE zip, the build number that avoids downloading it again, and the extracted tree. The new database is built as `sde.db.building` and replaces `sde.db` only when complete. |
| `telescope.default.toml` | The settings template shipped with the installer; copied as `telescope.toml` the first time an installed Telescope runs. Only read. |
| player database | Linked characters and one OAuth token set per character (`telescope.db` by default; the path is set in *Settings -> Application*), plus the intel rules (the `node`/`edge` graph tables). Its schema version is stored in `metadata`: on startup a database from an older version only gets the pending migration scripts (`MIGRATIONS` in `player_database.rs`), keeping its data, and the user is notified. A new database is created with the base schema (version 0) followed by every migration, so both paths end in the same schema. |

## Languages

Every text the user reads comes from `t!("section.key")` (`rust-i18n`), looked
up in `crates/telescope/locales/<code>.toml`: one file per language, all with
the same keys, embedded in the binary at compile time. A key missing from a
language falls back to English, but an empty value is shown blank. The `i18n`
tests check that every file has exactly the keys of `en.toml`, and the same
`%{placeholders}` in every value that isn't empty.

- **Adding a language** is adding its file (copy `en.toml`, translate the
  values, including `language.name`). *Settings -> Application* lists it on its own
  once `language.name` has a value. English (`en.toml`) and Spanish (`es.toml`)
  are included.
- **The chosen language** is `language` in `telescope.toml`'s `[ui]` table:
  `"auto"` (the operating system's language, English when there is no file for
  it) or a file name such as `"es"`. It is applied at start-up and as soon as
  it changes in *Settings -> Application* (a preview: *Cancel* goes back to
  the previous language), and saved with the other settings on *Apply*.
- **Not translated:** `tracing` output, the log panel messages and the Debug
  window, so bug reports read the same in every language.
- **Fonts:** Noto Sans CJK (`assets/NotoSansCJK-Medium.ttc`, Simplified
  Chinese face) is always the fallback after egui's own fonts, whatever the
  interface language: intel channels carry Chinese, Japanese, Korean and
  Russian lines. `TelescopeApp::font_definitions` sets it up and a test checks
  those scripts are drawn.

## Threads and messages

The UI runs on the main thread: every frame `TelescopeApp::ui` first drains
`event_manager()` and then draws. Anything slow or blocking runs elsewhere, on
its own thread with a small Tokio runtime, and reports back with messages:

![Threads and channels: background threads send Message to the UI thread, which sends MapSync to the map panes and CharacterSync to the watchdog](docs/architecture/messaging.svg)

| Channel | Type | Direction |
|---------|------|-----------|
| `Message` | `mpsc` | anything -> the app (`event_manager` dispatches it) |
| `MapSync` | `broadcast` | the dispatch thread (and the UI's test alerts) -> every map pane (center on, system alert, tooltip entry) |
| `CharacterSync` | `mpsc` | the app -> the watchdog (add / remove a linked character) |
| changed log names | `std::sync::mpsc` | the file watcher -> the reader thread |
| `InputEvent` | bounded `mpsc` | the reader -> the detection thread |
| `DetectedLine` | bounded `mpsc` | the detection thread -> the dispatch thread |

State shared instead of sent: the rule `Executor` (`RwLock`, swapped by the UI),
the `AlarmShared` snapshot the dispatch thread reads, the `IntelOffsets` and the
watched folder the reader uses, and the channel filter the watcher reads.

From the UI thread use `MessageSpawner::spawn`; from async code use
`send_app_message`. `Message::kind()` gives a short name for tracing without
logging the payload. A character's marker is not a `MapSync`: when the watchdog
reports a new location the app updates every pane directly
(`update_player_location`), so a hidden tab cannot fall behind the broadcast.

Background threads: the `notify` watcher callbacks (`file.rs`), the intel reader,
detection and dispatch threads, the audio thread, the OAuth callback server
(`AuthSpawner`), the watchdog and the `DatabaseUpdater`.

## Main flows

### Chat log -> alert

![Sequence diagram: from a line appended to a chat log to a notification or a map alert](docs/architecture/flow-intel.svg)

The pipeline runs on its own threads, so a stalled or hidden window never delays
an alarm: *watcher -> reader -> detection -> dispatch*.

1. `IntelEventHandler` (`file.rs`, in the `notify` thread) receives filesystem
   events for the intel directory. The name of a written monitored channel's log
   goes straight to the reader thread; files appearing or disappearing become
   `Message::ScanIntelFiles`. Channels are recognised with `IntelLogName::parse`,
   the only place that knows the log file name format.
2. The reader thread (`intel/reader.rs`) reads only the complete lines added since
   the last read (`IntelOffsets`: a monitored log starts at its end when first
   seen, so history is never replayed; a rescan never moves a known offset),
   decodes them from UTF-16LE (`intel/input.rs`, `ChatLogSource`) and emits one
   `InputEvent` per parsed line to the detection thread. The channel's last
   activity goes back to the UI as `Message::ChannelActivity` (the *Sources*
   page shows it).
3. The detection thread (`intel/detection.rs`) runs the graph `Executor` over the
   line and sends the resulting `DetectedLine` (the `Activation`s) on. The
   executor sits behind an `RwLock`: the UI swaps it in place when the rules
   change (`apply_graph`), without restarting the thread. The systems a Detection
   reports are resolved through the injected `SystemResolver` (`intel/resolve.rs`,
   backed by the SDE).
4. The dispatch thread (`intel/dispatch.rs`) acts on the Output nodes that fired:
   *visual* pulses the map node (`MapSync::SystemAlert`), *sound* plays the alarm
   (through the `AudioHandle` of `audio.rs`, which owns the output device on its
   own thread) and centers the maps (`MapSync::CenterOn`), *log* sends a
   `GenericNotification` shown in the status log (`notifications.rs`), *tooltip*
   lists the line in the node tooltips (`MapSync::SystemTooltip`), and *suppress*
   vetoes the visual and sound alerts. Everything it needs from the application
   (alarm settings, the characters' locations, the stargate jump graph) is an
   `AlarmShared` snapshot that the UI thread refreshes (`sync_alarm_shared`,
   `sync_alarm_jumps`), so the alarm neither reads UI state nor waits for a frame.

Every sender of work for the UI (app messages, the threads above, the log
bridge) wakes it with `repaint::request`, so nothing waits for the next mouse
move. While the window is minimized eframe calls `TelescopeApp::logic` instead of
`ui`, which still drains `event_manager`.

### Linking a character

![Sequence diagram: linking a character through EVE SSO until the watchdog starts](docs/architecture/flow-character.svg)

1. *Settings -> Characters -> Link character* asks `EsiManager` for the authorize URL,
   hands `AuthSpawner` an `AuthRequest` (a clone of the `EsiManager` plus that
   authorize info) and opens the URL in the browser.
2. The `AuthSpawner` thread runs `webb::auth_service` on
   `http://localhost:56123/login`. When the redirect's `code` and `state`
   arrive, that same background thread completes the authorization with its
   `EsiManager` clone (`EsiManager::auth_user`: the `state` must be the one
   sent in the login URL, then token exchange, character,
   corporation and alliance lookups, and storing the character), so the UI
   thread never waits on the network.
3. The result comes back as `Message::CharacterAuthenticated`.
   `handle_character_authenticated` (`character_link.rs`) takes that
   character's tokens from the clone (`EsiManager::adopt_session`), keeps the
   character (a re-linked character replaces its old entry), and starts the
   watchdog if it is the first one; otherwise it sends `CharacterSync::Add`
   (the watchdog then reloads the stored tokens).

### Location tracking

![Sequence diagram: the watchdog polling ESI for a character's location and updating the maps](docs/architecture/flow-location.svg)

`start_watchdog` polls ESI every 30 seconds (and at once when a character is linked). Each character is asked for its
location with its own token (an EVE SSO token only works for the character
that logged in), refreshed when needed. A change produces
`Message::PlayerNewLocation`, and the app stores the new location and moves the
marker on every map pane. If a character's token
is rejected (401/403) or can't be renewed, only that character stops being
followed, with a notification to link it again; linking it again resumes it.
The task ends when no characters are left.

### SDE database

`DatabaseUpdater` runs on its own thread: it checks CCP's SDE index, downloads
and extracts it when needed, creates the schema and builds the database
into `sde.db.building`, which replaces `sde.db` only once complete (a failed
update keeps the previous database), reporting through `Message::DatabaseUpdateProgress` and
`Message::DatabaseUpdated`. It starts automatically when the database does not
exist and on demand from *Settings -> Application* (its progress window shows
over the Settings screen too).

### Saving settings

The Settings screen is full-window, built with `egui-panels`. Its pages follow
an intel line (*Sources -> Rules -> Alerts*, with a stepper at the top of
those three), then *Maps*, *Characters* and *Application*.

Every change is a draft. Opening the screen rescans the chat log folder and
snapshots `Settings` (the rules editor keeps its own working graph); a page
whose values differ from the snapshot (or, for *Rules*, whose graph differs
from the running one) shows a dot in the navigation, and the bar lists them.
*Apply* merges the rule open in the node editor, validates the rules, calls
`save_settings` (`persistence.rs`: records the start-up regions, calls
`apply_intel_settings` to update the channel list the watcher reads and
re-register the directory watch, and writes the settings file), redraws the
maps with the new node style (the character glow) and applies the rules
(persists them to `telescope.db` and reloads the executor), then takes a new
snapshot; *Accept* does the same and closes. *Cancel* restores the snapshot,
the previewed language and the start-up region checks, and re-applies them.
Linking and unlinking characters are not drafts: they go through EVE SSO and
apply at once.

## Platform services and diagnostics

* **Where the files live** (`app_dirs.rs`): every file Telescope writes is a path
  relative to the working directory. `app_dirs::prepare` picks that directory once,
  at start-up. With a `telescope.toml` already in the current directory (a
  checkout, a portable copy) nothing changes. Otherwise Telescope is treated as
  installed: it moves to the per-user data folder
  (`%APPDATA%/rafaga/Telescope/data` on Windows,
  `~/Library/Application Support/org.rafaga.Telescope` on macOS,
  `~/.local/share/telescope` on Linux) and copies the settings template shipped
  with the installer (`telescope.default.toml`) there the first time. Files it
  only reads (the alarm sounds, the template) are found with `find_resource`,
  which also looks next to the executable.
* **Dependency diagnostics** (`log_bridge.rs`): `WARN` / `ERROR` events that
  dependencies only report through `tracing` / `log` (a portrait that fails to
  download, for example) are copied into the status log.
* **GPU loss** (`gpu_diagnostics.rs`): on Windows the GPU device can be dropped
  during sleep or a driver reset. The module records the adapter, the
  device-lost reason and the gaps between frames, writes a report when
  `egui-wgpu` panics on the failed write, and relaunches the program (up to
  three times in a row, and only if the device was reported lost) because eframe
  cannot rebuild its renderer.
* **Third-party licenses** (`build.rs`, `windows/licenses.rs`): the build lists
  every crate in the build with `cargo metadata` and reads their license files,
  so *Help -> Third-party licenses* always matches `Cargo.lock`.

## Tests

`cargo test --workspace` runs everything, and nothing in it touches the network,
the developer's `telescope.toml` or `sde.db`.

* `TelescopeApp::for_test(dir)` (`app/app_tests.rs`) builds the application on a
  temporary folder: the same start-up as `Default::default()`, without the SDE
  update check and with placeholder ESI credentials. The tests in that module
  drive `event_manager`, the Settings screen, the rule graph, the chat log
  watcher and the character link through it.
* The SDE updater is tested against a small HTTP server on `127.0.0.1` that plays
  CCP's index (`database_updater.rs`).
* `egui-panels` is tested on a headless egui context (`crates/egui-panels/tests`).
* `native_tools::zbus` is Linux-only, and so are its tests.

Not covered by tests: drawing code (the Settings pages, the maps), the ESI
polling loop of the watchdog and a full SDE rebuild from a real export. See
[BUILD.md](BUILD.md) for the coverage command and what CI runs.

## Diagrams

The diagrams in `docs/architecture` are written in [D2](https://d2lang.com/) and
the SVGs are generated from them (D2 v0.7.1):

```sh
cd docs/architecture
for f in *.d2; do d2 --layout elk "$f" "${f%.d2}.svg"; done
```

Edit the `.d2`, regenerate, and commit both.

## Extending Telescope

* **A settings page:** add a variant to `SettingsPage` with its `title()` and
  `icon()` (`messages.rs`), write `show_<name>_page` in a new file under
  `windows/settings/` with the `egui-panels` components (`page`,
  `page_header`, `Section`, `form`...), put it in a navigation group and the
  `match` in `windows/settings.rs`, and compare its values in
  `dirty_settings_pages`.
* **A rule:** edit it in *Settings -> Rules*: the rule cards (description
  and id editable in the card, an enable switch, what the rule is made of and
  the outputs it reaches) open the graph editor, whose toolbar creates detections, outputs and logic nodes
  (aggregator / gates / formatter). Import/export `rules.toml`; the model is
  `RuleGraph` in `crates/webb/src/graph.rs`, the tables (`node`/`edge`) are
  created by the player database's `migrate_1_to_2`, and the built-in default
  graph is the embedded `rules.toml` (`RuleGraph::default_graph`).
* **A message:** add a variant to `Message` and to `Message::kind()`
  (`messages.rs`), and handle it in `event_manager` (`app.rs`).
