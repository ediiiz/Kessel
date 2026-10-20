//! The Supervisor declarations the add-on config must carry.
//!
//! - `hassio_api: true`: without it `POST /addons/self/restart` returns 403, and since the restart
//!   route answers 202 before it acts, the refusal is invisible in the browser.
//! - `services: [mqtt:want]`: without it `GET /services/mqtt` returns 403 and autodiscovery falls
//!   back to "no broker". `want` rather than `need`, so a host with no broker still starts.
//! - `mqtt_autodiscovery` in both `options:` and `schema:`, so the Supervisor validates it.
//!
//! No Rust code can enforce a packaging property, so these are text assertions over the YAML file.
//! Every test first checks it is reading the real add-on config, and commented-out lines never
//! count: the file mentions these keys in prose.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

mod common;

/// `kessel/config.yaml` as text, with its path for failure messages.
fn addon_config() -> (std::path::PathBuf, String) {
    let path = common::workspace_root().join("kessel").join("config.yaml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    (path, text)
}

/// The lines the Supervisor actually parses: no blanks, no comments.
///
/// Trailing whitespace is stripped so an assertion on an exact line is not defeated by one; leading
/// whitespace is **kept**, because it is what tells a block entry from a top-level key.
fn declared_lines(config: &str) -> Vec<&str> {
    config
        .lines()
        .map(str::trim_end)
        .filter(|line| {
            let bare = line.trim_start();
            !bare.is_empty() && !bare.starts_with('#')
        })
        .collect()
}

/// The indented entries under a top-level `key:`, trimmed — empty if the key is absent.
///
/// Deliberately naive about YAML: the file is hand-written and flat, and the test needs to decide
/// for itself how `#` is treated.
fn block<'a>(lines: &[&'a str], key: &str) -> Vec<&'a str> {
    let header = format!("{key}:");
    lines
        .iter()
        .position(|line| *line == header)
        .map(|start| {
            lines
                .iter()
                .skip(start + 1)
                .take_while(|line| line.starts_with(' ') || line.starts_with('\t'))
                .map(|line| line.trim())
                .collect()
        })
        .unwrap_or_default()
}

/// The non-vacuity check every test in this file runs first: this is the add-on config, and it is
/// the one the Supervisor reads.
fn assert_is_the_addon_config(path: &std::path::Path, lines: &[&str]) {
    assert!(
        lines.contains(&"slug: kessel") && lines.contains(&"ingress: true"),
        "this must be the Kessel add-on config, with `slug: kessel` and `ingress: true` declared \
         (not merely mentioned in a comment), otherwise every assertion below is vacuous — a moved \
         or renamed file would report a missing declaration that is in fact present, or an absent \
         one as fine: {}",
        path.display()
    );
}

/// `hassio_api: true`, without which the Settings page's restart button silently 403s.
#[test]
fn the_addon_requests_the_supervisor_api() {
    let (path, config) = addon_config();
    let lines = declared_lines(&config);
    assert_is_the_addon_config(&path, &lines);

    assert!(
        lines.contains(&"hassio_api: true"),
        "kessel/config.yaml must declare `hassio_api: true` at the top level, uncommented. Without \
         it the Supervisor answers `POST /addons/self/restart` with 403 and the Settings page's \
         restart button does nothing the operator can see — the route answers 202 before it acts, \
         so a refusal is indistinguishable from success in the browser \
         (apps/server/src/api.rs::perform_restart). This file mentions \
         the key in prose in several comments; only a real declaration counts. Found: {}",
        path.display()
    );
}

