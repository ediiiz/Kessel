# The TLS-PSK transport

Sources (see [`README.md`](./README.md) for URLs and commit SHAs):

- `reference/hcpy/HCSocket.py`
- `reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py`
- `reference/go-homeconnect2mqtt/internal/homeconnect/socket_tls_psk.go` and `.../socket_tls.go`

Appliances whose profile says `connectionType: "TLS"` use this transport; the others use the
AES transport described in [`aes-chain.md`](./aes-chain.md). Of the four appliances this
reference was checked against, three (dishwasher, oven, fridge) are TLS-PSK.

---

## 1. Selection and endpoint

| | AES | TLS-PSK |
|---|---|---|
| URL | `ws://<host>:80/homeconnect` | `wss://<host>:443/homeconnect` |
| Selected by | `iv` present in the profile | no `iv`, `psk` present |
| Frame type | binary | **text** |
| App-layer crypto | yes | **none** |

`reference/hcpy/HCSocket.py:62-73` (the `if iv64:` branch chooses port 80 + `ws://`, the
`else` chooses 443 + `wss://`);
`reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:20` and `:116`;
`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:92-99`;
`reference/go-homeconnect2mqtt/internal/homeconnect/socket.go:46-66`.

The path is always `/homeconnect` and **no HTTP headers are sent** — no subprotocol, no
Origin, no authorization header. Authentication is the TLS-PSK handshake itself
(`reference/go-homeconnect2mqtt/docs/01-protocol.md:26-30`).

---

## 2. There is no app-layer crypto over the tunnel

This is the defining difference. Once the tunnel is up, messages are the **same JSON** as in
AES mode but sent as **plain UTF-8 WebSocket text frames**, with no padding, no CBC chain, no
HMAC chain, no `iv`, and no `enckey`/`mackey` derivation at all:

- hcpy's `send` branches on `self.http`: encrypt only in the AES case, otherwise
  `self.ws.send(buf)` — a text send (`reference/hcpy/HCSocket.py:183-186`). Its `recv`
  likewise decrypts only in the AES case (`reference/hcpy/HCSocket.py:193-194`). `reset()`
  returns immediately for TLS (`reference/hcpy/HCSocket.py:78-79`), so no chain state exists.
- `TlsSocket.send` is `send_str` and `_receive` returns `message.data` unchanged
  (`reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:156-165`).
- Go writes `websocket.MessageText` and rejects a non-text frame
  (`reference/go-homeconnect2mqtt/internal/homeconnect/socket_tls_psk.go:326-339`), commented
  "Messages are plain UTF-8 text (TLS protects everything)"
  (`reference/go-homeconnect2mqtt/internal/homeconnect/socket_tls_psk.go:320-321`).

Everything in [`message-model.md`](./message-model.md) and [`handshake.md`](./handshake.md)
applies unchanged; nothing in [`aes-chain.md`](./aes-chain.md) applies at all. In Kessel,
`hc-transport::tls_psk` therefore has no `hc-crypto` dependency.

---

## 3. The PSK

The `psk` is used **directly** as the TLS pre-shared key — no KDF, no `ENC`/`MAC` labels, no
`iv`:

- hcpy passes `self.psk` straight into the callback
  (`reference/hcpy/HCSocket.py:104`), where `self.psk = base64url(psk64 + "===")`
  (`reference/hcpy/HCSocket.py:58`).
- homeconnect-websocket: `psk = urlsafe_b64decode(psk64 + "===")`, then
  `set_psk_client_callback(lambda _: (None, psk))`
  (`reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:138`, `:144`).
- Go: the raw psk bytes are copied into C memory and returned verbatim by the callback
  (`reference/go-homeconnect2mqtt/internal/homeconnect/socket_tls_psk.go:43-46`).

32 bytes, URL-safe base64, padding optional — same decoding rules as
[`aes-chain.md`](./aes-chain.md) §2.

---

## 4. PSK identity

The three references send three different identities:

