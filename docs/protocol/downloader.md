# The SingleKey ID profile-download flow

The cloud-side flow that produces the profile ZIP `hc-profile` parses.

This document is **not** about the appliance protocol. Nothing here touches a socket to an
appliance; the flow runs entirely against Bosch/BSH cloud hosts and ends with a file on disk. Once
that file exists, the other protocol documents apply and this one does not.

> **Status.** Kessel does not implement this flow. Users produce the profile ZIP with
> [`homeconnect-profile-downloader`](https://github.com/bruestel/homeconnect-profile-downloader)
> and hand it to Kessel. This document records how the flow works, and what an in-app downloader
> would face, for anyone who builds one.

> **Evidence.** No login was performed to write this document. Every claim is one of:
> **confirmed by source** (a `file:line` citation into a reference implementation),
> **observed** (three unauthenticated authorize requests, §5.3, and one committed screenshot of a
> real login, §5.8.0), **maintainer-confirmed** (one claim, §4.0), *(inference)*, or a **gap** in
> §8 that only a real login can close.

**Three answers up front:**

- **The artifact is the same one `hc-profile` parses.** §4.0.
- **The export format is chosen entirely in the tool's UI. None of the five HTTP requests carries
  it.** Bosch serves exactly one artifact; "openHAB format" is local post-processing. §4.1.
- **A fully automatic, credentials-only login is not achievable.** It was implemented in `hcpy`, it
  worked, and it was **deleted on 2024-12-23 because SingleKey ID deployed a CAPTCHA** — 121 lines
  removed, with the reason in the commit message. §5.2.

---

## 0. Sources and pinned commits

| Repo | URL | Commit | Commit date | Role here |
|---|---|---|---|---|
| `homeconnect-profile-downloader` | `https://github.com/bruestel/homeconnect-profile-downloader` | `95404f1dc31c29c6d857bb33faa2850d5cf0df3c` | 2026-05-04 | **primary** — Electron app, MIT, the tool that produced the real profiles Kessel's fixtures were scrubbed from |
| `hcpy` | `https://github.com/hcpy2-0/hcpy` | `0f2136d39c15864e5a03a1ad5ade326525102313` | 2026-07-03 | **second implementation of the same flow** (`hc-login.py`) — the cross-check |
| `go-homeconnect2mqtt` | `https://github.com/SukramJ/go-homeconnect2mqtt` | `c8817afdde967ebf95461a6a4a057601cd0b7b6c` | 2026-07-28 | secondary — its `docs/03-profile-format.md` documents the ZIP it consumes; it implements **none** of this flow |
| `homeconnect-websocket` | `https://github.com/chris-mc1/homeconnect_websocket` | `508963b94d7b55dc13d950be0e6a3919c8bff237` | 2026-04-04 | contains nothing about this flow |

The last three SHAs are the ones [`README.md`](./README.md) records, so citations into them resolve
against the same checkouts the other protocol documents use.

The primary reference has two code files of substance: `main.js` (650 lines, the whole flow) and
`preload.js` (17 lines, the IPC bridge). Every citation below is into `main.js` unless stated.

### Citing deleted `hcpy` code

§5.2 rests on code that **no longer exists at hcpy's pinned SHA**: a fully automated login deleted
in December 2024. It is cited with an explicit commit:

> `reference/hcpy/hc-login.py@255c731:196-205`

meaning *that path, at that commit, those lines*. `255c731`
(`255c731074400168f06e7880bedc8a1072d7a32e`) is the last commit containing the automated login;
`1ebae3b` (`1ebae3bd31845d92b776372f8272e06c9de4345f`) removed it. A citation with no `@` means the
pinned SHA above.

**Checking these requires a full clone** (407 commits): a depth-50 clone does not reach December
2024. Run `git fetch --unshallow` in the hcpy checkout.

### Two implementations, not three

`homeconnect-websocket` and `go-homeconnect2mqtt` both require the user to bring a ZIP produced
elsewhere (`reference/go-homeconnect2mqtt/docs/00-research.md:151-152`, "No OAuth/cloud flow
needed during normal operation", and `:25`; `reference/go-homeconnect2mqtt/README.md:9-12`). So this
flow has a two-way cross-check. Where the two disagree there is no majority; §7 tabulates every
disagreement rather than resolving it.

**On the login leg the two are not fully independent.** `a40940c`, the hcpy commit that switched it
to the SingleKey ID page (§7 D9), was authored by `bruestel`, author and copyright holder of the
primary reference (`git log` author field). Agreement between them on the login leg is worth less
than it looks. Nothing below depends on it: the constants §2 lists as identical in both predate that
commit, and probe P1 (§5.3) checked the SingleKey stage against the *deleted* 2024 code.

### Licensing

| Reference | Licence | Copyright holder |
|---|---|---|
| `homeconnect-profile-downloader` | MIT | Jonas Brüstel (`reference/homeconnect-profile-downloader/LICENSE`, `package.json:14`) |
| `hcpy` | MIT | Trammell Hudson (osresearch), with portions © Blade, Seidel, Kaper and one further contributor (`reference/hcpy/LICENSE:1-9`) |
| `go-homeconnect2mqtt` | MIT | SukramJ (`reference/go-homeconnect2mqtt/LICENSE:1-3`) |

MIT covers hcpy's deleted revisions exactly as it covers the pinned one. **No code from any reference
is copied into this repository**: quotations below are identifiers, URLs, parameter names and
commit messages cited as evidence. The document is meant to be implementable from the specification
it states.

---

## 1. The flow in one paragraph

**OAuth 2.0 authorization code with PKCE (S256), public client, no client secret.** The client id,
scope list and redirect URI are constants taken from the Home Connect **Android app** —
`reference/hcpy/hc-login.py:2-5` cites `openid/AppAuth-Android` and an Auth0 walkthrough of
"authorization code flow with PKCE", and `:59` says the app id and scope are "hardcoded in the
application". The authorization request goes to a Home Connect host, which redirects into SingleKey
ID for the credential prompt (`hc-login.py:55-57`, "even after the singlekey detour"); the code
comes back on a **custom-scheme** redirect, `hcauth://auth/prod`. The code is exchanged for a JWT
access token; the account id is the token's `sub` claim; three endpoints on a regional asset host
then yield the appliance list, the per-appliance keys, and a ZIP of two XML documents per
appliance. The tool inserts a manifest JSON into that ZIP and writes it out. That manifest is what
`hc-profile` parses.

**Why this is harder than it reads:** the credential prompt is behind a CAPTCHA, and the custom
scheme is the *only* redirect the server accepts — so the login cannot be automated and the code
cannot be delivered anywhere a program is waiting. §5 is the consequence.

---

## 2. Constants

All confirmed by source, and identical in both implementations unless stated.

| Constant | Value | Citation |
|---|---|---|
| `client_id` | `9B75AC9EC512F36C84256AC47D813E2C1DD0D6520DF774B020E1E6E2EB29B1F3` | `main.js:15`; `reference/hcpy/hc-login.py:60` |
| redirect URI | `hcauth://auth/prod` | `main.js:16`; `reference/hcpy/hc-login.py:89` |
| full scope list | `Control DeleteAppliance IdentifyAppliance Images Monitor ReadAccount ReadOrigApi Settings WriteAppliance WriteOrigApi` (space-separated, 10 scopes) | `main.js:17`; as a comment in `reference/hcpy/hc-login.py:61-64` |
| scope hcpy actually requests | **`ReadOrigApi` alone** | `reference/hcpy/hc-login.py:65-67` |
| `code_challenge_method` | `S256` | `main.js:145`; `reference/hcpy/hc-login.py:84` |
| `response_type` | `code` | `main.js:143`; `reference/hcpy/hc-login.py:81` |
| `prompt` | `login` | `main.js:144`; `reference/hcpy/hc-login.py:82` |
| `grant_type` | `authorization_code` | `main.js:351`; `reference/hcpy/hc-login.py:104` |

### Endpoint hosts, per region

Region is a user choice in the UI (`index.html:19-23`: `EU` / `NA` / `CN`) and selects **two**
independent base URLs (`main.js:112-125`):

| Region | API base (`apiBaseUrl`) | Asset base (`assetBaseUrl`) |
|---|---|---|
| EU | `https://api.home-connect.com` | `https://eu.services.home-connect.com` |
| NA | `https://api-rna.home-connect.com` | `https://na.services.home-connect.com` |
| CN | `https://api.home-connect.cn` | `https://cn.services.home-connect.cn` |

Paths (`main.js:130-132`):

| Purpose | URL |
|---|---|
| authorize | `{apiBase}/security/oauth/authorize` |
| token | `{apiBase}/security/oauth/token` |
| appliance XML ZIP (IDDF) | `{assetBase}/api/iddf/v1/iddf/{haId}` |
| paired appliances | `{assetBase}/api/account/v2/accounts/{hcId}/paired-appliances` (`main.js:222`) |
| per-appliance keys | `{assetBase}/api/appliance/v2/appliances/{haId}/encryption-information` (`main.js:242`) |

hcpy names only the EU API host (`reference/hcpy/hc-login.py:48`) and only the EU and NA asset
hosts (`:49-52`); it has no CN region and **no `api-rna` host** — see §7 D1.

### What is a secret and what is not

- **`client_id` is a public client identifier.** The token request carries `grant_type`,
  `client_id`, `code_verifier`, `code`, `redirect_uri` and **nothing else** — no `client_secret` in
  either implementation (`main.js:350-356`; `reference/hcpy/hc-login.py:111-117`), and no
  `Authorization` header (`main.js:361-363` sets only `Content-Type`). Both references publish the
  value in plain source. Committing it here is not a credential disclosure.
- **`code_verifier`, `code`, `state`, `nonce`** are per-attempt values, generated locally and dead
  within one exchange. Do not log them; they are not durable secrets.
- **`access_token`** is a bearer credential for the *cloud account* — at least as sensitive as the
  appliance key material, and belongs in `hc_crypto::Secret` *(recommendation)*.
- **`hcId`** (the account id, §3 step 5) is an account identifier. Do not log it.
- **`key` / `iv`** from `encryption-information` are appliance key material — the same values that
  reach the profile manifest.
- **The SingleKey ID account password** never reaches Kessel under the browser-hosted designs (§5.6).
  If a design needs it, §5.9 is the handling spec.

The primary reference logs several of these to its console and renderer: `main.js:218-219` (the
account `hcId`), `main.js:316` and `:320` (the per-appliance **`key` and `iv`, in plaintext**),
`main.js:372` (`'Token response:', data` — the whole token JSON). Do not reimplement that (§6).

---

## 3. The flow, request by request

### Step 1 — build the authorization URL (no request yet)

Generate, per attempt:

- `code_verifier` — 32 random bytes, base64url, padding stripped (`main.js:136` → `generateNonce`
  at `:194-199`: `crypto.randomBytes(n).toString('base64')` with `+`→`-`, `/`→`_`, trailing `=`
  removed). hcpy: `b64(get_random_bytes(32))`, the same bytes and encoding
  (`reference/hcpy/hc-login.py:78`, `:70-71`).
- `code_challenge` — `base64url(SHA256(code_verifier))`, padding stripped, where the SHA-256 input
  is the **ASCII text of the verifier**, not the bytes it decodes to (`main.js:201-209`;
  `reference/hcpy/hc-login.py:83` hashes `verifier.encode("UTF-8")`). This is the standard RFC 7636
  S256 construction; the common mistake is hashing the pre-encoding random bytes.
- `state` and `nonce` — 16 random bytes, opaque (`main.js:134-135`). hcpy's `b64random`
  double-encodes (`reference/hcpy/hc-login.py:74-75`: base64url of the base64url of the bytes);
  both are opaque server-echoed strings, so this is cosmetic *(inference)*.

Then `GET {apiBase}/security/oauth/authorize` with these query parameters (`main.js:140-154`), in
this insertion order:

```
redirect_url, client_id, response_type, prompt, code_challenge_method,
code_challenge, state, nonce, scope
```

⚠️ **`redirect_url`, not `redirect_uri`.** The primary reference spells the authorize-leg
parameter `redirect_url` (`main.js:141`) and the token-leg parameter `redirect_uri`
(`main.js:355`). hcpy spells **both** `redirect_uri` (`reference/hcpy/hc-login.py:89`, `:116`).
Send `redirect_uri`; see §7 D2. Query-parameter order is almost certainly irrelevant — this is a
query string parsed by a normal web stack *(inference)*.

The URL is then **loaded in a browser**: the primary reference hands it to a browser engine
(`main.js:156`, `mainWindow.loadURL`); hcpy prints it for the user to paste into Chrome
(`reference/hcpy/hc-login.py:95-100`).

### Step 2 — the login: a human in a browser

The authorization server redirects into SingleKey ID, the user authenticates **and solves a
CAPTCHA**, and a navigation to `hcauth://auth/prod?code=…&state=…&grant_type=authorization_code` is
attempted.

**Neither implementation drives this leg today**, because a CAPTCHA ended the one implementation that
did (§5.2).

The first hop is observed (§5.3, probe P1): the authorize endpoint returns `302` with
`location: https://singlekey-id.com/auth/connect/authorize?…`, carrying a *second*, distinct OAuth
client — SingleKey client id `11F75C04-21C2-4DA9-A623-228B54E9A256`,
`redirect_uri=https://api.home-connect.com/security/oauth/redirect_target`,
`scope=openid email profile offline_access homeconnect.general`, `prompt=login`, `style_id=bsh_hc_01`,
and `state={"session_id":"<uuid>"}` threading the Home Connect session through. Every one of those
constants matches hcpy's deleted code (`hc-login.py@255c731:127-136`), so this stage is unchanged
since 2024 apart from the CAPTCHA.

What happens *after* that hop — form fields, the CAPTCHA widget, whether a consent step or 2FA can
appear — is undocumented by both implementations and unprobed (§8 G1). A browser-hosted design needs
none of it, which is also why Bosch can change this leg without breaking such a design (§6 F1).

`prompt=login` forces a fresh credential prompt every time, so there is no silent-SSO path and no
long-lived session to reuse (`main.js:144`) *(inference on the consequence; the parameter is
confirmed)*.

### Step 3 — capture the authorization code

The primary reference registers an Electron `webRequest.onBeforeRequest` listener on
`*://*/auth/prod*` (`main.js:18-20`, `:160`), reads `code` from the query string
(`main.js:162-164`), and **cancels the request** (`main.js:166-168`) so the browser never tries to
resolve the unknown scheme. It reads only `code`.

hcpy instructs the user to open devtools, watch the network panel, read the `hcauth://auth` request
and paste `code` and `state` at a prompt (`reference/hcpy/hc-login.py:95-103`). hcpy used to capture
the code programmatically, reading it out of a redirect `Location` header without following it
(`hc-login.py@255c731:196-205`) — so the custom scheme is no obstacle to an HTTP client. §5.2 explains
what removed that, and §5.3 why it cannot be done from a browser.

The callback carries **three** parameters: `code`, `state`, and a server-supplied
`grant_type=authorization_code` (observed, §5.8.0 item 2). The deleted code read `grant_type` from the
callback (`hc-login.py@255c731:223`); the current code hard-codes it (`hc-login.py:104`). Both work.

⚠️ **Neither implementation validates `state`.** The primary reference never reads it back
(`main.js:164`); hcpy reads it and only prints it (`reference/hcpy/hc-login.py:103`, `:106`). A
reimplementation should compare the returned `state` to the generated one and reject a mismatch —
more so when the code arrives by paste rather than by interception *(recommendation)*.

### Step 4 — exchange the code for a token

```
POST {apiBase}/security/oauth/token
Content-Type: application/x-www-form-urlencoded

grant_type=authorization_code
client_id=<the constant>
code_verifier=<step 1>
code=<step 3>
redirect_uri=hcauth://auth/prod
```

`main.js:349-365`; `reference/hcpy/hc-login.py:111-121`. Both agree on every field name, on
`redirect_uri` (with `_uri` in both, unlike step 1), and on the absence of a client secret. hcpy adds
`allow_redirects=False` (`:121`). Non-2xx is a hard failure in both (`main.js:367-369`;
`reference/hcpy/hc-login.py:122-125`).

The response is JSON; the only field either reads is **`access_token`** (`main.js:373`;
`reference/hcpy/hc-login.py:129`). Whether `refresh_token`, `expires_in`, `id_token` or `token_type`
is present is **unknown** (§8 G2). The token is needed only for the requests of one onboarding run.

**No cookie is sent on this request.** The primary reference makes it with `node-fetch`
(`main.js:3`, `:359`), a plain Node client with no connection to the Electron
`session.defaultSession` cookie jar the login used. hcpy constructs a `requests.Session()` at
`reference/hcpy/hc-login.py:46` and **never uses it** — every call is a bare
`requests.post`/`requests.get` (`:121`, `:144`, `:175`, `:200`). Both implementations are stateless
from the code onward, which is the fact §5 builds on.

### Step 5 — derive the account id from the token

Split the JWT on `.`, base64-decode the payload segment, read the **`sub`** claim; that is `hcId`
(`main.js:215-217`; `reference/hcpy/hc-login.py:136-139`). There is **no userinfo or account
endpoint call**.

Neither implementation verifies the JWT signature, and neither needs to: the token came over TLS
from the issuer and is used only as an opaque bearer credential plus one claim.
`reference/hcpy/hc-login.py:34-37` re-adds the base64 padding that `main.js:216` relies on Node's
decoder to tolerate — use a padding-tolerant base64url decoder, as `tls-psk.md` §3 also requires
for the psk.

### Step 6 — enumerate the appliances

```
GET {assetBase}/api/account/v2/accounts/{hcId}/paired-appliances
Authorization: Bearer <access_token>
Accept: application/json
```

`main.js:381-390`. Response: an object with an **`appliances`** array (`main.js:224` reads
`appliancesData.appliances`; `reference/hcpy/hc-login.py:163` iterates `appliances["appliances"]`).

Per-element fields, confirmed by the reads that consume them:

| Field | Read at | Notes |
|---|---|---|
| `haId` | `main.js:242`, `:255`; `reference/hcpy/hc-login.py:166` | the appliance key for both later endpoints |
| `haType` | `main.js:256`; `reference/hcpy/hc-login.py:165` | becomes the manifest's `type`, e.g. `Hob`, `Oven` |
| `serialNumber` | `main.js:257` | |
| `brand` | `main.js:258`; `reference/hcpy/hc-login.py:164` | casing unknown — see §7 D3 |
| `vib` | `main.js:259` | model designation |
| `mac` | `main.js:260` | `AA-BB-CC-DD-EE-FF` form *(inference, from the `-`-stripping at `main.js:572` and the form `hc-profile`'s `DeviceIdentity::mac` documents)* |
| `isDemo` | `main.js:231` | boolean; see below |

⚠️ **`401` on this call means "wrong region", not "bad token".** The primary reference maps it to
exactly that message (`main.js:392-393`), and hcpy probes the regions, taking the first asset host
that returns 200 (`reference/hcpy/hc-login.py:142-149`). Treating 401 as an auth failure and
re-running the login loops forever on a correct credential.

**Demo appliances.** The primary reference drops elements with `isDemo` truthy, because they "don't
support encryption or local communication" (`main.js:230-238`), and errors out if nothing is left
(`:232-234`). hcpy does not filter (`reference/hcpy/hc-login.py:163`). Filter: a demo appliance would
produce a profile with no keys, which `hc-profile` rejects later with a worse error *(inference)*.

### Step 7 — fetch the keys, per appliance

```
GET {assetBase}/api/appliance/v2/appliances/{haId}/encryption-information
Authorization: Bearer <access_token>
Accept: application/json
```

`main.js:408-417`; `reference/hcpy/hc-login.py:174-175`. Response: an object with optional **`tls`**
and optional **`aes`** sub-objects (`main.js:245-246`, `reference/hcpy/hc-login.py:184-185`):

- `tls.key` — the base64url psk (`main.js:280`, `:318`; `reference/hcpy/hc-login.py:190`)
- `aes.key` and `aes.iv` — psk and IV for the AES path (`main.js:283-284`, `:322-323`;
  `reference/hcpy/hc-login.py:194-195`)

**The transport is selected by which sub-object is present**, and `tls` wins if both are
(`main.js:315-328`; `reference/hcpy/hc-login.py:187-195`). This is the cloud-side origin of the rule
`tls-psk.md` §1 states for the manifest ("`iv` present") and that `hc-profile` applies via
`connectionType` — the same decision made at three points *(inference)*.

**Failure here is per-appliance and non-fatal** in the primary reference: a failure leaves both
fields undefined (`main.js:243-250`) and the appliance is skipped when the profile is built
(`main.js:324-328`, and `:271-275` on the hcpy target).

### Step 8 — fetch the appliance XML archive (IDDF)

```
GET {assetBase}/api/iddf/v1/iddf/{haId}
Authorization: Bearer <access_token>
```

Binary, read as an array buffer (`main.js:433-445`, `axios` with `responseType: 'arraybuffer'`); hcpy
writes the body straight to `{haId}.zip` (`reference/hcpy/hc-login.py:198-209`). No `Accept` header
is sent (`main.js:434-436`).

**The archive contains exactly two members and no manifest:**

```
{haId}_DeviceDescription.xml
{haId}_FeatureMapping.xml
```

hcpy opens those two names and nothing else (`reference/hcpy/hc-login.py:212-213`), and the primary
reference's hcpy-target branch suffix-scans the archive for them (`main.js:456-462`). **There is no
`.json` in what Bosch sends**; §4 is about the manifest the tool adds.

---

## 4. The output format

### 4.0 Provenance — maintainer-confirmed

**The real profile ZIPs `hc-profile` was developed against — the ones the committed fixtures under
`crates/hc-profile/tests/fixtures/` were scrubbed from — were produced by this tool with the openHAB
export option selected.** This is stated by the maintainer, who ran the tool: an observation of the
actual artifacts rather than an inference from code.

So **the downloader's output is the artifact `hc-profile` parses**, and an in-app downloader is a
fetch layer over a working parser. What Bosch's IDDF endpoint returns is *not* that artifact: it has
no manifest member, so `hc-profile` rejects it with `ProfileError::NoManifest`. The difference is
the tool's local post-processing (§4.1), and reproducing it is the only format work.

The archive's internals are documented elsewhere: `crates/hc-profile/tests/fixtures/README.md` for
the fixtures, [`message-model.md`](./message-model.md) and the rest of `docs/protocol/` for what the
XML means. This section covers only the seam: what the tool adds, and where each field comes from.

The code agrees independently: the fixtures' manifest key order is the object-literal insertion
order of `main.js:303-323`, and their member order is the two XMLs followed by the JSON — what
appending one member to a two-member archive produces at `main.js:579-581`.

### 4.1 The export format is not in any request

**The format is chosen purely in the UI. All five HTTP requests are format-agnostic.** Confirmed by
source, exhaustively.

The format lives in a variable named `target`:

1. Chosen from an HTML `<select id="target">` with three options — `homeconnectdirect` (labelled
   "openHAB - Home Connect Direct Binding"), `homeconnect_local_hass`, `hcpy` (`index.html:26-31`).
2. Read on button click and passed over the IPC bridge with the region (`index.html:46-51`;
   `preload.js:4-6`).
3. Stored in a module-level variable in the main process (`main.js:108`, `:127`).
4. Read at exactly **two** places, both after every network call has returned: the branch selecting
   the output writer (`main.js:265`, `target === 'hcpy'`) and the output **filename prefix**
   (`main.js:573`).

It appears in no request:

| Request | Where its parameters are built | `target` present? |
|---|---|---|
| authorize | `main.js:140-154` (9 query parameters, §3 step 1) | **no** |
| token | `main.js:350-363` (5 form fields + `Content-Type`) | **no** |
| `paired-appliances` | `main.js:222`, `:384-389` (URL has `hcId`; headers `Authorization` + `Accept`) | **no** |
| `encryption-information` | `main.js:242`, `:411-416` (URL has `haId`; same two headers) | **no** |
| IDDF archive | `main.js:433-439` (URL has `haId`; `Authorization` only) | **no** |

hcpy has no notion of an export format and calls the same five endpoints with the same parameters
(`reference/hcpy/hc-login.py:80-90`, `:111-117`, `:145`, `:174`, `:198`).

Consequences:

1. **There is no format parameter to get right.** Bosch serves one artifact per appliance, the
   two-XML IDDF archive (§3 step 8). Every "format" the tool offers is a local rendering of it.
2. **The openHAB format is defined entirely by local post-processing**: insert one JSON member and
   re-serialise (`main.js:579-582`). §4.2 specifies that member.
3. **`homeconnectdirect` and `homeconnect_local_hass` differ only in the output filename prefix**
   (`main.js:573`); the archive contents are identical. Only `hcpy` is a different renderer (§4.3).
4. No Bosch-side format negotiation exists to break, so §6 has no format row.

### 4.2 The manifest member

The tool builds a JSON object (`main.js:303-313`), conditionally appends `connectionType`, `key`
and — on AES only — `iv` (`main.js:315-323`), inserts it as `{haId}.json` (`main.js:579`), and
re-serialises the archive (`main.js:581-582`). Field by field, against `hc-profile`'s `RawManifest`:

| Manifest field | Source | `hc-profile` requires it? |
|---|---|---|
| `haId` | appliance `haId` (`main.js:304`) | **yes** |
| `type` | appliance `haType` (`main.js:305`) | **yes** |
| `serialNumber` | appliance `serialNumber` (`main.js:306`) | optional |
| `brand` | appliance `brand`, **uppercased** (`main.js:307`) | **yes** |
| `vib` | appliance `vib` (`main.js:308`) | **yes** |
| `mac` | appliance `mac` (`main.js:309`) | optional |
| `featureMappingFileName` | **synthesised** as `{haId}_FeatureMapping.xml` (`main.js:310`) | **yes** |
| `deviceDescriptionFileName` | **synthesised** as `{haId}_DeviceDescription.xml` (`main.js:311`) | **yes** |
| `created` | local wall clock (`main.js:312`, `generateTimestamp` at `:632-651`) | optional, opaque |
| `connectionType` | `"TLS"` or `"AES"` (`main.js:317`, `:321`) | **yes** |
| `key` | `tls.key` or `aes.key` (`main.js:318`, `:322`) | **yes** |
| `iv` | `aes.iv`, AES only (`main.js:323`) | required iff `connectionType == "AES"` |

Nothing `hc-profile` requires is missing, and nothing the tool writes is unrecognised.

⚠️ **The two XML filenames are a convention, not an observation.** `main.js:310-311` *composes* them
from `haId` and never checks that such a member exists; `reference/go-homeconnect2mqtt/docs/03-profile-format.md:30-33`
warns consumers not to rely on the convention. On real profiles it holds — the fixtures' declared
names equal `{haId}_…` exactly. If Bosch ever names an IDDF member differently, the tool writes a
manifest that lies, and `hc-profile` fails with `ProfileError::DeclaredMemberMissing`. **An
implementation should read the member names out of the archive** (suffix-match, as
`main.js:456-462` does on the other path).

The `created` timestamp (`main.js:632-651`) is local time with a fake nanosecond tail
(`{millis}000000`) and a `±HH:MM` offset whose sign is computed as `offsetMinutes > 0 ? '-' : '+'`.
`Date.getTimezoneOffset()` is west-positive, so the sign is correct even though it looks inverted.
`hc-profile` treats `created` as an opaque optional string.

### 4.3 The `hcpy` target

`hcpy` produces no ZIP: a single `hcpy-devices_{date}.json` array (`main.js:593-619`) whose
per-device objects carry `name`, `key`, `iv?`, `host`, `description` and `features`
(`main.js:277-289`) — the XML is pre-parsed into JSON by `xml2js` and a hand-written walk
(`main.js:448-560`). **Do not use this target with Kessel.** It discards `haId`, `serialNumber`,
`vib`, `mac` and `connectionType`.

### 4.4 What an in-app downloader has to build

1. Steps 1–8: three JSON GETs, one form POST, one binary GET, one JWT payload decode, with no
   format parameter (§4.1).
2. Synthesise the manifest — the twelve fields of §4.2 — from steps 6 and 7.
3. Hand the result to `hc-profile`, whose only entry points are `Profile::from_zip_bytes` /
   `from_zip_path` / `from_zip_reader`. Building the three-member ZIP in memory and calling
   `from_zip_bytes` needs no `hc-profile` change (the `zip` crate is already a workspace
   dependency), and lets the downloader save the same ZIP — so the downloader and paste-the-ZIP
   converge on one artifact.

---

## 5. Why a browser, and what that forces

§5.1–§5.3 are confirmed by source or observed. §5.4 onward draw conclusions from them and are
analysis.

### 5.0 The three candidate reasons for a browser

| Candidate | Required? | Basis |
|---|---|---|
| **Cookie handling** across the login and the API calls | **Not for the API calls**, but a cookie jar *was* required across the login itself. | The token exchange and all three API calls are cookie-free in both implementations (§3 step 4). hcpy's automated login carried a `requests.Session()` across the SingleKey detour (`hc-login.py@255c731:45-46`, `:105`, `:144`, `:169`, `:186`, `:199`); the unused `requests.Session()` at `hc-login.py:46` is what is left of it. |
| **A redirect the daemon cannot receive** | **Yes, and it is the binding constraint** — but not because a client cannot read the scheme. | A client that controls redirect-following reads `hcauth://…` out of a `Location` header, as hcpy did (§5.2). The constraint is that **only** that scheme is accepted (§5.3), and a *browser* cannot hand it to a program. |
| **A webview intercepting a navigation** | **Yes.** This is why the primary reference is an Electron app. | §5.1; §5.2 explains what made it unavoidable. |

The custom scheme by itself is not the obstacle; the CAPTCHA (§5.2) and the single accepted
redirect URI (§5.3) are.

### 5.1 What Electron buys

1. **Automatic capture of the authorization code**, by intercepting the navigation to
   `hcauth://auth/prod` and cancelling it (`main.js:18-20`, `:160-168`). A browser-engine
   capability; Electron does not *receive* the custom scheme either.
2. **A place for a human to solve a CAPTCHA.** It renders whatever SingleKey ID serves, in a window
   the user can type into. The upstream history corroborates this: `fix/5-cant-paste-password` and
   "fix: allow copy and paste (on Mac OS)" (`7d3e6c2`) are fixes for typing credentials into the
   rendered login page.

It buys nothing else:

- **Not the preload bridge.** `preload.js` carries only UI plumbing: submit the region/target form,
  list previously downloaded files, receive log lines.
- **Probably not the user agent.** The app sets an Android-mobile Chrome UA (`main.js:178`); the §5.3
  probes sent the same UA, so they do not discriminate. hcpy's flow works in ordinary desktop Chrome
  (`reference/hcpy/README.md:53-54`), weak evidence the spoof is unnecessary *(inference)*.
- **Not driving the login form.** Neither implementation contains a selector or form-field name for
  the SingleKey pages today. hcpy once did (§5.2).

### 5.2 The automation that existed, worked, and was deleted because of a CAPTCHA

`hcpy` used to perform the entire login from a username and password, with no browser. It was
**deleted on 2024-12-23**:

> `1ebae3bd31845d92b776372f8272e06c9de4345f` — *"Change hc-login due to hcaptcha"*
>
> > "The old authentication flow no-longer works due to HCaptcha implementation on the SingleKey
> > pages. Now requires manual user intervention."
>
> `hc-login.py | 128 ++++------ 1 file changed, 7 insertions(+), 121 deletions(-)`

Corroborated in the same repository:

- `reference/hcpy/README.md:53-54`: "The login process has changed as the HomeConnect SingleKey pages
  now implement a CAPTCHA. hc-login.py will now prompt users with a URL that they must follow in a
  normal browser window (Chromium), and use Development tools (F12)…"
- `reference/hcpy/CHANGELOG.md:112`: "the login process has become more complex after having CAPTCHAs
  introduced"; `:119` links the pull request.

There is no CAPTCHA detection or handling code anywhere in hcpy (`grep -ri captcha --include=*.py`
returns nothing). The project removed the automation and handed the user to a browser.

**What the deleted code did** (`reference/hcpy/hc-login.py@255c731`):

1. `GET` the authorize URL **with an extra parameter, `redirect_target=icore`** (`:98`), and scrape
   `sessionId` and `sessionData` out of the returned HTML with regexes (`:105-118`). The parameter
   was removed in `a40940c` ("Show SingleKey ID login page"), whose message says it skips "the old
   Home Connect login page" — so `redirect_target=icore` selected Bosch's older, scrapeable login
   page instead of SingleKey ID.
2. Drive SingleKey ID directly at `https://singlekey-id.com` (`:124-136`), passing the Home Connect
   session id as `state={"session_id":"…"}` — with the source comment *"important: no spaces!"*
   (`:135`).
3. Follow redirects **manually**, `allow_redirects=False`, reading `r.headers["location"]` and
   resolving relative locations against the host (`:142-152`).
4. Parse the anti-CSRF token out of the HTML with BeautifulSoup and POST the **e-mail**, then the
   **password**, as ASP.NET-style form fields — `UserIdentifierInput.EmailInput.StringValue`,
   `Password`, `RememberMe`, `__RequestVerificationToken` — plus an `X-CSRF-FORM-TOKEN` cookie
   echoed into a `RequestVerificationToken` header (`:161-194`).
5. **Capture the code from a `Location` header without following it**: loop on the return URL with
   redirects disabled, break when the location `startswith("hcauth://")` (`:196-205`), then
   `parse_qs` the code and state out of it (`:210-222`).

Step 5 shows the custom scheme was never an obstacle to a headless client. What ended the automation
was step 4 — a CAPTCHA in front of the password form.

The two later login commits restore no automation. `a40940c` (2026-02-05, "Show SingleKey ID login
page", 4 insertions, 13 deletions) removes `redirect_target=icore` and turns the ten-scope list into
a comment, leaving `ReadOrigApi`. `f344bd7` (2026-04-11, "Update to new login API") touches only the
post-code steps: it moved hcpy from `prod.reu.rest.homeconnectegw.com` + `account/details` to
`eu.services.home-connect.com` + `paired-appliances`/`encryption-information` and added the JWT `sub`
decode — corroborating §3 steps 5–7.

Two further facts from the deleted code:

- The historical asset hosts were `prod.reu.rest.homeconnectegw.com` /
  `prod.rna.rest.homeconnectegw.com` (`hc-login.py@255c731:49-52`), versus today's
  `eu.services.home-connect.com` / `na.services.home-connect.com`. **Bosch has renamed every asset
  host once already** (§6 F5).
- **The authorization code is short-lived, with an unmeasured expiry.** `reference/hcpy/README.md:56`:
  "This step is time-sensitive, but we have not identified the expiry time for the challenge. There
  is time to perform this step without too much haste but consider performing this process within
  60s to avoid issues." The ~60 s is hcpy's suggestion, not a measured expiry (§8 G3b).

### 5.3 Probe: is any other `redirect_uri` accepted? No.

Three unauthenticated `GET`s were sent to `https://api.home-connect.com/security/oauth/authorize`,
with redirect-following disabled (`curl --max-redirs 0`), two seconds apart, using the public
`client_id`, `scope=ReadOrigApi`, a throwaway PKCE challenge and the reference's mobile UA. No
credentials were sent, no login form was fetched, and `singlekey-id.com` was never requested. The
three differ *only* in `redirect_uri`:

| Probe | `redirect_uri` | Result |
|---|---|---|
| **P1 — control** | `hcauth://auth/prod` | **`HTTP/2 302`**, `location: https://singlekey-id.com/auth/connect/authorize?…` — **accepted** |
| **P2 — the question** | `http://127.0.0.1:8099/onboarding/callback` | **`HTTP/2 400`** `{"error":"unauthorized_client","error_description":"client or redirection URI not authorized"}` — **rejected** |
| **P3 — negative control** | `https://example.invalid/nope` | **`HTTP/2 400`**, byte-identical error body to P2 — **rejected** |

P1 shows the request shape is otherwise valid and the client id is live. P3 shows `redirect_uri` is
validated at the authorize endpoint, not deferred to the token exchange. P2 fails identically to the
bogus URI. **A loopback or Kessel-origin redirect URI is not accepted for this `client_id`.** No
variants were tried and no attempt was made to enumerate what else is registered.

P1's `Location` header also exposes the SingleKey ID request (§3 step 2), and every constant in it
matches `hc-login.py@255c731:127-136`. The `session_id` value is omitted here; only its shape matters.

### 5.4 Verdict

> **The flow cannot be made fully automatic — not in the daemon, and not in the web UI either.**
>
> 1. **Server-side programmatic login is dead.** It existed, worked, and a CAPTCHA ended it (§5.2).
> 2. **The login must therefore happen in a browser, with a human in it**, as in both
>    implementations.
> 3. **The code cannot get from that browser back to Kessel automatically**, because the only
>    accepted `redirect_uri` is a custom scheme (§5.3) the browser cannot open and a web page cannot
>    read across origins.
>
> The irreducible manual residue is: the human logs in and solves a CAPTCHA (true of the Electron
> tool too), **and** transfers one string back to Kessel. That string is **long** — the code is an
> encoded JSON object that wraps onto a second line in devtools (§5.8.0 item 3) — so the input must
> be a textarea-class field that tolerates whitespace on paste.

Everything from the code onward (§3 steps 4–8) is automatable: token exchange, account id,
appliance enumeration, keys, archive fetch, manifest synthesis.

### 5.5 The design options

| Option | Verdict | Why |
|---|---|---|
| **(a) Programmatic form login in the daemon**, following hcpy's deleted code | **Ruled out.** | The CAPTCHA (§5.2). It also means handling a password the browser-hosted designs never see (§5.6), and driving a CAPTCHA is hostile to the site operator. |
| **(b) Loopback / Kessel-origin `redirect_uri`**, so the web UI receives the code | **Ruled out by observation.** | §5.3, P2 vs P3: rejected as `unauthorized_client`. |
| **(c) Login in the user's browser + the user transfers the code** | **Works.** | hcpy is the existence proof (`README.md:53-56`). It keeps a manual step, and runs against an unmeasured code lifetime (§5.2, §8 G3b). Kessel can improve on hcpy's "open F12" — see below — but cannot remove the step. |
| **(d) Bundle a browser engine in Kessel**, as the reference does | **Rejected on architecture, not feasibility.** | It is the one design known to work end to end, but Kessel is a daemon shipped in a container; embedding Chromium contradicts the deployment target and the image size. A headless automation driver (CDP, Playwright) is option (a) with extra steps: it still meets the CAPTCHA. |
| **(e) Keep paste-the-ZIP** | **Always available, and what Kessel ships.** | No Bosch endpoint is touched. |
| **(f) Register Kessel's *own* OAuth client** with a loopback `redirect_uri` | **Not measured; very likely dead, for a reason the probe did not establish.** | §5.3 measured the Android app's `client_id`; it says nothing about a *different* client. A client registered for Kessel would remove the transfer leg, and *(inference)* possibly the CAPTCHA if a developer-portal authorization surface differs from the consumer login. But `ReadOrigApi`/`WriteOrigApi`, `encryption-information` and the IDDF endpoint read as first-party surfaces, not anything a third-party developer portal would grant, so there is very likely no registration that reaches the psk *(analysis, not confirmed by source or measured)*. It requires applying to BSH, with unbounded latency and a likely refusal; it cannot be tested without applying; and a granted registration makes Kessel the custodian of a project-level client identity. |
| **(g) Auto-ingest** — Kessel watches a directory and the user runs the existing Electron tool | **Available, cheaper than (c), but not "automatic login".** | No OAuth client, PKCE, token or `state` check, and none of §4.1's synthesis: the tool already produces the finished artifact. It saves a step over (e), touches no Bosch endpoint (so only §6 F9 applies), and has no code-expiry race. The user still runs a second tool and solves a CAPTCHA there. It needs a path shared between user and container (a bind mount); a watched directory of profile ZIPs is psks on disk; and a partial write can only be detected by attempting the parse and retrying. |
| **(h) A host-side `hcauth://` handler that POSTs the code to Kessel's loopback API** | **Costed, not prohibited. On a non-container desktop install it removes the transfer leg.** | Registering an OS URL-scheme handler for `hcauth://auth/prod?code=…` is the standard AppAuth-for-native-apps pattern, and the only design that removes the manual transfer *using the `redirect_uri` Bosch already accepts*. The CAPTCHA leg remains. Costs: it **collides with the installed Home Connect app**, which registers the same scheme — which handler wins is OS-dependent, and taking it breaks the real app for that user; it needs OS-level install steps (a `.desktop` entry, a registry key, an `Info.plist`); and it is meaningless in a container, where there is no host desktop. Like (d), these are deployment-target objections, not infeasibility. |
| **(i) Browser-first, then impersonate** — the user logs in in their own browser, the frontend hands the session cookies to the daemon, and the daemon continues the redirect chain and reads `hcauth://auth/prod?code=…` from a `Location` header | **Blocked** — by the web platform, not by Bosch. | The state that must cross the boundary — two hosts' session cookies and the `ReturnUrl` the daemon would poll — is what the platform makes unreadable to script on a foreign origin. Every variant that escapes the rule stops being "hand the cookies over" or collapses into (c). And once the CAPTCHA and password are done, the browser has already followed the remaining redirects itself. §5.8.2. |
| **(j) Backend-first, proxy the challenge** — the daemon drives the SingleKey form flow with its own cookie jar, and the challenge is shown to the human through Kessel's own page, with the solution posted back through the daemon | **Viable-but-costly *if* one server-side question passes**, and that question is not answerable from code. §5.8.5. | (j) has no state-crossing problem: cookies, both anti-forgery tokens, the PKCE verifier and `ReturnUrl` stay in the daemon (§5.8.1). It stands or falls on whether SingleKey accepts a challenge token solved on a foreign origin (§5.8.3, layer 2), which only a real login can show (E3, §5.8.4). Regardless of the answer, the daemon receives the account **password** (§5.9), and Kessel maintains a proxy of a third party's login UI (§5.8.6). |

(c), (e), (g) and (h) are all shippable; they trade the manual step against the deployment target
against implementation cost. They are not mutually exclusive: (e) is the floor under all of them,
and (g) is an increment on (e) that (c) and (h) would sit beside.

**The best achievable UX for (c)** *(analysis)*: the web UI composes and opens the authorize URL; the
user logs in and solves the CAPTCHA in their own browser; the browser fails to open
`hcauth://auth/prod?code=…`; the user copies **that URL** and pastes it into a field the UI is
already showing; the daemon extracts `code`, verifies `state`, and finishes. One paste, no devtools,
no file. Whether a current browser leaves that URL copyable is **unverified** (§8 G1).

If no browser leaves it copyable, (c) degrades to hcpy's devtools instructions — the known-working
floor. Two cheaper fallbacks are worth checking in the same experiment: whether the **preceding
`https://api.home-connect.com/security/oauth/redirect_target` URL** is copyable (a real HTTP
navigation the browser can render), and whether the failed `hcauth://` navigation leaves a
**history entry** whose URL can be copied.

### 5.6 The browser-hosted designs keep the password out of Kessel

When the login happens in the user's browser (§5.4), **Kessel never sees the SingleKey ID
password.** That is a better posture than any design that receives it:

- An account password is a different class of secret from appliance key material. Appliance keys
  are scoped to hardware on the user's own LAN; the password unlocks the Bosch cloud account,
  including appliance control and account data far beyond Kessel's scope, and is likely reused
  elsewhere.
- Option (a) would have to accept it, hold it across a multi-request form sequence and, for any
  non-interactive re-run, persist it. The browser-hosted designs need none of that.
- This makes (a) undesirable independently of the CAPTCHA.

Handling the password is acceptable if a design such as (j) needs it; §5.9 is the handling spec.

On this path Kessel handles the `access_token` and `hcId` (§2) and the appliance `key`/`iv`. The
`access_token` is needed only for the 4–6 requests of one onboarding run and is never persisted.

**Do not follow the reference's example.** It prints the appliance `key` and `iv` in plaintext
(`main.js:316`, `:320`), the entire token response (`main.js:372`) and the account `hcId`
(`main.js:218-219`). In Kessel all of these must be wrapped in `hc_crypto::Secret` at the parse
boundary, as `hc-profile` already does for the same values read from the manifest.

### 5.7 What not to do

- **Do not drive the CAPTCHA** — with an automation framework, a solving service, or a headless
  browser. §5.2 shows it does not work, and it is hostile to the site operator.
- **Do not design around a `redirect_uri` other than `hcauth://auth/prod` *for this `client_id`*.**
  That is what §5.3 measured. It says nothing about a differently registered client (§5.5 (f)).

## 5.8 The two hybrid browser/daemon splits — options (i) and (j)

Everything here is **confirmed by source**, **observed in a committed screenshot** (§5.8.0), a
**platform-specification** claim labelled as such, or **unverified** and named as the experiment
that would settle it. No request was sent to a Bosch endpoint for this section, and no CAPTCHA was
interacted with.

Both options split the flow so the `hcauth://` callback lands where a program can read it. Whether
they work depends on **which side of the split holds the session.**

### 5.8.0 hcpy's committed devtools screenshot

`reference/hcpy/images/hclogin_dev_tools.png` (added 2025-01-19, `976fa56`, committed by Ben
Campbell / `Meatballs1`, who also authored `1ebae3b`) is a Chrome 131 screenshot of a **real**
login — one capture, one browser, January 2025, and the only direct observation of the post-CAPTCHA
flow available. Five things are legible:

1. **The failed callback is visible in the Network panel**: `Request URL:
   hcauth://auth/prod?code=…&state=…&grant_type=authorization_code`, **`Response Headers (0)`**,
   `Request Headers (6)`. The browser attempted it, got nothing back, and kept the record — which is
   why hcpy's instructions work (`hc-login.py:95-100`).
2. **The callback carries three query parameters**: `grant_type=authorization_code` is supplied by
   the server (§3 step 3).
3. **The authorization code begins `eyJyZWdpb`.** The *Headers* pane redacts the URL after
   `code=eyJy`, but the Network panel's **Name** column lists the same request as
   `prod?code=eyJyZWdpb…`, truncated by devtools. The first eight characters decode to `{"regi`;
   the ninth (`b`) constrains the seventh byte to `l`/`m`/`n`/`o`. So the code is a base64url-encoded
   JSON object opening with a key beginning `regi` — `{"region"…` is the obvious reading
   *(inference)*. **It is long**: in the Headers pane (1915×916 PNG) the value overflows the
   ~940 px value column and wraps onto a second line, ending `…%3D` before `&state=`.
4. **The user is left on a SingleKey-branded interstitial** reading "Redirecting… You will be
   automatically redirected back to the application in 3 seconds" with a `CONTINUE>` link. The
   `hcauth://` hop fires from a page that stays loaded. The address bar is not in the capture (§8
   G1), but the interstitial makes "the address bar holds the `hcauth://` URL" the less likely
   outcome *(inference)*.
5. **The SingleKey login UI is `htmx`-driven** — `htmx.00GdzawUILg.mjs` and `base.veDyGCvmY8w.mjs`
   are in the request list: server-rendered HTML fragments swapped into the page (§5.8.6).

The screenshot shows no CAPTCHA request, which is **not** evidence of absence: the panel is scrolled
to the end of a preserved log of **709 requests / 1.6 MB**, with about twenty entries in frame.

### 5.8.1 The state the redirect chain depends on

From hcpy's **deleted** implementation (`reference/hcpy/hc-login.py@255c731`, the only one that ever
drove this leg from a program) and from `main.js`. The last column decides both designs.

| # | State | Where it comes from | Set/held by | Crossing the browser→daemon boundary |
|---|---|---|---|---|
| S1 | PKCE `code_verifier` | generated locally (`@255c731:86`), consumed only at the token POST (`:233`) | whoever composed the authorize URL | **Not a problem.** Never sent to SingleKey, never enters the browser if the daemon composes the URL. |
| S2 | `state`, `nonce` (Home Connect leg) | generated locally (`@255c731:95-96`), in the authorize query (`:88-99`) | same | **Not a problem**, as S1. |
| S3 | Home Connect **`sessionId`** | scraped from the authorize response HTML by regex `"sessionId" value="(.*?)"` (`@255c731:111-114`); hard failure if absent (`:112-113`) | `api.home-connect.com` | **Blocked for (i).** It exists only inside a document the browser loaded from a foreign origin. Consumed by S4. |
| S4 | SingleKey-leg `state` = `{"session_id":"<S3>"}` | `@255c731:135` (*"important: no spaces!"*) | composed from S3 | Inherits S3's problem. Still current: matches probe P1 (§5.3). |
| S5 | Home Connect **`sessionData`** | scraped at `@255c731:115-118`, hard failure if absent | `api.home-connect.com` | **Scraped and never used** — the identifier appears nowhere after assignment. The old flow required it to be present and carried nothing. A reimplementation need not carry it. |
| S6 | Cookies for **`api.home-connect.com`** | set by the authorize `GET` into the shared jar (`@255c731:45-46`, `:105`) | `api.home-connect.com` | **Blocked for (i)** (§5.8.2). Names and flags are not determinable from the code: nothing reads the jar, and `requests` does not expose `HttpOnly`/`SameSite`. |
| S7 | Cookies for **`singlekey-id.com`** | accumulated across the redirect loop (`:144`), the identifier POST (`:169`), the password `GET` (`:182`), the password POST (`:186`) and the callback loop (`:199`) | `singlekey-id.com` | **Blocked for (i)**, on a second host — so (i) must move cookies for two unrelated origins. Names beyond S8 and flags unknown. |
| S8 | **`X-CSRF-FORM-TOKEN`** cookie, echoed into a **`RequestVerificationToken` header** | read at `@255c731:161-162`, merged into the session headers at `:163` | `singlekey-id.com` | Blocked for (i) as part of S7, and half of an ASP.NET-style antiforgery pair with S9 — the two must stay matched, which a split across two HTTP clients makes fragile. |
| S9 | **`__RequestVerificationToken`** hidden form field | parsed from the HTML separately on the identifier page (`:167-168`) and the password page (`:183-184`), posted with each form (`:173`, `:191`) | `singlekey-id.com` | Two distinct values, one per form. Requires parsing pages the daemon itself fetched. |
| S10 | **`ReturnUrl`** | `parse_qs` of the final preauth URL after the redirect loop (`@255c731:157-158`); polled at `:196-205` until a `Location` starts `hcauth://` | `singlekey-id.com` | **Independently blocked for (i)**: a cross-origin window's URL is unreadable, like its cookies (§5.8.2). Even a cookie transfer would not give (i) the URL to poll. |
| S11 | Relative-`Location` resolution against `https://singlekey-id.com` | `@255c731:150-151`, `:179-180`, `:197-198` | — | Behaviour, not state, but the chain does not complete without it. |
| S12 | SingleKey-leg constants — client id `11F75C04-21C2-4DA9-A623-228B54E9A256`, `redirect_uri=…/security/oauth/redirect_target`, `scope`, `prompt=login`, `style_id=bsh_hc_01` | `@255c731:129-134` | constants | **Not a problem.** Unchanged per probe P1 (§5.3). |
| S13 | **e-mail** and **password** | posted as `UserIdentifierInput.EmailInput.StringValue` (`@255c731:172`) and `Password` (`:189`, `RememberMe` at `:190`) | the human | Crosses **into the daemon** under (j), not at all under (i). §5.9. |
| S14 | `redirect_target=icore` on the authorize request | `@255c731:98`, removed in `a40940c` | constant | Made S3/S5 scrapeable by selecting Bosch's older login page. Do not send it (§7 D9). |
| S15 | `code`, `state`, `grant_type` from the callback | `@255c731:221-223`; observed in §5.8.0 item 2 | `singlekey-id.com` → `Location` header | The only state the daemon needs out of the chain. |
| S16 | **A CAPTCHA response token** | **no citation exists** — the deleted code predates the CAPTCHA and its password POST (`@255c731:186-194`) has no such field | the challenge provider | §5.8.3. Field name, binding and validation are unknown from source. |

**The set is a lower bound**: S6/S7 are "whatever the two hosts set", enumerated by behaviour rather
than name. **The primary reference adds nothing to it**; it never touches this leg. It uses
Electron's persistent `session.defaultSession` jar (`main.js:160`, `:178`), encrypted via the
`EnableCookieEncryption` fuse (`forge.config.js:54`), and `prompt=login` makes the persistence moot.

### 5.8.2 Option (i) is blocked by the web platform

Two independent rules, either of which alone is fatal *(platform specification)*:

- **Cookies are scoped to their own host, and `document.cookie` exposes only the current document's
  cookies.**
  - **Storage** is keyed by the cookie's `Domain`/`Path`: RFC 6265 **§5.3**, with matching rules at
    **§5.1.3** (domain) and **§5.1.4** (path).
  - **The `document.cookie` getter is defined in the HTML Standard**, which delegates to RFC 6265
    **§5.4** to compute the cookie-string for the document's URL as a **non-HTTP API** — so it
    serialises only cookies matching the current document.
  - **`HttpOnly`** is defined at RFC 6265 **§4.1.2.6**; §5.4 excludes `http-only` cookies from the
    cookie-string for a non-HTTP API, **even same-origin**.

  A page served from Kessel's origin cannot see S6 or S7 at all — they are not in its cookie store —
  and an identity provider's session cookie is very likely `HttpOnly` too *(the flags on S6/S7 are
  unverified)*.
- **A cross-origin `Window` and its `Location` are opaque to script.** The HTML Standard exposes a
  fixed allowlist on a cross-origin `Window` (`window`, `self`, `location`, `close`, `closed`,
  `focus`, `blur`, `frames`, `length`, `top`, `opener`, `parent`, `postMessage`) and, on a
  cross-origin `Location`, **only `href` as a setter and `replace()`**. Reading `href` throws. A
  popup or `<iframe>` on `singlekey-id.com` yields neither its cookies nor its URL — which rules out
  S10 as well.

These hold for every origin; nothing was tested against `singlekey-id.com`. Check the cited sections
in RFC 6265 and the HTML Standard directly.

**A third objection, independent of the platform:** (i) has nothing left to do. The chain the daemon
would continue is the loop at `@255c731:196-205`, and in a browser those hops happen automatically
once the password POST succeeds — §5.8.0 item 1 shows the browser reaching `hcauth://auth/prod?code=…`
by itself. The code has then been issued, and it is single-use (RFC 6749 §4.1.2: *"The client MUST
NOT use the authorization code more than once"*), so a daemon re-polling `ReturnUrl` races a spent
grant. **Whenever (i) has anything to hand over, option (c) already has the code.**

**Variants that escape the platform rule:**

| Variant | Escapes the rule? | Why it is not a design |
|---|---|---|
| **A browser extension** with cookie and host permissions for `singlekey-id.com` | **Yes** — extension cookie APIs bypass the same-origin policy and can read `HttpOnly` cookies. | A per-browser, store-reviewed extension whose only function is to exfiltrate an identity provider's live session to a local process — indistinguishable in shape from a credential stealer. A web page cannot install one; a container deployment cannot use one. |
| **The human copies cookies out of devtools** | **Yes** — devtools shows `HttpOnly` cookies. | Worse UX than (c)'s single paste, and it hands the daemon a live account session rather than a one-shot code. |
| **The daemon as an HTTP proxy with a locally trusted CA** | **Yes** — it reads the wire. | Requires installing a CA on the host and deliberately intercepting TLS to an identity provider; no host desktop exists in a container deployment. |
| **Let the browser finish and return the *code*** | The rule never applies. | This is option (c) (or (h)), not (i). |
| **A Kessel-driven browser over CDP / WebDriver** | **Yes** — a driver reads cookies (including `HttpOnly`) and cross-origin URLs from the browser it controls. | This is option (d): it needs a real browser on the host, which a container does not have, and once Kessel drives the browser it owns the CAPTCHA leg — option (a) again. |

**Verdict (i): blocked.** The state (i) must move — two hosts' session cookies and the URL to poll —
is exactly what the web platform makes unreadable to script on a foreign origin, and every escape
either stops being a cookie handover or is option (c) under another name.

### 5.8.3 Option (j): two layers, only one cheaply testable

In (j) the challenge is triggered by the backend's session and solved by the user in Kessel's
frontend.

**(j) has no state-crossing problem.** Every row of §5.8.1 except S13 and S16 stays inside the
daemon: S3/S5 because the daemon makes the authorize request, S6–S9 because it *is* the HTTP client,
S10 because it runs the redirect loop, S1/S2 because it composed the URL, and S15 because it holds
the session and therefore receives the `Location`. Only **credentials inbound** (S13, §5.9) and
**the challenge outbound with its solution inbound** (S16) cross. So (j) reduces to one question in
two layers.

#### Layer 1 — will the challenge render and issue a token on a foreign origin?

A modern CAPTCHA is not relayable as data. It is an interactive script with its own anti-automation,
so "the backend sends the captcha" can only mean *Kessel's page embeds the widget with Bosch's site
key, and the user's browser loads the challenge from the provider directly.* A daemon cannot fetch a
challenge and forward it as bytes.

What is known about the challenge:

| Question | Answer | Basis |
|---|---|---|
| Is a CAPTCHA present on the SingleKey pages? | **Yes**, as of December 2024. | `1ebae3b` (2024-12-23), *"…due to HCaptcha implementation on the SingleKey pages"*; `reference/hcpy/README.md:53`, `CHANGELOG.md:112`, `:119`. |
| Which vendor? | **"hCaptcha", asserted in prose by the author of `1ebae3b` in December 2024. Never as code.** | Those three lines are the only mentions of a CAPTCHA in either reference: `grep -rin captcha` over hcpy returns them and nothing else, and over the primary reference returns nothing. |
| Which vendor **today**? | **Unknown.** | The login script has been touched twice since (`a40940c`, `f344bd7`, §5.2) without going near the challenge. Nothing in either repository observes the current challenge. |
| Is a **site key** visible in the references? | **No.** | `grep -rinE 'sitekey|site_key|recaptcha|turnstile|h-captcha'` over both repositories returns nothing. |
| Which mode (visible, invisible, passive risk-scoring)? | **Unknown**, and it matters: an invisible or passive mode binds to the page's own JS environment far more tightly than a checkbox, and there may be nothing for a human to solve. | — |

Layer 1 cannot be set up from the code; its prerequisite is one Bosch request (E1, §5.8.4).

#### Layer 2 — would Bosch accept a token solved on a foreign origin?

**The references reveal nothing about how the token is submitted or validated.** The deleted
automation predates the CAPTCHA: its password POST carries `Password`, `RememberMe` and
`__RequestVerificationToken` and no challenge field (`@255c731:186-194`). The primary reference
never sees the form.

Three candidate mechanisms decide whether the question is even knowable from outside (none verified;
the provider's public verification API documentation is the source to consult):

- **(M-a) The provider enforces it.** The widget refuses to issue a challenge for a site key on a
  host outside an allowlist. Then layer 1 answers layer 2, and E2 settles (j) with no credentials and
  no Bosch request.
- **(M-b) The provider *reports* it and the relying party decides.** The verification response
  includes the hostname the challenge was solved on, and checking it is SingleKey's choice. Then
  nothing observable from outside answers layer 2; only E3 does.
- **(M-c) There is no host signal at all.** A token solved on Kessel's page is indistinguishable from
  one solved on Bosch's, so layer 2 is not a gate. From outside, (M-c) looks the same as (M-b) with
  the check disabled.

**Design against (M-b) until (M-a) is demonstrated**: assuming (M-a) means trusting a cheap experiment
that may be silent. (M-c) does not change that — a design cannot rely on a check being absent, and an
unchecked condition can start being checked at any time (the same class of risk as the second kill
switch in §5.8.6).

The site-key binding is uncontroversial (a challenge is issued *for* a site key). Whether the
*hostname* is validated is exactly the (M-a)/(M-b)/(M-c) split, and is not established here.

### 5.8.4 The three experiments

None has been run. Their costs differ by orders of magnitude.

- **E1 — identify the challenge. One unauthenticated `GET` chain; prerequisite for the rest.**
  Follow redirects from the authorize URL (probe P1's shape, §5.3) to the SingleKey identifier page
  and record **only** response headers — `Content-Security-Policy`, `X-Frame-Options`, and
  `Set-Cookie` **names and flags, values redacted** — plus grep hits for
  `hcaptcha|recaptcha|turnstile|sitekey|data-sitekey|h-captcha`. No form submission, no credential,
  no CAPTCHA interaction. *Yields:* the vendor, mode, site key, the S16 field name, the S6/S7 flags,
  and — from `X-Frame-Options`/`frame-ancestors` — whether (i)'s iframe variant is even framable.
  *Cost:* one request to a BSH-operated host.
- **E2 — layer 1. No credentials, no Bosch endpoint, but needs E1's site key.** Serve a static page
  from a non-Bosch origin that loads the provider's widget script with Bosch's site key, and observe
  only whether the widget initialises or refuses with a site-key/domain error. **Do not solve it.**
  If the provider has a public site-config preflight keyed by site key + host, that call alone may
  answer layer 1 — check the provider's documentation for one. *Cost:* near zero after E1.
- **E3 — layer 2. A real login with real credentials on the account owner's own account.** Complete
  an authentication through a Kessel-origin-hosted challenge and observe whether SingleKey accepts
  the token. Under (M-b) this is the only way to know. *Cost:* a real account, a real password and a
  completed real authentication.

### 5.8.5 Verdict (j)

**Viable-but-costly if layer 2 passes, blocked if it fails, and not resolvable from code.**

(j) is the only hybrid with no state-crossing problem, so it turns entirely on whether SingleKey
accepts a challenge token solved on a foreign origin — a server-side decision outside Kessel's
control, unknowable without a real login.

Two things hold regardless of E3's outcome:

1. **The password reaches the daemon** (S13), which makes §5.9 necessary.
2. **§5.8.6.** Even a passing E3 buys a design whose largest cost is permanent.

### 5.8.6 Fragility: option (j) gives up the browser-hosted designs' main advantage

§6 F1 is the best property of the browser-hosted designs: when the login pages change, Kessel has no
code in that leg to break. It is what hcpy bought by deleting 121 lines. A daemon that drives the
form flow and re-serves its interactive parts owns a reimplementation of a third party's login UI,
and inherits every change to it:

- **The UI is `htmx`-driven** (§5.8.0 item 5). A proxy must relay fragment exchanges, and the S8/S9
  antiforgery pairing is per-fragment. A widget-layer version bump is a plausible breakage.
- **Any new interactive step breaks it**: 2FA, a consent screen, an e-mail verification
  interstitial, a locale or cookie-consent gate, a passkey rollout (§8 G1 cannot rule any out).
- **The challenge vendor can change** — it already changed once (§6 F0). Under (j) that is a Kessel
  outage; under (c)/(e)/(g) it is invisible.
- **Two HTTP clients, one session** *(inference)*: the daemon makes the form requests while the
  browser solves the challenge, so user agent and TLS fingerprint differ within one authentication.
  Risk scoring exists to notice that — a second server-side kill switch, independent of layer 2,
  that can be turned on without notice.
- **`prompt=login`** (§2) means every run repeats the whole sequence; nothing is amortised.

Neither hybrid changes the CAPTCHA's existence, and accepting the password does not revive option
(a).

## 5.9 Credential handling for a design that receives the password

This applies only to a design in which the daemon receives the SingleKey ID password — today, only
option (j).

| Design | Password in the daemon? |
|---|---|
| (c), (e), (g), (h) | **No.** The user types it into their own browser, on Bosch's origin. |
| (i) | **No.** The login happens in the browser; (i) moves session cookies, not the password. Blocked anyway (§5.8.2). |
| **(j)** | **Yes.** The user types it into Kessel's page and the daemon POSTs it to SingleKey (S13). |
| (a) | Yes. Ruled out by the CAPTCHA (§5.2). |

### The secret class, and the requirements

**Cloud-account credentials** are a different and higher-value class than appliance key material:
they unlock the Bosch account, including appliance control and account data beyond Kessel's scope,
and passwords are commonly reused. Members:

**the SingleKey ID password; the authorization `code`; the `access_token`; any `refresh_token` or
`id_token`; the entire token JSON response; and the account `hcId`.**

Requirements:

1. **Never persisted** — not to a file, config field, environment variable or database. Held only
   for the exchange that consumes it: the password for one form POST, the code for one token
   exchange, the token for the 4–6 requests of one onboarding run.
2. **Never logged, at any level** — not `trace`, not behind a feature flag, not in a `Debug` impl.
   Wrap at the receive boundary in `hc_crypto::Secret`.
3. **Never emitted into a trace event or an SSE frame.**
4. **Carried only over the loopback API, never in a URL or query string.** The authorization `code`
   *arrives* in a URL by construction (the callback query), so the rule is that Kessel never itself
   places one in a URL it generates, logs, redirects to or echoes back.
5. **Do not follow the reference's example** of printing key material, the token JSON and `hcId`
   (§5.6).

### The leak sweeps have no needle for this class

Kessel's leak-sweep tests — `crates/hc-transport/tests/frame_event_leak_sweep.rs`,
`crates/hc-transport/tests/debug_redaction.rs`, `crates/hc-transport/tests/tls_psk_redaction.rs`,
`apps/server/tests/sse_leak_sweep.rs` — build their needles from the fixture corpus's synthetic
**appliance** identifiers only. No sweep carries a needle for a password, an authorization code, or
an access or refresh token, so **a leak of this class would pass every existing test.** Before any
code that handles these credentials lands, extend the sweeps with a synthetic password and a
synthetic JWT.

## 6. Fragility surface

What breaks this flow, roughly in order of likelihood *(estimate)*. An in-app downloader should be
optional, with paste-the-ZIP always available; this is what to fail *gracefully* on.

| # | Change | Detected as | Graceful failure |
|---|---|---|---|
| **F0** | **Already happened.** SingleKey ID deployed a CAPTCHA in late 2024, ending programmatic login | hcpy's automation stopped working; 121 lines were deleted (§5.2) | A precondition, not a future risk; listed first so nobody designs option (a). The corollary risk is the CAPTCHA's scope widening — e.g. to the authorize endpoint itself, which today answers without one (§5.3) — which would break even the browser-hosted designs. |
| F1 | The login pages change again (consent step, 2FA, stricter CAPTCHA) | nothing — it happens in the user's browser | **Free.** Kessel has no code in that leg. Both references share this property. |
| **F1b** | The authorization code expires before the user's paste | token exchange returns 4xx on a code that was valid seconds earlier | hcpy suggests under 60 s while saying there is "time to perform this step without too much haste"; the true expiry is unmeasured (`reference/hcpy/README.md:56`; §8 G3b). Under option (c), retry must be one click that opens a *fresh* authorize URL, never a retry of the same code. |
| F2 | `redirect_uri` / `client_id` de-registered, or the Android app's OAuth config rotated | authorize endpoint 4xx, or no `hcauth://` navigation | Make the constants overridable by config, and fail with "the downloader is broken, paste your ZIP". |
| F3 | The `hcauth://auth/prod` redirect target changes | the user finds no matching request | As F2; match "the URL containing a `code` parameter", nothing tighter. |
| F4 | Token response shape changes | `access_token` missing | Hard fail with a clear message; do not guess. |
| F5 | A regional host or path version bumps (`/v2/`, `/v1/iddf/`) | 404 | Make all five URLs configurable; §2's table is the default. |
| F6 | `paired-appliances` field renamed (`haType`, `vib`, `mac`, `isDemo`) | absent field | **Skip that appliance, not the run.** `haId`, `haType`, `brand`, `vib` and `key` are required; `serialNumber` and `mac` are optional in `hc-profile`. The reference is less robust: `main.js:570-572` lowercases `brand` and `vib` and strips `-` from `mac` to build the filename, so a missing `mac` throws before any file is written. |
| F7 | `encryption-information` returns neither `tls` nor `aes` (demo, or newly paired appliance) | both sub-objects absent | Skip that appliance with a reason, keep the others (`main.js:243-250`, `:324-328`). |
| F8 | Wrong region chosen | `401` on `paired-appliances` | Say "wrong region" (`main.js:392-393`), never "login failed"; or probe the regions as hcpy does (`reference/hcpy/hc-login.py:142-149`). |
| F9 | IDDF archive member naming changes | manifest declares a member that is not there | Avoided by reading member names from the archive (§4.2). |
| F10 | Token expires mid-run | 401 on a later call | The run is 4–6 requests; re-run onboarding rather than implementing refresh (§8 G2: nobody knows whether a refresh token exists). |

**Two things in the reference not to reimplement:**

- Printing appliance `key` and `iv`, the token JSON and `hcId` (§5.6).
- Writing the profile ZIP into the OS temp directory (`main.js:621-630`) and serving a file listing
  from it (`main.js:82-102`). Keep the bytes in memory and hand them to `hc-profile`; a profile ZIP
  in `/tmp` is a psk in `/tmp`.

---

## 7. Where the two implementations disagree

With two implementations there is **no majority to break a tie**; each row is an open question
unless observation settled it. On the login leg the two share a contributor (§0), so agreement there
is worth less too.

hcpy's login is a **degraded remnant** of an automated implementation the CAPTCHA ended (§5.2); the
Electron tool never had automation to lose. Where hcpy is thinner or odder, the usual explanation is
amputation rather than disagreement — D2 and D9 are both examples.

| # | Point | `homeconnect-profile-downloader` | `hcpy` | Note |
|---|---|---|---|---|
| **D1** | API host for non-EU regions | `api-rna.home-connect.com` (NA), `api.home-connect.cn` (CN) (`main.js:117`, `:121`) | `api.home-connect.com` for everyone; only the asset host varies (`reference/hcpy/hc-login.py:48-52`) | Both cannot be right unless the EU API host also serves NA accounts. Needs a non-EU account (§8 G4). Irrelevant for EU accounts. |
| **D2** | Authorize-leg redirect parameter name | `redirect_url` (`main.js:141`) | `redirect_uri` (`reference/hcpy/hc-login.py:89`) | Both send `redirect_uri` on the token leg (`main.js:355`, `:116`). RFC 6749 says `redirect_uri`. **Settled by observation (§5.3): send `redirect_uri`** — the probes used it, and the server both accepted the registered value (P1) and enforced validation on it (P3). Why `redirect_url` also works is unresolved: either the server falls back to the client's single registered URI when the parameter is absent, or it accepts both spellings. The probes never sent `redirect_url`, and nothing depends on the answer. |
| **D3** | `brand` casing | uppercased into the manifest (`main.js:307`), left as-is for the hcpy target's host (`main.js:281`) | used verbatim to compose the TLS host (`reference/hcpy/hc-login.py:189`) | `.toUpperCase()` implies the API does not return uppercase. `hc-profile`'s `derived_host()` uses `brand` verbatim, so the case here changes the derived hostname's case; mDNS is case-insensitive, so probably harmless *(inference)*. **Uppercase it**, to match the tool's output. |
| **D4** | Scope requested | all 10 (`main.js:17`) | `ReadOrigApi` only (`reference/hcpy/hc-login.py:65-67`) | hcpy works, so **`ReadOrigApi` alone suffices for steps 6–8**. Requesting `Control`, `WriteAppliance`, `DeleteAppliance` and `Settings` for a read-only download is a needlessly alarming consent screen. **Request `ReadOrigApi`** and widen only on a 403. Probe P1 sent `scope=ReadOrigApi` and got 302, not an invalid-scope error (§5.3). |
| **D5** | Locating the XML members in the IDDF archive | composes `{haId}_…` on the profile path (`main.js:310-311`), suffix-matches on the hcpy path (`main.js:456-462`) | opens `{haId}_…` by construction (`reference/hcpy/hc-login.py:212-213`) | Suffix-matching is more robust (§4.2). |
| **D6** | Demo appliances | filtered on `isDemo` (`main.js:230-238`) | not filtered | No conflict. Filter. |
| **D7** | Region failure handling | user picks the region; 401 → "wrong region" (`main.js:392-393`) | probes EU then NA, takes the first 200 (`reference/hcpy/hc-login.py:142-149`) | Probing is friendlier and costs one request. Either is defensible. |
| **D8** | `state` validation | not read back (`main.js:164`) | read and never checked (`:103`, `:106`) | Both wrong (§3 step 3). With the code arriving by paste, the check matters more. |
| **D9** | The `redirect_target=icore` authorize parameter | never sent | **sent historically** (`hc-login.py@255c731:98`), removed in `a40940c` | An amputation, not a disagreement. It selected Bosch's older, scrapeable login page that the dead automation depended on. **Do not send it**; whether it is still honoured is unprobed. |

---

## 8. Gaps — what only a real login can answer

Each row is phrased as the experiment that would close it.

| # | Unknown | The experiment |
|---|---|---|
| **G1** | Whether a consent step or 2FA can appear after the SingleKey hop, and — the one that matters — **what a current browser leaves copyable after failing to open `hcauth://…`** (address bar? error page? nothing?) | One real login in Chrome and in Firefox, recording three things: whether the failed `hcauth://` URL is copyable; whether the preceding `…/security/oauth/redirect_target` URL is; and whether the failed navigation leaves a copyable history entry. It bounds the quality of option (c), not its existence (§5.5). |
| **G2** | The token response beyond `access_token`: `refresh_token`, `expires_in`, `id_token`? The access token's lifetime? | Print the token response's *key set* (never its values) once. Low value: the token serves one 4–6 request run. |
| **G3b** | The authorization code's true expiry (`reference/hcpy/README.md:56`) | Time one exchange. Matters for option (c) (§6 F1b), not for (g). |
| **G3c** | Whether the CAPTCHA is shown unconditionally or only on a risk signal | **Not worth pursuing**: a sometimes-absent CAPTCHA is no foundation, and probing for its absence is the behaviour §5.7 rules out. |
| **G4** | D1: which API host serves NA and CN accounts | Needs a non-EU account. |
| **G5** | The full `paired-appliances` and `encryption-information` response shapes — extra fields, nullability, whether `appliances` can be absent rather than empty | Log the response *key sets* once. |
| **G6** | Whether the mobile-Chrome UA (`main.js:178`) is required | The §5.3 probes all sent it, so they show it is sufficient, not necessary. One authorize request with a desktop UA settles it. |
| **G8** | **Which challenge the SingleKey pages use today, in what mode, with what site key** (§5.8.3) | **E1** (§5.8.4). Also yields the S6/S7 cookie flags and whether (i)'s iframe variant is framable. |
| **G9** | Whether a challenge for Bosch's site key initialises on a foreign origin (layer 1, §5.8.3) | **E2**; depends on G8/E1 for the site key. |
| **G10** | Whether SingleKey accepts a challenge token solved on a foreign origin (layer 2, §5.8.3), and whether the provider enforces the host, reports it, or carries no host signal ((M-a)/(M-b)/(M-c)) | The mechanism needs the provider's public verification documentation. Under (M-b) only **E3** settles it. Assume (M-b) until (M-a) is shown. |
| **G11** | **Cookie names and flags** (`HttpOnly`, `Secure`, `SameSite`) for `api.home-connect.com` and `singlekey-id.com` (S6/S7) | Falls out of E1. Matters only if (i) is reconsidered, and (i) is blocked for two other reasons (§5.8.2). |

Closed: whether a loopback `redirect_uri` is accepted **for this `client_id`** — no (§5.3). That does
not cover a differently registered client (§5.5 (f)).

---

## 9. Errors in secondary sources

- **`reference/go-homeconnect2mqtt/docs/03-profile-format.md` is wrong in two places.** `:12-14` says
  "A **ZIP** is downloaded that contains three files per registered device", implying one archive per
  account; the tool writes **one ZIP per appliance**, each with three members (`main.js:562-585`, a
  fresh `zipFilePath` per loop iteration). Its field table at `:38-51` lists a `model` key the tool
  never writes (`main.js:303-323`) and `hc-profile`'s `RawManifest` does not have — apparently
  inherited from `homeconnect_local_hass`, which that document says it was reconstructed from (`:5`).
  Neither error affects Kessel.
- **`reference/hcpy/README.md:64`** shows a sample authorize URL still ending
  `&redirect_target=icore`, a parameter the code stopped sending in `a40940c` (§7 D9). Do not copy
  it.
