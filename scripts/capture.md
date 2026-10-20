# Capturing an appliance session

The procedure to regenerate `fixtures/aes/hob-session.json`.

> ## ⚠ The hob and the oven are never written to
>
> This codebase can physically start a cooktop. The
> capture is **read-only**: connect, complete the handshake, subscribe, record. `capture.py`
> checks every outbound frame against `ALLOWED_SENDS` inside the `encrypt` wrapper — the last
> point at which the frame is still clear JSON, so no hcpy code path can reach the wire around
> it — and aborts the capture on anything else. There is no flag to turn that off. Do not add
> a resource to that allowlist unless you are certain it cannot change appliance state.
>
> The target address is pinned to the alias's entry in `secrets/IP.md`. `--host` may only
> differ from it with an explicit `--allow-unpinned-host`.
>
> If a capture step looks like it needs a write to produce interesting traffic, **it does
> not**. `/ro/allMandatoryValues` and `/ro/allDescriptionChanges` are reads.
> `NOTIFY /ro/values` frames arrive when a human physically touches the appliance or when it
> changes state on its own. If none arrive in your window, record what you got and say so.

---

## 0. What gets produced

| Tier | Path | Committed? | Contents |
|---|---|---|---|
| 1 — raw | `secrets/captures/<alias>/<timestamp>.json` | **no**, gitignored | exact wire bytes, the real psk/iv, real identifiers |
| 2 — golden | `fixtures/aes/hob-session.json` | **yes** | identifiers rewritten, re-encrypted under a published synthetic key |

`secrets/captures/` is gitignored twice over, by the blanket `secrets/` rule and by its own
entry. Never `git add -f` anything under
it.

---

## 1. Prerequisites

* Python 3.10+.
* `pycryptodome` and `websocket-client` (hcpy's AES path), plus `pytest` for the scrubber
  tests. `scripts/requirements.txt` lists them.
* An hcpy checkout. `reference/` is gitignored and is not on `main`, so clone it:

  ```sh
  git clone --depth 50 https://github.com/hcpy2-0/hcpy reference/hcpy
  ```

  The corpus in `fixtures/aes/` was captured against
  `0f2136d39c15864e5a03a1ad5ade326525102313`. The capture records the commit it ran against
  in `client.commit`.

* The appliance profile ZIP in `secrets/`, and `secrets/IP.md` mapping alias to address.
  A git worktree does **not** contain `secrets/` — pass `--secrets-dir` pointing at the main
  checkout, or set `KESSEL_SECRETS_DIR`.

If your Python cannot create a venv (`ensurepip` missing on some distributions), install into
a directory and put it on `PYTHONPATH`:

```sh
pip3 install --target scripts/.pylibs -r scripts/requirements.txt
export PYTHONPATH="$PWD/scripts/.pylibs:$PWD/scripts"
```

`scripts/.pylibs/` is gitignored by `scripts/.gitignore`; a root-level `.pylibs` is not
ignored. `just test-py` installs there and runs the tests in one step.

---

## 2. Run the scrubber's tests first

Test the scrubber **before** taking a capture, so you know what a safe fixture looks like
before you are holding unsafe data.

```sh
python3 -m pytest scripts/tests -q
```

That relies on the `PYTHONPATH` exported in §1 — it must **extend** it, never replace it, or
you lose `pycryptodome` and `pytest` themselves. If you have those installed normally and did
not need the §1 workaround, `PYTHONPATH=scripts python3 -m pytest scripts/tests -q` is enough.

These tests touch nothing in `secrets/`; they build a throwaway profile ZIP with fake
identifiers in `tmp_path`.

> **They are not part of `just ci` and do not run under `cargo test`.** Run them with
> `just test-py` or the command above.

---

## 3. Take the capture

```sh
python3 scripts/capture.py \
    --alias hob \
    --secrets-dir /path/to/repo/secrets \
    --duration 150
```

What it does:

1. Reads the profile ZIP matching `--alias` and refuses anything whose `connectionType` is
   not `AES` (the TLS capture is a separate, separately device-locked task).
2. Resolves the address from `secrets/IP.md` and **pins** it: `--host` is accepted only when
   it matches the alias's entry, or when `--allow-unpinned-host` is also passed (which prints
   a warning), so `--host <other-appliance> --alias hob` cannot reach the wrong appliance.