| Implementation | Identity sent | Citation |
|---|---|---|
| hcpy | **`"HCCOM_Local_App"`** | `reference/hcpy/HCSocket.py:104` |
| homeconnect-websocket | `None` (zero-length on the wire) | `reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:144` |
| Go | **empty string** (`identity[0] = '\0'`) | `reference/go-homeconnect2mqtt/internal/homeconnect/socket_tls_psk.go:42` |

hcpy's comment explains where its value comes from:

> "Identity hint from server is `HCCOM_Local_App` but can be null"
> (`reference/hcpy/HCSocket.py:103`)

On the wire these are only two distinct values: `None` and `""` both produce a zero-length
identity.

**Observed on hardware** (dishwasher, oven and fridge — the three TLS appliances available):

- every appliance advertises the identity hint `HCCOM_Local_App` in its ServerKeyExchange;
- `""` is accepted by all three, and `"HCCOM_Local_App"` by the dishwasher (not separately tried
  on the other two);
- one arbitrary value that no reference sends and no appliance advertises (`"kessel-nonsense"`)
  is accepted by all three.

So the appliance does not check the client identity for its single-PSK configuration, at least
at the firmware tested. One nonsense value was tried, once per appliance; the identity input
space (over-length, non-UTF-8, …) was not swept. The arbitrary-identity probe ran through system
OpenSSL's `s_client` (3.0.13, `-tls1_2 -cipher 'PSK@SECLEVEL=0' -noservername`), not through
Kessel's client, and no verbatim transcript of it was kept. Kessel's own client connects all
three appliances with `""`.

Kessel sends `""` and keeps a fallback to `"HCCOM_Local_App"` on handshake failure (see
[Identity fallback order](#identity-fallback-order)). On the tested firmware the fallback is
purely defensive.

---

## 5. TLS version, cipher suites, verification

What the references configure:

| Setting | hcpy | homeconnect-websocket | Go |
|---|---|---|---|
| max version | TLS 1.2 (`reference/hcpy/HCSocket.py:100`) | TLS 1.2 (`.../hc_socket.py:140`) | TLS 1.2 (`.../socket_tls_psk.go:61`) |
| min version | **SSLv3** (`reference/hcpy/HCSocket.py:101`) | not set (library default) | **TLS 1.2** (`.../socket_tls_psk.go:60`) |
| cipher string | `"PSK"` (`reference/hcpy/HCSocket.py:102`) | `"PSK"` (`.../hc_socket.py:141`) | `"PSK"` (`.../socket_tls_psk.go:63`) |
| security level | n/a (Python `ssl`) | n/a | **`SSL_CTX_set_security_level(ctx, 0)`** (`.../socket_tls_psk.go:62`) |
| hostname check | off (`reference/hcpy/HCSocket.py:97`) | off (`.../hc_socket.py:142`) | n/a (no cert) |
| certificate verification | `CERT_NONE` (`reference/hcpy/HCSocket.py:98`) | `CERT_NONE` (`.../hc_socket.py:143`) | `SSL_VERIFY_NONE` (`.../socket_tls_psk.go:65`) |

**TLS 1.3 is not supported by the appliance.** hcpy notes "TLSv1.3 doesn't appear to be
supported" (`reference/hcpy/HCSocket.py:99`) and caps at 1.2; Go pins min *and* max to
`TLS1_2_VERSION` (`reference/go-homeconnect2mqtt/internal/homeconnect/socket_tls_psk.go:60-61`).
Observed on hardware: a client offering 1.2–1.3 still negotiates 1.2.

**Security level.** Go sets level 0 with the comment "OpenSSL 3 hides these behind security
level 0" (`reference/go-homeconnect2mqtt/internal/homeconnect/socket_tls_psk.go:13-14`, code at
`:62`), as a **separate API call** with the plain cipher string `"PSK"` — not via an
`@SECLEVEL=0` suffix. The `openssl` Rust crate exposes the same call as
`SslContextBuilder::set_security_level`. Against the dishwasher with OpenSSL 3.6.3, level 0 is
**not** required (see the attempt log below); setting it is still recommended because it costs
nothing and other OpenSSL builds or appliances may filter the plain-PSK CBC suites at a higher
level.

### Cipher suites

The references disagree in their comments about which suite family is used:

- Go's header comment records: "The appliance negotiates an ECDHE-PSK suite (**verified:
  ECDHE-PSK-CHACHA20-POLY1305**)"
  (`reference/go-homeconnect2mqtt/internal/homeconnect/socket_tls_psk.go:12-13`).
