# Compiling Instructions

## Requirements

* A recent stable Rust toolchain (edition 2024, Rust 1.89 or newer).
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
`ESI_CLIENT_ID` and `ESI_SECRET_KEY` environment variables. `.cargo/config.toml`
declares both in its `[env]` section, empty, so the project builds but
logging in with a character will not work until you provide real values.

Cargo does not override variables that are already set in the environment, so
the safest option is to set them in your shell before building:

```sh
# Linux / macOS
export ESI_CLIENT_ID="your client id"
export ESI_SECRET_KEY="your secret key"

# Windows (PowerShell)
$env:ESI_CLIENT_ID = "your client id"
$env:ESI_SECRET_KEY = "your secret key"
```

Alternatively, put the values in the `[env]` section of `.cargo/config.toml`,
but **never commit them**: that file is tracked by git.

If you change the values, rebuild the `telescope` crate (for example
`cargo clean -p telescope`), since they are baked into the binary.

## Compilation Instructions

* You do not need to provide the SDE database (`sde.db`). If the file does
  not exist, Telescope downloads CCP's SDE and builds the database by itself
  (this needs network access). Its path can be changed in
  *Settings -> Data Sources*, which also has a *Check for SDE updates* button.
* Run the native application:

```sh
cargo run              # debug build
cargo run --release    # optimized build
```

* Optional features of the `telescope` crate (see
  [Profiling with Tracy](#profiling-with-tracy)):
  `profile` and `profile-memory`.
* Web build (uses `Trunk.toml` and `index.html`):

> **Warning:** the `wasm32-unknown-unknown` target is still under development.
> There is no guarantee that it compiles, and the native build is the only
> supported one for now.

```sh
trunk build            # or `trunk serve` to try it in the browser
```

## Checks

`check.sh` runs the same checks as CI: `cargo check` (native and wasm),
`cargo fmt --check`, `cargo clippy` with warnings as errors, the tests
(including doc tests) and `trunk build`. Since the wasm target is still under
development, its steps (`cargo check ... --target wasm32-unknown-unknown` and
`trunk build`) may fail even when the native build is fine.

```sh
./check.sh
```

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
