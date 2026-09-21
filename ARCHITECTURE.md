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
| `telescope` | `crates/telescope` | The application: a binary plus a small library (`TelescopeApp` and `patterns`). UI, settings, file watching and alerting. |
| `webb` | `crates/webb` | EVE back end with no UI: the local OAuth callback server, the ESI client and the local player database. |
| `native_tools` | `crates/native_tools` | OS-specific code: native file / folder dialogs and per-machine identification. |

![Crate dependencies: telescope depends on webb and native_tools in this workspace, and on the external sde and egui-map crates](docs/architecture/crates.svg)

All the diagrams in this document are generated with [D2](https://d2lang.com/)
from the `.d2` files in [`docs/architecture`](docs/architecture). Edit the
source and regenerate the SVGs (the sequence diagrams ignore the layout
option):

```sh
for f in docs/architecture/*.d2; do d2 --layout=elk "$f" "${f%.d2}.svg"; done
```

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
| `esi/data` | ESI client configuration. |
| `objects` | Domain types: tokens and the `Character`, `Corporation` and `Alliance` entities. |

### `crates/native_tools`

| Module | Purpose |
|--------|---------|
| `dialog` | Open file / folder dialogs: `IFileOpenDialog` on Windows, `NSOpenPanel` on macOS, the XDG desktop portal on Linux. |
| `zbus` (Linux only) | Machine identification through DMI, D-Bus and machine-id fallbacks. |
| `lib.rs` | Per-OS `get_*_unique_id` functions. |

## The `telescope` crate

```text
crates/telescope/src
├── main.rs                  entry point: logging / tracing setup, opens the window
├── lib.rs                   exports TelescopeApp and patterns
├── app.rs                   TelescopeApp: state, construction, per-frame ui(),
│                            event_manager() and map pane management
└── app/
    ├── data.rs              static ESI application data (scopes, callback URL, keys)
    ├── database.rs          storing linked characters, SDE build cache, reload after an SDE update
    ├── database_updater.rs  progress window + background SDE check / build
    ├── file.rs              notify event handler for the chat log directory
    ├── intel.rs             chat log name parsing (IntelLogName), reading and decoding
    │                        logs, running the patterns, apply_intel_settings()
    ├── messages.rs          Message, MapSync, CharacterSync, spawners and send helpers
    ├── notifications.rs     the on-screen status log
    ├── patterns.rs          pattern engine and the patterns.toml format (has its own docs)
    ├── persistence.rs       save_settings()
    ├── settings.rs          Settings: paths, map options, channels; persisted to a TOML file
    ├── tiles.rs             egui_tiles panes: UniversePane, RegionPane, TreeBehavior
    ├── watchdog.rs          background location polling of the linked characters
    ├── windows.rs
    └── windows/
        ├── about.rs         About window
        ├── debug.rs         debug window
        ├── settings.rs      Settings window frame (menu, page match, Save button)
        └── settings/
            ├── intelligence.rs   alerts, monitored channels, start-up maps
            ├── data_sources.rs   database paths, SDE update button
            └── characters.rs     linked characters
```

![Module map of the telescope crate: entry points, app.rs with TelescopeApp, and the user interface, intel pipeline, EVE / ESI and shared core groups](docs/architecture/modules.svg)

`TelescopeApp` is one struct; the modules above only add `impl TelescopeApp`
blocks (or free helpers), so the state stays in a single place and every
submodule can reach it.

## Runtime files

Telescope reads and writes these in the directory it runs from:

| File | Content |
|------|---------|
| `telescope.toml` | User settings (`Settings`). |
| `patterns.toml` | Alert rules. Created from a built-in template when missing, and regenerated (keeping a backup) when corrupt. |
| `sde.db` | The SDE database. Built automatically when it does not exist. |
| player database | Linked characters and their tokens. Its path is set in *Settings -> Data Sources*. |

## Threads and messages

The UI runs on the main thread: every frame `TelescopeApp::ui` first drains
`event_manager()` and then draws. Anything slow or blocking runs elsewhere, on
its own thread with a small Tokio runtime, and reports back with messages:

![Threads and channels: background threads send Message to the UI thread, which sends MapSync to the map panes and CharacterSync to the watchdog](docs/architecture/messaging.svg)

| Channel | Type | Direction |
|---------|------|-----------|
| `Message` | `mpsc` | anything -> the app (`event_manager` dispatches it) |
| `MapSync` | `broadcast` | the app -> every map pane (center on, system alert, player moved) |
| `CharacterSync` | `mpsc` | the app -> the watchdog (add / remove a linked character) |

From the UI thread use `MessageSpawner::spawn`; from async code use
`send_app_message`. `Message::kind()` gives a short name for tracing without
logging the payload.

Background threads: the `notify` watcher callbacks (`file.rs`), the OAuth
callback server (`AuthSpawner`), the watchdog and the `DatabaseUpdater`.

## Main flows

### Chat log -> alert

![Sequence diagram: from a line appended to a chat log to a notification or a map alert](docs/architecture/flow-intel.svg)

1. `IntelEventHandler` (`file.rs`) receives filesystem events for the intel
   directory. Writes to the log of a monitored channel become
   `Message::IntelFileChanged`; files appearing or disappearing become
   `Message::ScanIntelFiles`. Channels are recognised with
   `IntelLogName::parse`, the only place that knows the log file name format.
2. `event_manager` calls `load_intel_file`, which reads only the bytes added
   since the last read and decodes them from UTF-16LE.
3. `parse_intel_data` runs `PatternEngine::evaluate` on the text. Each match
   carries an action:
   * `notify` sends a `GenericNotification`, shown in the status log
     (`notifications.rs`).
   * `map_alert` resolves the captured solar system through the SDE and sends
     `MapSync::SystemNotification`, which the map panes highlight.

### Linking a character

![Sequence diagram: linking a character through EVE SSO until the watchdog starts](docs/architecture/flow-character.svg)

1. *Settings -> Characters -> Add* asks `EsiManager` for the authorize URL and
   opens it in the browser.
2. The `AuthSpawner` thread runs `webb::auth_service` on
   `http://localhost:56123/login`. The redirect's `code` and `state` come back
   as `Message::EsiAuthSuccess`.
3. `update_character_into_database` completes the authorization
   (`EsiManager::auth_user`), keeps the character, and starts the watchdog if
   it is the first one; otherwise it sends `CharacterSync::Add`.

### Location tracking

![Sequence diagram: the watchdog polling ESI for a character's location and updating the maps](docs/architecture/flow-location.svg)

`start_watchdog` polls ESI every 25 seconds. It refreshes the access token when
needed and asks for the location of each linked character. A change produces
`Message::PlayerNewLocation` (the app stores the new location) and
`MapSync::PlayerMoved` (the map panes move the marker). The task ends when no
characters are left.

### SDE database

`DatabaseUpdater` runs on its own thread: it checks CCP's SDE index, downloads
and extracts it when needed, creates the schema and builds the database,
reporting through `Message::DatabaseUpdateProgress` and
`Message::DatabaseUpdated`. It starts automatically when the database does not
exist and on demand from *Settings -> Data Sources*.

### Saving settings

The Save button calls `save_settings` (`persistence.rs`): it records the
start-up regions, calls `apply_intel_settings` (updates the channel list the
watcher reads and re-registers the directory watch) and writes the settings
file.

## Extending Telescope

* **A settings page:** add a variant to `SettingsPage` and to `SettingsPage::ALL`
  and `title()` (`messages.rs`), write `show_<name>_page` in a new file under
  `windows/settings/`, and add its arm to the `match` in `windows/settings.rs`.
* **A pattern action:** add a variant to `ActionConfig` (`patterns.rs`) and
  handle it in `parse_intel_data` (`intel.rs`).
* **A message:** add a variant to `Message` and to `Message::kind()`
  (`messages.rs`), and handle it in `event_manager` (`app.rs`).
