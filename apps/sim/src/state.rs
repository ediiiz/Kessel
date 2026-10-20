//! The simulated appliance: the current value of every feature, plus the access/availability
//! changes it has announced since the connection started.

use std::collections::BTreeMap;

use hc_profile::{Access, Entity, EntityKind, EntityModel, NumericRange, ProtocolType, ValueType};
use rand::seq::IndexedRandom as _;
use rand::{Rng as _, RngCore as _, SeedableRng as _};
use rand_chacha::ChaCha8Rng;
use serde_json::{Map, Value, json};

/// Chance that a successful Setting/Option write is followed by a `/ro/descriptionChange`.
const DESCRIPTION_CHANGE_PROBABILITY: f64 = 0.5;
/// Where a remaining-time countdown restarts after reaching zero, in seconds.
const REMAINING_TIME_START: i64 = 5400;
/// How far the countdown moves per tick, in seconds.
const REMAINING_TIME_STEP: i64 = 60;
/// How far a progress counter moves per tick, in percent.
const PROGRESS_STEP: i64 = 5;

/// How `access` words are spelled on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccessCase {
    /// `READ`, `READWRITE`: what a real appliance sends (the recorded hob session).
    #[default]
    Upper,
    /// `read`, `readWrite`: the profile XML's spelling.
    Camel,
}

impl AccessCase {
    fn word(self, access: &Access) -> String {
        let camel = match access {
            Access::None => "none",
            Access::ReadStatic => "readStatic",
            Access::WriteOnly => "writeOnly",
            Access::ReadWrite => "readWrite",
            Access::Other(word) => word.as_str(),
            // `read`, and anything a later hc-profile adds.
            _ => "read",
        };
        match self {
            Self::Upper => camel.to_ascii_uppercase(),
            Self::Camel => camel.to_owned(),
        }
    }
}

/// Why a write was refused. The appliance answers it with `code: 400`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal(pub String);

fn refuse<T>(reason: impl Into<String>) -> Result<T, Refusal> {
    Err(Refusal(reason.into()))
}

/// One feature value that changed: `(uid, value)`.
pub type Change = (u16, Value);

#[derive(Debug, Clone, Default)]
struct LiveDescription {
    access: Option<Access>,
    available: Option<bool>,
}

/// The status the periodic tick moves, so a UI shows live updates.
#[derive(Debug, Clone, Copy)]
enum Ticker {
    CountDown(u16),
    CountUp(u16),
    Toggle(u16),
}

/// The appliance's state, shared by every connection to the simulator.
#[derive(Debug)]
pub struct Appliance {
    model: EntityModel,
    values: BTreeMap<u16, Value>,
    live: BTreeMap<u16, LiveDescription>,
    ticker: Option<Ticker>,
    rng: ChaCha8Rng,
    access_case: AccessCase,
}

impl Appliance {
    /// Builds the initial state from a profile's entity model. Everything random is drawn from
    /// `seed`, so a run is reproducible.
    #[must_use]
    pub fn new(model: EntityModel, seed: u64, access_case: AccessCase) -> Self {
        let values: BTreeMap<u16, Value> = model
            .entities()
            .values()
            .filter_map(|entity| initial_value(&model, entity).map(|value| (entity.uid, value)))
            .collect();
        let ticker = choose_ticker(&model, &values);
        Self {
            model,
            values,
            live: BTreeMap::new(),
            ticker,
            rng: ChaCha8Rng::seed_from_u64(seed),
            access_case,
        }
    }

    /// The profile's entity model.
    #[must_use]
    pub const fn model(&self) -> &EntityModel {
        &self.model
    }

    /// The current value of a feature.
    #[must_use]
    pub fn value(&self, uid: u16) -> Option<&Value> {
        self.values.get(&uid)
    }

    /// A random `u32`, for session and message ids.
    pub fn random_u32(&mut self) -> u32 {
        self.rng.next_u32()
    }

    /// 32 random bytes, for the `/ci/authentication` response.
    pub fn random_bytes(&mut self) -> [u8; 32] {
        let mut raw = [0_u8; 32];
        self.rng.fill_bytes(&mut raw);
        raw
    }

