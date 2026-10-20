//! Applying `/ro` value-update items.
//!
//! An update for a `uid` the client does not know is logged at debug and skipped, never fatal.
//! The caller owns the entity table and supplies the set of known `uid`s.

use std::collections::BTreeSet;

use serde_json::Value;

/// The description fields an update item revised, as opposed to the value it carried.
///
/// An update item may carry `access`, `available`, `min`, `max` and `stepSize` alongside its
/// value, so an appliance can change what a feature is at run time.
///
/// Sparse: [`None`] means the item said nothing about that field, never that the appliance cleared
/// it. `access` is kept as the raw word; bounds are integers, and a fractional bound is dropped
/// rather than rounded. Not `#[non_exhaustive]`, so other crates' tests can build one in full.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DescriptionUpdate {
    /// `access`, verbatim — `readWrite`, `read`, `none`, or a word we have not seen.
    pub access: Option<String>,
    /// `available`.
    pub available: Option<bool>,
    /// `min`.
    pub min: Option<i64>,
    /// `max`.
    pub max: Option<i64>,
    /// `stepSize`.
    pub step: Option<i64>,
}

impl DescriptionUpdate {
    /// Whether this item revised anything about the description.
    #[must_use]
    pub const fn is_some(&self) -> bool {
        self.access.is_some()
            || self.available.is_some()
            || self.min.is_some()
            || self.max.is_some()
            || self.step.is_some()
    }
}

/// One applied value update: a known `uid`, its new value, and whatever it said about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueUpdate {
    /// The feature id.
    pub uid: u64,
    /// The new value, whatever JSON type the appliance sent, or [`None`] when the item carried no
    /// `value` and only revised the description. A reported `null` is `Some(Value::Null)`.
    pub value: Option<Value>,
    /// What this item revised about the feature's description. Empty for a plain value update.
    pub description: DescriptionUpdate,
}

/// Keeps the items whose `uid` is in `known`, logging and dropping the rest. Never fails: an
/// unknown or malformed item is skipped.
#[must_use]
pub fn apply_updates(known: &BTreeSet<u64>, items: &[Value]) -> Vec<ValueUpdate> {
    let mut applied = Vec::new();
    for item in items {
        let Some(uid) = item.get("uid").and_then(Value::as_u64) else {
            tracing::debug!("update item without a uid ignored");
            continue;
        };
        if known.contains(&uid) {
            applied.push(ValueUpdate {
                uid,
                value: item.get("value").cloned(),
                description: description_of(item),
            });
        } else {
            // A uid is a feature index, not an identifier; the value is not logged.
            tracing::debug!(uid, "update for unknown uid ignored");
        }
    }
    applied
}

/// Reads the description fields of one update item.
///
/// A key of the wrong JSON type is treated as absent, so a malformed bound leaves the previously
/// known bound in place.
fn description_of(item: &Value) -> DescriptionUpdate {
    DescriptionUpdate {
        // Verbatim, so an unrecognised word still reaches the layer that interprets it.
        access: item
            .get("access")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        available: item.get("available").and_then(Value::as_bool),
        min: item.get("min").and_then(Value::as_i64),
        max: item.get("max").and_then(Value::as_i64),
        step: item.get("stepSize").and_then(Value::as_i64),
    }
}
