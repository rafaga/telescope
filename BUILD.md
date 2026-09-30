# Compiling Instructions

## Requirements

* A recent stable Rust toolchain (edition 2024, Rust 1.89 or newer).
* Linux only: the ALSA and OpenSSL development files and `pkg-config`
  (Debian / Ubuntu: `sudo apt-get install libasound2-dev libssl-dev
  pkg-config`), for the alarm sound and the encrypted player database.
* Windows only: a full Perl (for example
  [Strawberry Perl](https://strawberryperl.com/)) ahead of Git's own `perl` on
  the `PATH`. The player database builds OpenSSL from source, and the `perl`
  that ships with Git for Windows is too limited for that (the build fails in
  `openssl-sys`).
* An ESI application from CCP (client id and secret key), registered at the
  [EVE Online developers portal](https://developers.eveonline.com/) with:
  * Callback URL: `http://localhost:56123/login`
  * Scopes: `publicData`, `esi-location.read_location.v1`,
    `esi-clones.read_clones.v1`, `esi-characters.read_contacts.v1`,
    `esi-ui.write_waypoint.v1`, `esi-location.read_online.v1`,
    `esi-corporations.read_standings.v1` and `esi-alliances.read_contacts.v1`.
* Optional, only for the experimental web build (see the warning under
  *Compilation Instructions*): the `wasm32-unknown-unknown` target
  (`rustup target add wasm32-unknown-unknown`) and
  [Trunk](https://trunkrs.dev/) (`cargo install trunk`).

## ESI credentials

The client id and the secret key are read **at compile time** from the
`ESI_CLIENT_ID` and `ESI_SECRET_KEY` environment variables. Without them
`telescope` does not build (`AppData::new` names the missing one), and with
placeholder values it builds but logging in with a character will not work.

The repository does not carry them: `.cargo/config.toml` is not tracked and
neither is a `.env` file (both are ignored by git). Set the variables in your
shell before building:

```sh
# Linux / macOS
export ESI_CLIENT_ID="your client id"
export ESI_SECRET_KEY="your secret key"

# Windows (PowerShell)
$env:ESI_CLIENT_ID = "your client id"
$env:ESI_SECRET_KEY = "your secret key"
```

Cargo does not read `.env` files by itself: if you keep the values in one,
load it into the shell (or your IDE's run configuration) first. Another option
is the `[env]` section of a local `.cargo/config.toml`. Either way, **never
commit the values**; if they ever reach a commit, rotate them in the developer
portal, since deleting the file does not remove them from the history.

CI reads them from the `ESI_CLIENT_ID` and `ESI_SECRET_KEY` repository secrets
(see `.github/workflows/release.yml`). The unit tests do not need them: they
build the app with placeholder credentials (see *Tests* below).

If you change the values, rebuild the `telescope` crate (for example
`cargo clean -p telescope`), since they are baked into the binary.

## Compilation Instructions

* You do not need to provide the SDE database (`sde.db`). If the file does
  not exist, Telescope downloads CCP's SDE and builds the database by itself
  (this needs network access). Its path can be changed in
  *Settings -> Application*, which also has a *Check for updates* button.
* Run the native application:

```sh
cargo run              # debug build
cargo run --release    # optimized build
```

* Optional features of the `telescope` crate (see
  [Profiling with Tracy](#profiling-with-tracy)):
  `profile` and `profile-memory`.
* Web build (Trunk reads `index.html`; there is no `Trunk.toml`):

> **Warning:** the `wasm32-unknown-unknown` target is still under development.
> There is no guarantee that it compiles, and the native build is the only
> supported one for now.

```sh
trunk build --filehash false    # or `trunk serve --filehash false` to try it in the browser
```

`--filehash false` keeps the output file names as they are (`telescope_bg.wasm`):
the service worker (`assets/sw.js`) lists them by name, and Trunk adds a hash to
them by default. It used to be set in `Trunk.toml`, which was removed.

## Checks

`Cargo.lock` is versioned and CI builds with `--locked`, so a change to a
`Cargo.toml` needs its lockfile update in the same commit.

CI (GitHub Actions, `.github/workflows/`):

* `ci.yml`, on pull requests to `master` and `dev` and on pushes to `master`
  (documentation-only changes are skipped; a new push to a pull request
  cancels the run in progress). `fmt` checks the formatting once. On Linux,
  Windows and macOS one job runs, in this order, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo check --workspace --all-features` and
  `cargo test --workspace`, so the dependencies (OpenSSL, Tracy, egui) are
  compiled once per system. `ci-ok` sums it all up: it is the one check to
  require if branch protection is turned on. The build cache is written by
  pushes to `master` and only read by pull requests. Pushes to `master` also
  run `coverage` (`cargo llvm-cov`): the summary is in the run's page and
  `lcov.info` is kept as an artifact.
* `security.yml`, when a manifest, `Cargo.lock` or `deny.toml` changes and
  every Monday: `cargo deny check` (RustSec advisories, licenses, banned and
  duplicated crates, sources). The two advisories that have no fix yet
  (`rsa` through `jsonwebtoken`, and the unmaintained `paste`) are listed in
  `deny.toml` with the reason; run `cargo deny check` locally
  (`cargo install cargo-deny --locked`) before touching them.
* `release.yml`, when a release is published (or by hand, with a tag, to try
  the packaging: the installers are then kept as artifacts). It stops early if
  the `ESI_CLIENT_ID` or `ESI_SECRET_KEY` repository secrets are missing, and
  attaches a `SHA256SUMS-<system>.txt` next to the installers.

`check.sh` runs the same checks locally with all features, and the wasm check
too when the `wasm32-unknown-unknown` target is installed. Since the wasm
target is still under development, that step may fail even when the native
build is fine.

```sh
./check.sh
```

## Tests

`cargo test --workspace` runs everything; nothing in it touches the network,
your `telescope.toml` or your `sde.db`.

* `webb`, `egui-panels` and most of `telescope` are tested on their own
  (`egui-panels` on a headless egui context, in `tests/`).
* `TelescopeApp` is built for tests with `TelescopeApp::for_test(dir)`
  (`crates/telescope/src/app/app_tests.rs`). It runs the same startup as
  `Default::default()` (`with_settings`) on a temporary folder, but skips the
  SDE update check and uses placeholder ESI credentials, so a test can drive
  `event_manager`, the Settings screen, the rule graph, the chat log watcher
  and the character link without side effects.
* The SDE updater is tested against a small HTTP server on `127.0.0.1` that
  plays CCP's `latest.jsonl` index (`database_updater.rs`, module
  `updater_run_tests`). Tests that touch its process-wide update lock or
  cancel flag take the module's `serial()` guard first.

Not covered: drawing code of the Settings pages and the map, the ESI polling
loop of the location watchdog and a full SDE rebuild from a real export, since
those need a window, the network or CCP's data.

### Coverage

```sh
cargo install cargo-llvm-cov    # once
rustup component add llvm-tools-preview
cargo llvm-cov --workspace --summary-only
```

Add `--html` for a browsable report under `target/llvm-cov/html`, or
`cargo llvm-cov report --show-missing-lines` after a run to list the lines no
test reaches. It compiles into its own directory (`target/llvm-cov-target`), so
the first run is a full build.

## Logging

Telescope's diagnostics go through [`tracing`](https://docs.rs/tracing) rather
than `println!`/`eprintln!`. By default they're printed to stderr and
controlled with the `RUST_LOG` environment variable:

```sh
# only errors (the default when RUST_LOG isn't set)
cargo run

# everything at debug level and above
RUST_LOG=debug cargo run

# per-module filtering: telescope itself at trace, everything else at warn
RUST_LOG=telescope=trace,warn cargo run
```

See the [`tracing-subscriber` `EnvFilter` syntax](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html)
for the full directive grammar (per-target, per-span-field filters, etc.).

## Profiling with Tracy

Native builds can stream live profiling data — every `#[tracing::instrument]`d
span, plus allocation/deallocation tracking — to the
[Tracy profiler](https://github.com/wolfpld/tracy), behind the
`profile` feature (off by default, since the allocation tracking
has a small always-on runtime cost and Tracy broadcasts discovery packets on
the local network):

```sh
cargo run --features profile
```

Then:

1. Download and open the Tracy desktop app. This workspace currently pulls in
   `tracing-tracy` 0.12 / `tracy-client` 0.19, which speak the wire
   protocol of **Tracy v0.14.1** — grab that release from Tracy's
   [releases page](https://github.com/wolfpld/tracy/releases). It is the
   version verified to work with Telescope.
2. Click "Connect" in the Tracy app — it auto-discovers Telescope running on
   the local machine/network, no extra flags needed on Telescope's side.

Notes:

* Not available for the wasm/web build (`profile` is gated to
  native targets only).
* Tracy data is deliberately **not** filtered by `RUST_LOG` — turning stderr
  logging down or off never hides anything from the profiler.
* Upgrading the `tracing-tracy`/`tracy-client` versions in `Cargo.toml` may
  require a newer/older Tracy desktop app to match; check
  [`tracy-client-sys`'s version table](https://github.com/nagisa/rust_tracy_client/blob/main/tracy-client-sys/README.mkd#version-support-table)
  before bumping either side independently.
