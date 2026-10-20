# The wire message model

Sources (see [`README.md`](./README.md) for URLs and commit SHAs):

- `reference/hcpy/HCDevice.py`, `reference/hcpy/HCSocket.py`
- `reference/homeconnect-websocket/src/homeconnect_websocket/message.py`, `.../session.py`, `.../helpers.py`
- `reference/go-homeconnect2mqtt/internal/homeconnect/message.go`, `.../session.go`

Everything below applies identically to both transports: the JSON layer sits above the AES
chain (AES mode) or above the TLS-PSK tunnel (TLS mode) and does not change between them.
The only transport-visible difference is the WebSocket frame type — binary for AES
(`reference/hcpy/HCSocket.py:184`), text for TLS
(`reference/hcpy/HCSocket.py:186`;
`reference/go-homeconnect2mqtt/internal/homeconnect/socket_tls_psk.go:326-328`).

---

## 1. Field set

```json
{"sID":<int>,"msgID":<int>,"resource":"<path>","version":<int>,"action":"<verb>","data":[…],"code":<int>}
```

| Field | Type | Notes |
|---|---|---|
| `sID` | int | session id, adopted from the appliance's `/ei/initialValues` |
| `msgID` | int | correlation id |
| `resource` | string | e.g. `/ci/services`, always leading-slash, `/<service>/<name>` |
| `version` | int | *service* version, not protocol version |
| `action` | string | `GET` \| `POST` \| `RESPONSE` \| `NOTIFY` |
| `data` | list of objects | **omitted entirely when absent** |
| `code` | int | **only in responses**; present ⇒ error |

Citations: the five always-present fields and their JSON names appear as the struct tags in
`reference/go-homeconnect2mqtt/internal/homeconnect/message.go:29-37`, as the dict literal in
`reference/homeconnect-websocket/src/homeconnect_websocket/message.py:64-70`, and as the dict
built in `reference/hcpy/HCDevice.py:347-353`.

### Field order on the wire

All three clients emit the five mandatory fields in the order
`sID, msgID, resource, version, action`, followed by `data` and then `code`:

- hcpy builds the dict in that order (`reference/hcpy/HCDevice.py:347-353`) and adds `data`
  afterwards (`reference/hcpy/HCDevice.py:371`); Python preserves dict insertion order in
  `json.dumps`.
- homeconnect-websocket: same order (`reference/homeconnect-websocket/src/homeconnect_websocket/message.py:64-78`).
- Go: same order, because `encoding/json` emits **struct** fields in declaration order
  (`reference/go-homeconnect2mqtt/internal/homeconnect/message.go:30-36`). *(inference — the
  declaration-order rule is Go stdlib behaviour, not something stated in the repo.)*

⚠️ **Object *inside* `data` is a different story in Go**, and this is the subject of the 403
analysis — see [`handshake.md`](./handshake.md) §"The `/ci/services` 403 divergence".

### Actions

`GET`, `POST`, `RESPONSE`, `NOTIFY`
(`reference/homeconnect-websocket/src/homeconnect_websocket/message.py:9-15`;
`reference/go-homeconnect2mqtt/internal/homeconnect/message.go:18-22`). The Go repo adds a
fifth, `DELETE`, used to clear the active program
(`reference/go-homeconnect2mqtt/internal/homeconnect/message.go:23-24`); neither Python
client defines it. *(inference)* An `Action` enum should carry `DELETE` as well — it costs nothing and the hood fan-off path needs it —
but nothing in the handshake uses it.

---

## 2. Serialisation rules on send

### 2.1 Compact separators

`json.dumps(msg, separators=(",", ":"))` — **no space after `,` or `:`**
(`reference/hcpy/HCSocket.py:179`;
`reference/homeconnect-websocket/src/homeconnect_websocket/message.py:79`). Go's
`json.Marshal` is compact by default and the repo pins it with a test that rejects any
`", "` or `": "` (`reference/go-homeconnect2mqtt/internal/homeconnect/message_test.go:11-29`).

