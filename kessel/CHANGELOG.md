# Changelog

## [0.2.0](https://github.com/ediiiz/Kessel/compare/v0.1.0...v0.2.0) (2026-10-20)

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
