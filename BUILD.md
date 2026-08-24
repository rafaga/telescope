# Compiling Instructions

## Requeriments

* SDE database made by [Database Creator](http://github.com/rafaga/databaseCreator/)
* an ESI API KEY from CCP.

## Compilation Instructions

* place sde.db into assets

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
   `tracing-tracy` 0.11.4 / `tracy-client` 0.18.4, which speak the wire
   protocol of **Tracy v0.13.1** — grab that release (or another one Tracy's
   own [compatibility notes](https://github.com/wolfpld/tracy/releases) list
   as protocol-compatible) from Tracy's releases page.
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