### 2.2 `data` is always a list on send

If a caller supplies a bare object it is wrapped:

- hcpy: `if isinstance(data, list) is False: data = [data]`
  (`reference/hcpy/HCDevice.py:356-357`).
- homeconnect-websocket: same wrap in `dump()`
  (`reference/homeconnect-websocket/src/homeconnect_websocket/message.py:71-76`).
- Go: the field is typed `[]map[string]any`, so it is structurally impossible to send a bare
  object (`reference/go-homeconnect2mqtt/internal/homeconnect/message.go:35`).

### 2.3 `data` and `code` are omitted, not nulled

`data` is added to the outbound dict only when non-`None`
(`reference/hcpy/HCDevice.py:355-371`;
`reference/homeconnect-websocket/src/homeconnect_websocket/message.py:71-72`); Go uses
`omitempty` on both `data` and `code`
(`reference/go-homeconnect2mqtt/internal/homeconnect/message.go:35-36`), pinned by a test
that asserts neither key appears in a `GET /ci/services`
(`reference/go-homeconnect2mqtt/internal/homeconnect/message_test.go:31-41`).

*(inference)* In Rust this is `#[serde(skip_serializing_if = "Option::is_none")]` on `data`
and `code`, and **not** `Option<T>` serialised as `null`. Emitting `"data":null` would be a
wire divergence from all three references.

### 2.4 The apostrophe substitution (a quirk, probably a no-op)

Both Python clients run `re.sub("'", '"', buf)` over the serialised JSON before sending
(`reference/hcpy/HCSocket.py:181`;
`reference/homeconnect-websocket/src/homeconnect_websocket/message.py:80-81`), commented
"swap `'` for `"`". Go has no equivalent.

*(inference)* This looks like a defence against a caller having built the payload with
Python `str(dict)` (which uses single quotes) rather than `json.dumps`. Applied to genuine
`json.dumps` output it is at best a no-op and at worst **corrupting**: it rewrites any
apostrophe occurring inside a legitimate string value into a `"`, producing invalid JSON.
Do **not** replicate it. It is documented here only so that a future frame-level
diff against an hcpy capture does not mistake its absence for a divergence — and so that if
a captured hcpy frame ever contains a mangled apostrophe, the cause is known.

---

## 3. Defensive parsing on receive (mandatory)

### 3.1 Numeric coercion — `sID`, `msgID`, `version`

Appliances sometimes send these as **quoted strings**
(`reference/go-homeconnect2mqtt/internal/homeconnect/message.go:40-41`;
`reference/go-homeconnect2mqtt/docs/01-protocol.md:193`). All readers coerce:

- homeconnect-websocket casts unconditionally: `int(msg["sID"])`, `int(msg["msgID"])`,
  `int(msg["version"])`
  (`reference/homeconnect-websocket/src/homeconnect_websocket/message.py:22-25`).
- Go keeps them as `json.RawMessage` and runs `coerceInt`
  (`reference/go-homeconnect2mqtt/internal/homeconnect/message.go:42-49`, `:70-76`), which
  strips surrounding quotes and parses as **float64 then truncates**
  (`reference/go-homeconnect2mqtt/internal/homeconnect/message.go:116-129`) — so `"4.2"`
  becomes `4`. This is pinned by
  `reference/go-homeconnect2mqtt/internal/homeconnect/message_test.go:43-45`.
- hcpy does not coerce at all: it reads `msg["version"]` raw
  (`reference/hcpy/HCDevice.py:420`) and compares it to the int `1`
  (`reference/hcpy/HCDevice.py:439`). Its own test fixtures nevertheless contain
  **string, non-integer** versions such as `"4.2"` and `"3.1"`
  (`reference/hcpy/tests/conftest.py:63`, `:155`).

