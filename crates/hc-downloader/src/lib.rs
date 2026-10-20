//! Profile downloader: `SingleKey ID` OAuth plus the profile fetch, behind
//! `feature = "downloader"`. Fragile by nature; pasting the profile ZIP is the permanent fallback.
//!
//! Currently a stub.

#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic; library code may not"
    )
)]

/// The crate's own name, as it appears in `[workspace.members]`.
pub const CRATE_NAME: &str = "hc-downloader";

#[cfg(test)]
mod tests {
    use super::CRATE_NAME;

    #[test]
    fn crate_name_is_stable() {
        assert_eq!(
            CRATE_NAME, "hc-downloader",
            "crate name is part of the workspace layout"
        );
    }
}
