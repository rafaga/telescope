# Telescope

[![Windows](https://github.com/rafaga/telescope/actions/workflows/windows.yml/badge.svg)](https://github.com/rafaga/telescope/actions/workflows/windows.yml)
[![MacOS](https://github.com/rafaga/telescope/actions/workflows/macos.yml/badge.svg)](https://github.com/rafaga/telescope/actions/workflows/macos.yml)

Telescope is a desktop application that watches your EVE Online chat logs,
evaluates them against configurable pattern rules and shows the resulting
alerts on interactive maps of New Eden. Characters linked through EVE SSO are
followed through ESI so the maps can show where they are and warn about
nearby threats. It is similar to other intel gathering tools, but with some
key differences.

* Designed to be multiplatform (Windows, macOS and Linux).
* Designed to be as easy to use as possible.
* Designed with privacy in mind. Your data resides in your computer.

## Features

* **Intel watching** — monitors the EVE chat log directory and parses new
  lines as they are written, recognising the channel from the log file name.
* **Pattern engine** — alert rules are driven by `patterns.toml` (regex
  rules, dictionaries, `notify` and `map_alert` actions), so alerts can be
  tuned at will.
* **Interactive maps** — universe and per-region maps with system alerts
  raised directly from intel matches.
* **Character linking via EVE SSO** — authorizes through ESI and keeps
  linked characters, their corporations and alliances in a local database.
* **Location tracking** — a background watchdog polls ESI for each linked
  character's location and moves their marker on the maps.
* **Automatic SDE updates** — the CCP Static Data Export database is checked,
  downloaded and built automatically the first and updates itself.

Platforms tested:

* MacOS Tahoe (ARM64)
* Windows 11 (ARM64)
* Windows 11 (Intel x86-64)

Screenshots:

<img width="400" height="315" alt="Universe Map" src="https://github.com/user-attachments/assets/9a2fcc8d-c52c-42cd-b6c3-9529345ea29e" />
<img width="400" height="315" alt="Great Wildlands Region" src="https://github.com/user-attachments/assets/2960adac-d001-4fe2-9f5e-0e502a408c90" />
<img width="400" height="352" alt="Settings" src="https://github.com/user-attachments/assets/e4e0b25c-54bb-42b9-a9fd-dc07966556ae" />
<img width="400" height="317" alt="Aridia Region on MacOS" src="https://github.com/user-attachments/assets/1f821a55-2ae7-4aa3-b3f6-dea25e3a2f32" />

## Building and running

```sh
cargo run --release
```

You do not need to provide the SDE database yourself, Telescope builds it on
first run. You do need your own ESI application (client id and secret key)
from CCP, set at compile time — see [BUILD.md](BUILD.md) for the full
requirements, ESI credential setup, logging and profiling instructions.

## Documentation

* [ARCHITECTURE.md](ARCHITECTURE.md) — a map of the code: the workspace's
  crates, the `telescope` crate's modules, threads and messages, and the main
  flows (chat log to alert, linking a character, location tracking, SDE
  updates), illustrated with diagrams.
* [BUILD.md](BUILD.md) — requirements, ESI credentials, building, checks
  (`check.sh`), logging and Tracy profiling.
* [CHANGELOG.md](CHANGELOG.md) — notable changes, following
  [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## License

MIT, see [LICENSE.md](LICENSE.md).
