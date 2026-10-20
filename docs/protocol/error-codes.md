# Response / error codes

Sources (see [`README.md`](./README.md) for URLs and commit SHAs):

- `reference/homeconnect-websocket/src/homeconnect_websocket/const.py:248-297` — the
  `ERROR_CODES` dict; the most complete primary source
- `reference/go-homeconnect2mqtt/internal/homeconnect/codes.go:10-59` — `responseCodes`, the
  Go port of the above
- `reference/go-homeconnect2mqtt/docs/01-protocol.md:325-360` — the same table in prose, plus
  a note on which codes matter in practice
- **hcpy has no code table at all.** It never maps a code to a name and never raises on one;
  it records `{"error": <code>, "resource": …}` and continues
  (`reference/hcpy/HCDevice.py:424-428`).

Semantics: a `code` in a RESPONSE means the request failed. See
[`message-model.md`](./message-model.md) §4 for the three-way divergence on the *predicate*
(key-present / truthy / non-null) and for why non-null is the right one.

---

## 1. The table

Where the two references disagree on wording, the Go spelling is given first and the
homeconnect-websocket spelling in parentheses. The two differ only cosmetically (Go removes
the internal spaces) except where a ⚠️ appears.

### HTTP-derived range

| Code | Name | hcws | Go |
|---|---|---|---|
| 200 | OK | ✓ `const.py:249` | ✓ `codes.go:11` |
| 202 | Accepted | ✓ `const.py:250` | ✓ `codes.go:12` |
| 400 | BadRequest | ✓ `const.py:251` | ✓ `codes.go:13` |
| **403** | **Forbidden** | ✓ `const.py:252` | ✓ `codes.go:14` |
| 404 | NotFound | ✓ `const.py:253` | ✓ `codes.go:15` |
| 405 | MethodNotAllowed | ✓ `const.py:254` | ✓ `codes.go:16` |
| 412 | ⚠️ see §2.1 | ✓ (mislabelled) `const.py:256` | **absent** |
| 413 | RequestEntityTooLong | ✓ `const.py:255` | ✓ `codes.go:17` |
| 414 | RequestUriTooLong | ⚠️ under key 412 | ✓ `codes.go:18` |
| 429 | TooManyRequests | ✓ `const.py:257` | ✓ `codes.go:19` |
| 500 | InternalServerError | ✓ `const.py:258` | ✓ `codes.go:20` |
| 501 | NotImplemented | ✓ `const.py:259` | ✓ `codes.go:21` |
| 502 | BadGateway | ✓ `const.py:260` | ✓ `codes.go:22` |
| 503 | ServiceUnavailable | ✓ `const.py:261` | ✓ `codes.go:23` |
| 504 | GatewayTimeout | ✓ `const.py:262` | ✓ `codes.go:24` |
| 507 | InsufficientMemory | ✓ `const.py:263` | ✓ `codes.go:25` |

### Home Connect application range (512–544)

| Code | Name (Go / hcws) | hcws | Go |
|---|---|---|---|
| 512 | UnknownUID | `const.py:289` | `codes.go:26` |
| 513 | WriteRequestUnknownUID (`WriteRequest UnknownUID`) | `const.py:296` | `codes.go:27` |
| 514 | ReadRequestUnknownUID (`ReadRequest UnknownUID`) | `const.py:286` | `codes.go:28` |
| 515 | Busy | `const.py:265` | `codes.go:29` |
| 516 | WriteRequestBusy | `const.py:291` | `codes.go:30` |
| 517 | ReadRequestBusy | `const.py:281` | `codes.go:31` |
| 518 | NoAccess | `const.py:274` | `codes.go:32` |
| 519 | WriteRequestNoAccess | `const.py:292` | `codes.go:33` |
| 520 | ReadRequestNoAccess | `const.py:282` | `codes.go:34` |
| 521 | NoAccessByList | `const.py:275` | `codes.go:35` |
| 522 | WriteRequestNoAccessByList | `const.py:293` | `codes.go:36` |
| 523 | ReadRequestNoAccessByList | `const.py:283` | `codes.go:37` |
| 524 | NotAvailable | `const.py:277` | `codes.go:38` |
| 525 | WriteRequestNotAvailable | `const.py:294` | `codes.go:39` |
| 526 | ReadRequestNotAvailable | `const.py:284` | `codes.go:40` |
| 527 | NotAvailableByList | `const.py:278` | `codes.go:41` |
| 528 | WriteRequestNotAvailableByList | `const.py:295` | `codes.go:42` |
| 529 | ReadRequestNotAvailableByList | `const.py:285` | `codes.go:43` |
| 530 | NoExecution | `const.py:276` | `codes.go:44` |
| 531 | ValueOutOfRange | `const.py:290` | `codes.go:45` |
| 532 | InvalidUIDValue | `const.py:272` | `codes.go:46` |
| 533 | Incomplete | `const.py:269` | `codes.go:47` |
| 534 | Inconsistent | `const.py:270` | `codes.go:48` |
| 535 | CmdViolation | `const.py:266` | `codes.go:49` |
| 536 | InvalidFormat | `const.py:271` | `codes.go:50` |
| 537 | RemoteControlNotActive | `const.py:287` | `codes.go:51` |
| 538 | RemoteStartNotActive | `const.py:288` | `codes.go:52` |
| 539 | LockedByLocalControl | `const.py:273` | `codes.go:53` |
| 540 | DeviceStateNotCompliant | `const.py:267` | `codes.go:54` |
| 541 | ProcessStateNotCompliant | `const.py:280` | `codes.go:55` |
| 542 | BackendNotConnected | `const.py:264` | `codes.go:56` |
| 543 | EnergyManagementNotConnected | `const.py:268` | `codes.go:57` |
| 544 | NotInLocalWiFi | `const.py:279` | `codes.go:58` |