3. Drives the session with **unmodified hcpy** — `HCSocket` + `HCDevice` — because hcpy is
   the reference implementation and the corpus exists to pin *its* bytes.
   `HCDevice` is constructed with an empty feature table, which is enough for the handshake
   and avoids needing the parsed DeviceDescription.
4. Wraps `HCSocket.send`, `.encrypt` and `.decrypt`:
   * `encrypt` is where the read-only allowlist is enforced. It is the narrowest choke point
     available: the last place the frame exists as clear JSON before it becomes ciphertext, so
     the guard holds no matter how hcpy's call graph is refactored. It fails closed — a frame
     that is not decodable JSON is refused rather than passed through.
   * `send` re-checks the same predicate for an earlier and clearer error, and takes a lock
     across encrypt+write, which hcpy itself does not do.
     That also guarantees the recorded order is the wire order. The safety property does not
     depend on this wrapper.
   * `encrypt`/`decrypt` record the frame and then **re-derive the chain state independently**
     with `scripts/hc_aes.py`, cross-checking against hcpy's own `last_tx_hmac` /
     `last_rx_hmac` after every frame. A divergence aborts the capture.
   * A mirror `Opener` per direction recovers the **padding bytes**, which hcpy discards on
     receive and never surfaces on send, so the frame is exactly reproducible.
5. Stays connected for `--duration` seconds, then closes.
6. Verifies the whole capture against its own key with `hc_corpus.verify_capture` before
   writing, and writes mode `0600` to `secrets/captures/<alias>/<timestamp>.json`.

Expect roughly this, in wire order:

```
rx POST     /ei/initialValues          tx RESPONSE /ei/initialValues
tx GET      /ci/services               rx NOTIFY   /ci/registeredDevices   (unsolicited)
rx RESPONSE /ci/services               tx GET      /ci/authentication
tx GET      /ci/info                   tx NOTIFY   /ei/deviceReady
tx GET      /ni/info                   tx GET      /ro/allMandatoryValues
tx GET      /ro/allDescriptionChanges  rx RESPONSE … for each of the above
```

The script prints the frame census and the count of live `NOTIFY /ro/values`. Zero is a
normal result for an idle appliance and is **not** a reason to write to it.

---

## 4. Scrub it

```sh
python3 scripts/scrub-capture.py \
    --secrets-dir /path/to/repo/secrets \
    scrub -i secrets/captures/hob/<timestamp>.json \
          -o fixtures/aes/hob-session.json
```

The scrubber refuses to write if the raw capture does not verify under its own key, if the
scrubbed corpus does not verify under the synthetic key, or if any real identifier survives
into the output. See `fixtures/README.md` for the scheme and the synthetic key.

## 5. Audit before committing

Do not trust the scrubber; check it.

```sh
python3 scripts/scrub-capture.py --secrets-dir /path/to/repo/secrets audit $(git ls-files)
```

`LEAK` lines are identifiers that must never be published (haId, MAC, serial, psk, iv) and
are a hard failure. `note` lines are other secrets-derived values, chiefly LAN addresses, and
are reported rather than failed because some are committed deliberately.

---

## 6. Regenerating deterministically

The corpus is a pure function of the raw capture plus the constants at the top of
`scrub-capture.py` (`SYNTH_NAMESPACE`, `PAD_SEED`, `SYNTHETIC_PSK`, `SYNTHETIC_IV`). Scrubbing
the same raw capture twice gives byte-identical output — pinned by `scrub_is_deterministic`.

Changing any of those constants rewrites every byte of `fixtures/aes/*.json`, which will
invalidate any Rust test that hardcodes a value from it. Don't, unless you mean to.

A **new** capture produces a different corpus even under the same constants: `sID`, `msgID`
and the appliance's nonce differ per session.

---

## 7. Extending to TLS

Not supported. The frame format is transport-agnostic in principle, but
`hc_corpus.verify_capture` assumes app-layer AES: over TLS-PSK there is none, so a TLS capture
has plaintext frames and no chain state, and `verify_capture` would need a "no crypto" mode.
`capture.py` refuses non-AES profiles outright rather than half-supporting them.
