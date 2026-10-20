# The handshake FSM

Sources (see [`README.md`](./README.md) for URLs and commit SHAs):

- `reference/hcpy/HCDevice.py` — hcpy, **the behavioural oracle** (it does *not* exhibit the
  `/ci/services` 403)
- `reference/homeconnect-websocket/src/homeconnect_websocket/session.py` and `.../appliance.py`
- `reference/go-homeconnect2mqtt/internal/homeconnect/session.go` — the implementation that
  **does** exhibit the 403

The three implementations agree on the *shape* of the handshake and disagree on several
conditions and on the ordering of the tail. Every disagreement is tabulated in §10, and the
403 analysis is §11. Where Kessel's `hc-proto` handshake makes a choice, it is stated.

---

## 1. Overview

```
        ── appliance initiates ──
[0]  ← POST /ei/initialValues                (device → client, always first)
[1]  → RESPONSE /ei/initialValues            (client → device)
[2]  → GET  /ci/services            v1       ⇒ populates the service-version table
[3]  → GET  /ci/authentication      {nonce}  (conditional — see §5)
[4]  → GET  /ci/info                         (conditional; failure tolerated)
[5]  → GET  /iz/info                         (conditional on service "iz")
[6]  → NOTIFY /ei/deviceReady                (conditional — see §7)
[7]  → GET  /ni/info                         (conditional on service "ni")
                                             ⇒ state := CONNECTED
[8]  → GET  /ro/allDescriptionChanges        (post-connect; tolerate 500)
[9]  → GET  /ro/allMandatoryValues           (post-connect; tolerate 500)
        ── steady state ──
     ← NOTIFY /ro/values, NOTIFY /ro/descriptionChange
```

The client **never** speaks first. Everything is driven off step [0].

---

## 2. Step [0] — `POST /ei/initialValues` (device-initiated)

The appliance sends this immediately after the WebSocket upgrade. Example frame
(`reference/go-homeconnect2mqtt/docs/01-protocol.md:210`):

```json
{"sID":<sid>,"msgID":<server_mid>,"resource":"/ei/initialValues","version":2,"action":"POST","data":[{"edMsgID":<client_start_mid>}]}
```

The client adopts two things from it:

| Adopted | From | Citations |
|---|---|---|
| session id | `msg.sID` | `reference/hcpy/HCDevice.py:433`; `reference/homeconnect-websocket/src/homeconnect_websocket/session.py:379`; `reference/go-homeconnect2mqtt/internal/homeconnect/session.go:344` |
| starting msgID | `msg.data[0].edMsgID` | `reference/hcpy/HCDevice.py:434`; `reference/homeconnect-websocket/src/homeconnect_websocket/session.py:380`; `reference/go-homeconnect2mqtt/internal/homeconnect/session.go:345` |

Both Python clients index `data[0]` directly and will raise if `data` is empty;
homeconnect-websocket catches that and converts it to a handshake error
(`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:387-390`). Go's
`firstInt` returns `0` for an empty `data`
(`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:572-577`) — i.e. it silently
starts the msgID counter at zero. *(inference)* A missing `edMsgID` is best treated as a hard
handshake error, as the Python clients effectively do; starting at `0` when the device expected
a specific base silently corrupts every subsequent correlation.

**The first frame must be `/ei/initialValues`.** homeconnect-websocket reads exactly one
message *before* starting the receive loop and fails the connection if it is anything else
(`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:372-385`). Go instead
starts the receive loop first and waits for the init message on a dedicated channel, with the
60 s handshake timeout (`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:294-300`,
`:330-341`). hcpy has no separate pre-handshake at all: `/ei/initialValues` is handled
inline by the normal message handler
(`reference/hcpy/HCDevice.py:429-445`).

---

## 3. Step [1] — `RESPONSE /ei/initialValues`

The client answers the appliance's POST, **reusing** its `sID`, `msgID`, `resource` and
`version`, with `action = "RESPONSE"` and a one-element `data` list:

```json
{"sID":<sid>,"msgID":<server_mid>,"resource":"/ei/initialValues","version":<v>,"action":"RESPONSE",
 "data":[{"deviceType":<2 | "Application">,"deviceName":"<app name>","deviceID":"<app id>"}]}
```

- hcpy's `reply()` echoes all four fields verbatim from the received message
  (`reference/hcpy/HCDevice.py:324-334`) with the payload built at
  `reference/hcpy/HCDevice.py:436-443`.
- homeconnect-websocket's `Message.responde()` copies the same four
  (`reference/homeconnect-websocket/src/homeconnect_websocket/message.py:51-60`), payload at
  `reference/homeconnect-websocket/src/homeconnect_websocket/session.py:396-404`.
- Go reconstructs the message field by field
  (`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:354-365`).

**`deviceType` rule** — identical in all three:

```
deviceType = 2 if <the /ei/initialValues message version> == 1 else "Application"
```

i.e. the **int** `2` on ei v1, the **string** `"Application"` otherwise.
`reference/hcpy/HCDevice.py:439`;
`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:399`;
`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:350-353`.