- hcpy's `sslpsk` fallback path hardcodes `ciphers="ECDHE-PSK-CHACHA20-POLY1305"`
  (`reference/hcpy/HCSocket.py:116`), and its native-PSK path comments
  "Originally ECDHE-PSK-CHACHA20-POLY1305a" before widening to `"PSK"`
  (`reference/hcpy/HCSocket.py:102`).
- The Go repository's prose spec says "legacy TLS 1.2 PSK cipher suites (`TLS_PSK_WITH_AES_*`)"
  (`reference/go-homeconnect2mqtt/docs/01-protocol.md:153`).

**Observed on hardware** (dishwasher): both families are offered. With `"PSK"` the appliance
picks `ECDHE-PSK-CHACHA20-POLY1305` (`TLS_ECDHE_PSK_WITH_CHACHA20_POLY1305_SHA256`); offered only
`PSK-AES256-CBC-SHA:PSK-AES128-CBC-SHA`, it negotiates `TLS_PSK_WITH_AES_256_CBC_SHA`. The bare
`"PSK"` string selects both families and is the right choice. The oven and fridge negotiated
`ECDHE-PSK-CHACHA20-POLY1305` with `PSK@SECLEVEL=0`; their support for the CBC family was not
probed.

### No rustls

rustls implements neither family for TLS 1.2 external PSK, so the transport needs OpenSSL. Go
hit the same wall — "Go's `crypto/tls` has no external-PSK cipher suites" — and quarantined the
path behind a cgo build tag
(`reference/go-homeconnect2mqtt/internal/homeconnect/socket_tls.go:12-15`,
`reference/go-homeconnect2mqtt/internal/homeconnect/socket_tls_psk.go:4-9`), with a
compile-time `TLSPSKSupported` constant
(`reference/go-homeconnect2mqtt/internal/homeconnect/tlspsk_unsupported.go:10`) and a
fail-fast stub that a device worker treats as "offline"
(`reference/go-homeconnect2mqtt/internal/homeconnect/socket_tls.go:15`, `:56-59`).

---

## 6. Structuring the connection: TLS first, then the WebSocket upgrade

Both non-trivial implementations establish the TLS tunnel over a **raw TCP socket** and only
then run the WebSocket upgrade over it:

- hcpy creates a plain `socket.create_connection`, wraps it with the PSK context, and hands
  the wrapped socket to `websocket.WebSocketApp(..., socket=sock)`
  (`reference/hcpy/HCSocket.py:207`, `:255`, `:287-294`).
- Go dials TCP, completes the OpenSSL handshake over memory BIOs, then dials `ws://` (not
  `wss://`) through a one-shot `http.Transport` whose `DialContext` returns the already-TLS
  connection (`reference/go-homeconnect2mqtt/internal/homeconnect/socket_tls_psk.go:350-381`,
  the `ws://` substitution at `:356`).

In Rust this is `tungstenite`'s client over an `openssl`-wrapped stream, rather than the
WebSocket library's built-in TLS. The OpenSSL handshake is blocking; Go instead drives OpenSSL
through memory BIOs so the surrounding socket keeps full deadline/context support
(`reference/go-homeconnect2mqtt/internal/homeconnect/socket_tls_psk.go:16-19`) — a fully async
alternative.

---

## 7. Heartbeat

| | Interval | Citation |
|---|---|---|
| homeconnect-websocket | **20 s** (aiohttp `heartbeat=20`) | `reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:68` |
| hcpy | **120 s** ping interval, 10 s ping timeout | `reference/hcpy/HCSocket.py:301` |
| Go | configurable; zero disables the probe | `reference/go-homeconnect2mqtt/internal/homeconnect/session.go:24-27`, loop at `:254-285` |