All `const.py` citations are `reference/homeconnect-websocket/src/homeconnect_websocket/const.py`;
all `codes.go` citations are `reference/go-homeconnect2mqtt/internal/homeconnect/codes.go`.

Note the 512–529 block is systematic: a base code, then `+1` for the write variant and `+2`
for the read variant (512/513/514 UnknownUID, 515/516/517 Busy, 518/519/520 NoAccess,
521/522/523 NoAccessByList, 524/525/526 NotAvailable, 527/528/529 NotAvailableByList).
*(inference — the pattern is visible in the table but is not stated in any source.)*

---

## 2. Discrepancies, flagged rather than resolved

### 2.1 ⚠️ `412` is mislabelled in homeconnect-websocket

`reference/homeconnect-websocket/src/homeconnect_websocket/const.py:256` reads:

```
412: "414 RequestUriTooLong",
```

The **key is 412** but the **label says 414**, and it sits immediately after
`413: "413 RequestEntityTooLong"` (`const.py:255`) — every other entry in that dict has a
label whose numeric prefix matches its key. Go corrected it to `414: "RequestUriTooLong"`
with no 412 entry at all (`reference/go-homeconnect2mqtt/internal/homeconnect/codes.go:18`),
and the Go prose table lists 413 and 414 consecutively with no 412
(`reference/go-homeconnect2mqtt/docs/01-protocol.md:340-341`).

**Not resolved here.** *(inference)* The Go reading is almost certainly right — this looks
like an off-by-one typo in a hand-transcribed table — but it cannot be settled from source
alone, and no capture has produced either code. Map **both** 412 and 414 to
`RequestUriTooLong` so neither becomes "Unknown", and note the ambiguity in a comment. This
has no behavioural consequence: both are errors either way.

### 2.2 Cosmetic: internal spaces in names

homeconnect-websocket writes `"ReadRequest Busy"`, `"WriteRequest NoAccess"` etc. with a space
(`reference/homeconnect-websocket/src/homeconnect_websocket/const.py:281`, `:292`); Go removes
it (`reference/go-homeconnect2mqtt/internal/homeconnect/codes.go:31`, `:33`).
homeconnect-websocket also prefixes the numeric code into the label for the HTTP-derived range
(`"200 OK"`, `const.py:249`) but not for the application range (`"Busy"`, `const.py:265`).
No behavioural consequence. *(inference)* Use the Go spelling — a bare
`PascalCase` name — since the code is carried as a separate field anyway.

### 2.3 Unknown codes must not be fatal

Both references degrade gracefully: `ERROR_CODES.get(response_message.code, "Unknown")`
(`reference/homeconnect-websocket/src/homeconnect_websocket/session.py:324`) and `CodeName`
returning `"Unknown"` (`reference/go-homeconnect2mqtt/internal/homeconnect/codes.go:62-67`).
The code is still an error; only its *name* is unknown. *(inference)* In Rust this is a
`#[non_exhaustive]`-style enum with an `Other(u16)` variant, not a parse failure — the numeric
value must survive into the error and into logs so an unrecognised code is
diagnosable rather than swallowed.

---

## 3. Which codes matter in practice

From `reference/go-homeconnect2mqtt/docs/01-protocol.md:358-360`:

| Code | Where it bites |
|---|---|
| **400** | wrong program start; sending a float where an int is required |
| **404** | handshake reconnect loop (upstream issue #403) |
| **500** | `/ro/allMandatoryValues` during handshake — **must be tolerated** |
| **541** | writing outside the READWRITE window (upstream issue #384) |

Two more seen in practice:

| Code | Where it bites |
|---|---|
| **403** on `/ci/services` | the regression that motivated Kessel. Analysis in [`handshake.md`](./handshake.md) §11. |
| **403** on `/ni/…` | caused by omitting `NOTIFY /ei/deviceReady` — hcpy's own comment, `reference/hcpy/HCDevice.py:403`. See [`handshake.md`](./handshake.md) §7. |

### Tolerance policy by step

Distilled from [`handshake.md`](./handshake.md); the citations are there.

| Step | On a `code` |
|---|---|
| `GET /ci/services` | **fatal** — this is the one that must never 403 |
| `GET /ci/authentication` | fatal |
| `GET /ci/info` | **tolerated** (all three references) |
| `GET /iz/info` | tolerated by Go, fatal in homeconnect-websocket — divergence |
| `GET /ni/info` | tolerated by Go |
| `GET /ro/allDescriptionChanges` | **tolerated for `>= 500`** |
| `GET /ro/allMandatoryValues` | **tolerated for `>= 500`** |

Go's tolerance predicate for the last two is `code >= 500`, not `code == 500`
(`reference/go-homeconnect2mqtt/internal/homeconnect/session.go:448-451`) — which means the
whole 512–544 application range is also swallowed there. *(inference)* That is the right
behaviour: a bulk read failing for an availability reason should not abort a connection that
is otherwise healthy.
