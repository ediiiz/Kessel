# `fixtures/` — golden frame corpus

A real appliance session, captured from the wire and scrubbed so it is safe to publish. Tests use
it to check Kessel's crypto chain, message encoding and handshake byte for byte against what a
working client and a real appliance actually exchanged.

| File | What it is |
|---|---|
| [`aes/hob-session.json`](./aes/hob-session.json) | A complete AES handshake between unmodified hcpy and a real NEFF hob, scrubbed and re-encrypted. 16 frames. |

There is no TLS corpus. TLS-PSK appliances have no application-layer crypto, so a TLS capture
would only add the JSON and handshake sequence, which the AES corpus already pins.

Consumers include `hc-crypto` (`tests/aes_chain.rs`), `hc-proto`, `hc-transport` (handshake and
notification tests), the `hc-core` and server secret-leak sweeps, and `scripts/hc_corpus.py`.

---

## 1. Raw capture vs golden corpus

A byte-exact AES replay needs the psk and iv, and the captured plaintext is full of serials and
haIds — none of which may be published. So there are two tiers:

| Tier | Location | Committed? | Contents |
|---|---|---|---|
| raw | `secrets/captures/<alias>/<timestamp>.json` | **no**, gitignored | exact wire bytes, the real psk/iv, real identifiers |
| golden | `fixtures/aes/…` (here) | **yes** | identifiers rewritten to synthetic values of the same shape; ciphertext **re-encrypted under a synthetic psk/iv** |

`scripts/scrub-capture.py` walks the raw capture, substitutes each real identifier with a
deterministic fake of the same shape, then **re-runs the whole session through the crypto chains
under the synthetic key**. Because the chains are re-run end to end rather than patched, the
corpus stays internally consistent: HMAC chain state, CBC continuity and frame lengths are all
exact, just under a key that is safe to publish.

Substitution happens on the **plaintext bytes**, never by round-tripping through parsed JSON
(§4).

---

## 2. The synthetic key

Fixed, published, and derived from labels so it is reproducible from `scripts/scrub-capture.py`
alone. It has no relationship to any appliance key. (The label strings are derivation inputs and
must stay exactly as written.)

```
psk64  5-L0aeVRdTyIalwtHimvDJA5nhr-AvQAIz0qf2FooHM     SHA-256("kessel/C-0/synthetic-psk/v1")
iv64   YeZ7Qf6EMabo-RN_K-bWVg                          SHA-256("kessel/C-0/synthetic-iv/v1")[..16]
```

Both are URL-safe base64, unpadded — the shape real profiles use
([`aes-chain.md`](../docs/protocol/aes-chain.md) §2). The corpus also carries them at
`key.psk64` / `key.iv64`; read them from there rather than hardcoding.

The pad bytes are **not** random. [`aes-chain.md`](../docs/protocol/aes-chain.md) §5.1 puts
`pad_len - 2` random bytes in the middle of every pad, which would make the corpus
irreproducible, so the scrubber uses a seeded stream:

```
pad RNG   shake_256("kessel/C-0/pad/v1"), consumed sequentially across the whole session
```

Sequentially across the *session*, not per frame: both directions share the stream in frame
order, so a frame's pad depends on every frame before it.

---

## 3. The synthetic identifier set

No real identifier appears in the corpus. These are the values that do; the secret-leak tests
grep `fixtures/` with them in mind. Each is also listed machine-readably under
`synthetic_identifiers`, with its source field and kind.

| Field | Kind | Value in `aes/hob-session.json` |
|---|---|---|
| `haId` | identifier | `IGEE-U94MH8TF1-38F49A1455A9` |
| `serialNumber` | identifier | `258603004700104930` |
| `mac` (hyphen form, as `/ci/info` and `/ni/info` report it) | identifier | `38-F4-9A-14-55-A9` |
| `mac` (compact, the haId tail) | identifier | `38F49A1455A9` |
| `vib` | identifier | `Q46WK5BW5` |
| `eNumber` | identifier | `Q82AC4MF6/14` |
| `deviceID` of a previously-paired client | identifier | `a4d7c797395d08be` |
| `deviceName` of a previously-paired client | identifier | `VV-N949X` |
| `ssid` | network | `PfdtDtipflIzwcuLhbd` |
| `ipAddress` (v4) | network | `52.44.8.36` |
| `ipAddress` (v6) | network | `ef11:da78:5357:0:4af0:cda:ab64:03e7` |
| `gateway` / `dnsServer` (v4) | network | `95.17.7.6` |
| `gateway` / `dnsServer` (v6) | network | `2:9:8:2:0:4:6:4` |
| `nonce` (client, `/ci/authentication`) | session | `ik2G8cHRWCjc2jZFPubn7F3O_BVuW8dfvgieJz9bYnW` |
| `response` (appliance, `/ci/authentication`) | session | `E9tUIjbVXGAgeCb-LgVtsuuz2DH1sy6sshK7YcwRBoE` |