Kessel uses 20 s, matching homeconnect-websocket rather than hcpy. The heartbeat is a
WebSocket-level ping, not part of the application protocol, and a shorter interval detects a
dead peer sooner; keep the difference in mind when diffing captures against hcpy at the frame
level. hcpy also sets TCP keepalive (idle 30 s, interval 10 s, count 3) on Linux
(`reference/hcpy/HCSocket.py:209-215`).

Go's heartbeat, on failure, closes the socket so the reconnect manager reacts, rather than
retrying (`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:269-283`).

---

## Verified against real hardware

### Environment

| | |
|---|---|
| Device | dishwasher **BOSCH SMD8TCX01E**, `connectionType: "TLS"`, profile has `key` and **no** `iv` (so §1's selection rule holds) |
| OpenSSL | **3.6.3**, from `openssl-src 300.6.1+3.6.3` via the `openssl` crate 0.10.81 with `features = ["vendored"]` |
| Client stack | blocking `std::net::TcpStream` → `openssl::ssl::SslConnector` → `tungstenite::client` over the wrapped stream (§6: TLS first, then a `ws://` upgrade over the tunnel) |

### The configuration that works

```
SslConnector::builder(SslMethod::tls_client())
  .set_min_proto_version(Some(SslVersion::TLS1_2))
  .set_max_proto_version(Some(SslVersion::TLS1_2))
  .set_security_level(0)          // separate call, NOT an @SECLEVEL=0 suffix
  .set_cipher_list("PSK")         // REQUIRED — see the attempt log
  .set_verify(SslVerifyMode::NONE)
  .set_psk_client_callback(...)   // identity "", psk = the 32 raw bytes — full contract below
  → .configure().verify_hostname(false), set_use_server_name_indication(false)
  → connect()
then: tungstenite client upgrade to  ws://<host>/homeconnect  (no extra headers)
```

#### The PSK callback contract (get this wrong and it fails silently)

`set_psk_client_callback` hands you `(ssl, hint, identity_out, psk_out)` and expects you to
return `Ok(<number of psk bytes written>)`. Two details are load-bearing and neither is
enforced by the type system:

1. **`identity_out` must contain a NUL-terminated C string, not a bare byte slice.** OpenSSL
   reads it with C string semantics. For the empty identity that means writing a single zero
   byte — `identity_out[0] = 0` — *not* writing nothing. For a non-empty identity, copy the
   bytes and then write the terminator at `identity_out[id.len()]`. The buffer must therefore
   have room for `id.len() + 1`; check that and return an error rather than panicking.
2. **The return value is the psk length, not the identity length.** Returning the wrong count
   sends a truncated or over-long pre-shared key, and the failure surfaces as a generic
   handshake failure much later, with nothing pointing at the callback.

```rust
b.set_psk_client_callback(move |_ssl, _hint, identity_out, psk_out| {
    let id: &[u8] = b"";                       // see the fallback order below
    if id.len() + 1 > identity_out.len() || psk.len() > psk_out.len() {
        return Err(openssl::error::ErrorStack::get());
    }
    identity_out[..id.len()].copy_from_slice(id);
    identity_out[id.len()] = 0;                // REQUIRED: NUL terminator
    psk_out[..psk.len()].copy_from_slice(&psk);
    Ok(psk.len())                              // REQUIRED: the *psk* length
});
```

The callback is a **safe API** in the `openssl` crate 0.10.81 — an ordinary closure over
`&mut [u8]` buffers. The whole configuration needs no `unsafe`; needing it is a sign of reaching
for a raw `openssl-sys` entry point where a safe wrapper exists.

#### Build-level notes

- **Keep `tungstenite` at `default-features = false`.** At `tungstenite 0.30` the defaults are
  exactly `["handshake"]` and bring in no TLS, so this is defensive: every TLS feature the crate
  offers (`native-tls`, `rustls-tls-*`) is opt-in, and enabling one would link a second TLS stack
  alongside the vendored OpenSSL. The upgrade runs over an already-TLS stream with scheme `ws`,
  so `tungstenite` needs no TLS at all.
- **Checking for a vendored build.** Out of process: `ldd` shows no `libssl`/`libcrypto`, and
  `libssl.a`/`libcrypto.a` exist under `target/*/build/openssl-sys-*/out/`. In process: a
  vendored build links OpenSSL statically, so no `libssl.so`/`libcrypto.so` is mapped in
  `/proc/self/maps`. `openssl::version::version()` reports the linked library's version (e.g.
  "OpenSSL 3.6.3 9 Jun 2026") but is not by itself proof of a vendored link, and
  `openssl::version::dir()` is **not** a discriminator: a vendored build reports openssl-src's
  upstream default `/usr/local/ssl`, not a path inside `target/`.

