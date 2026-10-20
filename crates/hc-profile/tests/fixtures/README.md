# `hc-profile` parser fixtures

Four scrubbed appliance profile ZIPs, one per real appliance Kessel was developed against, plus
the script that produced them. The `hc-profile` tests parse them (`profile.rs`, `fingerprint.rs`,
`content_type.rs`), and other crates' tests load them as realistic profiles.

| file | appliance | `connectionType` | why it is here |
|---|---|---|---|
| `hob-aes.zip` | NEFF hob | **AES** | the only AES device: has `iv64`, and the composed `haId` shape |
| `dishwasher-tls.zip` | BOSCH dishwasher | TLS | programs with `StartInRelative` (delayed start) |
| `oven-tls.zip` | BOSCH oven | TLS | the 426 KB stress case |
| `fridge-tls.zip` | SIEMENS fridge/freezer | TLS | the degenerate case: no programs at all |

## These files contain no real identifier

Every `serialNumber`, `mac`, `haId`, `key` and `iv` has been replaced by a synthetic value of the
**same length and the same character class at every position**, so parser behaviour — string
lengths, ZIP member names, XML structure — is unchanged. The substitution rules and the leak
check are in [`scrub.py`](./scrub.py), which hard-codes nothing real.

Two shapes are preserved deliberately, because `haId` parsing depends on them:

- the AES `haId` stays in `BRAND-VIB-MAC` form (it is rewritten through its MAC component);
- a TLS `haId` stays equal to the `serialNumber` (both are rewritten to the same synthetic value).

`brand`, `vib`/`model`, `type`, `connectionType`, every `uid`/`enid` and every dotted feature key
are kept **verbatim**. They are product metadata and protocol structure, not identifiers of a
particular appliance, and the entity model is built out of them.

The manifest's `created` is rewritten too, to the fixed `2020-01-01T00:00:00+00:00`. That is
**not** a redaction — an export timestamp identifies no appliance — but keeps the output
deterministic, so the ZIPs do not carry the wall-clock minute of whichever run produced them. It is the one field whose byte length is not preserved;
`manifest.rs` keeps `created` as an opaque `Option<String>` and nothing parses it.

## Regenerating

The real ZIPs live in the gitignored `secrets/` at the workspace root and are never committed.

```sh
python3 crates/hc-profile/tests/fixtures/scrub.py secrets crates/hc-profile/tests/fixtures
```

The script re-opens each output and fails loudly if any real identifier survived.

**The scrub is idempotent and its output is byte-reproducible**, so these four files can also be
regenerated from *themselves*, without access to `secrets/`. Scrub into a scratch directory (writing back onto the inputs is refused) and move the
results in:

```sh
python3 crates/hc-profile/tests/fixtures/scrub.py crates/hc-profile/tests/fixtures /tmp/out
```

Byte-reproducibility comes from pinning everything in the ZIP entry headers that would otherwise
come from the clock or the host — mtime, permission bits, create-system — plus a fixed entry order
(the input's) and a fixed deflate level; see `_zip_info` in [`scrub.py`](./scrub.py). Idempotence
comes from `synth` depending only on the role and on the template's length and per-position
character class, all of which a scrubbed value shares with the real one. Two consequences:

- re-running the scrubber over this directory produces byte-identical files, so the fixtures never
  churn in a diff;
- the leak check reports its identifier fields as *already synthetic* rather than as leaks, and
  says so on stdout rather than exempting them silently.

## Running the tests against the real ZIPs

The default `cargo test` uses only the files in this directory, so a fresh clone without
`secrets/` is green. To additionally exercise the real profiles:

```sh
KESSEL_RAW_PROFILES=1 cargo test -p hc-profile              # looks in <workspace>/secrets
KESSEL_RAW_PROFILES=/path/to/secrets cargo test -p hc-profile
```

Unset, the gated assertions print a `SKIP …` line on stderr rather than passing quietly. The path
form exists because a git worktree has no `secrets/` of its own.