The `nonce` (frame 5, `tx`, `/ci/authentication`) decodes to 32 bytes but is **not** canonical
`URL_SAFE_NO_PAD` base64: decode→re-encode does not round-trip (`…bYnW` becomes `…bYnU`), an
artifact of shape-preserving substitution on the low bits of the final character. Treat it as an
opaque wire string, not a decoded key.

`device.host` is the literal `appliance.invalid`; the real address is never written to a tracked
file.

### Deliberately not substituted

* `"deviceName":"hcpy"` and `"deviceID":"0badcafe"` — hcpy hardcodes both
  (`reference/hcpy/HCDevice.py:63-64`), so they leak nothing. Tests compare Kessel's
  `/ei/initialValues` RESPONSE against the captured one byte for byte using these constants;
  rewriting them would break that comparison.
* `"brand":"NEFF"` — public product metadata.
* Firmware versions (`haVersion`, `swVersion`, `hwVersion`) and `fdString` — not identifying.

### Shape preservation and its limits

Every substitution keeps the original's **length** and **per-character class** (uppercase /
lowercase / digit / literal separator), so nothing that parses these values behaves differently.
A token whose alphanumerics are all hex digits stays hex, so a MAC still parses as a MAC and an
IPv6 literal as an IPv6 literal.

Two things are not preserved; neither matters to any test:

1. **Derived relationships between fields.** In real data `eNumber` is `<vib>/<customerIndex>`
   and the IPv6 address's lower 64 bits are the EUI-64 of the MAC; values are substituted
   independently, so these break. The haId/MAC relationship *is* kept — the synthetic haId ends
   with the synthetic MAC — because the compact MAC is a substring of the haId.
2. **Special values.** The IPv6 unspecified address `0:0:0:0:0:0:0:0` becomes ordinary digits.

---

## 4. Bytes, not parsed JSON

**The plaintext is stored as raw bytes (`plaintext_hex`), and that is the authoritative field.**

JSON key order is the leading explanation for the `/ci/services` 403 (see
[`handshake.md`](../docs/protocol/handshake.md)): Go builds the `/ei/initialValues` response as a
`map[string]any`, and `encoding/json` sorts map keys, so it emits `deviceID, deviceName,
deviceType` where both Python clients emit `deviceType, deviceName, deviceID`. hcpy's own comment
corroborates that appliances care about order: *"UID has to be the first attribute in the dict
because the devices require it that way"* (`reference/hcpy/HCDevice.py:303`).

A capture stored as parsed JSON loses key order. So:

* the scrubber substitutes on **bytes**, with a literal longest-first replacement, and never
  parses and re-serialises;
* `plaintext_utf8` and `json` are stored **alongside** `plaintext_hex` for readability, and the
  tests assert all three agree;
* comparisons of the `/ei/initialValues` RESPONSE must be order-sensitive on the bytes. Do not
  compare `serde_json::Value`: its equality is order-blind.

The captured hcpy RESPONSE, verbatim:

```json
{"sID":2879624297,"msgID":1474258175,"resource":"/ei/initialValues","version":2,"action":"RESPONSE","data":[{"deviceType":"Application","deviceName":"hcpy","deviceID":"0badcafe"}]}
```

---

## 5. File format

One JSON document per session, `kessel.capture/1`. Both tiers share the schema; they differ
only in `tier`, `key`, and whether the identifiers are real.