*(inference)* The float-tolerant coercion is the one to implement: it is a strict superset of
the others and it is the only one that survives the `"4.2"` shape that hcpy's own fixtures
show is real. Note the consequence for the handshake: with a version of `"4.2"`, hcpy's
`version == 1` test (`reference/hcpy/HCDevice.py:439`) is False for a *string* reason while
a numeric comparison is False for a *numeric* reason — same outcome, different path.

### 3.2 The `]"` → `]` malformed-object workaround

Some appliances emit an object field whose JSON has a stray quote after a closing bracket.
The fix is: on a decode failure, replace the two-byte sequence `]"` with `]` and re-parse
once. If that also fails, give up.

- homeconnect-websocket applies it to a *nested* object-valued field, in `load_object`
  (`reference/homeconnect-websocket/src/homeconnect_websocket/helpers.py:27-41`, the
  substitution at `:35`), which is wired into the `"Object"` entry of the entity type map
  (`reference/homeconnect-websocket/src/homeconnect_websocket/helpers.py:44-51`). It is
  **not** applied to whole frames.
- Go applies it in **two** places: to the whole frame, if the frame fails to parse
  (`reference/go-homeconnect2mqtt/internal/homeconnect/message.go:61-69`), and again to the
  `data` payload alone (`reference/go-homeconnect2mqtt/internal/homeconnect/message.go:100-104`).
- hcpy has no such workaround anywhere (grepping the whole checkout for `]"` returns
  nothing).

*(inference)* Follow Go and apply it at both levels — a whole-frame retry plus a
per-value retry when parsing an `Object`-typed feature value — since these are disjoint
failure modes and the Python placement covers only the second.

The replacement is a **blind global byte substitution**, not a parse-aware repair, in both
implementations that have it. It can therefore in principle corrupt a legitimate payload
containing `]"`. Both accept that risk, and it only runs on an already-failed parse.

### 3.3 `data` normalisation on receive

Go additionally accepts a **single object** where a list was expected and wraps it
(`reference/go-homeconnect2mqtt/internal/homeconnect/message.go:94-99`, pinned by
`reference/go-homeconnect2mqtt/internal/homeconnect/message_test.go:60`), and treats a
missing or `null` `data` as `None`
(`reference/go-homeconnect2mqtt/internal/homeconnect/message.go:88-90`). The Python clients
store `data` as whatever arrived (`msg.get("data", None)`,
`reference/homeconnect-websocket/src/homeconnect_websocket/message.py:27`).

### 3.4 Unknown `uid` is ignored, not fatal

An update item for a `uid` the client does not know about is logged at debug level and
skipped:
`reference/homeconnect-websocket/src/homeconnect_websocket/appliance.py:170-178` (the
`else` at `:177-178`). hcpy instead **creates** a feature entry from the change
(`reference/hcpy/HCDevice.py:484-487`) — a divergence, and hcpy's variant is the more
forgiving. The Go spec restates the ignore rule
(`reference/go-homeconnect2mqtt/docs/01-protocol.md:264`). Either way it is never fatal.

### 3.5 A decode failure drops the frame, not the connection

homeconnect-websocket catches `JSONDecodeError`/`KeyError` in the receive loop, logs, and
continues (`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:159-167`).
Go does the same (`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:226-230`)
and additionally wraps dispatch in a panic recover so one malformed appliance-controlled
payload drops that frame rather than the daemon
(`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:236-246`). hcpy catches
broadly around `handle_message` and returns `None`
(`reference/hcpy/HCDevice.py:317-321`).

Note this is **not** in tension with the crypto-layer rule in
[`aes-chain.md`](./aes-chain.md) §8: a *crypto* failure is fatal because the chain state is
then unknowable; a *JSON* failure after successful decryption leaves the chain intact and is
therefore safely skippable. *(inference)*

---

## 4. Errors: `code`

