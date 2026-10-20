//! Resolving an inbound MQTT topic to the feature it addresses.
//!
//! An inbound `…/set` is the one place a broker message can reach the write path. A topic parser
//! that resolved a fridge topic to the dishwasher would write the wrong value to the wrong
//! appliance, and nothing downstream would object. So the rule is exact prefix or nothing:
//! `<base>/<slug>/` must match, and anything else resolves to `None` rather than a best guess.
//!
//! Pure string work; no socket, no broker, no writer.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use server::mqtt::relative_set_topic;

const BASE: &str = "kessel";
const SLUG: &str = "dishwasher";

#[test]
fn an_inbound_set_topic_resolves_to_its_feature() {
    assert_eq!(
        relative_set_topic(
            BASE,
            SLUG,
            "kessel/dishwasher/BSH/Common/Setting/PowerState/set"
        ),
        Some("BSH/Common/Setting/PowerState/set"),
        "the device-relative path is what `handle_set` matches against a mapping entry's \
         command_topic — the base and slug segments are stripped and nothing else is"
    );

    // A nested feature path, because the contract's `<Feature/Path>` has no fixed depth.
    assert_eq!(
        relative_set_topic(BASE, SLUG, "kessel/dishwasher/a/b/c/d/set"),
        Some("a/b/c/d/set")
    );
}

#[test]
fn a_topic_for_another_device_or_another_base_resolves_to_nothing() {
    // Nothing downstream would object, so this is the only defence against writing a fridge value
    // to the dishwasher.
    assert_eq!(
        relative_set_topic(
            BASE,
            SLUG,
            "kessel/fridge/BSH/Common/Setting/PowerState/set"
        ),
        None,
        "another device's topic must not resolve — the guard cannot catch this, since both devices \
         are allowlisted and each writer would happily accept it"
    );
    assert_eq!(
        relative_set_topic(BASE, SLUG, "somewhere-else/dishwasher/x/set"),
        None,
        "another base's topic is not ours"
    );

    // A slug that merely starts with ours. `dishwasher-2` is what a second appliance of the same
    // class is called, so a prefix match on the slug rather than the whole segment would send one
    // dishwasher's writes to the other.
    assert_eq!(
        relative_set_topic(BASE, SLUG, "kessel/dishwasher-2/x/set"),
        None,
        "the slug must match a whole path segment, not a prefix of one"
    );
    // The base likewise.
    assert_eq!(
        relative_set_topic(BASE, SLUG, "kessel2/dishwasher/x/set"),
        None
    );

    // Nothing after the prefix is not a feature.
    assert_eq!(relative_set_topic(BASE, SLUG, "kessel/dishwasher/"), None);
    assert_eq!(relative_set_topic(BASE, SLUG, "kessel/dishwasher"), None);
    assert_eq!(relative_set_topic(BASE, SLUG, ""), None);
}

#[test]
fn only_a_set_topic_resolves() {
    // The subscription is per `…/set` topic, but a broker delivers whatever a retained message or a
    // stray publisher puts on a subscribed filter. A `…/state` topic resolving to a writable path
    // would turn Kessel's own state publish into a write command.
    assert_eq!(
        relative_set_topic(
            BASE,
            SLUG,
            "kessel/dishwasher/BSH/Common/Setting/PowerState/state"
        ),
        None,
        "a state topic is not a command; Kessel publishes to those itself"
    );
    assert_eq!(
        relative_set_topic(BASE, SLUG, "kessel/dishwasher/availability"),
        None
    );
}
