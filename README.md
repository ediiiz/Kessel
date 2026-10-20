# Kessel

Local control for **Home Connect** appliances (Bosch, Siemens, Neff, Gaggenau) — no cloud, no
account at run time.

Kessel talks to each appliance directly over your LAN, keeps a live connection open, and exposes it
three ways:

- **Home Assistant** via MQTT discovery: sensors, switches, selects, numbers and buttons appear on
  their own, with stable entity ids you can build automations on;
- a **web UI** for onboarding, live values, settings and controls;
- a **frame inspector** that shows every protocol message in both directions, with secrets redacted.

Both of the protocol's transports are supported: TLS-PSK (`wss://…:443`, most appliances) and the
AES-encrypted WebSocket (`ws://…:80`).

## Install as a Home Assistant add-on

[![Add repository to Home Assistant](https://my.home-assistant.io/badges/supervisor_add_addon_repository.svg)](https://my.home-assistant.io/redirect/supervisor_add_addon_repository/?repository_url=https%3A%2F%2Fgithub.com%2Fediiiz%2FKessel)

1. **Settings → Add-ons → Add-on store → ⋮ → Repositories**, add `https://github.com/ediiiz/Kessel`.
2. Install **Kessel** and start it. For MQTT, install the **Mosquitto broker** add-on too — Kessel
   finds it on its own.
3. Open the web UI and onboard each appliance (see below).

Two channels are published:

| Add-on | Updates | For |
|---|---|---|
| **Kessel** | tagged releases | everyone |
| **Kessel (beta)** | every change on `main` that passes CI | trying fixes early |

Run only one of them at a time: both would connect to the same appliances and publish the same
entities.

## Onboarding an appliance

Kessel needs each appliance's **profile ZIP** once. It contains the appliance's keys and its feature
description. To get it:

1. On a computer, download
   [Home Connect Profile Downloader](https://github.com/bruestel/homeconnect-profile-downloader/releases/latest)
   for Windows, macOS or Linux. It is unsigned, so your system may warn before opening it.
2. Sign in with the account you use in the Home Connect app.
3. It saves one ZIP per appliance on that account.

In the web UI choose **Add appliance**, drop the ZIP, check the detected model, and enter the
appliance's IP address. Kessel tests the connection before saving it. Give appliances a fixed
address in your router; Kessel does not discover them.

The ZIP's keys are stored only in Kessel's data directory and never appear in logs, MQTT topics or
the inspector.

## Home Assistant

Each appliance becomes one device. Kessel publishes:

| Topic | Content |
|---|---|
| `kessel/<device>/availability` | `online` / `offline` (with a broker last will) |
| `kessel/<device>/connection_state` | connection state of the appliance |
| `kessel/<device>/<Feature/Path>/state` | current value, retained |
| `kessel/<device>/<Feature/Path>/set` | write a value (writable features only) |
| `kessel/<device>/_control/stop_program/set` | abort the running programme |

The everyday features (power, door, operation state, programme, remaining time, …) are enabled and
prominent; the long tail is filed under *Configuration* and *Diagnostic* and starts disabled, one
click away. Enumerations use the appliance's own names, so a select's options are what you write
back.

When you change a setting, Home Assistant shows the new value once the appliance confirms it. If the
appliance refuses a change (for example because it is not allowed in the current programme), the
control snaps back and the reason is logged and shown in the web UI.

> **Kessel can start programmes and change settings on any onboarded appliance, including cooktops
> and ovens.** There is no read-only mode. Treat its MQTT topics and web UI like the appliance's
> own control panel.

## Configuration

Settings live in the web UI (**Settings**); a restart applies them. Environment variables override
the stored settings:

| Variable | Default | |
|---|---|---|
| `KESSEL_PORT` | `8099` | HTTP port |
| `KESSEL_LOG` | `info` | log filter (`debug`, `info,hc_transport=debug`, …) |
| `KESSEL_MQTT_HOST` / `_PORT` / `_USERNAME` / `_PASSWORD` | — | MQTT broker |
| `KESSEL_MQTT_BASE` | `kessel` | topic root |
| `KESSEL_MQTT_DISCOVERY` | `full` | `full` or `curated` |
| `KESSEL_CONFIGURATION_URL` | the add-on panel | the device page's "Visit" link in Home Assistant |
| `KESSEL_REQUIRE_AUTH` | `false` | require the API token for writes |
| `KESSEL_CONFIG_FILE`, `KESSEL_PROFILE_DIR`, `KESSEL_TOKEN_FILE` | under `/data` in the container | storage paths |

Under the add-on, MQTT normally needs no configuration: with `mqtt_autodiscovery` on, Kessel asks the
Supervisor for Home Assistant's broker. A broker entered on the Settings page, or set through the
environment, takes precedence.

Each appliance appears in Home Assistant as one device, announced on
`homeassistant/device/kessel_<slug>/config`. Raw values are under `<base>/<slug>/…/state`, and
writable features take commands on the matching `…/set`. Upgrading from a release that announced
every entity separately moves those entities over without renaming them.

## Running without Home Assistant

```sh
docker run -d --name kessel -p 8099:8099 -v kessel-data:/data ghcr.io/ediiiz/kessel-amd64:latest
```

The API is unauthenticated unless `KESSEL_REQUIRE_AUTH=true`; don't expose the port beyond your LAN.

## Development

You need the Rust toolchain from `rust-toolchain.toml`, [just](https://github.com/casey/just) and
[Bun](https://bun.sh).

```sh
just check      # fmt, clippy, tests
just ci         # everything CI runs, including the web UI
just build      # web UI, then the server that embeds it
just sim        # a simulated appliance, so you can work without hardware
```

See [CONTRIBUTING.md](./CONTRIBUTING.md) for the layout of the code and the conventions, and
[`docs/protocol/`](./docs/protocol/) for the reverse-engineered Home Connect protocol.

## Acknowledgements

The protocol knowledge builds on [hcpy](https://github.com/hcpy2-0/hcpy),
[homeconnect_websocket](https://github.com/chris-mc1/homeconnect_websocket) and
[go-homeconnect2mqtt](https://github.com/SukramJ/go-homeconnect2mqtt). Home Connect is a
trademark of BSH Hausgeräte GmbH; this project is not affiliated with it.

## License

[MIT](./LICENSE)