    /// `/ro/allMandatoryValues` items: `{"uid","value"}` for every value-carrying feature.
    #[must_use]
    pub fn mandatory_values(&self) -> Vec<Value> {
        self.values
            .iter()
            .map(|(uid, value)| value_item(*uid, value))
            .collect()
    }

    /// `/ro/allDescriptionChanges` items: everything announced since start-up.
    #[must_use]
    pub fn description_changes(&self) -> Vec<Value> {
        self.live
            .iter()
            .map(|(uid, live)| self.description_item(*uid, live))
            .collect()
    }

    /// Applies a `POST /ro/values` body. All items are checked before any is applied, so a refused
    /// write changes nothing.
    ///
    /// # Errors
    ///
    /// A [`Refusal`] naming the first item that is unknown, not writable or out of range.
    pub fn write(&mut self, items: &[Value]) -> Result<Vec<Change>, Refusal> {
        if items.is_empty() {
            return refuse("no items in the write");
        }
        let changes = items
            .iter()
            .map(|item| self.check_item(item))
            .collect::<Result<Vec<_>, _>>()?;
        self.apply(&changes);
        Ok(changes)
    }

    /// Applies a `POST /ro/selectedProgram` (or `/ro/activeProgram`) body
    /// `[{"program":uid,"options":[…]}]`: the program becomes both selected and active.
    ///
    /// # Errors
    ///
    /// A [`Refusal`] for an unknown program or an invalid option.
    pub fn start_program(&mut self, data: &[Value]) -> Result<Vec<Change>, Refusal> {
        let Some(body) = data.first() else {
            return refuse("no program in the request");
        };
        let Some(program) = body
            .get("program")
            .and_then(Value::as_u64)
            .and_then(|uid| u16::try_from(uid).ok())
        else {
            return refuse("the request has no numeric `program`");
        };
        if self.model.program(program).is_none() {
            return refuse(format!("program {program} is not declared"));
        }
        let options = body.get("options").and_then(Value::as_array);
        let mut changes = options
            .into_iter()
            .flatten()
            .map(|item| self.check_item(item))
            .collect::<Result<Vec<_>, _>>()?;
        for kind in [EntityKind::SelectedProgram, EntityKind::ActiveProgram] {
            if let Some(uid) = self.uid_of_kind(kind) {
                changes.push((uid, json!(program)));
            }
        }
        self.apply(&changes);
        Ok(changes)
    }

    /// Applies a `DELETE /ro/activeProgram`: no program is active any more.
    ///
    /// # Errors
    ///
    /// A [`Refusal`] when the profile declares no active program.
    pub fn stop_program(&mut self) -> Result<Vec<Change>, Refusal> {
        let Some(uid) = self.uid_of_kind(EntityKind::ActiveProgram) else {
            return refuse("the profile declares no activeProgram");
        };
        let changes = vec![(uid, json!(0))];
        self.apply(&changes);
        Ok(changes)
    }

    /// After a successful write to `written`, sometimes revise the description of a different
    /// option: flip its access between readWrite and read, or else its availability. Returns the
    /// `/ro/descriptionChange` item.
    pub fn maybe_redescribe(&mut self, written: u16) -> Option<Value> {
        let kind = self.model.entity(written)?.kind;
        if !matches!(kind, EntityKind::Setting | EntityKind::Option) {
            return None;
        }
        if !self.rng.random_bool(DESCRIPTION_CHANGE_PROBABILITY) {
            return None;
        }
        let others: Vec<&Entity> = self
            .model
            .entities()
            .values()
            .filter(|entity| entity.kind == EntityKind::Option && entity.uid != written)
            .collect();
        let flippable: Vec<u16> = others
            .iter()
            .filter(|entity| entity.access == Some(Access::ReadWrite))
            .map(|entity| entity.uid)
            .collect();
        let others: Vec<u16> = others.iter().map(|entity| entity.uid).collect();
        if let Some(&uid) = flippable.choose(&mut self.rng) {
            let access = if self.current_access(uid).is_some_and(Access::is_writable) {
                Access::Read
            } else {
                Access::ReadWrite
            };
            let live = self.live.entry(uid).or_default();
            live.access = Some(access.clone());
            let item = LiveDescription {
                access: Some(access),
                available: None,
            };
            return Some(self.description_item(uid, &item));
        }
        let &uid = others.choose(&mut self.rng)?;
        let available = !self.current_available(uid);
        self.live.entry(uid).or_default().available = Some(available);
        let item = LiveDescription {
            access: None,
            available: Some(available),
        };
        Some(self.description_item(uid, &item))
    }