/// `services: [mqtt:want]`, without which autodiscovery cannot learn the broker.
#[test]
fn the_addon_wants_the_mqtt_service() {
    let (path, config) = addon_config();
    let lines = declared_lines(&config);
    assert_is_the_addon_config(&path, &lines);

    let services = block(&lines, "services");
    assert!(
        services.contains(&"- mqtt:want"),
        "kessel/config.yaml must declare a top-level `services:` block containing `- mqtt:want`, \
         uncommented. Without it `GET /services/mqtt` returns 403 and MQTT autodiscovery falls \
         back to \"no broker\" — the daemon starts cleanly and the appliances simply never appear \
         in Home Assistant (apps/server/src/hassio.rs). `want`, not `need`: `need` would refuse to \
         start a local bridge on a host that runs no broker, which is a supported way to use this \
         add-on. Declared services: {services:?} in {}",
        path.display()
    );
}

/// The autodiscovery option, and the schema entry that makes the Supervisor validate it.
#[test]
fn the_autodiscovery_option_is_offered_and_validated() {
    let (path, config) = addon_config();
    let lines = declared_lines(&config);
    assert_is_the_addon_config(&path, &lines);

    let options = block(&lines, "options");
    let schema = block(&lines, "schema");

    // `log_level` is in both blocks, so an empty or unfound block is a parse problem here rather
    // than a missing declaration.
    assert!(
        options.iter().any(|entry| entry.starts_with("log_level:"))
            && schema.iter().any(|entry| entry.starts_with("log_level:")),
        "the `options:` and `schema:` blocks must both be found and both carry `log_level`, \
         otherwise the assertions below cannot tell an absent option from an unparsed \
         block. options: {options:?}, schema: {schema:?} in {}",
        path.display()
    );

    assert!(
        options.contains(&"mqtt_autodiscovery: true"),
        "kessel/config.yaml's `options:` must offer `mqtt_autodiscovery: true`. It is on by default \
         on purpose: an operator who installs the Mosquitto add-on and starts Kessel gets their \
         appliances in Home Assistant having typed nothing at all. Remove it and the switch \
         disappears from the add-on's Configuration tab: `docker/entrypoint.sh`'s `read_option \
         mqtt_autodiscovery true` then always falls through to its default, so discovery stays on \
         and the operator's only way to turn it off is setting `KESSEL_MQTT_AUTODISCOVER` on the \
         container by hand. Found: {options:?} in {}",
        path.display()
    );
    assert!(
        schema.contains(&"mqtt_autodiscovery: bool"),
        "kessel/config.yaml's `schema:` must declare `mqtt_autodiscovery: bool`. An option the \
         Supervisor does not validate is one an operator can typo into silence: without the schema \
         entry the value is neither validated nor rendered as a toggle, and whatever the operator \
         typed reaches `docker/entrypoint.sh` and then \
         `apps/server/src/hassio.rs::should_autodiscover`, which treats any word that is not \
         `false`/`0`/`no`/`off` as \"on\" — so a typo means discovery stays on and nothing says so. \
         Found: {schema:?} in {}",
        path.display()
    );
}

/// No top-level key is declared twice, and no line that looks like a key is a mangled comment.
///
/// Common YAML parsers silently take the last duplicate, so a duplicated key works here and fails on
/// a duplicate-strict parser on someone's Supervisor.
#[test]
fn no_top_level_key_is_declared_twice() {
    let (path, config) = addon_config();
    let lines = declared_lines(&config);
    assert_is_the_addon_config(&path, &lines);

    let mut seen: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for line in &lines {
        // Top level only: an indented line is a block entry, and `- mqtt:want` is a sequence item.
        if line.starts_with(' ') || line.starts_with('\t') || line.starts_with('-') {
            continue;
        }
        if let Some((key, _)) = line.split_once(':') {
            *seen.entry(key.trim()).or_default() += 1;
        }
    }

    let duplicates: Vec<String> = seen
        .iter()
        .filter(|(_, count)| **count > 1)
        .map(|(key, count)| format!("{key} ({count}×)"))
        .collect();
    assert!(
        duplicates.is_empty(),
        "kessel/config.yaml declares a top-level key more than once: {duplicates:?}. Either an edit \
         duplicated a real key, or it mangled a comment into something that parses as one — the second \
         is what happened last time. Both are silent: the parsers here take the last value, so the file \
         works until it meets a strict one. In {}",
        path.display()
    );
}
