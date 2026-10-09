# webb

The back end of [Telescope](../../README.md), with no user interface. It is named
after the James Webb space telescope.

`webb` is not meant to be used on its own: it exists to give the Telescope
application two things.

## The intel engine

Available in every build, including the experimental web one
(`webb = { default-features = false }`):

| Module | Purpose |
|--------|---------|
| `graph` | The rule graph: typed nodes (input, detection, output, aggregator, gate, formatter) and edges, its validation, and the per-line `Executor`. The built-in default graph is embedded from `rules.toml`. |
| `rules` | The typed building blocks the graph shares: detection and output kinds, the built-in dictionaries (`dictionaries.toml`) and `parse_line`. |
| `intel` | What the intel modules share: `IntelLine`, `IntelCategory`, `PatternError` and the size limits and text helpers. |
| `map_alerts` | `AlertSummary` and `AlertLog`: what a map node's tooltip shows about the alerts on it. |

## The EVE back end (feature `esi`)

On by default. It needs threads, sockets and SQLite, so it does not build for
`wasm32-unknown-unknown`.

| Module | Purpose |
|--------|---------|
| `auth_service` | The small local server that receives the EVE SSO redirect. |
| `esi` | `EsiManagerCore`: authorization, token refresh, location and portrait queries, and the local player database (characters, corporations, alliances and the stored rule graph). |
| `esi/player_database`, `esi/cipher` | The SQLite schema and its migrations, and the encryption of the file. |
| `objects` | Domain types: tokens and the `Character`, `Corporation` and `Alliance` entities. |

## Features

| Feature | Effect |
|---------|--------|
| `esi` (default) | The EVE back end above. |
| `crypted-db` (default) | Encrypts the player database with SQLCipher, keyed from the machine identifier that `native_tools` reads. Telescope enables it explicitly. |
| `esi-api-test` | Implies `esi`. No code is gated on it at the moment. |
| `native-auth-flow` | `esi`, with the application authentication flow of `esi-openapi`. |

The modules and how they fit with the rest of the workspace are described in
[ARCHITECTURE.md](../../ARCHITECTURE.md). Run its tests with
`cargo test -p webb`.
