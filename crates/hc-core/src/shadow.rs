//! The optimistic shadow of each device's values.
//!
//! A local write updates the shadow immediately so the dashboard reflects it before the appliance
//! confirms. The authoritative value arrives later as `NOTIFY /ro/values` and overwrites the entry;
//! the appliance is always right. The initial state is seeded from the post-CONNECT `/ro` reads
//! (see [`Shadow::seed`]).
//!
//! Shadow values are appliance body content and can include identifiers such as `deviceID` or
//! `serialNumber`, so `Debug` renders uid keys and value types only. Read a value through
//! [`Shadow::get`].

use core::fmt;
use std::collections::BTreeMap;

use hc_proto::ValueUpdate;
use serde_json::Value;

/// Where a shadow entry's current value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Seeded from the post-CONNECT `/ro` bulk read, or reconciled from a `NOTIFY` — authoritative.
    Confirmed,
    /// Written locally and not yet confirmed by a `NOTIFY`.
    Optimistic,
}

/// One entry in the shadow.
#[derive(Clone, PartialEq, Eq)]
pub struct ShadowEntry {
    value: Value,
    origin: Origin,
}

impl ShadowEntry {
    /// The current value. Not exposed through `Debug`.
    #[must_use]
    pub const fn value(&self) -> &Value {
        &self.value
    }

    /// Whether the value is still an unconfirmed optimistic write.
    #[must_use]
    pub const fn origin(&self) -> Origin {
        self.origin
    }
}

impl fmt::Debug for ShadowEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never render the value; its JSON type and origin are safe.
        f.debug_struct("ShadowEntry")
            .field("origin", &self.origin)
            .field("value_type", &value_type(&self.value))
            .finish()
    }
}

/// The per-device optimistic shadow, keyed by `uid`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Shadow {
    entries: BTreeMap<u64, ShadowEntry>,
}

impl fmt::Debug for Shadow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Render the uid set and a count, never the values.
        f.debug_struct("Shadow")
            .field("uids", &self.entries.keys().collect::<Vec<_>>())
            .field("len", &self.entries.len())
            .finish()
    }
}

impl Shadow {
    /// An empty shadow.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Seeds the shadow from the decoded `/ro` initial state. Every seeded value is
    /// [`Origin::Confirmed`] — it came from the appliance, not a local write.
    pub fn seed(&mut self, updates: &[ValueUpdate]) {
        for update in updates {
            self.entries.insert(
                update.uid,
                ShadowEntry {
                    value: update.value.clone(),
                    origin: Origin::Confirmed,
                },
            );
        }
    }

    /// Records a local optimistic write. The dashboard reflects it immediately;
    /// a later `NOTIFY` confirms or corrects it.
    pub fn set_optimistic(&mut self, uid: u64, value: Value) {
        self.entries.insert(
            uid,
            ShadowEntry {
                value,
                origin: Origin::Optimistic,
            },
        );
    }

    /// Reconciles an authoritative `NOTIFY /ro/values` update. The appliance's value always wins
    /// and the entry becomes [`Origin::Confirmed`], whether or not it matched the optimistic value.
    pub fn reconcile(&mut self, update: &ValueUpdate) {
        self.entries.insert(
            update.uid,
            ShadowEntry {
                value: update.value.clone(),
                origin: Origin::Confirmed,
            },
        );
    }

    /// The current value for a uid, if known.
    #[must_use]
    pub fn get(&self, uid: u64) -> Option<&ShadowEntry> {
        self.entries.get(&uid)
    }

    /// How many entries the shadow holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the shadow is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The JSON type name of a value, for `Debug`.
const fn value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