This response does **not** consume a new msgID; the counter still points at `edMsgID` when
step [2] is sent (see [`message-model.md`](./message-model.md) §5.1).

**No response is expected** to this RESPONSE. All three fire and continue.

---

## 4. Step [2] — `GET /ci/services`, version 1

Always sent, always with an explicit `version: 1` (the services table that would otherwise
supply the version is precisely what this call populates):

- hcpy: `self.get("/ci/services")` with the `version=1` default
  (`reference/hcpy/HCDevice.py:386`, signature at `:337`); the services-table override at
  `:338-346` cannot fire yet because `_services_event` is still clear.
- homeconnect-websocket: `Message(resource="/ci/services", version=1)`
  (`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:407`).
- Go: `&Message{Resource: "/ci/services", Version: 1, Action: ActionGet}`
  (`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:371`).

Response shape: `data: [{"service":"ci","version":x}, {"service":"iz","version":y}, …]`,
stored into a `service → version` map
(`reference/hcpy/HCDevice.py:520-525`;
`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:468-476`;
`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:535-549`, which defaults a
missing `version` to `1` at `:543-546`).

**This call is synchronous and blocking in all three.** hcpy waits on an event with a **10 s**
timeout and closes the connection if it expires
(`reference/hcpy/HCDevice.py:387-390`); the other two use the standard 20 s `send_sync`
correlation. A `code` on this response is where the 403 surfaces (§11).

hcpy runs steps [2]–[9] on a **separate thread** started from the message handler
(`reference/hcpy/HCDevice.py:445`), so its `/ci/services` request races the socket's read
loop rather than blocking it. The other two are sequential within an async task. *(inference)*
This is an internal-structure difference with no wire consequence: in every case the
RESPONSE to `/ei/initialValues` is written before the `/ci/services` GET.

---

## 5. Steps [3]–[4] — `/ci/authentication` and `/ci/info` (conditional)

⚠️ **The three implementations use two different conditions.**

| | Condition for sending `/ci/authentication` + `/ci/info` | Citation |
|---|---|---|
| hcpy | `"iz" not in services` | `reference/hcpy/HCDevice.py:395-401` |
| homeconnect-websocket | `services.get("ci", 1) < 3` | `reference/homeconnect-websocket/src/homeconnect_websocket/session.py:414` |
| Go | `services["ci"] < 3` | `reference/go-homeconnect2mqtt/internal/homeconnect/session.go:377-380` |
| Kessel (`hc-proto`) | `ci < 3` | — |

hcpy's own comment explains its rule: "iz-capable devices (ci:3) use `/iz/info` for device
identity. Non-iz devices (ci:2) use `/ci/authentication` + `/ci/info`"
(`reference/hcpy/HCDevice.py:392-394`), i.e. it treats *presence of `iz`* and *`ci >= 3`* as
the same population. Its tests pin the behaviour both ways:
`test_skips_ci_authentication` / `test_skips_ci_info` on an iz fixture
(`reference/hcpy/tests/test_HCDevice.py:80-88`) and `test_queries_ci_authentication` /
`test_queries_ci_info` on a non-iz fixture (`reference/hcpy/tests/test_HCDevice.py:117-121`).

*(inference)* The two rules coincide on every device either author has seen, but they are not
the same predicate: a device advertising `ci:2` **and** `iz` would get authentication from
hcws/Go and not from hcpy, and a device with `ci:3` and no `iz` would get it from hcpy and
not from the other two. The question is open: Kessel implements `ci < 3`, and the AES hob in
the golden corpus (`fixtures/aes/hob-session.json`) reports `ci:2` without `iz`, so both
predicates agree on it and it does not discriminate. Settling this needs a capture from a
device on which the two rules differ.

The **nonce**: `base64url(32 random bytes)` with `=` padding stripped.
`reference/hcpy/HCDevice.py:398-399`;
`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:416-417`; Go uses
`base64.RawURLEncoding` (already unpadded)
(`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:594-600`). Sent as
`data: [{"nonce": "<token>"}]`.

The **version** on `/ci/authentication`: hcpy passes `version=2` explicitly
(`reference/hcpy/HCDevice.py:400`) but that argument is overridden by the services-table
lookup, which is live by this point (`reference/hcpy/HCDevice.py:338-343`) — so the wire
value is `services["ci"]`. Go sends `Version: ciVersion` explicitly
(`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:386`); homeconnect-websocket
leaves it unset and lets `_set_message_info` resolve it the same way
(`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:480-483`).
All three therefore send `services["ci"]`. *(inference — for hcpy this is a read of the
control flow, not a literal.)*