### Observed values

| Item | Observed value |
|---|---|
| cipher string | `"PSK"` |
| security level | `0`, via `SslContextBuilder::set_security_level(0)` |
| PSK identity used | `""` (empty string) — accepted |
| psk | the profile's 32 raw bytes, passed verbatim, no KDF (§3 confirmed) |
| negotiated protocol version | **TLSv1.2** |
| negotiated cipher suite | **`ECDHE-PSK-CHACHA20-POLY1305`** = `TLS_ECDHE_PSK_WITH_CHACHA20_POLY1305_SHA256` |
| appliance identity hint | **`HCCOM_Local_App`**, as hcpy's comment at `reference/hcpy/HCSocket.py:103` says |
| SNI | not sent (`set_use_server_name_indication(false)`); no certificate is presented |
| WebSocket upgrade | `HTTP 101 Switching Protocols`, no subprotocol, no extra headers |
| first device frame | `POST /ei/initialValues` **as a plaintext WebSocket text frame**, ≈30 ms after the upgrade, arriving unprompted — confirming §2 and `handshake.md` §1 ("the client never speaks first") |

Redacted first frame (the `sID`/`msgID`/`edMsgID` values are fresh random per connection;
the frame carries no serial, MAC, haId or shipSki):

```json
{"sID":<u32>,"msgID":<u32>,"resource":"/ei/initialValues","version":2,"action":"POST","data":[{"edMsgID":<u32>}]}
```

The appliance's `/ei/initialValues` is **version 2**, so `handshake.md` §3's rule selects the
string `"Application"` for `deviceType`. The `RESPONSE` sent back over the tunnel was accepted
(no error frame, no close); the appliance then waits for the client to drive step [2], as
`handshake.md` §1 describes.

### Attempt log (dishwasher)

All attempts were read-only. Every attempt held `set_verify(NONE)` + `verify_hostname(false)` +
no SNI, and varied only the columns shown.

| # | cipher string | seclevel | min–max | identity | Result |
|---|---|---|---|---|---|
| 1 | `"PSK"` | 0 | 1.2–1.2 | **`""`** (Go) | **OK** — TLSv1.2, ECDHE-PSK-CHACHA20-POLY1305 |
| 2 | `"PSK"` | 0 | 1.2–1.2 | `"HCCOM_Local_App"` (hcpy) | OK — same suite |
| 3 | `"PSK"` | 0 | 1.2–1.2 | `None`→empty (homeconnect-websocket) — **wire-identical to #1** | OK — same suite |
| 4 | `"PSK"` | 0 | 1.2–1.2 | echo of the server hint — **wire-identical to #2** | OK — same suite |
| 5 | `"ECDHE-PSK-CHACHA20-POLY1305"` | 0 | 1.2–1.2 | `"HCCOM_Local_App"` | OK — same suite |
| 6 | `"PSK-AES256-CBC-SHA:PSK-AES128-CBC-SHA"` | 0 | 1.2–1.2 | `"HCCOM_Local_App"` | **OK** — negotiated `PSK-AES256-CBC-SHA` (`TLS_PSK_WITH_AES_256_CBC_SHA`) |
| 7 | `"PSK"` | 1 | 1.2–1.2 | `"HCCOM_Local_App"` | OK — ECDHE-PSK-CHACHA20-POLY1305 |
| 8 | `"PSK"` | 0 | **SSLv3**–1.2 | `"HCCOM_Local_App"` | OK — still negotiates 1.2 |
| 9 | `"PSK"` | **2** | 1.2–1.2 | `"HCCOM_Local_App"` | OK — ECDHE-PSK-CHACHA20-POLY1305 |
| 10 | `"PSK-AES256-CBC-SHA"` | 1 | 1.2–1.2 | `"HCCOM_Local_App"` | OK — `PSK-AES256-CBC-SHA` |
| 11 | `"PSK"` | 0 | 1.2–**1.3** | `"HCCOM_Local_App"` | OK — server still picks **1.2** |
| 12 | `"DEFAULT"` | 0 | 1.2–1.2 | `"HCCOM_Local_App"` | OK — ECDHE-PSK-CHACHA20-POLY1305 |
| 13 | *not set* (crate default list) | *not set* | 1.2–1.2 | `"HCCOM_Local_App"` | **FAIL** — `error:0A0000F8:SSL routines:set_client_ciphersuite:unknown cipher returned:ssl/statem/statem_clnt.c:1400` |
| 14 | *not set* (crate default list) | *not set* | 1.0–1.3 | `"HCCOM_Local_App"` | **FAIL** — same `unknown cipher returned` error |
| 15 | `"DEFAULT"` | *not set* | 1.2–1.2 | `"HCCOM_Local_App"` | OK — ECDHE-PSK-CHACHA20-POLY1305 |
| 16 | `"PSK"` | *not set* | 1.2–1.2 | `"HCCOM_Local_App"` | OK — ECDHE-PSK-CHACHA20-POLY1305 |

