//! `refCID` → content type: the fine-grained half of the type code, from which a unit and a Home
//! Assistant `device_class` are derived.
//!
//! The code table is third-party documentation, so it is checked against features in the fixtures
//! whose meaning is independently known from their name and bounds.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests may panic; library code may not"
)]

mod support;

use hc_profile::{ContentType, EntityModel};
use support::fixture_profile;

/// The entity with this dotted feature key, or a panic naming it.
fn entity_named<'m>(model: &'m EntityModel, key: &str) -> &'m hc_profile::Entity {
    model
        .entities()
        .values()
        .find(|entity| entity.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("the fixture declares no feature `{key}`"))
}

#[test]
fn a_ref_cid_decodes_to_the_content_type_its_feature_name_and_bounds_corroborate() {
    // Every row is a feature whose meaning is knowable without the table, from its name or its
    // declared bounds, so a wrong table shows up as a contradiction.
    let cases: [(&str, &str, ContentType); 7] = [
        // 07 → 7. "Celsius" is in the feature's own name.
        (
            "oven-tls.zip",
            "Cooking.Oven.Status.Server.Cavity.001.SetpointTemperatureCelsius",
            ContentType::TemperatureCelsius,
        ),
        // 10 → 16. "RemainingProgramTime" is a duration by name; the oven declares it in seconds.
        (
            "hob-aes.zip",
            "BSH.Common.Option.RemainingProgramTime",
            ContentType::TimeSpan,
        ),
        // 11 → 17. A progress is a percentage by name, and is declared 0..100.
        (
            "hob-aes.zip",
            "BSH.Common.Option.ProgramProgress",
            ContentType::Percent,
        ),
        // 12 → 18. A Wi-Fi signal strength is dBm; nothing else it could be.
        (
            "hob-aes.zip",
            "BSH.Common.Status.WiFiSignalStrength",
            ContentType::Dbm,
        ),
        // 13 → 19. Named `Weight`.
        (
            "oven-tls.zip",
            "Cooking.Oven.Option.Weight",
            ContentType::Weight,
        ),
        // 03 → 3. An enumeration; cross-checks `ValueType::Enumeration`.
        (
            "hob-aes.zip",
            "BSH.Common.Status.OperationState",
            ContentType::Enumeration,
        ),
        // 01 → 1. A boolean; cross-checks `ProtocolType`.
        (
            "hob-aes.zip",
            "BSH.Common.Setting.ChildLock",
            ContentType::Boolean,
        ),
    ];

    for (fixture, key, expected) in cases {
        let profile = fixture_profile(fixture);
        let entity = entity_named(profile.model(), key);
        assert_eq!(
            entity.content_type(),
            Some(expected),
            "{fixture}: `{key}` declares refCID={:?}, which must decode to {expected:?} — the \
             feature's own name and bounds say so independently of the table",
            entity.ref_cid
        );
    }
}

#[test]
fn an_absent_or_unknown_ref_cid_is_none_rather_than_a_guess() {
    // Gaps decode to `None`: a guessed `°C` on a feature that is not a temperature is worse than
    // no unit, because HA will record and graph it.
    assert_eq!(ContentType::of_ref_cid(None), None, "absent is not a guess");
    assert_eq!(
        ContentType::of_ref_cid(Some("")),
        None,
        "empty is not a guess"
    );
    assert_eq!(
        ContentType::of_ref_cid(Some("zz")),
        None,
        "unparsable is not a guess"
    );
    // 0 and 9 are gaps in the published table, 11..15 likewise.
    for gap in ["00", "09", "0B", "0F"] {
        assert_eq!(
            ContentType::of_ref_cid(Some(gap)),
            None,
            "refCID={gap} is a gap in the table and must decode to None, not to a neighbour"
        );
    }
}

#[test]
fn the_content_type_and_the_protocol_type_are_decoded_independently() {
    // `refCID=07` and `refCID=21` are both `ProtocolType::Float` but a temperature and a length,
    // so the coarse type cannot stand in for the fine one.
    let temperature = ContentType::of_ref_cid(Some("07"));
    let length = ContentType::of_ref_cid(Some("21"));
    assert_eq!(temperature, Some(ContentType::TemperatureCelsius));
    assert_eq!(length, Some(ContentType::Length));
    assert_ne!(
        temperature, length,
        "two codes that share a protocol type must not share a content type"
    );
    assert_eq!(
        hc_profile::ProtocolType::of_ref_cid(Some("07")),
        hc_profile::ProtocolType::of_ref_cid(Some("21")),
        "…and they do share the protocol type, which is what makes the second decode necessary"
    );
}