The appliance replies with its own nonce, read back as `data[0]["response"]`
(`reference/hcpy/HCDevice.py:514-518`, with the honest comment "unsure if this is for us to
use or to authenticate the server. Doesn't appear to be needed").

**`/ci/info` failure is tolerated** in all three: hcpy never raises on a `code` at all
(`reference/hcpy/HCDevice.py:424-428`); homeconnect-websocket wraps it in
`contextlib.suppress(CodeResponsError)`
(`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:424-429`); Go logs at
debug and continues (`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:391-394`).
`/ci/authentication` failure, by contrast, is **fatal** in hcws and Go
(`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:385-389` returns the error).

---

## 6. Step [5] — `GET /iz/info` (conditional on service `iz`)

Sent iff `iz` appears in the services table:
`reference/hcpy/HCDevice.py:395-396`;
`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:431-436`;
`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:397-402`.

Its `data[0]` is device identity — serial number, software versions, MAC
(`reference/hcpy/HCDevice.py:450-453`), merged into the device info map
(`reference/homeconnect-websocket/src/homeconnect_websocket/appliance.py:166-168`).

⚠️ **This response carries device identifiers** (`serialNumber`, `mac`, `haId`). Kessel keeps
them behind `Secret<T>` and redacts them in the trace bus and the frame inspector.

Failure is tolerated in Go (`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:399-401`,
debug-log and continue) but **fatal** in homeconnect-websocket, which does not suppress it
(`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:431-436`).

---

## 7. Step [6] — `NOTIFY /ei/deviceReady` (conditional — three-way divergence)

| | Condition | Citation |
|---|---|---|
| hcpy | **unconditional** | `reference/hcpy/HCDevice.py:404` — no `if` guards it |
| homeconnect-websocket | `services["ei"] == 2` (the **services-table** version) | `reference/homeconnect-websocket/src/homeconnect_websocket/session.py:438-443` |
| Go | `eiVersion == 2` (the **`/ei/initialValues` message** version) | `reference/go-homeconnect2mqtt/internal/homeconnect/session.go:404-409` |
| Kessel (`hc-proto`) | **unconditional** (follows hcpy) | — |

Three different predicates. Note that hcws and Go read the ei version from **different
places** — the services table versus the init message — which coincide only if the appliance
reports them consistently. *(inference)*

hcpy's unconditional send carries an explicit comment that is directly relevant to this
project:

> "We need to send deviceReady for some devices or `/ni/` will come back as **403 unauth**"
> (`reference/hcpy/HCDevice.py:403`)

So a **403 caused by a missing `/ei/deviceReady` is a documented, real phenomenon on this
protocol** — just, per that comment, on `/ni/`, and `/ei/deviceReady` is sent *after*
`/ci/services` in every implementation. It therefore cannot explain a `/ci/services` 403
(§11), but it is strong evidence that these appliances gate resources on handshake
completeness and answer with 403 when unsatisfied. hcpy pins the unconditional send with
`test_sends_device_ready` (`reference/hcpy/tests/test_HCDevice.py:96-99`).

Fire-and-forget: no response is awaited
(`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:443` uses `send`, not
`send_sync`; Go likewise at `reference/go-homeconnect2mqtt/internal/homeconnect/session.go:406`).

Kessel follows **hcpy** and sends it unconditionally: the comment records a concrete failure
the conditional versions would reintroduce, and an unnecessary fire-and-forget NOTIFY is cheap.
The golden corpus is ei v2, so whether an ei v1 device objects to the NOTIFY is untested.

---

## 8. Step [7] — `GET /ni/info` (conditional on service `ni`)

Sent iff `ni` is in the services table:
`reference/hcpy/HCDevice.py:406-407`;
`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:445-447`;
`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:411-416`. Returns network
information / IP address (`reference/hcpy/HCDevice.py:489-492`).

After this, state := **CONNECTED**
(`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:449-450`).

---

## 9. Steps [8]–[9] — post-connect bulk init

Two bulk GETs, issued once the session is CONNECTED:

| | Order | Citation |
|---|---|---|
| hcpy | `/ro/allMandatoryValues` **then** `/ro/allDescriptionChanges` | `reference/hcpy/HCDevice.py:409-410` |
| homeconnect-websocket | `/ro/allDescriptionChanges` **then** `/ro/allMandatoryValues` | `reference/homeconnect-websocket/src/homeconnect_websocket/appliance.py:269-279` |
| Go | `/ro/allDescriptionChanges` **then** `/ro/allMandatoryValues` | `reference/go-homeconnect2mqtt/internal/homeconnect/session.go:424-443` |
| Kessel (`hc-proto`) | `/ro/allMandatoryValues` **then** `/ro/allDescriptionChanges` | — |

⚠️ hcpy sends them in the **opposite order** to the other two. Descriptions-first looks more
logical (the description supplies the metadata the values are interpreted against, which is
why hcws feeds the description response through `_update_entities` before the values one),
but the golden capture of a real appliance session shows hcpy's order, mandatory values first,
and Kessel reproduces it. Neither GET gates CONNECTED.

**Both tolerate `500`.** Go's `tolerableInit` swallows any code `>= 500` and returns `nil` for
that half of the pair, keeping the connection up
(`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:424-451`; the `>= 500`
predicate is at `:448-451`). homeconnect-websocket wraps the whole `_init` in a bare
`except Exception` with a logged traceback
(`reference/homeconnect-websocket/src/homeconnect_websocket/appliance.py:266-281`). hcpy
cannot fail here because it never raises on a `code`
(`reference/hcpy/HCDevice.py:424-428`).

Go tolerates the whole `5xx` range, not just `500`, and Kessel does the same: the `5xx` block
of the code table (`524`–`530` etc., see [`error-codes.md`](./error-codes.md)) contains codes
that are equally benign here.

---

## 10. Divergence summary (hcpy vs homeconnect-websocket vs Go)

| # | Point | hcpy (oracle) | homeconnect-websocket | Go | Kessel |
|---|---|---|---|---|---|
| D1 | `deviceType` on ei v1 / else | `2` / `"Application"` | same | same | same — **no divergence** |
| D2 | `deviceName` | `"hcpy"` | caller-supplied | `"go-homeconnect2mqtt"` default | configured constant |
| D3 | `deviceID` | `"0badcafe"` (fixed) | caller-supplied | random 16 upper-hex **per connect** | configured constant, never per connect |
| D4 | key order inside the response `data[0]` | `deviceType, deviceName, deviceID` | same | **`deviceID, deviceName, deviceType`** | hcpy's order |
| D5 | `/ci/authentication` gate | `"iz" not in services` | `ci < 3` | `ci < 3` | `ci < 3` |
| D6 | `/ei/deviceReady` gate | **unconditional** | `services["ei"] == 2` | init-message `version == 2` | unconditional |
| D7 | post-connect order | mandatory, then descriptions | descriptions, then mandatory | descriptions, then mandatory | mandatory, then descriptions |
| D8 | `code` predicate | key present | truthy | non-nil | non-null |
| D9 | `code` on handshake steps | never raises | raises (except `/ci/info`) | raises (except `/ci/info`, `/iz/info`, `/ni/info`) | typed error |
| D10 | echoed `version` in the RESPONSE | **verbatim**, including a string | `int(...)`-coerced | `coerceInt`, float-truncating | verbatim, type-preserving |
| D11 | `/ci/services` wait | 10 s, then close | 20 s `send_sync` | 20 s `send_sync` | — |

Citations for each row are in the section that discusses it: D1 §3, D2/D3/D4 §11, D5 §5,
D6 §7, D7 §9, D8/D9 [`message-model.md`](./message-model.md) §4, D10 §3 and
[`message-model.md`](./message-model.md) §3.1, D11 §4.

---

## 11. The /ci/services 403 divergence

go-homeconnect2mqtt fails against at least one AES appliance with
`homeconnect: /ci/services -> code 403 (Forbidden)`; hcpy and homeconnect-websocket do not.
The error string is Go's
(`reference/go-homeconnect2mqtt/internal/homeconnect/codes.go:77-79` produces exactly that
format; `403: "Forbidden"` at `:14`), raised from `sendSync`'s `codeError` check
(`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:502-505`) on the
`/ci/services` call at `reference/go-homeconnect2mqtt/internal/homeconnect/session.go:371-374`.

Because `/ci/services` is step [2], the only client behaviour that can influence it is:

1. the TCP/WebSocket upgrade,
2. the AES chain (if AES mode),
3. the `RESPONSE /ei/initialValues` at step [1], and
4. the bytes of the `GET /ci/services` request itself.

(2) is eliminated by construction: a **decrypted, well-formed 403** proves both chains are
in sync, and [`aes-chain.md`](./aes-chain.md) confirms all three crypto implementations are
byte-identical. (1) is eliminated: no client sends any HTTP header, subprotocol or Origin
(`reference/go-homeconnect2mqtt/docs/01-protocol.md:28-30`), and Go dials with an empty
`DialOptions{}` (`reference/go-homeconnect2mqtt/internal/homeconnect/socket.go:101`). (4) is
byte-identical across all three (§4) modulo `sID`/`msgID`, which are echoed from the device.

**That leaves (3): the `/ei/initialValues` RESPONSE.**

### 11.1 Exact fields each implementation sends

**hcpy** (`reference/hcpy/HCDevice.py:436-443`, values from `:63-64`):

```json
{"sID":<echoed>,"msgID":<echoed>,"resource":"/ei/initialValues","version":<echoed verbatim>,
 "action":"RESPONSE","data":[{"deviceType":2,"deviceName":"hcpy","deviceID":"0badcafe"}]}
```

`deviceName = "hcpy"` and `deviceID = "0badcafe"` are **hardcoded constants**
(`reference/hcpy/HCDevice.py:63-64`) — they are the same on every connection, on every
device, for every hcpy user in the world. `sID`/`msgID`/`resource`/`version` are echoed
verbatim from the incoming message by `reply()`
(`reference/hcpy/HCDevice.py:326-333`), so `version` keeps whatever JSON type the device sent
(no coercion — see [`message-model.md`](./message-model.md) §3.1).

**homeconnect-websocket**
(`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:396-404`):

```json
… "data":[{"deviceType":2,"deviceName":"<app_name>","deviceID":"<app_id>"}]
```

`app_name`/`app_id` are constructor arguments with **no defaults**
(`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:184-186`), so they are
whatever the embedding application passes and are stable for as long as that application
keeps passing the same values. `version` is `int()`-coerced at load
(`reference/homeconnect-websocket/src/homeconnect_websocket/message.py:25`) and then copied by
`responde()` (`reference/homeconnect-websocket/src/homeconnect_websocket/message.py:51-60`).

**Go** (`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:354-365`):

```json
… "data":[{"deviceID":"3F1A…","deviceName":"go-homeconnect2mqtt","deviceType":2}]
```

- `deviceName` ← `cfg.AppName`, defaulted to the literal `"go-homeconnect2mqtt"`
  (`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:81-83`,
  `reference/go-homeconnect2mqtt/internal/config/defaults.go:15` and `:49-51`).
- `deviceID` ← `s.appID()`
  (`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:551-560`): if `cfg.AppID`
  is empty it calls `randomHexID()`, which returns **16 uppercase hex characters from 8
  random bytes** (`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:602-614`).
  `appID()` is called *inside* `handshake()`, so with `APP_ID` unset **the deviceID is
  different on every single connection and every reconnect**. There is no default for `AppID`
  in `reference/go-homeconnect2mqtt/internal/config/defaults.go:40-59` (only `AppName` is
  defaulted, at `:49-51`), and `APP_ID` is not exposed in the Home Assistant add-on options
  (`reference/go-homeconnect2mqtt/addon/config.yaml:45-48` lists `profile_zip`, `devices`,
  `mqtt_server`, … and no `APP_ID`).
- **Key order is alphabetical**, not source order. The payload is a Go
  `map[string]any` literal (`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:360-364`)
  inside a `[]map[string]any` field
  (`reference/go-homeconnect2mqtt/internal/homeconnect/message.go:35`) encoded with
  `json.Marshal` (`reference/go-homeconnect2mqtt/internal/homeconnect/message.go:53-55`).
  `encoding/json` sorts map keys; `"deviceID"` < `"deviceName"` < `"deviceType"` because
  `'I'`(0x49) < `'N'`(0x4E) < `'T'`(0x54). *(inference — the sorting is documented Go stdlib
  behaviour; it is derived from the source shape, not demonstrated by running Go. The
  repository's own tests never assert the encoded byte string; `reference/go-homeconnect2mqtt/internal/homeconnect/message_test.go:11-41`
  only checks compactness and key absence.)*

### 11.2 Side-by-side field table

| Field | hcpy (no 403) | homeconnect-websocket (no 403) | Go (**403**) |
|---|---|---|---|
| JSON key order in `data[0]` | `deviceType`, `deviceName`, `deviceID` | `deviceType`, `deviceName`, `deviceID` | **`deviceID`, `deviceName`, `deviceType`** — *derived, not executed* (†) |
| `deviceType` type/value (ei v1) | int `2` | int `2` | int `2` |
| `deviceType` type/value (ei ≠ 1) | `"Application"` | `"Application"` | `"Application"` |
| `deviceName` | `"hcpy"` (4 chars, fixed) | caller-supplied | `"go-homeconnect2mqtt"` (19 chars) |
| `deviceID` | `"0badcafe"` (8 chars, lowercase hex, **fixed forever**) | caller-supplied, stable | 16 chars, **uppercase** hex, **fresh random on every connect** |
| `sID` | echoed | echoed | echoed |
| `msgID` | echoed (server's) | echoed | echoed |
| `resource` | echoed | echoed | reconstructed, same literal |
| `version` | echoed **verbatim, type-preserving** | `int()`-coerced | `coerceInt`, float-truncating |
| `action` | `"RESPONSE"` | `"RESPONSE"` | `"RESPONSE"` |
| extra fields | none | none | none |

Citations: hcpy `reference/hcpy/HCDevice.py:63-64` and `:324-334`, `:436-443`;
homeconnect-websocket `reference/homeconnect-websocket/src/homeconnect_websocket/session.py:184-186`,
`:396-404` and `.../message.py:25`, `:51-60`;
Go `reference/go-homeconnect2mqtt/internal/homeconnect/session.go:354-365`, `:551-560`,
`:602-614`, `reference/go-homeconnect2mqtt/internal/config/defaults.go:49-51`.

(†) Every other row in the Go column is read directly off the source. The key-order row is the
one exception: it is **derived** from `encoding/json`'s documented map-key sorting applied to
the `map[string]any` literal at
`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:360-364`, and has **not been
executed**;
`reference/go-homeconnect2mqtt/internal/homeconnect/message_test.go:11-41` asserts compactness
and key absence but never an encoded byte string. See §11.1.

### 11.3 Conclusion

**The obvious suspect — `deviceType` being int `2` versus the string `"Application"` — is
REFUTED by the source.** All three implementations compute `deviceType` with the identical
expression against the identical input (§3, D1). There is no version of the Go code that sends
the wrong one. Tests pinning `deviceType` per ei version are still worth having as regression
guards, but they cannot reproduce the bug.

**No root cause is confirmed by the source.** The Go repository contains no acknowledgement
of the `/ci/services` 403 anywhere — every `403` in the repository falls into one of three
categories, none of which is an acknowledgement of this bug:

1. an unrelated **HTTP** status for `not_writable` on the repo's own web API
   (`reference/go-homeconnect2mqtt/internal/web/web_test.go:108`;
   `reference/go-homeconnect2mqtt/docs/08-web-api.md:102`, `:181`, `:242`);
2. the **code-table entry** `403: "Forbidden"`
   (`reference/go-homeconnect2mqtt/internal/homeconnect/codes.go:14`;
   `reference/go-homeconnect2mqtt/docs/01-protocol.md:337`);
3. references to *upstream issue number* **#403** — a `local_hass` 404/reconnect-loop issue,
   nothing to do with the code 403 (`reference/go-homeconnect2mqtt/docs/05-resilience.md:23`,
   `:155`; `reference/go-homeconnect2mqtt/docs/00-research.md:91`, `:142`, `:160`, `:180`;
   `reference/go-homeconnect2mqtt/docs/09-implementation-plan.md:140`;
   `reference/go-homeconnect2mqtt/docs/01-protocol.md:360`).

The citations above are the complete set of matches for `403` in the repository as of the
recorded SHA. The bug is a field observation, and
the divergence that produces it is not annotated anywhere. What follows is therefore a
**ranked list of candidates**, each labelled with what is confirmed and what is hypothesis.

---

#### Candidate 1 — JSON key order inside `data[0]` (**strongest**)

- **Divergence: CONFIRMED BY SOURCE, plus documented stdlib behaviour — NOT EXECUTED.** The
  Python side is confirmed by source outright: both clients emit `deviceType, deviceName,
  deviceID` in that literal order. The Go side is **derived, not observed**: the payload is a
  `map[string]any` handed to `json.Marshal`, and the claim that it comes out as
  `deviceID, deviceName, deviceType` rests on `encoding/json`'s *documented* map-key sorting
  rather than on an encoded byte string anyone produced, and the repository's own tests never
  assert the encoded bytes
  (`reference/go-homeconnect2mqtt/internal/homeconnect/message_test.go:11-41`). See §11.1 for
  the full disclosure. Subject to that qualifier it is the *only* byte-level difference that
  separates Go from **both** non-403 implementations while being independent of configuration.
  A one-line `json.Marshal` check by anyone with a Go toolchain closes this gap permanently.
- **Causation: HYPOTHESIS.**
- **Corroboration, and it is unusually strong:** hcpy contains an explicit, load-bearing
  workaround for appliance firmware that parses JSON objects **positionally**:

  > `# UID has to be the first attribute in the dict because the devices require it that way`
  > (`reference/hcpy/HCDevice.py:303`)

  followed by code that deletes and re-inserts keys purely to control their emitted order
  (`reference/hcpy/HCDevice.py:304-306`). An appliance JSON parser that requires `uid` first
  in a `/ro/values` write item is very plausibly the same parser that requires `deviceType`
  first in an `/ei/initialValues` response. *(inference)*
- **Fits the symptom:** a malformed-to-the-device handshake response would leave the session
  unauthenticated, and the appliance's way of saying "you are not authenticated" is documented
  in this very protocol as **403** — see the `/ei/deviceReady` comment quoted in §7,
  "`/ni/` will come back as 403 unauth" (`reference/hcpy/HCDevice.py:403`).
- **Fits the reported population:** the 403 occurs against at least one AES appliance, not
  universally — consistent with a firmware-specific strict/positional parser rather than a
  protocol-wide rule.
- **Cheap to test.** See §11.4.

#### Candidate 2 — `deviceID` churn and format

- **Divergence: CONFIRMED BY SOURCE.** hcpy: the fixed literal `"0badcafe"` forever
  (`reference/hcpy/HCDevice.py:64`). Go with `APP_ID` unset (the default, and not settable
  from the add-on): a fresh random 16-uppercase-hex string on every connect
  (`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:551-560`, `:602-614`).
- **Causation: HYPOTHESIS.**
- **Corroboration:** the protocol has a whole client-registration surface —
  `/ci/registeredDevices`, `/ci/pairableDevices`, `/ci/register`, `/ci/deregister`,
  `/ci/delregistration` (`reference/hcpy/HCDevice.py:18-29`) — and hcpy notes that
  `/ci/registeredDevices` "contains details of Phone/HCPY registered as clients to the device"
  (`reference/hcpy/HCDevice.py:507-509`). A device that tracks registered clients by
  `deviceID` and has a bounded table would, after a handful of Go reconnects, be facing an
  unbounded stream of never-seen client identities. Refusing further service with **403
  Forbidden** is exactly the shape of that failure. Observed on hardware: the appliance does
  keep a registered-client table keyed by `deviceID`; that this causes the 403 is
  *(inference)*.
- **Also fits "sometimes":** the failure would be *latent* — the first Go connection to a
  fresh device might well succeed, and the 403 appear only after enough reconnects to fill or
  trip the table. A bug that reproduces reliably is compatible with a device that has
  already been poisoned.
- **Counter-evidence:** nothing in the appliance profile carries a client identity; the
  app-side `deviceID` is arbitrary. But *arbitrary* and *stable* are different properties:
  hcpy proves the value may be arbitrary, not that it may **change between connections**.
- Secondary sub-candidate: **format**. 8 lowercase hex vs 16 uppercase hex. A firmware field
  with an 8-character bound would truncate or reject the longer value. Pure speculation with
  no supporting citation. *(inference)*

#### Candidate 3 — `deviceName` length

- **Divergence: CONFIRMED BY SOURCE.** `"hcpy"` (4 chars,
  `reference/hcpy/HCDevice.py:63`) versus `"go-homeconnect2mqtt"` (19 chars,
  `reference/go-homeconnect2mqtt/internal/config/defaults.go:15`).
- **Causation: HYPOTHESIS, weak.** A fixed-size name buffer in firmware would explain a
  device-specific rejection, and error code `413 RequestEntityTooLong` exists in the table
  (`reference/go-homeconnect2mqtt/internal/homeconnect/codes.go:17`) — but the device returns
  `403`, not `413`, which argues against a length problem. No citation supports a name-length
  limit. Ranked third for that reason.

#### Candidate 4 — the echoed `version` field's JSON type

- **Divergence: CONFIRMED BY SOURCE.** hcpy echoes `msg["version"]` **verbatim**
  (`reference/hcpy/HCDevice.py:330`), preserving a string if the device sent a string; Go and
  hcws coerce to int (`reference/go-homeconnect2mqtt/internal/homeconnect/message.go:74`;
  `reference/homeconnect-websocket/src/homeconnect_websocket/message.py:25`). hcpy's own
  fixtures contain string, non-integral versions like `"4.2"` and `"3.1"`
  (`reference/hcpy/tests/conftest.py:63`, `:155`), so string versions are real on this
  protocol.
- **Causation: HYPOTHESIS, weak.** If an appliance sent `"version":"2"` on
  `/ei/initialValues`, hcpy would echo `"2"` and Go would send `2`. A strict device could
  reject the type change.
- **Ranked fourth** because it does not separate Go from homeconnect-websocket — hcws coerces
  too, and hcws is not reported to 403. It is ruled out for the AES hob in the golden corpus,
  which sends the JSON integer `2`; it stays possible for a device that sends a string
  version.

#### Explicitly refuted

| Suspect | Status |
|---|---|
| `deviceType` int `2` vs `"Application"` | **REFUTED** — identical expression in all three (§3). |
| AES crypto (KDF, padding, chains, direction bytes, truncation) | **REFUTED** — byte-identical in all three ([`aes-chain.md`](./aes-chain.md)); and a decrypted 403 self-evidently proves the chains are in sync. |
| `msgID` / `sID` on `/ci/services` | **REFUTED** — all three send `sID = <echoed>`, `msgID = edMsgID`, and none consumes an extra id for the RESPONSE (§3, [`message-model.md`](./message-model.md) §5.1). |
| `version` on `GET /ci/services` | **REFUTED** — explicitly `1` in all three (§4). |
| Step ordering before `/ci/services` | **REFUTED** — RESPONSE then GET, in all three (§3, §4). |
| WebSocket-level differences (headers, subprotocol, Origin, path) | **REFUTED** — none sends any; path is always `/homeconnect` (`reference/go-homeconnect2mqtt/docs/01-protocol.md:26-30`; `reference/hcpy/HCSocket.py:69`; `reference/go-homeconnect2mqtt/internal/homeconnect/socket.go:26`, `:101`). |
| Missing `/ei/deviceReady` | **REFUTED as a cause of *this* 403** — it is sent at step [6], after `/ci/services`. It remains the documented cause of a `/ni/` 403 (§7) and the divergence at D6 is real, just not this bug. |

### 11.4 What a regression test must assert

A replay test cannot reproduce the bug — a fake device never 403s — so it guards Kessel's
*emitted shape* instead. Driving the `hc-proto` handshake against the golden AES corpus
(`fixtures/aes/hob-session.json`), it asserts:

1. **No 403.** The handshake reaches `CONNECTED` and no response consumed during it carries
   `code == 403`. Assert on the code value, not merely on success.

2. **An order-sensitive comparison of the `/ei/initialValues` RESPONSE against the captured
   frame:**
   - the **serialised key order** of `data[0]` matches, `deviceType` first — compare the bytes
     or the ordered key sequence, **not** a `serde_json::Value` equality, which is order-blind
     and would pass on the exact divergence at D4. This is the most important assertion.
   - the top-level key order matches (`sID, msgID, resource, version, action, data`).
   - `deviceType` matches the captured value **and its JSON type** (int `2` versus string
     `"Application"`, per the ei version in the init frame).
   - `deviceName` and `deviceID` match the configured constants byte for byte, including case
     and length (the scrubbed corpus carries synthetic values).
   - `sID` and `msgID` equal the init frame's — the RESPONSE reuses the server's msgID and
     allocates none.
   - the `version` field's **JSON type** matches the init frame's.
   - `data` is a list of exactly one object with no extra keys, and there is no `code` key.

3. **`deviceID` stability across reconnects.** Two replays emit the identical `deviceID`.
   `deviceID` and `deviceName` are configuration, defaulted to fixed constants, never
   generated per connection; this rules out Candidate 2 for Kessel.

4. **The `/ci/services` request itself:** `version == 1` (integer), `action == "GET"`, no
   `data` key, no `code` key, `msgID == edMsgID` from the init frame.

A live test against a real appliance should assert the absence of a 403 code specifically and
log the observed `/ci/services` response code either way.

**The decisive experiment has not been run.** Run hcpy against an affected appliance
(succeeds), then the same handshake with `data[0]`'s keys reordered to Go's alphabetical order
and nothing else changed. If that 403s, Candidate 1 is proven; if not, Candidate 1 is dead and
Candidate 2 becomes the target.

---

## 12. Appliance URL construction (including IPv6)

This is the step *before* [0]: turning a configured host into the WebSocket URL. It is trivial
for IPv4 and DNS names and is a divergence for IPv6, so it is documented here rather than left
to the implementer.

The URL shape is fixed and identical in all three references — path `/homeconnect`, port and
scheme chosen by transport, never by the URL:

| Transport | URL |
|---|---|
| AES (app-layer crypto) | `ws://<host>:80/homeconnect` |
| TLS-PSK | `wss://<host>:443/homeconnect` |

`reference/hcpy/HCSocket.py:69` and `:73`;
`reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:176` (AES) and `:116`
(TLS); `reference/go-homeconnect2mqtt/internal/homeconnect/socket.go:51-54`, `:65`.

### 12.1 The IPv6 rules (take Go's)

Go is the only reference that gets IPv6 right. `applianceURL` documents itself as "fixing the
upstream IPv6 double-bracket bug (#409): existing brackets are stripped before wrapping, and a
link-local zone id is percent-encoded per RFC 6874"
(`reference/go-homeconnect2mqtt/internal/homeconnect/socket.go:43-45`). The algorithm, in
order (`reference/go-homeconnect2mqtt/internal/homeconnect/socket.go:46-65`):

1. Trim surrounding whitespace; an empty host is an error (`:47-50`).
2. Choose scheme/port from the transport (`:51-54`).
3. **Unwrap first:** if the host both starts with `[` and ends with `]`, strip that one pair
   (`:56-58`).
4. If the *unwrapped* host still contains a `:`, it is an IPv6 literal (`:59`):
   a. if it contains a `%`, replace that single `%` with `%25` — the RFC 6874 percent-encoding
      of the zone-id delimiter (`:60-62`);
   b. wrap the result in exactly one pair of brackets (`:63`).
5. Format `<scheme>://<host>:<port>/homeconnect` (`:65`).

Strip-then-wrap is what makes the operation **idempotent**: an already-bracketed host is not
re-wrapped. A host with no `:` is never bracketed, so IPv4 and DNS names are untouched.

The Go repo pins this with a table-driven test, `TestApplianceURL`
(`reference/go-homeconnect2mqtt/internal/homeconnect/crypto_test.go:207-231`; cases at
`:213-217`, assertions at `:220-227`, empty-host rejection at `:228-230`). Its vectors are
the cases any implementation should pin:

| input host | expected URL (AES) | pins |
|---|---|---|
| `2a0a::1` | `ws://[2a0a::1]:80/homeconnect` | `ipv6_host_bracketed_once` |
| `[2a0a::1]` | `ws://[2a0a::1]:80/homeconnect` | `ipv6_already_bracketed_not_rewrapped` (the source comment on this row is literally "no double bracket (#409)") |
| `fe80::1%eth0` | `ws://[fe80::1%25eth0]:80/homeconnect` | `ipv6_zone_id_preserved` |
| `192.168.1.50` | `ws://192.168.1.50:80/homeconnect` | IPv4 unchanged |
| `myhost` (secure) | `wss://myhost:443/homeconnect` | DNS name unchanged, TLS port |

Note the zone id is **preserved, percent-encoded** — `%eth0` becomes `%25eth0`, it is not
dropped and not left bare. A bare `%` in the host component of a URL is invalid, which is why
the encoding is required rather than cosmetic.

### 12.2 Divergence

| Behaviour | hcpy | homeconnect-websocket | Go |
|---|---|---|---|
| Brackets a bare IPv6 literal | **no** | yes | yes |
| Strips existing brackets before wrapping | n/a | **no** | yes |
| Percent-encodes a zone id | **no** | **no** | yes |

- **hcpy does nothing at all.** It interpolates the host straight into the f-string
  (`reference/hcpy/HCSocket.py:69`, `:73`), so a bare `2a0a::1` produces
  `ws://2a0a::1:80/homeconnect`, which is not a valid URL. hcpy's callers must therefore pass
  an already-bracketed host; nothing in hcpy enforces or documents that. *(inference — read of
  the f-strings; hcpy has no other host handling, its only host manipulation being the
  domain-suffix append at `reference/hcpy/HCSocket.py:53-56`.)*
- **homeconnect-websocket wraps unconditionally on the presence of a `:`**
  (`reference/homeconnect-websocket/src/homeconnect_websocket/hc_socket.py:41-44`), with no
  unwrap step — so an already-bracketed `[2a0a::1]` becomes `[[2a0a::1]]`. That is exactly the
  upstream double-bracket bug Go's comment names (#409). It also never touches a zone id.

**Kessel takes Go's algorithm** (`hc-proto`'s `url` module): it is a strict superset of the
other two and the only one that handles all the cases above. *(inference — the three references
disagree and only one is correct; there is no wire-observable reason to prefer the buggy
forms.)*