Attempt 6 passed the cipher list without an `@SECLEVEL=0` suffix and reached level 0 through
`set_security_level(0)`; the result is about the suite enumeration, not the suffix syntax.

### What the log establishes

1. **Setting a cipher list is the one load-bearing knob.** Attempts 13/14 are the only
   failures, because the `openssl` crate's `SslConnector::builder` installs a curated
   (Mozilla-style) default cipher list with **no PSK suites**. Attempt 15 — just
   `set_cipher_list("DEFAULT")` with everything else at library default — succeeds. The
   requirement is "do not leave the crate's default list in place", not "use this exact string".
2. **The failure mode is a confusing client-side error, not a clean alert.** When the client
   offers no PSK suite the appliance replies with a PSK suite anyway, and OpenSSL rejects its own
   peer's choice with `set_client_ciphersuite:unknown cipher returned` rather than a
   `handshake_failure`. `hc-transport` maps this signature to a dedicated "missing PSK cipher"
   error.
3. **Security level 0 is not required against this device with OpenSSL 3.6.3** (attempts 7, 9,
   15, 16 pass at levels 1, 2 and the library default), and plain-PSK AES-CBC negotiates at
   level 1 (attempt 10). Go's comment that OpenSSL 3 hides PSK suites behind level 0 does not
   reproduce here. Level 0 remains the recommended setting (§5).
4. **Both suite families are offered** (attempts 1 and 6); `"PSK"` selects both and the
   appliance prefers ECDHE-PSK.
5. **The TLS 1.2 cap is not load-bearing but is harmless.** Offering 1.3 (attempt 11) still lands
   on 1.2. Keeping the cap makes negotiation deterministic and avoids OpenSSL's separate TLS 1.3
   PSK API.

The cipher and security-level matrix was run against the dishwasher only. The oven and fridge
were checked for identity acceptance (§4) with `PSK@SECLEVEL=0`, which negotiated the same
ECDHE-PSK suite; whether they also offer the CBC family, or need level 0, was not measured.

#### Identity fallback order

`""` and `"HCCOM_Local_App"` are both accepted, and on the tested firmware so is an arbitrary
value (§4). The order still matters for an untested appliance or firmware, and it follows from
asymmetry of risk:

- `"HCCOM_Local_App"` is what the device advertises as its identity hint and what hcpy sends.
  If an appliance *does* check the identity, this value succeeds.
- If an appliance checks the identity, `""` fails. There is no scenario in which `""` succeeds
  and `"HCCOM_Local_App"` fails — the hint value is weakly dominant.

Against an untested appliance, changing one thing at a time:

1. **Identity `""`.** Expected to work.
2. **On handshake failure, retry with identity `"HCCOM_Local_App"`** — before touching anything
   else.
3. Only then vary the cipher string, and only then the security level.

If step 2 is ever what makes a device connect, that appliance checks the identity after all.
