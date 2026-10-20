# kessel-sim

A development-only Home Connect appliance simulator. It serves one profile ZIP as a TLS-PSK
appliance on loopback, so Kessel can be run end to end (onboarding, live values, writes, MQTT
discovery) with no hardware. It never dials out.

## Run it

```sh
just sim                                    # the dishwasher fixture
just sim profile=crates/hc-profile/tests/fixtures/oven-tls.zip
cargo run -p kessel-sim -- --profile <zip> [--bind 127.0.0.1] [--port 8443] [--seed 1] [--access-words upper|camel]
```

TLS-PSK profiles only; an AES profile is refused. Logs go to stderr (`KESSEL_SIM_LOG=debug` for
more): connections, every request (action and resource), and every write with whether it was
accepted or refused.

## What it does

- TLS 1.2 with cipher list `PSK`, accepting any PSK identity with the profile's key, then a
  WebSocket at `/homeconnect`.
- The appliance side of the handshake, in the shape of the recorded hob session: `POST
  /ei/initialValues`, a `/ci/registeredDevices` NOTIFY, and answers to `/ci/services` (ro 1, ei 2,
  ci 2, ni 1), `/ci/authentication`, `/ci/info`, `/iz/info`, `/ni/info`, `/ro/allMandatoryValues`
  and `/ro/allDescriptionChanges`. Every identifier in those answers is synthetic.
- Initial values from the profile: an enumeration's first declared value, a number's `min` (or 0),
  `false`, `""`; no active program, the first program selected.
- Every 5 s a NOTIFY `/ro/values` moves one status: the remaining program time if the profile has
  one, else the program progress, else a boolean status.
- `POST /ro/values` always gets a RESPONSE with the request's `msgID`. A write to a declared,
  writable (`readWrite`, `writeOnly` or `none`), available feature with a valid value (a declared
  enum ordinal, a number within bounds, a bool for a boolean) is applied, answered with no `code`
  and followed by a NOTIFY with the new value. Anything else gets `"code":400` and changes nothing.
- After an accepted Setting/Option write, with probability 1/2 (reproducible from `--seed`), a
  NOTIFY `/ro/descriptionChange` flips a different option's `access` between `READWRITE` and
  `READ`, or its `available`. Access words are uppercase, as real appliances send them;
  `--access-words camel` sends the profile XML spelling instead.
- `POST /ro/selectedProgram` (or `/ro/activeProgram`) with `[{"program":uid,"options":[…]}]`
  selects and activates the program; `DELETE /ro/activeProgram` sets the active program to 0.

## Onboard it in Kessel

Run the daemon with state in a throwaway directory, so it never sees a real profile:

```sh
dir=$(mktemp -d) && cd "$dir"
KESSEL_PROFILE_DIR=$dir/profiles KESSEL_CONFIG_FILE=$dir/config.toml \
KESSEL_TOKEN_FILE=$dir/kessel.token \
  cargo run --manifest-path <repo>/Cargo.toml -p server --features tls-psk
```

Open the UI on `http://127.0.0.1:8099`, upload the same profile ZIP, and set host `127.0.0.1`,
port `8443`. Or through the API:

```sh
curl -X POST --data-binary @<zip> 'http://127.0.0.1:8099/api/onboarding/commit?slug=sim&host=127.0.0.1&port=8443'
curl http://127.0.0.1:8099/api/devices
```

The committed fixture ZIPs were scrubbed character by character, and their keys are not canonical
base64 (the last character carries non-zero padding bits). The simulator accepts them; Kessel's
client decodes strictly and refuses them with "the `key` is not valid URL-safe base64". Until
the fixtures or that decoder change, onboard a profile whose key is canonical.