```jsonc
{
  "schema": "kessel.capture/1",
  "tier": "golden",
  "generator": { "script": …, "namespace": …, "pad_seed": …, "pad_rng": … },
  "device":  { "alias": "hob", "transport": "aes", "connection_type": "AES", "host": "appliance.invalid" },
  "client":  { "name": "hcpy", "commit": "0f2136d…", "device_name": "hcpy", "device_id": "0badcafe" },
  "captured_at": "…",
  "key": { "psk64": …, "iv64": …, "note": … },
  "synthetic_identifiers": [ { "field": …, "kind": …, "synthetic": … } ],
  "frames": [ … ]
}
```

Each frame:

| Field | Meaning |
|---|---|
| `seq` | position in the session; both directions interleaved in wire order |
| `direction` | `"tx"` = client → appliance (direction byte `0x45`), `"rx"` = appliance → client (`0x43`) |
| `t_ms` | milliseconds since the capture started |
| `wire_hex` | the WebSocket **binary** frame; equals `ciphertext_hex + mac_hex` |
| `ciphertext_hex`, `mac_hex` | the split; the MAC is the 16-byte suffix |
| `plaintext_hex` | **authoritative** — the exact UTF-8 bytes of the decrypted message |
| `plaintext_utf8`, `json` | readability only; see §4 |
| `pad_len`, `pad_hex` | the custom pad (`docs/protocol/aes-chain.md` §5.1) |
| `s_id`, `msg_id`, `resource`, `version`, `action`, `code` | lifted from the JSON; `null` when absent |
| `chain.direction_hmac_before` / `_after` | the rolling HMAC for *this frame's* direction |
| `chain.cbc_iv_before` / `_after` | the CBC chaining block; `_before` is the `iv` only for the first frame in a direction, thereafter the previous frame's last ciphertext block |
| `chain.tx_hmac_after`, `chain.rx_hmac_after` | **both** chain states as of this frame |

`hc_corpus.verify_capture` (`scripts/hc_corpus.py`) re-derives all of it from `wire_hex` and the
key alone and checks every recorded field against the recomputation. The scrubber refuses to
write a corpus that fails it.

---

## 6. What this capture pins

Observed against the real hob, hcpy driving, **no `code` on any frame — in particular no 403
on `/ci/services`**. Several of these settle questions
[`handshake.md`](../docs/protocol/handshake.md) leaves to observation:

| Question | Observed |
|---|---|
| `/ci/services` 403? | **No.** Every response carries no `code`. |
| services table | `ro:1, ei:2, ci:2, ni:1` — **no `iz`** |
| `/ci/authentication` gate | hcpy's `"iz" not in services` and the spec's `ci < 3` **agree here**; both fire. Not a discriminating device. |
| `/ei/deviceReady` gate | ei is 2 in both the init message and the services table; all three predicates agree. Not discriminating. |
| post-connect order | hcpy sends `/ro/allMandatoryValues` **then** `/ro/allDescriptionChanges`, i.e. the *opposite* of the spec and of the other two references. The capture records hcpy's order. |
| `version` JSON type | the init message's `version` is the **integer** `2`, not a string, so a string-typed version cannot explain a 403 on this device. |
| `deviceType` on ei v2 | the string `"Application"`, as all three references compute it |
| unsolicited NOTIFY | `NOTIFY /ci/registeredDevices` arrives mid-handshake, unrequested — NOTIFY routing must handle it |
| live `NOTIFY /ro/values` | **none** — the hob was idle and was not touched during the capture. |

---

## 7. Regenerating

Prerequisites and safety rules: [`scripts/capture.md`](../scripts/capture.md).

```sh
python3 scripts/capture.py --alias hob --secrets-dir …/secrets --duration 150
python3 scripts/scrub-capture.py --secrets-dir …/secrets \
        scrub -i secrets/captures/hob/<timestamp>.json -o fixtures/aes/hob-session.json
python3 scripts/scrub-capture.py --secrets-dir …/secrets audit $(git ls-files)
```

Scrubbing the same raw capture twice is byte-identical. A *new* capture is not: `sID`, `msgID`
and the appliance nonce differ per session.

Live `NOTIFY /ro/values` frames only arrive when someone uses the appliance or it changes state
on its own, so a capture of an idle appliance contains none.
