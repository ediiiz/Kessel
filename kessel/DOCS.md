# Kessel

Kessel connects to your Home Connect appliances directly over your network and publishes them to
Home Assistant through MQTT. No cloud connection is used.

## Setup

1. Install the **Mosquitto broker** add-on if you don't have an MQTT broker yet, and make sure the
   MQTT integration is set up in Home Assistant.
2. Start Kessel and open its web UI from the sidebar.
3. Choose **Add appliance**, drop the appliance's profile ZIP and enter its IP address.

To get the profile ZIP, download
[Home Connect Profile Downloader](https://github.com/bruestel/homeconnect-profile-downloader/releases/latest)
on a computer, sign in with your Home Connect app account, and it saves one ZIP per appliance. The
ZIP holds the appliance's keys; Kessel stores them in its own data folder and never logs them. Give
each appliance a fixed IP address in your router.

Each appliance then appears as a device under **Settings → Devices & services → MQTT**.

## Options

| Option | Default | |
|---|---|---|
| `log_level` | `info` | Add-on log verbosity. Use `debug` to diagnose connection problems. |
| `mqtt_autodiscovery` | `true` | Use Home Assistant's MQTT broker automatically. |

Everything else is configured in the web UI under **Settings**: a different MQTT broker, the topic
root, how many entities to publish (*full* or *curated*), and whether writes require sign-in.
Settings take effect after a restart, which the page offers to do for you.

## Entities

The everyday features — power, door, operation state, selected and active programme, remaining time
and the like — are enabled and shown first. The many remaining settings and status values are
created under *Configuration* and *Diagnostic*, disabled; enable the ones you want.

Entity ids are derived from the appliance's feature names and stay the same across restarts and
updates, so they are safe to use in automations.

If an appliance refuses a change — some options can only be changed while a programme is selected,
for example — the entity shows the appliance's actual value again and the reason appears in the
add-on log and the web UI.

**Kessel can start programmes and change settings on any onboarded appliance, including cooktops
and ovens.** Anything that can publish to your MQTT broker can do the same.

## Troubleshooting

- **Appliance shows as unavailable:** check that it is powered and reachable at the IP you entered.
  The web UI shows the last connection error on the device page. Kessel retries on its own.
- **No entities in Home Assistant:** check the MQTT integration is configured and that the Settings
  page in Kessel shows a connected broker.
- **What did the appliance actually say?** The web UI's **Inspector** shows every message in both
  directions, with keys redacted.

## Data

Profiles, settings and the optional API token are stored in the add-on's `/data` folder and are
included in Home Assistant backups. Uninstalling the add-on deletes them.