A `code` present on a RESPONSE means the request failed.

- homeconnect-websocket tests truthiness — `if response_message.code:`
  (`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:320`) — and raises
  `CodeResponsError(code, resource)` (`:328`).
- Go tests for a non-nil pointer (`reference/go-homeconnect2mqtt/internal/homeconnect/codes.go:83-88`)
  and raises `CodeResponseError`, whose message is literally
  `homeconnect: %s -> code %d (%s)`
  (`reference/go-homeconnect2mqtt/internal/homeconnect/codes.go:77-79`) — that format is the
  exact error users of that project report:
  `homeconnect: /ci/services -> code 403 (Forbidden)`.
- hcpy tests **key presence**, not value: `if "code" in msg`
  (`reference/hcpy/HCDevice.py:424`), and then merely records
  `{"error": code, "resource": …}` (`:425-428`) without raising — the handshake carries on.

⚠️ Three-way divergence on the *predicate*: presence (hcpy) vs truthiness (Python lib) vs
non-nil (Go). These differ on `"code":0` and on `"code":null`. *(inference)* Use the Go
rule, "`code` is non-null ⇒ error" (`Option<i32>::is_some()` in Rust), and specifically **not**
truthiness, which would silently treat `code: 0` as success.

The `code` → meaning table is in [`error-codes.md`](./error-codes.md).

---

## 5. Correlation, timeouts, and routing

### 5.1 msgID assignment

The client's msgID counter starts at the `edMsgID` the appliance supplies in
`/ei/initialValues` (see [`handshake.md`](./handshake.md) §2) and increments by one per
outbound message that needs a fresh id:

- hcpy: `self.tx_msg_id += 1` after each send (`reference/hcpy/HCDevice.py:379`).
- homeconnect-websocket: `message.msg_id = self._last_msg_id; self._last_msg_id += 1`
  (`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:489-492`).
- Go: identical (`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:529-532`).

A **RESPONSE reuses the msgID of the request it answers** and does not consume a new one:
hcpy's `reply()` copies `msg["msgID"]` (`reference/hcpy/HCDevice.py:328`);
homeconnect-websocket's `responde()` copies `msg_id`
(`reference/homeconnect-websocket/src/homeconnect_websocket/message.py:51-60`); Go sets
`MsgID: initMsg.MsgID` explicitly
(`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:354-358`) and its
`resolveDefaults` only assigns when the field is still zero
(`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:529`).

### 5.2 Default resolution before sending

Applied in order, each only if the field is unset
(`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:516-533`;
`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:478-492`;
restated as spec at `reference/go-homeconnect2mqtt/docs/01-protocol.md:240-246` §6.4):

1. **`version`** ← `service_versions[service_key(resource)]`, defaulting to `1`.
   `service_key` is the two characters after the leading `/`: `/ci/services` → `ci`
   (`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:564-570`;
   `reference/homeconnect-websocket/src/homeconnect_websocket/session.py:482-483` uses
   `resource[1:3]`).
2. **`sID`** ← the current session id.
3. **`msgID`** ← `last_msg_id`, then increment.

hcpy does the same lookup but only **after** `/ci/services` has returned, gated on an event
(`reference/hcpy/HCDevice.py:338-346`), and it *overrides* an explicitly-passed `version`
when the service is known (`:342-343`) — so hcpy's `get("/ci/authentication", version=2, …)`
at `reference/hcpy/HCDevice.py:400` does **not** actually send version 2 once the services
table is populated; it sends `services["ci"].version`. *(inference — read of the control
flow: `reconnect()` waits on `_services_event` at `:387` before reaching `:400`, so the
`is_set()` branch at `:338` is always taken by then.)*

### 5.3 Correlation and timeout

One single-slot queue per outstanding msgID; an inbound `RESPONSE` is matched by msgID and
delivered to it:

