# `docs/`

[`protocol/`](./protocol/README.md) is the reference for the local Home Connect protocol as
Kessel implements it, reconstructed from three open-source reference implementations and checked
against real appliances where noted.

| File | Contents |
|---|---|
| [`protocol/README.md`](./protocol/README.md) | Index, citation convention, and the reference repositories with URLs and commit SHAs. |
| [`protocol/aes-chain.md`](./protocol/aes-chain.md) | AES-CBC + HMAC-SHA256 application-layer crypto: key derivation, the custom padding, the two continuous chains, framing and length rules. |
| [`protocol/tls-psk.md`](./protocol/tls-psk.md) | The TLS-PSK transport: identity, cipher suites, TLS version, verification posture, and the configuration verified against real appliances. |
| [`protocol/handshake.md`](./protocol/handshake.md) | The connection handshake step by step, which steps are conditional or may fail, the `/ci/services` 403 analysis, and appliance URL construction (including IPv6). |
| [`protocol/message-model.md`](./protocol/message-model.md) | The wire JSON: fields and order, compact separators, `data` as a list, type coercion, correlation and NOTIFY routing. |
| [`protocol/error-codes.md`](./protocol/error-codes.md) | The `code` → meaning table across the three references. |
| [`protocol/downloader.md`](./protocol/downloader.md) | The cloud-side SingleKey ID flow that produces an appliance profile ZIP. |
