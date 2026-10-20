# Changelog

## [0.3.0](https://github.com/ediiiz/Kessel/compare/v0.2.1...v0.3.0) (2026-10-20)


### Features

* **labels:** name appliance languages in their own language ([a92bdd9](https://github.com/ediiiz/Kessel/commit/a92bdd98dcf29ac2024f3e344ad13dce85bb0383))


### Bug Fixes

* **mqtt:** keep entity states when the appliance only revises a description ([775a924](https://github.com/ediiiz/Kessel/commit/775a9243855a0846dcd2688d986a5ef372b7da70))
* **server:** stop promptly while the web UI is open ([ec4df60](https://github.com/ediiiz/Kessel/commit/ec4df601ea112487223232822563f7c0cccf8061))

## [0.2.1](https://github.com/ediiiz/Kessel/compare/v0.2.0...v0.2.1) (2026-10-20)

### Bug Fixes

* Switching pages in the add-on no longer shows "404: Not Found". Links now stay inside Home Assistant's ingress path, and pages change without a reload.

### Improvements

* **Add appliance** explains where the profile ZIP comes from, with a link to the Home Connect Profile Downloader.

## 0.2.0 (2026-10-20)

The first release of Kessel: local control of Home Connect appliances (Bosch, Siemens, Neff and other BSH brands) from Home Assistant, with no cloud in the loop.

### Features

* **Local connection** to dishwashers, ovens, cooktops and fridges over TLS-PSK or AES, using the appliance profile you download once with your Home Connect account.
* **Onboarding wizard** in the add-on panel: upload a profile, find the appliance on your network, test the connection.
* **Home Assistant over MQTT.** Each appliance is one device with its controls, sensors and buttons. Programmes start from a select, there is a Stop programme button, and faults show as problems. The broker is found automatically when the Mosquitto add-on is installed.
* **Dependable controls.** A setting the appliance refuses snaps back to its real value, and changing one setting never resets another.
* **Web UI** with a live device page, quick settings, a searchable list of every feature, and a frame inspector for troubleshooting.
* **Community catalogue** for feature names, icons and which entities are enabled, editable as TOML with editor completion.
* **Two channels:** the stable *Kessel* add-on, and *Kessel (beta)* built from every change on main.

### Safety

* Kessel can start programmes and change settings on any onboarded appliance, including ovens and cooktops. Heat-producing appliances always ask for confirmation in the web UI.