    /// Advances the live status by one step and returns its new value.
    pub fn tick(&mut self) -> Option<Change> {
        let (uid, value) = match self.ticker? {
            Ticker::CountDown(uid) => {
                let current = self.int_value(uid);
                let next = if current <= 0 {
                    REMAINING_TIME_START
                } else {
                    (current - REMAINING_TIME_STEP).max(0)
                };
                (uid, json!(next))
            }
            Ticker::CountUp(uid) => {
                let next = (self.int_value(uid) + PROGRESS_STEP) % (100 + PROGRESS_STEP);
                (uid, json!(next))
            }
            Ticker::Toggle(uid) => {
                let current = self.values.get(&uid).and_then(Value::as_bool);
                (uid, json!(!current.unwrap_or(false)))
            }
        };
        self.values.insert(uid, value.clone());
        Some((uid, value))
    }

    fn apply(&mut self, changes: &[Change]) {
        for (uid, value) in changes {
            if let Some(slot) = self.values.get_mut(uid) {
                slot.clone_from(value);
            }
        }
    }

    fn check_item(&self, item: &Value) -> Result<Change, Refusal> {
        let Some(uid) = item
            .get("uid")
            .and_then(Value::as_u64)
            .and_then(|uid| u16::try_from(uid).ok())
        else {
            return refuse("an item has no valid `uid`");
        };
        let Some(value) = item.get("value") else {
            return refuse(format!("uid {uid}: no `value`"));
        };
        let Some(entity) = self.model.entity(uid) else {
            return refuse(format!("uid {uid} is not declared"));
        };
        if !self.current_access(uid).is_some_and(writable) {
            return refuse(format!("uid {uid} is not writable"));
        }
        if !self.current_available(uid) {
            return refuse(format!("uid {uid} is not available"));
        }
        if !self.value_is_valid(entity, value) {
            return refuse(format!("uid {uid}: value {value} is not valid"));
        }
        Ok((uid, value.clone()))
    }

    fn value_is_valid(&self, entity: &Entity, value: &Value) -> bool {
        if matches!(
            entity.kind,
            EntityKind::ActiveProgram | EntityKind::SelectedProgram
        ) {
            return value
                .as_u64()
                .and_then(|uid| u16::try_from(uid).ok())
                .is_some_and(|uid| uid == 0 || self.model.program(uid).is_some());
        }
        if let ValueType::Enumeration { .. } = entity.value_type {
            return value.as_i64().is_some_and(|ordinal| {
                self.model
                    .enum_type_of(entity)
                    .is_none_or(|enumeration| enumeration.members.contains_key(&ordinal))
            });
        }
        let range = match entity.value_type {
            ValueType::Numeric(range) => range,
            _ => NumericRange::default(),
        };
        match entity.protocol_type() {
            Some(ProtocolType::Boolean) => value.is_boolean(),
            Some(ProtocolType::String) => value.is_string(),
            Some(ProtocolType::Integer) => value.as_i64().is_some_and(|n| in_range(range, n)),
            Some(ProtocolType::Float) => value.as_f64().is_some_and(|n| in_range_f64(range, n)),
            _ => !value.is_null() && !value.is_array() && !value.is_object(),
        }
    }

    fn current_access(&self, uid: u16) -> Option<&Access> {
        self.live
            .get(&uid)
            .and_then(|live| live.access.as_ref())
            .or_else(|| {
                self.model
                    .entity(uid)
                    .and_then(|entity| entity.access.as_ref())
            })
    }

    fn current_available(&self, uid: u16) -> bool {
        self.live
            .get(&uid)
            .and_then(|live| live.available)
            .or_else(|| self.model.entity(uid).and_then(|entity| entity.available))
            .unwrap_or(true)
    }

