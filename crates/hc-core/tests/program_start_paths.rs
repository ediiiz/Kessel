//! Device-specific program-start paths, construction only: these build the command shape per
//! appliance class and send nothing. They assert the resource, the `validate` flag and the uid,
//! which are what distinguish the paths.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use hc_core::write::{
    DeviceRole, FINISH_IN_RELATIVE_UID, StartRequest, WriteError, check_access, start_command,
};
use hc_profile::Access;

const fn request() -> StartRequest {
    StartRequest {
        program_uid: 0x1234,
        start_option_uid: 0x022E,
        relative_time: 0,
    }
}

#[test]
fn hob_starts_via_selected_program_validate_false() {
    let command = start_command(&DeviceRole::Cooktop, &request()).expect("hob has a start path");
    assert_eq!(
        command.resource, "/ro/selectedProgram",
        "the cooktop starts via a direct selectedProgram write"
    );
    assert!(
        !command.validate,
        "the cooktop selectedProgram write carries validate=false"
    );
    assert_eq!(
        command.target_uid,
        Some(0x1234),
        "the selectedProgram write targets the program uid"
    );
}

#[test]
fn dishwasher_starts_via_start_in_relative() {
    let command =
        start_command(&DeviceRole::Dishwasher, &request()).expect("dishwasher has a start path");
    assert_eq!(
        command.field.as_deref(),
        Some("StartInRelative"),
        "the dishwasher starts via StartInRelative"
    );
    assert_eq!(
        command.target_uid,
        Some(0x022E),
        "StartInRelative addresses the option uid from the model"
    );
}

#[test]
fn washer_dryer_starts_via_finish_in_relative_uid_551() {
    assert_eq!(
        FINISH_IN_RELATIVE_UID, 551,
        "the FinishInRelative option uid is 551"
    );
    for role in [DeviceRole::Washer, DeviceRole::Dryer] {
        let command = start_command(&role, &request())
            .unwrap_or_else(|_| panic!("{} has a start path", role.label()));
        assert_eq!(
            command.field.as_deref(),
            Some("FinishInRelative"),
            "{} starts via FinishInRelative",
            role.label()
        );
        assert_eq!(
            command.target_uid,
            Some(FINISH_IN_RELATIVE_UID),
            "{} FinishInRelative pins uid 551, not the caller's option uid",
            role.label()
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

/// The oven starts a programme through `/ro/selectedProgram`, like the cooktop. The fridge stays
/// unsupported because its description declares no programmes.
#[test]
fn oven_starts_via_selected_program_like_the_hob() {
    let command = start_command(&DeviceRole::Oven, &request()).expect("the oven has a start path");
    assert_eq!(
        command.resource, "/ro/selectedProgram",
        "the oven takes the direct program-select path, like the cooktop"
    );
    assert_eq!(
        command.target_uid,
        Some(request().program_uid),
        "the program uid is what the frame emits as `program`"
    );
    assert!(
        !command.validate,
        "validate=false on the direct path, as for the cooktop"
    );

    let fridge = start_command(&DeviceRole::Fridge, &request());
    assert!(
        fridge.is_err(),
        "the fridge declares no programs, so there is nothing to start: {fridge:?}"
    );
}
