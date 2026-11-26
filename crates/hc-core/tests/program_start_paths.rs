//! Program-start paths, construction only: these build the start command each appliance's own
//! description calls for and send nothing. They assert the resource, the `validate` flag and the
//! uid, which are what distinguish the paths.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use std::path::PathBuf;

use hc_core::write::{WriteError, check_access, start_command};
use hc_profile::{Access, Profile};
use serde_json::Value;

fn fixture_profile(name: &str) -> Profile {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../hc-profile/tests/fixtures")
        .join(name);
    Profile::from_zip_bytes(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn a_writable_active_program_starts_via_active_program() {
    // The dishwasher declares `<activeProgram access="readWrite">`: selecting only selects, and
    // the start is `POST /ro/activeProgram` with the programme and its options.
    let profile = fixture_profile("dishwasher-tls.zip");
    let options = vec![(0x022E, Value::from(600))];
    let command = start_command(profile.model(), 0x2000, options.clone());
    assert_eq!(command.resource, "/ro/activeProgram");
    assert_eq!(command.target_uid, Some(0x2000));
    assert_eq!(
        command.options, options,
        "the options travel with the start"
    );
    assert!(command.validate);
}

#[test]
fn a_read_only_active_program_starts_via_the_selection() {
    // The hob and the oven declare `<activeProgram access="read">`: their selection is the start,
    // a direct `POST /ro/selectedProgram` with `validate=false`.
    for fixture in ["hob-aes.zip", "oven-tls.zip"] {
        let profile = fixture_profile(fixture);
        let command = start_command(profile.model(), 0x1234, Vec::new());
        assert_eq!(command.resource, "/ro/selectedProgram", "{fixture}");
        assert_eq!(command.target_uid, Some(0x1234), "{fixture}");
        assert!(
            !command.validate,
            "{fixture}: the direct path is validate=false"
        );
    }
}

#[test]
fn access_none_is_attempted_rather_than_refused() {
    // `access="none"` means "unknown, ask the appliance", not "no": the fridge declares it on all
    // its settings and reports values for them anyway, and the appliance's own error response is a
    // better authority than a local guess.
    //
    // Across the four real descriptions it appears on 19 elements: 17 fridge settings,
    // `Dishcare.Dishwasher.Command.AbortProgram`, and the hob's `<protectionPort>` (not an entity).
    // The oven declares none, so this opens no hob or oven control.
    check_access(Some(&Access::None))
        .expect("access=none is attempted, not refused — the appliance answers");
}

#[test]
fn write_rejected_when_access_says_no() {
    // The access gate — distinct from, and earlier than, the device allowlist choke point. Default-
    // deny survives everywhere the appliance actually said something, or said nothing at all.
    assert!(
        matches!(
            check_access(Some(&Access::Read)),
            Err(WriteError::AccessNotWritable(_))
        ),
        "read-only is not writable"
    );
    assert!(
        matches!(check_access(None), Err(WriteError::AccessNotWritable(_))),
        "an absent access is refused (default-deny), not assumed writable — this is NOT the same \
         as `access=\"none\"`, which is the appliance declining to answer rather than the \
         description omitting the question"
    );
    assert!(
        matches!(
            check_access(Some(&Access::Other("mystery".to_owned()))),
            Err(WriteError::AccessNotWritable(_))
        ),
        "an unknown access word is refused, not guessed"
    );

    // The two writable words pass the access gate.
    check_access(Some(&Access::ReadWrite)).expect("readWrite is writable");
    check_access(Some(&Access::WriteOnly)).expect("writeOnly is writable");
}
