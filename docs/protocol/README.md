# The local Home Connect protocol

These documents describe the protocol Home Connect appliances speak on the local network,
distilled from three open-source reference implementations. They are meant to be enough to
implement a client without reading the references.

## Citations

Factual statements carry a citation of the form `reference/<repo>/<path>:<line>`, relative to a
checkout of the reference repositories listed below at the recorded commit. Statements that are
reasoning or extrapolation rather than something a source says are marked *(inference)*.
Behaviour confirmed against a real appliance is labelled as observed on hardware.

## Index

| File | Contents |
|---|---|
| [`aes-chain.md`](./aes-chain.md) | AES-CBC + HMAC-SHA256 application-layer crypto: KDF, custom padding, the two continuous chains, framing and length rules. |
| [`tls-psk.md`](./tls-psk.md) | How the TLS-PSK transport differs: identity, cipher suites, TLS version, verification posture, no application-layer crypto, and the configuration verified against real appliances. |
| [`handshake.md`](./handshake.md) | The handshake step by step, per-step conditionality and failure tolerance, the `/ci/services` 403 analysis, and URL construction. |
| [`message-model.md`](./message-model.md) | The wire JSON: field set, compact separators, `data`-is-a-list, type coercion, the `]"` → `]` workaround, correlation and NOTIFY routing. |
| [`error-codes.md`](./error-codes.md) | The `code` → meaning table across all three references, with discrepancies flagged. |
| [`downloader.md`](./downloader.md) | The cloud-side SingleKey ID login and profile download. Not part of the appliance protocol. |

## Reference implementations

The first three are the appliance-protocol references.


| Repo | URL | Commit | Commit date | Notes |
|---|---|---|---|---|
| `hcpy` | `https://github.com/hcpy2-0/hcpy` | `0f2136d39c15864e5a03a1ad5ade326525102313` | 2026-07-03 | The primary behavioural reference. |
| `homeconnect-websocket` | `https://github.com/chris-mc1/homeconnect_websocket` | `508963b94d7b55dc13d950be0e6a3919c8bff237` | 2026-04-04 | The project behind the `homeconnect_websocket` PyPI distribution (`reference/homeconnect-websocket/pyproject.toml:7`). Cited as `reference/homeconnect-websocket/`; the upstream package name uses an underscore. |
| `go-homeconnect2mqtt` | `https://github.com/SukramJ/go-homeconnect2mqtt` | `c8817afdde967ebf95461a6a4a057601cd0b7b6c` | 2026-07-28 | "Pure-Go bridge from local Home Connect appliances (Bosch/Siemens/Gaggenau/Neff)". Carries the `/ci/services` 403 analysed in `handshake.md`. |
| `homeconnect-profile-downloader` | `https://github.com/bruestel/homeconnect-profile-downloader` | `95404f1dc31c29c6d857bb33faa2850d5cf0df3c` | 2026-05-04 | Cited only by `downloader.md`, for the cloud-side profile download. |

Each was cloned with `git clone --depth 50 <url> reference/<dir>`; the SHA is the tip at clone
time, not a tag. The Go JSON encoder's key ordering (`handshake.md`) is cited from the source
rather than demonstrated by running it, and is marked *(inference)*.

## Provenance between the references

`reference/go-homeconnect2mqtt/docs/01-protocol.md:1-9` states that the Go repository's protocol
specification was "reconstructed from `chris-mc1/homeconnect_websocket` (Python, v1.5.3)". The Go
implementation therefore derives from homeconnect-websocket, not from hcpy: where Go differs from
hcpy, the difference is usually inherited from homeconnect-websocket, and the two Python clients
themselves disagree in several places. Three-way disagreements are tabulated rather than resolved
silently.

`reference/go-homeconnect2mqtt/docs/*.md` is treated as a secondary source, cited only where it
records an observation (such as a negotiated cipher suite) that the code does not show.
