# Contributing

Thanks for helping. You don't need a Home Connect appliance: the test suite runs offline, and
`just sim` starts a simulated appliance you can onboard.

## Setup

You need the Rust toolchain pinned in `rust-toolchain.toml` (rustup installs it automatically),
[just](https://github.com/casey/just) and [Bun](https://bun.sh) ≥ 1.3. The TLS-PSK transport builds
vendored OpenSSL, which needs `perl`, `make` and a C compiler.

```sh
just check      # rustfmt, clippy, tests
just ci         # plus web lint, type check, tests and build
just run        # the daemon on :8099, with state kept in ./.dev
just sim        # a simulated appliance on 127.0.0.1:8443, in another terminal
just cache      # build cache size and limits
```

`just` builds into `.cache/target`, shared by the checkout and all of its git worktrees. Before
each build it prunes the cache if it has grown past 15 GiB or the disk has less than 5 GiB free.
Set `KESSEL_CACHE_MAX_GB` / `KESSEL_MIN_FREE_GB` to change the limits, or run `just cache-prune`
to clear it.

To try the full flow, run both, open http://localhost:8099, choose **Add appliance**, drop
`crates/hc-profile/tests/fixtures/dishwasher-tls.zip` and connect to `127.0.0.1` port `8443`.

For UI work, `bun run --filter web dev` serves the SPA with hot reload and proxies `/api` to the
daemon on :8099.

## How it fits together

- `hc-profile` turns a profile ZIP into an entity model (features, enums, programmes, access).
- `hc-transport` + `hc-proto` hold one connection per appliance and run the handshake.
- `hc-core` keeps the latest values, maps features to Home Assistant entities, and owns the only
  write path (`GuardedWriter`).
- `apps/server` runs one task per appliance, the MQTT bridge, the HTTP API and the SSE stream the
  web UI listens to.

`docs/protocol/` describes the protocol as far as it is known. It is reverse-engineered: if you
learn something new about it, update the document and say how you know.

## Conventions

- **Safety first.** Kessel commands real appliances. Never run `just live` (it talks to real
  hardware and writes) unless you own the appliances and mean to.
- **Secrets never leave `secrets/`.** Keys are wrapped in `hc_crypto::Secret`; logs, topics and the
  inspector only ever see redacted values.
- **Tests first** for fixes and features. Live tests are `#[ignore]`d; everything else must pass.
- **Lints are strict.** Refactor rather than `#[allow]`; when you must allow, give a `reason`.
- **`just types`** after changing an API type (including doc comments).
- **Comments** explain why, briefly.
- **Commits** follow [Conventional Commits](https://www.conventionalcommits.org/). `feat` and `fix`
  produce a release; the changelog is generated from them.

## Releases

Every green commit on `main` is published as a beta (`Kessel (beta)` add-on). release-please keeps
a release PR open; merging it publishes the stable add-on.