    fn uid_of_kind(&self, kind: EntityKind) -> Option<u16> {
        self.model
            .entities()
            .values()
            .find(|entity| entity.kind == kind)
            .map(|entity| entity.uid)
    }

    fn int_value(&self, uid: u16) -> i64 {
        self.values.get(&uid).and_then(Value::as_i64).unwrap_or(0)
    }

    fn description_item(&self, uid: u16, live: &LiveDescription) -> Value {
        let mut item = Map::new();
        item.insert("uid".to_owned(), json!(uid));
        if let Some(parent) = self.model.entity(uid).and_then(|entity| entity.parent) {
            item.insert("parentUID".to_owned(), json!(parent));
        }
        if let Some(access) = &live.access {
            item.insert("access".to_owned(), json!(self.access_case.word(access)));
        }
        if let Some(available) = live.available {
            item.insert("available".to_owned(), json!(available));
        }
        Value::Object(item)
    }
}

/// A `{"uid","value"}` item, `uid` first as the firmware expects.
#[must_use]
pub fn value_item(uid: u16, value: &Value) -> Value {
    let mut item = Map::new();
    item.insert("uid".to_owned(), json!(uid));
    item.insert("value".to_owned(), value.clone());
    Value::Object(item)
}

/// `readWrite`, `writeOnly` and `none` accept writes. `none` is included because some appliances
/// declare writable setpoints that way in the archive and only open them up at run time.
const fn writable(access: &Access) -> bool {
    matches!(access, Access::ReadWrite | Access::WriteOnly | Access::None)
}

fn in_range(range: NumericRange, value: i64) -> bool {
    range.min.is_none_or(|min| value >= min) && range.max.is_none_or(|max| value <= max)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "profile bounds are small integers, exactly representable as f64"
)]
fn in_range_f64(range: NumericRange, value: f64) -> bool {
    range.min.is_none_or(|min| value >= min as f64)
        && range.max.is_none_or(|max| value <= max as f64)
}

fn initial_value(model: &EntityModel, entity: &Entity) -> Option<Value> {
    match entity.kind {
        EntityKind::ActiveProgram => Some(json!(0)),
        EntityKind::SelectedProgram => {
            Some(json!(model.programs().keys().next().copied().unwrap_or(0)))
        }
        EntityKind::Status | EntityKind::Setting | EntityKind::Option | EntityKind::Event => {
            scalar_initial(model, entity)
        }
        _ => None,
    }
}

fn scalar_initial(model: &EntityModel, entity: &Entity) -> Option<Value> {
    if let Some(enumeration) = model.enum_type_of(entity) {
        let first = enumeration
            .declared_values
            .first()
            .or_else(|| enumeration.members.keys().next())
            .copied()
            .unwrap_or(0);
        return Some(json!(first));
    }
    match entity.protocol_type()? {
        ProtocolType::Boolean => Some(json!(false)),
        ProtocolType::String => Some(json!("")),
        ProtocolType::Integer | ProtocolType::Float => {
            let min = match entity.value_type {
                ValueType::Numeric(range) => range.min,
                _ => None,
            };
            Some(json!(min.unwrap_or(0)))
        }
        _ => None,
    }
}

fn choose_ticker(model: &EntityModel, values: &BTreeMap<u16, Value>) -> Option<Ticker> {
    let has_key = |suffix: &str| {
        model
            .entities()
            .values()
            .find(|entity| {
                values.contains_key(&entity.uid)
                    && entity
                        .key
                        .as_deref()
                        .is_some_and(|key| key.ends_with(suffix))
            })
            .map(|entity| entity.uid)
    };
    if let Some(uid) = has_key(".RemainingProgramTime") {
        return Some(Ticker::CountDown(uid));
    }
    if let Some(uid) = has_key(".ProgramProgress") {
        return Some(Ticker::CountUp(uid));
    }
    model
        .entities()
        .values()
        .find(|entity| {
            entity.kind == EntityKind::Status
                && entity.protocol_type() == Some(ProtocolType::Boolean)
                && values.contains_key(&entity.uid)
        })
        .map(|entity| Ticker::Toggle(entity.uid))
}