- homeconnect-websocket: `_response_queues[msg_id]`, `asyncio.Queue(maxsize=1)`
  (`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:307`, registration at
  `:313-314`, dispatch at `:342-350`).
- Go: `pending map[int]chan *Message` with a buffered channel of 1
  (`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:479-482`, take-and-delete at
  `:309-317`).

A response for an unknown msgID is logged and dropped, never fatal
(`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:347-350`).

**Timeout: 20 s.**
`DEFAULT_SEND_TIMEOUT: Final[int] = 20`
(`reference/homeconnect-websocket/src/homeconnect_websocket/const.py:6`), applied via
`asyncio.wait_for` (`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:318`);
Go defaults `SendTimeout` to `20 * time.Second`
(`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:75-77`) and applies it at
`:506-507`. Each pending msgID is a one-shot with this timeout.

Related timeouts, for completeness:

| Timeout | Value | Citation |
|---|---|---|
| whole handshake | 60 s | `reference/homeconnect-websocket/src/homeconnect_websocket/const.py:5`; `reference/go-homeconnect2mqtt/internal/homeconnect/session.go:78-80`, used at `:337` |
| hcpy's wait for `/ci/services` specifically | 10 s, then **close the connection** | `reference/hcpy/HCDevice.py:387-390` |
| max connect timeout / backoff factor | 60 s / ×1.2 | `reference/homeconnect-websocket/src/homeconnect_websocket/const.py:7-8` |

### 5.4 NOTIFY routing

Anything that is not a correlated RESPONSE goes to the general handler:

- Go: `dispatch` first tries the pending table for `RESPONSE`, then special-cases
  `POST /ei/initialValues` into the handshake channel, then falls through to the notify
  handler (`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:287-307`).
- homeconnect-websocket: `RESPONSE` → queue, everything else → the external message handler
  (`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:342-363`).

The messages that arrive as NOTIFY in steady state are `NOTIFY /ro/values` and
`NOTIFY /ro/descriptionChange`
(`reference/homeconnect-websocket/src/homeconnect_websocket/appliance.py:157-159`). Both are
handled by the same entity-update path as the bulk `/ro/allDescriptionChanges` and
`/ro/allMandatoryValues` responses
(`reference/homeconnect-websocket/src/homeconnect_websocket/appliance.py:160-165`).

Note a Go subtlety worth copying: a `RESPONSE` whose msgID has **no** pending entry falls
through to the notify handler rather than being dropped
(`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:288-292` — the `return` is
inside the `if ch != nil`). homeconnect-websocket drops it
(`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:347-350`).
*(inference)* This matters for `/ci/services`, which homeconnect-websocket deliberately feeds
to the general handler a second time after correlating it
(`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:410-412`) — the
same information reaches the handler either way.

### 5.5 Update-item shape

Each item inside a `/ro/values`, `/ro/descriptionChange`, `/ro/allMandatoryValues` or
`/ro/allDescriptionChanges` payload is
`{"uid":<int>, "value":…, "access":…, "available":…, "min":…, "max":…, "stepSize":…, "execution":…}`,
with everything except `uid` optional
(`reference/go-homeconnect2mqtt/docs/01-protocol.md:264`); hcpy consumes exactly this set of
keys when folding description changes into its feature table
(`reference/hcpy/HCDevice.py:464-477`).

`access` on the wire is upper case: the recorded sessions carry `"READ"` and `"NONE"`
(`fixtures/`), while the profile XML writes the same words in camelCase (`read`, `readWrite`). A
parser has to accept both spellings.

**Key order inside a write item is load-bearing.** hcpy deletes and re-inserts `uid` and
`value` to force `uid` first, with the comment "UID has to be the first attribute in the dict
because the devices require it that way"
(`reference/hcpy/HCDevice.py:303-306`). This is direct evidence that at least some appliance
firmware parses JSON objects **positionally**, and it is the linchpin of the 403 analysis in
[`handshake.md`](./handshake.md).
