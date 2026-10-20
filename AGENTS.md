# AGENTS.md

Instructions for AI coding agents working in this repository. Human contributors: see
[CONTRIBUTING.md](./CONTRIBUTING.md), which covers the same ground.

## Real appliances are on this network

Kessel controls physical appliances, including cooktops and ovens, and nothing in the code refuses
a write. So:

- **Never open a connection to an appliance** unless the user asked for that specific action. Use
  the simulator (`just sim`) or the in-process fakes in the test suites instead.
- **Never run `just live` or `cargo test -- --ignored`.** Those tests talk to real hardware and one
  of them writes. Never remove an `#[ignore]` from a live test.
- Never start the daemon with the default data paths: `secrets/` holds real profiles. Use
  `just run`, which keeps state in `./.dev`.

## Secrets

`secrets/` (gitignored) holds real appliance profiles and keys. Nothing derived from it — key, PSK,
IV, serial number, MAC, haId, device ID — may reach a tracked file, a commit message, a log line, an
MQTT topic or the inspector stream. Key material is wrapped in `hc_crypto::Secret`, whose `Debug`,
`Display` and `Serialize` redact; wrap at the parse boundary. Test fixtures use synthetic keys only.

## Layout

```
crates/hc-crypto     AES-CBC + HMAC chains, key derivation, Secret<T>
crates/hc-proto      wire messages and the handshake state machine
crates/hc-transport  WebSocket transports: AES (ws://:80) and TLS-PSK (wss://:443, feature tls-psk)
crates/hc-profile    profile ZIP and DeviceDescription/FeatureMapping parsing
crates/hc-downloader profile download from the vendor cloud (feature-gated, optional)
crates/hc-core       device supervisor, state shadow, feature mapping, MQTT discovery, write path
apps/server          the daemon (binary `kessel`): HTTP API, SSE, MQTT bridge, embedded web UI
apps/web             SvelteKit SPA
apps/sim             simulated TLS-PSK appliance for development
packages/api-types   TypeScript types generated from the Rust API (`just types`)
kessel/, kessel-beta/  Home Assistant add-ons (stable and beta channels)
docs/protocol/       the reverse-engineered protocol
```

## Working rules

- `just check` must pass before you finish; `just ci` if you touched the web UI.
- The protocol is reverse-engineered and the firmware parses JSON positionally. Implement from
  `docs/protocol/`; don't invent protocol details. If something is unknown, say so.
- `hc_core::write::GuardedWriter` is the only path to a transport; keep it that way
  (`write_choke_point.rs` and `no_bare_handle_in_production.rs` check it).
- Run `just types` after changing any type exported to TypeScript, including its doc comments.
- Lints are strict (`[workspace.lints]`, `clippy.toml`). Fix complexity warnings by refactoring;
  every `#[allow]` needs a `reason`.
- Write tests first for bug fixes and new behaviour.
- Comments explain *why*, briefly. No ticket IDs, no references to planning documents, no history.
- Commit messages are conventional commits (`feat:`, `fix:`, …). release-please derives versions
  and the changelog from them.
