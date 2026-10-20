# The AES app-layer crypto chain

Sources (see [`README.md`](./README.md) for URLs and commit SHAs):

- `reference/hcpy/HCSocket.py` — hcpy, the primary behavioural reference
- `reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py` — Python lib
- `reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go` — Go

**All three implementations agree byte-for-byte on everything in this document.** One
consequence: the `/ci/services` 403 analysed in [`handshake.md`](./handshake.md) is **not** a
crypto problem — a 403 that arrives cleanly decrypted proves both chains are working.

This document is intended to be sufficient to implement the chain (Kessel's `hc-crypto`)
without reading Python or Go.

---

## 1. Scope: when app-layer crypto applies at all

App-layer AES applies **only** to the AES transport (`ws://<host>:80/homeconnect`). It is
selected by the presence of an `iv` in the profile, not by the `connectionType` string:
hcpy takes the crypto path iff `iv64` was passed (`reference/hcpy/HCSocket.py:62-73`), and
homeconnect-websocket picks `AesSocket` iff `iv64` is truthy, else `TlsSocket`
(`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:92-99`). The Go repo
keys the transport off `connectionType` (`reference/go-homeconnect2mqtt/internal/homeconnect/socket.go:20-23`)
but its own spec records the same iv-presence rule as the underlying logic
(`reference/go-homeconnect2mqtt/docs/01-protocol.md:18-24`).

Over TLS-PSK there is **no** app-layer crypto — see [`tls-psk.md`](./tls-psk.md).

---

## 2. Key material and its decoding

`psk64` and `iv64` come from the appliance profile JSON as **URL-safe base64, padding
optional**. All three references decode leniently:

- hcpy appends `"==="` before decoding: `base64url(psk64 + "===")`
  (`reference/hcpy/HCSocket.py:58`, and the same for `iv64` at
  `reference/hcpy/HCSocket.py:65`).
- homeconnect-websocket does the identical `+ "==="` trick
  (`reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:200-201`).
- Go uses `base64.RawURLEncoding` (unpadded)
  (`reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:30-32`).

Python's `urlsafe_b64decode` ignores surplus `=`, so `+"==="` accepts both padded and
unpadded input; `RawURLEncoding` accepts only unpadded. *(inference)* For Rust, decode with
a URL-safe alphabet and **padding-indifferent** configuration so both profile shapes parse.

Sizes: `psk` is 32 bytes, `iv` is 16 bytes
(`reference/go-homeconnect2mqtt/docs/01-protocol.md:44-46`). The Go constructor enforces the
IV length explicitly, rejecting anything that is not the AES block size
(`reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:72-74`). Neither Python client
validates lengths. *(inference)* Kessel should validate both (`psk == 32`, `iv == 16`) at
construction and return a typed error, because a wrong-length `iv` otherwise fails much later
and much more confusingly.

---

## 3. Key derivation (KDF)

Two independent single-shot HMACs. **No HKDF, no salt, no info string, no expand round.**

```
enckey = HMAC_SHA256(key = psk, msg = "ENC")     # 32 bytes → the AES-256 key
mackey = HMAC_SHA256(key = psk, msg = "MAC")     # 32 bytes → the HMAC key
```

- hcpy: `self.enckey = hmac(self.psk, b"ENC")`, `self.mackey = hmac(self.psk, b"MAC")`
  (`reference/hcpy/HCSocket.py:66-67`), where `hmac()` is `HMAC.new(key, msg, SHA256).digest()`
  (`reference/hcpy/HCSocket.py:45-47`) — the **full 32-byte** digest, not truncated.
- homeconnect-websocket: `hmac.digest(psk, b"ENC", digest="sha256")` /
  `b"MAC"` (`reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:202-203`).
- Go: `deriveKeys` returns `hmacSHA256(psk, "ENC")` and `hmacSHA256(psk, "MAC")`
  (`reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:36-46`).

Label bytes are plain ASCII: `"ENC"` = `0x45 0x4E 0x43`, `"MAC"` = `0x4D 0x41 0x43`
(`reference/go-homeconnect2mqtt/docs/01-protocol.md:59`).

The `iv` is **not** derived. It is used raw, in two distinct roles (§4, §6):
1. the initial CBC IV for both the encryptor and the decryptor, and
2. a **constant prefix in every single HMAC computation**, for the whole connection
   (`reference/go-homeconnect2mqtt/docs/01-protocol.md:61-62`; visible in
   `reference/hcpy/HCSocket.py:88`).

Because `enckey` is 32 bytes, the cipher is **AES-256**-CBC. *(inference — the key length
follows from the SHA-256 digest size above; none of the three references names the variant, and
all three simply hand the 32-byte key to a generic AES constructor:
`reference/hcpy/HCSocket.py:83`;
`reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:212`;
`reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:76`.)*

---

## 4. Per-connection chain state

Four pieces of state, **reset on every `connect()`** and never carried across a reconnect:

```
last_tx_hmac = 16 × 0x00
last_rx_hmac = 16 × 0x00
aes_encrypt  = one persistent AES-256-CBC encryptor, seeded with iv
aes_decrypt  = one persistent AES-256-CBC decryptor, seeded with iv
```

- hcpy resets these in `reset()` (`reference/hcpy/HCSocket.py:77-84`), which is called at the
  top of `run_forever()` (`reference/hcpy/HCSocket.py:202`) — i.e. once per connection attempt.
- homeconnect-websocket resets them at the top of `AesSocket.connect()`
  (`reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:207-213`).
- Go resets them in `AESCrypto.Reset()`
  (`reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:95-105`), called from
  `AESSocket.Connect()` under the send lock so a reset cannot race an in-flight send
  (`reference/go-homeconnect2mqtt/internal/homeconnect/socket.go:112-117`).

Whether the state is *also* initialised at construction time varies between the references and
is **not** part of the contract: Go's `NewAESCrypto` calls `c.Reset()` before returning
(`reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:87`), whereas the two Python
clients set the fields only in their per-connection reset path (hcpy's `__init__` does not call
`reset()`, `reference/hcpy/HCSocket.py:50-73`). The binding rule is that the state is reset
per **connection**; a construction-time reset is harmless and Kessel may do either.

### Message counter

The msgID counter is **not** part of this per-connection crypto state and is deliberately not
specified here: it is a message-layer concern, seeded from the appliance's `edMsgID` and
carried by the session rather than the cipher, and none of the three references keeps it in
the crypto object. It is specified in full in
[`message-model.md`](./message-model.md) §5.1 (assignment and increment) and §5.2 (default
resolution before sending); see [`handshake.md`](./handshake.md) §2 for where the seed value
comes from.

**The two cipher objects are stateful and persistent for the whole connection.** They are
*not* re-created per message. This is the single most important implementation fact in this
document:

- **CBC continuity.** Because one `AES.new(..., MODE_CBC, iv)` object encrypts every outbound
  message in sequence (`reference/hcpy/HCSocket.py:83` used at
  `reference/hcpy/HCSocket.py:169`), the CBC IV for message *n+1* is the **last ciphertext
  block of message *n***, not `iv`. Only the very first message of a connection uses `iv` as
  its IV. The same holds independently for the receive direction
  (`reference/hcpy/HCSocket.py:84`, used at `reference/hcpy/HCSocket.py:145`).
  In Rust this maps onto the *stateful* `cbc::Encryptor`/`cbc::Decryptor` held across calls,
  **not** to a per-message `encrypt` helper. *(inference)*
- **HMAC continuity.** Each direction carries a rolling 16-byte value that feeds into the next
  message's HMAC input (§5, §6).

TX and RX are **fully independent**: separate cipher objects, separate rolling HMACs, separate
direction bytes. Go gives each its own mutex
(`reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:59-65`) with the explicit note
that "two concurrent sends would corrupt last_tx_hmac and the CBC chain"
(`reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:50-51`).

### Direction bytes

| Direction | Byte | ASCII |
|---|---|---|
| Client → appliance (TX / "encrypt") | `0x45` | `E` |
| Appliance → client (RX / "decrypt") | `0x43` | `C` |

`reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:168-169`;
`reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:22-23`; in hcpy they appear as
literals `b"\x45"` (`reference/hcpy/HCSocket.py:173`) and `b"\x43"`
(`reference/hcpy/HCSocket.py:136`).

The bytes are from the **client's** point of view and are fixed constants for a client
implementation: the client always signs with `E` and verifies with `C`
(`reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:17-19`). *(inference)* A test
harness that plays the *appliance* side must therefore seal with `C` and open with `E`; Go
parameterises `seal`/`open` by direction for exactly this reason
(`reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:122-124`), and a "mirror"
decryptor used in tests needs the same affordance.

---

## 5. Send path — encrypt-then-MAC

### 5.0 What `clear` is

`clear` is the **UTF-8 bytes of the compact JSON string** described in
[`message-model.md`](./message-model.md) §2.1 — the JSON is serialised to text first and encoded
to bytes with UTF-8, with no length prefix, no BOM and no trailing newline. Everything below
operates on those bytes.

- hcpy converts explicitly at the top of `encrypt()`: `clear_msg = bytes(clear_msg, "utf-8")`,
  under the comment "convert the UTF-8 string into a byte array"
  (`reference/hcpy/HCSocket.py:156-157`).
- homeconnect-websocket does the same in `AesSocket.send()`
  (`reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:225-226`).
- Go's `AESSocket.Send` takes a `string` and converts with `[]byte(message)`
  (`reference/go-homeconnect2mqtt/internal/homeconnect/socket.go:141`), which is a UTF-8 byte
  copy; `AESCrypto.Encrypt` itself takes `[]byte`
  (`reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:110`), and the `Socket`
  interface documents that Send/Receive "exchange already decoded UTF-8 message text"
  (`reference/go-homeconnect2mqtt/internal/homeconnect/socket.go:32-34`).

This is symmetric with the RX side (§6 step 6), which decodes the unpadded plaintext as UTF-8.
It is load-bearing for reproducing captured ciphertext: the padding length is computed over the
**byte** length, so any non-ASCII character in a value changes `pad_len`
and therefore the whole ciphertext. *(inference — arithmetic on §5.1's rule.)*

### 5.1 Padding (custom, **not** PKCS#7)

```
pad_len = 16 - (len(clear) % 16)
if pad_len == 1:
    pad_len += 16                                  # ⇒ pad_len ∈ {2..16} ∪ {17}
padded = clear ‖ 0x00 ‖ random(pad_len - 2) ‖ byte(pad_len)
```

- hcpy: `reference/hcpy/HCSocket.py:160-165`.
- homeconnect-websocket: `reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:228-233`.
- Go: `AESCrypto.pad`, `reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:185-200`.

Properties, all three identical:

- The **first** pad byte is always `0x00`.
- The **last** byte is always `pad_len` itself.
- The bytes in between are **cryptographically random** and carry no information — hcpy uses
  `Crypto.Random.get_random_bytes` (`reference/hcpy/HCSocket.py:15`, used at
  `reference/hcpy/HCSocket.py:163`); Go uses an injectable `io.Reader` defaulting to
  `crypto/rand.Reader` (`reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:57`,
  `:85`) specifically so tests can be deterministic.
- There are **always at least 2 pad bytes** (the `0x00` and the length byte), which is why a
  would-be `pad_len == 1` is bumped to 17.
- The result is always a multiple of 16.
- `pad_len` is **never 0** — a plaintext that is already a multiple of 16 gets a full 16-byte
  pad block, not zero padding.

Worked examples (stated as "verified" in
`reference/go-homeconnect2mqtt/docs/01-protocol.md:106`):

| plaintext | `len % 16` | `pad_len` | padded length |
|---|---|---|---|
| `""` (0 bytes) | 0 | **16** | 16 |
| `"a"` (1 byte) | 1 | **15** | 16 |
| 15 bytes | 15 | 1 → **17** | 32 |
| 16 bytes | 0 | **16** | 32 |

For the 15-byte case the padded buffer is
`clear(15) ‖ 0x00 ‖ random(15) ‖ 0x11` = 32 bytes. *(inference — arithmetic on the rule
above.)*

Note the interaction with §7's minimum-length rule: the smallest possible **wire frame** is
16 bytes of ciphertext + 16 bytes of MAC = 32 bytes, which is exactly
`MINIMUM_MESSAGE_LENGTH`. *(inference)*

### 5.2 Encrypt

```
ct = aes_encrypt.encrypt(padded)      # continuous CBC — see §4
```

`reference/hcpy/HCSocket.py:169`;
`reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:235`;
`reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:133-134`.

### 5.3 MAC and chain advance

```
mac_input    = iv ‖ 0x45 ‖ last_tx_hmac ‖ ct
last_tx_hmac = HMAC_SHA256(mackey, mac_input)[0:16]     # truncated to 16 bytes
```

hcpy expresses this as `hmac_msg(b"\x45" + self.last_tx_hmac, enc_msg)`
(`reference/hcpy/HCSocket.py:173`) where `hmac_msg(direction, enc_msg)` builds
`self.iv + direction + enc_msg` and truncates to 16
(`reference/hcpy/HCSocket.py:87-89`) — so the `direction` argument there is really
*direction byte concatenated with the previous HMAC*, and the resulting input is
`iv ‖ 0x45 ‖ last_tx_hmac ‖ ct`, exactly as spelled out in
`reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:237-238` and
`reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:172-179`.

The **order of the four parts is load-bearing** and is `iv`, then direction, then previous
HMAC, then ciphertext.

The MAC is **truncated to the first 16 bytes** of the 32-byte HMAC-SHA256 output
(`reference/hcpy/HCSocket.py:89`;
`reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:238`;
`reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:178`).

`last_tx_hmac` is updated to the **newly computed** MAC, i.e. the chain advances on every
send unconditionally (there is no failure mode on the send path other than the RNG failing).

### 5.4 Frame

```
wire = ct ‖ last_tx_hmac              # sent as a WebSocket BINARY frame
```

`reference/hcpy/HCSocket.py:176` (sent via `ws.send_bytes` at
`reference/hcpy/HCSocket.py:184`);
`reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:240`;
`reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:139-142`, written as
`websocket.MessageBinary` at
`reference/go-homeconnect2mqtt/internal/homeconnect/socket.go:145`.

### 5.5 Determinism caveat

Because of the random middle pad bytes, **two encryptions of the same plaintext differ**.
Tests must handle this explicitly: either seed the RNG in test mode (Go's injectable `rand`
field, `reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:57`, is precedent for
exactly this design) or compare structurally via a mirror decryptor. Kessel's `hc-crypto`
takes the pad RNG as a constructor parameter for the same reason.

---

## 6. Receive path — MAC-then-decrypt

```
1. reject unless the frame is BINARY, len ≥ 32, and len % 16 == 0
2. ct        = buf[ .. len-16 ]
   recv_hmac = buf[ len-16 .. ]
3. calc = HMAC_SHA256(mackey, iv ‖ 0x43 ‖ last_rx_hmac ‖ ct)[0:16]
   if not constant_time_eq(recv_hmac, calc):  -> hard authentication error
4. last_rx_hmac = recv_hmac          # advance ONLY after a successful verification
5. plain = aes_decrypt.decrypt(ct)   # continuous CBC — see §4
6. pad_len = plain[len(plain)-1]
   if len(plain) < pad_len: -> padding error
   message = utf8(plain[ .. len(plain)-pad_len ])
```

`reference/hcpy/HCSocket.py:124-153`;
`reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:242-280`;
`reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:146-169` and `:203-212`.

Points that matter for a byte-exact implementation:

- The MAC is verified **before** decryption, against the *ciphertext*, and the RX chain value
  stored is the **received** MAC (`recv_hmac`), which on success equals `calc`
  (`reference/hcpy/HCSocket.py:142`;
  `reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:269`;
  `reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:164`).
- **The chain must not advance on failure.** Go states this explicitly ("Advance the chain
  only after a successful verification",
  `reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:163`) and returns before the
  assignment on mismatch (`:160-162`).
- **Comparison must be constant time.**
  homeconnect-websocket uses `hmac.compare_digest`
  (`reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:264`); Go uses
  `hmac.Equal` (`reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:160`).
  **hcpy does not** — it uses a plain `!=` (`reference/hcpy/HCSocket.py:138`). hcpy is the
  weaker implementation here; a timing side channel is not wire-observable behaviour, so
  there is no compatibility reason to follow it. Kessel uses the constant-time form.
- Unpadding trusts the trailing length byte, bounded only by the buffer length. Go
  additionally rejects `pad_len < 1` and an empty plaintext
  (`reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:205-210`); the Python
  clients check only `len(msg) < pad_len` (`reference/hcpy/HCSocket.py:149`;
  `reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:273`).
  *(inference)* Take the Go behaviour (reject `pad_len == 0` and `pad_len > len(plain)`),
  which is a strict superset of the Python checks and cannot change what a well-formed
  appliance frame decodes to.
- The plaintext is UTF-8 and is decoded as such
  (`reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:277`).

---

## 7. Framing and length rules

| Rule | Value | Citation |
|---|---|---|
| WebSocket frame type (AES mode) | **binary**, both directions | send: `reference/hcpy/HCSocket.py:184`; receive check: `reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:243-246` and `reference/go-homeconnect2mqtt/internal/homeconnect/socket.go:160-162` |
| Minimum inbound frame length | **32** bytes | `reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:170` and `:249`; `reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:24`, `:147-149`; hcpy `reference/hcpy/HCSocket.py:125` |
| Inbound length must be a multiple of 16 | yes | `reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:253-256`; `reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:150-152` |
| MAC tag length on the wire | 16 bytes (suffix) | `reference/hcpy/HCSocket.py:132-133` |
| Read limit | 1 MiB (Go only) | `reference/go-homeconnect2mqtt/internal/homeconnect/socket.go:30`, `:108` |

**Divergence on the alignment rule:** hcpy *warns* on a non-multiple-of-16 frame but then
continues to process it (`reference/hcpy/HCSocket.py:128-129` — the `print` is not followed by
a `return`), whereas both the other implementations raise. *(inference)* Treat it as a hard
error; hcpy's leniency here is a bug, not behaviour worth carrying, and an unaligned frame
cannot be CBC-decrypted anyway.

**Divergence on non-binary frames:** hcpy's `recv` treats an empty/`None` payload as "no
message" and returns `None` (`reference/hcpy/HCSocket.py:190-191`), and its `decrypt` returns
`None` on a short frame (`reference/hcpy/HCSocket.py:125-127`) — i.e. it *keeps reading*.
homeconnect-websocket and Go both raise/close. Kessel takes the strict form: first failure ⇒
drop the socket and reconnect, never continue on a chain whose state can no longer be
trusted.

---

## 8. Failure taxonomy for `hc-crypto`

Every one of these is a **hard error** that the caller must treat as "close + full
reconnect". None of them is recoverable in place, because once the RX
chain's position is uncertain every subsequent frame will also fail.

| Condition | Where detected |
|---|---|
| frame shorter than 32 bytes | `reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:147-149` |
| frame length not a multiple of 16 | `reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:150-152` |
| HMAC mismatch | `reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:160-162`; `reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:264-267` raises `AuthenticationError` |
| pad length byte out of range | `reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:205-210` |
| plaintext is not valid UTF-8 | *(inference)* — implied by the `.decode("utf-8")` at `reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:277`, which would raise |
| non-binary WebSocket frame | `reference/go-homeconnect2mqtt/internal/homeconnect/socket.go:160-162` |

---

## 9. Concurrency obligations

`hc-crypto` itself is pure and I/O-free, but its *state* is shared, and the
references document what happens when that sharing is unsynchronised:

- The TX CBC state and `last_tx_hmac` must advance in **exactly** the order frames hit the
  wire. Go therefore holds a single `sendMu` across *encrypt + write*, not just across
  encrypt (`reference/go-homeconnect2mqtt/internal/homeconnect/socket.go:74-77`, `:134-146`),
  noting that otherwise "the peer desyncs fatally"
  (`reference/go-homeconnect2mqtt/internal/homeconnect/socket.go:75-77`). Encrypting under a
  lock and then writing outside it is **not** sufficient. *(inference — but it is the stated
  rationale for the wider lock.)*
- The chain reset on reconnect must also be inside that same lock, so an in-flight send
  cannot advance the fresh chain for a frame aimed at the dead connection
  (`reference/go-homeconnect2mqtt/internal/homeconnect/socket.go:109-117`).
- TX and RX take **separate** locks
  (`reference/go-homeconnect2mqtt/internal/homeconnect/crypto.go:59-65`); a single lock would
  needlessly serialise the read loop against sends.

---

## 10. Implementation checklist

Each item is specified above.

- KDF derives `enckey` and `mackey` — §3
- Padding: empty → 16, one byte → 15, fifteen bytes → 17, sixteen bytes → 16 — §5.1
- `pad_len` is never 1; first pad byte is `0x00`, last is `pad_len` — §5.1
- Unpadding rejects an out-of-range length byte — §6, §8
- TX chain reproduces captured ciphertext with a seeded RNG — §5.5
- RX chain decrypts captured frames — §6
- Round trip through a mirror decryptor (note the direction-byte inversion) — §4
- HMAC comparison is constant time — §6
- A bad HMAC is a hard error and does not advance the RX chain — §6, §8
- Chain state resets on every new connection — §4
- Short frames and frames not a multiple of 16 are rejected — §7, §8
