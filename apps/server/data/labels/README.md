# Contributed display names

The appliance's own archive carries **no human-readable names**. `FeatureMapping.xml` holds only
`<feature>`, `<enumDescription>`, `<enumMember>` and `<error>` elements — no description text, no
localisation payload. So `Dishcare.Dishwasher.Setting.TimeLight` and its members `Off` / `On` are the
only names the protocol supplies, and everything friendlier is contributed here.

Files in this directory are embedded in the daemon at compile time. Adding one is the whole
contribution: no code change, no index to update.

## The three layers

A name is looked up in this order, and **the first hit wins**:

| Layer | File | Covers |
|---|---|---|
| `model` | `models/<BRAND>/<VIB>.toml` | one exact appliance model |
| `prefix` | `prefixes/<Vendor.Section>.toml` | every appliance sharing that key prefix |
| `derived` | — | a spelling transform of the identifier itself (`server::labels::humanize`) |

`derived` is not a file and cannot be missing: `TimeLight` becomes `Time light` with no table at all.
So a contributed file only ever needs the entries where the derived spelling is *wrong* or
uninformative — you are not obliged to name everything.

## Finding your brand and VIB

The device page shows both, and they are the filename: brand upper-cased as the directory, VIB as the
file stem. `BOSCH` + `SMD8TCX01E` → `models/BOSCH/SMD8TCX01E.toml`.

## Editor support

`schema.json` describes these files, and `.taplo.toml` at the repository root points taplo-based
editors at it: VS Code with Even Better TOML, Zed, Helix. You get completion for every field, the
allowed `entity_category` and `device_class` words, and a squiggle under a typo. The schema is
generated from the loader's own types; after changing them, `just types` rewrites it and a test
fails until the new copy is committed. To check from a terminal:

```sh
bunx @taplo/cli check 'apps/server/data/labels/**/*.toml'
```

## The format

```toml
# Every table key is a full dotted feature key, quoted.
[features."Dishcare.Dishwasher.Setting.TimeLight"]
label = "Time light"
# Optional. Shown as help text; omit it rather than guessing at what a setting does.
description = "Projects the remaining programme time onto the floor."

# Optional. Keyed by the *raw* enum member name, exactly as the appliance declares it.
[features."Dishcare.Dishwasher.Setting.TimeLight".values]
Off = "Off"
On = "On"

# Programs are named the same way, in their own table.
[programs."Dishcare.Dishwasher.Program.Eco50"]
label = "Eco 50 °C"
```

Both tables are optional, and so is every field inside an entry.

## Deciding what Home Assistant shows

Two optional fields per feature, and they are the difference between a usable device page and a
wall of 288 entities.

```toml
[features."Refrigeration.Common.Setting.EcoMode"]
label = "Eco mode"
# Whether Home Assistant enables the entity on discovery.
enabled_by_default = true
# Which section of the device page it lands in: "primary" | "config" | "diagnostic".
# "primary" is the prominent one at the top, with no category at all.
entity_category = "primary"
```

Leave both out and a built-in heuristic decides. That heuristic is fifteen leaf names —
`PowerState`, `OperationState`, `DoorState`, `RemainingProgramTime` and so on — plus "a writable
setting is Configuration, a status is Diagnostic, everything else arrives disabled". Across the four
appliances that is 976 features and about a dozen enabled, which is the right default for a long
tail and wrong for the handful an appliance is actually *for*: a fridge's eco and holiday modes are
not called `PowerState`.

Nothing derivable separates `Setting.EcoMode` from `Setting.Light.Internal.Brightness`. A person who
has looked at the appliance does. That is what these two fields are for, and it is the only place
that judgement can live.

**Disabled is not hidden.** The config is still published and the state topic is still written; the
entity sits behind "+123 disabled entities" and is one click from on. So the cost of leaving
something out is small, and the cost of enabling everything is a page nobody can read.

Each field falls through the layers on its own, so a model file that renames a feature does not
thereby erase the prefix file's judgement about where it belongs.

## Icons and device classes

Two more optional fields shape how an entity looks in Home Assistant:

```toml
[features."Dishcare.Dishwasher.Event.SaltLack"]
# Home Assistant's vocabulary for the entity's platform. "problem" turns an event into a fault
# that shows red and can drive an alert.
device_class = "problem"

[features."BSH.Common.Status.DoorState"]
# Any Material Design Icon: https://pictogrammers.com/library/mdi/
icon = "mdi:door"
```

Neither needs a `label`, so an entry can set just one of them. Leave `icon` out of a binary sensor
that has a `device_class`: Home Assistant draws a different icon for each state (a door open or
closed), and a fixed icon replaces both. The tests reject an icon that is not `mdi:<name>` and a
class the entity's platform does not have.

## Two rules

1. **A name is presentation only.** The raw dotted key and the raw member name are still sent to the
   browser and still rendered beside the friendly name, and a write always carries the member
   verbatim. Nothing you write here can change what reaches an appliance.
2. **Describe what you have observed, not what you assume.** An appliance-specific setting whose
   effect you have not seen is better left to the derived spelling than given a confident wrong
   description; when in doubt, omit the field.
