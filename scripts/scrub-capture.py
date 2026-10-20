#!/usr/bin/env python3
"""Turn a raw appliance capture into the committed golden corpus.

The replay fixtures are committed, but the psk, iv, serial number, MAC and haId must never
leak, and a raw capture is full of them. This script bridges the two tiers of the corpus:

1. Read the **raw** capture (``secrets/captures/…``, gitignored).
2. Substitute every real identifier with a deterministic fake **of the same shape** — same
   length, same character class — so parser behaviour is unchanged.
3. **Re-run the whole session through the crypto chains under a synthetic psk/iv**, with a
   seeded pad RNG.
4. Emit the **golden** corpus (``fixtures/aes/…``, committed) and verify it end to end.

Because the chains are re-run rather than patched, the golden corpus stays internally
consistent: HMAC chain state, CBC continuity and frame lengths are all still exact, just
under a key that is safe to publish.

Substitution is performed on the **plaintext bytes**, never by round-tripping through a
parsed JSON object, so JSON key order survives exactly. The appliance firmware parses objects
positionally, and a dict round-trip would silently normalise the order.

Usage::

    scrub-capture.py scrub -i secrets/captures/hob/session.json -o fixtures/aes/hob-session.json
    scrub-capture.py audit fixtures/ docs/ scripts/          # leak check over tracked files
    scrub-capture.py identifiers                             # print the SYNTHETIC set only

Every mode takes ``--secrets-dir`` (default: ``$KESSEL_SECRETS_DIR`` or ``<repo>/secrets``).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import string
import sys
import zipfile
from dataclasses import dataclass
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import hc_aes  # noqa: E402
import hc_corpus  # noqa: E402

# ---------------------------------------------------------------------------------------
# Published constants. These are the only key material that ever appears in a tracked file.
# ---------------------------------------------------------------------------------------

#: Namespace for the deterministic identifier generator. The label is a fixed seed: changing
#: it rewrites the corpus.
SYNTH_NAMESPACE = b"kessel/C-0/scrub/v1"

#: Seed for the pad RNG, so the corpus is byte-for-byte reproducible. The label is a fixed seed.
PAD_SEED = b"kessel/C-0/pad/v1"

#: The synthetic psk and iv the golden corpus is encrypted under. Derived from fixed labels
#: so they are reproducible from this file alone and have no relationship to any real key.
#: The labels are fixed seeds; do not edit them.
SYNTHETIC_PSK = hashlib.sha256(b"kessel/C-0/synthetic-psk/v1").digest()
SYNTHETIC_IV = hashlib.sha256(b"kessel/C-0/synthetic-iv/v1").digest()[: hc_aes.IV_LENGTH]
SYNTHETIC_PSK64 = hc_aes.b64url_encode(SYNTHETIC_PSK)
SYNTHETIC_IV64 = hc_aes.b64url_encode(SYNTHETIC_IV)

#: The profile fields that must never leak into a tracked file, plus ``shipSki`` for TLS profiles.
SENSITIVE_FIELDS = ("haId", "serialNumber", "mac", "key", "iv", "shipSki")

#: JSON keys in captured frames whose *values* are identifying and must be substituted even
#: when they appear in no profile ZIP. Observed on the real hob session: ``/ci/info`` repeats
#: the haId as ``deviceID`` and adds ``eNumber``/``vib``; ``NOTIFY /ci/registeredDevices``
#: lists every previously-paired client by name and ID (a phone, in practice);
#: ``/ni/info`` carries the home SSID, the LAN addressing, and an IPv6 address whose lower 64
#: bits are derived from the appliance MAC.
#:
#: ⚠ **This list is the whole of capture-only discovery, and it is deny-by-omission.** An
#: identifier arriving under a key nobody listed — ``hostname``, ``bssid``, ``token``,
#: ``ownerId``, an e-mail address — passes through silently, and nothing downstream catches
#: it: ``broad_scan`` draws its needles from the profile ZIPs, so it structurally cannot see a
#: capture-only value. :data:`HARVEST_MIN_LEN` additionally drops anything shorter, so a
#: 5-character SSID slips through even under a listed key. Adding a key is a one-line change;
#: **re-read this list against the frames of every new capture.**
HARVEST_KEYS: dict[str, str] = {
    "haId": "identifier",
    "deviceID": "identifier",
    "deviceName": "identifier",
    "serialNumber": "identifier",
    "mac": "identifier",
    "euiAddress": "identifier",
    "eNumber": "identifier",
    "vib": "identifier",
    "shipSki": "identifier",
    "ssid": "network",
    "ipAddress": "network",
    "gateway": "network",
    "dnsServer": "network",
    "nonce": "session",
    "response": "session",
}

#: Values that look like identifiers but are public constants of the *client*, not of the
#: appliance or its owner. hcpy hardcodes them, so they are identical for every hcpy user,
#: and the corpus is used to compare Kessel's emitted values against them byte for byte.
PUBLIC_VALUES = frozenset({"hcpy", "0badcafe", "Application"})

#: Below this length, a literal substitution is more likely to collide with unrelated text
#: than to remove anything worth removing.
HARVEST_MIN_LEN = 6

#: Host recorded in the golden corpus. The real address is not written to a tracked file.
SYNTHETIC_HOST = "appliance.invalid"

_HEX_DIGITS = set("0123456789abcdefABCDEF")


# ---------------------------------------------------------------------------------------
# Shape-preserving synthetic identifiers
# ---------------------------------------------------------------------------------------


def char_class(char: str) -> str:
    """``'U'`` uppercase, ``'L'`` lowercase, ``'D'`` digit, otherwise the character itself.

    Two strings with the same per-position class sequence are interchangeable to any parser
    that classifies characters rather than matching literals.
    """
    if char.isupper() and char.isalpha():
        return "U"
    if char.islower() and char.isalpha():
        return "L"
    if char.isdigit():
        return "D"
    return char


def _is_hex_token(value: str) -> bool:
    """True when every alphanumeric character is a hex digit (separators are ignored)."""
    alnum = [c for c in value if c.isalnum()]
    return bool(alnum) and all(c in _HEX_DIGITS for c in alnum)


def synth_token(real: str, role: str) -> str:
    """A deterministic, shape-preserving stand-in for ``real``.

    Same length; each character replaced by one of the same class. A token whose alphanumeric
    characters are all hex digits stays hex, so anything downstream that parses it as hex
    still can.
    """
    hexish = _is_hex_token(real)
    for salt in range(64):
        seed = SYNTH_NAMESPACE + b"\x00" + role.encode() + b"\x00"
        seed += str(salt).encode() + b"\x00" + real.encode()
        stream = hashlib.shake_256(seed).digest(max(len(real), 1))
        out = []
        for char, byte in zip(real, stream):
            klass = char_class(char)
            if klass == "U":
                alphabet = "ABCDEF" if hexish else string.ascii_uppercase
            elif klass == "L":
                alphabet = "abcdef" if hexish else string.ascii_lowercase
            elif klass == "D":
                alphabet = string.digits
            else:
                out.append(char)
                continue
            out.append(alphabet[byte % len(alphabet)])
        candidate = "".join(out)
        if candidate != real:
            return candidate
    raise RuntimeError(f"could not synthesise a distinct stand-in for role {role!r}")


@dataclass(frozen=True)
class Substitution:
    """One real -> synthetic mapping. ``real`` must never be written to a tracked file.

    ``kind`` splits the shape contract in two, because the two categories cannot both be
    satisfied by the same rule:

    ``"identifier"``
        haId, MAC, serial, shipSki. Same length **and** same per-character class, so parser
        behaviour is unchanged. This is the category the corpus's plaintexts
        actually contain.
    ``"key-material"``
        the profile ``key`` and ``iv``. These map onto the single *published* synthetic key
        the whole corpus is encrypted under, so their replacement is a fixed constant and
        cannot track the real value's character classes; only length is preserved. They are
        in the table purely as defence in depth — key material is not expected to appear in
        any plaintext, and if it ever did, mapping it to the published key is the safe
        landing spot.
    """

    role: str
    """Unique key for this row. Scoped per profile, because four appliances each have a
    ``haId``; an unscoped role would collapse them onto one synthetic value."""

    field: str
    """The profile/JSON field this row came from — ``haId``, ``mac``, ``ssid``, … Several
    rows can share a field (MAC variants, or two different ``deviceID`` values)."""

    real: str
    synthetic: str
    kind: str = "identifier"


def _mac_variants(scope: str, real_compact: str, synth_compact: str) -> list[Substitution]:
    """A MAC turns up compact, colon-separated and hyphen-separated, in either case."""
    subs: list[Substitution] = []

    def split(value: str, sep: str) -> str:
        return sep.join(value[i : i + 2] for i in range(0, len(value), 2))

    forms = [
        ("", real_compact, synth_compact),
        (":", split(real_compact, ":"), split(synth_compact, ":")),
        ("-", split(real_compact, "-"), split(synth_compact, "-")),
    ]
    for sep, real_form, synth_form in forms:
        suffix = {"": "compact", ":": "colon", "-": "hyphen"}[sep]
        base = f"{scope}/mac/{suffix}"
        subs.append(Substitution(base, "mac", real_form.upper(), synth_form.upper()))
        subs.append(Substitution(base + "/lower", "mac", real_form.lower(), synth_form.lower()))
    return subs


# ---------------------------------------------------------------------------------------
# Reading appliance profiles out of secrets/
# ---------------------------------------------------------------------------------------


def default_secrets_dir() -> Path:
    env = os.environ.get("KESSEL_SECRETS_DIR")
    if env:
        return Path(env)
    return Path(__file__).resolve().parents[1] / "secrets"


def load_profiles(secrets_dir: Path) -> list[dict]:
    """Every appliance profile JSON under ``secrets_dir`` — inside ZIPs or loose."""
    secrets_dir = Path(secrets_dir)
    if not secrets_dir.is_dir():
        raise FileNotFoundError(
            f"secrets directory {secrets_dir} not found; pass --secrets-dir or set "
            f"KESSEL_SECRETS_DIR (a git worktree does not contain it)"
        )
    profiles: list[dict] = []
    for path in sorted(secrets_dir.glob("*.zip")):
        with zipfile.ZipFile(path) as archive:
            for name in sorted(archive.namelist()):
                if name.lower().endswith(".json"):
                    profiles.append(json.loads(archive.read(name)))
    for path in sorted(secrets_dir.glob("*.json")):
        profiles.append(json.loads(path.read_text(encoding="utf-8")))
    if not profiles:
        raise FileNotFoundError(f"no appliance profile JSON found under {secrets_dir}")
    return profiles


def load_ip_aliases(secrets_dir: Path) -> list[Substitution]:
    """Substitutions for the addresses in ``secrets/IP.md``, if that file exists.

    The capture harvest already catches whatever addressing the appliance reports in
    ``/ni/info``; this exists so that ``audit`` can also catch an address that reached a
    tracked file by some other route.
    """
    ip_file = Path(secrets_dir) / "IP.md"
    if not ip_file.is_file():
        return []
    subs = []
    for line in ip_file.read_text(encoding="utf-8").splitlines():
        parts = line.split(":", 1)
        if len(parts) != 2:
            continue
        alias, address = parts[0].strip(), parts[1].strip()
        if not alias.isalnum() or not address or " " in address:
            continue
        role = f"ip/{alias}"
        subs.append(Substitution(role, "ip", address, synth_token(address, role), "network"))
    return subs


def build_substitutions(profiles: list[dict]) -> list[Substitution]:
    """The full real -> synthetic table across every profile, longest ``real`` first.

    Longest-first matters: the compact MAC is a substring of the haId, so the haId has to be
    rewritten before the bare MAC is. The synthetic haId is built to *end with* the synthetic
    MAC so the two stay mutually consistent after both passes.
    """
    subs: dict[str, Substitution] = {}

    def add(sub: Substitution) -> None:
        if not sub.real or sub.real in subs:
            return
        subs[sub.real] = sub

    for index, profile in enumerate(profiles):
        tag = profile.get("haId") or f"profile{index}"
        scope = hashlib.sha256(tag.encode()).hexdigest()[:8]

        real_mac = str(profile.get("mac", "") or "")
        mac_compact = real_mac.replace(":", "").replace("-", "").upper()
        synth_mac_compact = synth_token(mac_compact, f"{scope}/mac") if mac_compact else ""
        if mac_compact:
            for sub in _mac_variants(scope, mac_compact, synth_mac_compact):
                add(sub)

        real_haid = str(profile.get("haId", "") or "")
        if real_haid:
            if mac_compact and real_haid.upper().endswith(mac_compact):
                head = real_haid[: len(real_haid) - len(mac_compact)]
                synth_haid = synth_token(head, f"{scope}/haId") + synth_mac_compact
            else:
                synth_haid = synth_token(real_haid, f"{scope}/haId")
            add(Substitution(f"{scope}/haId", "haId", real_haid, synth_haid))
            add(
                Substitution(
                    f"{scope}/haId/lower", "haId", real_haid.lower(), synth_haid.lower()
                )
            )

        for field in ("serialNumber", "shipSki"):
            value = str(profile.get(field, "") or "")
            if value:
                role = f"{scope}/{field}"
                add(Substitution(role, field, value, synth_token(value, role)))

        # Key material maps onto the published synthetic key, so a raw capture that happens to
        # echo it (it should not) still lands on the value the corpus is encrypted under.
        if profile.get("key"):
            add(
                Substitution(
                    f"{scope}/key", "key", str(profile["key"]), SYNTHETIC_PSK64, "key-material"
                )
            )
        if profile.get("iv"):
            add(
                Substitution(
                    f"{scope}/iv", "iv", str(profile["iv"]), SYNTHETIC_IV64, "key-material"
                )
            )

    return sorted(subs.values(), key=lambda sub: (-len(sub.real), sub.real))


# ---------------------------------------------------------------------------------------
# Substitution and auditing
# ---------------------------------------------------------------------------------------


def _walk(node, callback) -> None:
    if isinstance(node, dict):
        for key, value in node.items():
            callback(key, value)
            _walk(value, callback)
    elif isinstance(node, list):
        for item in node:
            _walk(item, callback)


def harvest_capture_identifiers(capture: dict, existing: list[Substitution]) -> list[Substitution]:
    """Identifiers that live in the capture but in no profile ZIP.

    Walks every frame's decoded JSON and picks up the values of :data:`HARVEST_KEYS`. Values
    already covered by ``existing`` are skipped, so a haId harvested from ``/ci/info`` maps to
    the same synthetic value the profile-derived table already assigned.
    """
    known = {sub.real for sub in existing}
    harvested: dict[str, Substitution] = {}

    def consider(key: str, value) -> None:
        kind = HARVEST_KEYS.get(key)
        if kind is None or not isinstance(value, str):
            return
        if len(value) < HARVEST_MIN_LEN or value in PUBLIC_VALUES:
            return
        if value in known or value in harvested:
            return
        role = f"capture/{key}/{hashlib.sha256(value.encode()).hexdigest()[:8]}"
        harvested[value] = Substitution(role, key, value, synth_token(value, role), kind)

    for record in capture.get("frames", []):
        doc = record.get("json")
        if doc is None:
            doc = json.loads(bytes.fromhex(record["plaintext_hex"]).decode("utf-8"))
        _walk(doc, consider)

    return list(harvested.values())


def full_substitution_table(capture: dict, base: list[Substitution]) -> list[Substitution]:
    """``base`` plus everything harvested from ``capture``, longest ``real`` first."""
    combined = list(base) + harvest_capture_identifiers(capture, base)
    return sorted(combined, key=lambda sub: (-len(sub.real), sub.real))


def scrub_text(text: str, subs: list[Substitution]) -> str:
    """Literal, longest-first replacement. No parsing, so nothing can be reordered."""
    for sub in sorted(subs, key=lambda s: -len(s.real)):
        if sub.real and sub.real != sub.synthetic:
            text = text.replace(sub.real, sub.synthetic)
    return text


def scrub_bytes(data: bytes, subs: list[Substitution]) -> bytes:
    return scrub_text(data.decode("utf-8"), subs).encode("utf-8")


def residual_scan(blob: str, subs: list[Substitution]) -> list[str]:
    """Every real identifier still present in ``blob``. Empty list means clean."""
    return sorted({sub.real for sub in subs if sub.real and sub.real in blob})


def broad_scan(blob: str, profiles: list[dict], min_len: int = 6) -> list[str]:
    """Informational: any profile string value of ``min_len``+ characters found in ``blob``.

    Wider than :data:`SENSITIVE_FIELDS`, and that is the whole of its reach. **It is not a backstop for an
    unlisted key.** Its needles come from ``profiles`` — and only from top-level
    ``profile.values()`` — so it can only ever re-find something the profile ZIP already
    knows. A *capture-only* identifier under a key name absent from :data:`HARVEST_KEYS` is
    exactly the category it cannot see, and that is the category :data:`HARVEST_KEYS` exists
    to cover. ``residual_scan`` and the refuse-to-write gate in ``scrub`` share the blind spot:
    all three are deny-by-omission over a known list.

    Closing it would need the inverted design: report every string of ``min_len``+ characters
    whose key is *not* on a known-safe list, and force per-capture triage.
    """
    found: set[str] = set()
    for profile in profiles:
        for value in profile.values():
            if isinstance(value, str) and len(value) >= min_len and value in blob:
                found.add(value)
    return sorted(found)


def json_key_sequence(text: str) -> list[str]:
    """The ordered key sequence of a JSON document, as written.

    Used by ``scrub_preserves_json_key_order``. ``json.loads`` invokes ``object_pairs_hook``
    once per object in a deterministic order, and the hook sees the pairs in *source* order.
    """
    keys: list[str] = []

    def hook(pairs):
        keys.extend(key for key, _ in pairs)
        return dict(pairs)

    json.loads(text, object_pairs_hook=hook)
    return keys


# ---------------------------------------------------------------------------------------
# The scrub itself
# ---------------------------------------------------------------------------------------


def scrub_capture(
    raw: dict,
    subs: list[Substitution],
    *,
    psk64: str = SYNTHETIC_PSK64,
    iv64: str = SYNTHETIC_IV64,
    pad_seed: bytes = PAD_SEED,
) -> dict:
    """Scrub identifiers, then re-run every frame through the chains under the synthetic key."""
    subs = full_substitution_table(raw, subs)
    session = hc_corpus.SessionSealer(
        hc_aes.b64url_decode(psk64), hc_aes.b64url_decode(iv64), hc_aes.ShakeRng(pad_seed)
    )

    frames = []
    for index, record in enumerate(raw.get("frames", [])):
        clear = bytes.fromhex(record["plaintext_hex"])
        scrubbed = scrub_bytes(clear, subs)
        if len(scrubbed) != len(clear):
            raise AssertionError(
                f"frame {record.get('seq', index)}: scrubbing changed the plaintext length "
                f"({len(clear)} -> {len(scrubbed)}); a substitution is not shape-preserving"
            )
        frames.append(
            session.seal(
                record.get("seq", index),
                record["direction"],
                scrubbed,
                t_ms=record.get("t_ms", 0.0),
            )
        )

    raw_device = raw.get("device", {}) or {}
    device = {
        "alias": raw_device.get("alias", "unknown"),
        "transport": raw_device.get("transport", "aes"),
        "connection_type": raw_device.get("connection_type", "AES"),
        "host": SYNTHETIC_HOST,
    }

    # Only the substitutions that actually matched this capture, so the published set
    # describes *this* device and not the rest of the fleet.
    haystack = "".join(record["plaintext_utf8"] for record in raw.get("frames", []))
    applied = [sub for sub in subs if sub.real in haystack]
    golden = {
        "schema": hc_corpus.SCHEMA,
        "tier": "golden",
        "generator": {
            "script": "scripts/scrub-capture.py",
            "namespace": SYNTH_NAMESPACE.decode(),
            "pad_seed": pad_seed.decode(),
            "pad_rng": "shake_256(pad_seed), consumed sequentially across the whole session",
        },
        "device": device,
        "client": raw.get("client", {}),
        "captured_at": raw.get("captured_at"),
        "key": {
            "psk64": psk64,
            "iv64": iv64,
            "note": (
                "SYNTHETIC. Not the appliance psk/iv. Published so the corpus is replayable; "
                "see fixtures/README.md."
            ),
        },
        "synthetic_identifiers": [
            {"field": sub.field, "kind": sub.kind, "synthetic": sub.synthetic}
            for sub in sorted(applied, key=lambda s: (s.field, s.synthetic))
        ],
        "frames": frames,
    }
    return golden


# ---------------------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------------------


L10_KINDS = frozenset({"identifier", "key-material"})


def base_table(secrets_dir: Path, profiles: list[dict]) -> list[Substitution]:
    return build_substitutions(profiles) + load_ip_aliases(secrets_dir)


def _cmd_scrub(args: argparse.Namespace) -> int:
    profiles = load_profiles(args.secrets_dir)
    base = base_table(args.secrets_dir, profiles)
    raw = hc_corpus.load(args.input)
    subs = full_substitution_table(raw, base)

    raw_problems = hc_corpus.verify_capture(raw)
    if raw_problems and not args.skip_raw_verify:
        print("raw capture does not verify under its own key:", file=sys.stderr)
        for problem in raw_problems:
            print(f"  {problem}", file=sys.stderr)
        return 2

    golden = scrub_capture(raw, base)

    problems = hc_corpus.verify_capture(golden)
    if problems:
        print("scrubbed corpus failed verification:", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 3

    blob = json.dumps(golden, ensure_ascii=False)
    leaked = residual_scan(blob, subs)
    if leaked:
        print(f"REFUSING TO WRITE: {len(leaked)} real identifier(s) survived", file=sys.stderr)
        return 4

    hc_corpus.dump(golden, args.output)
    print(
        f"wrote {args.output} ({len(golden['frames'])} frames, verified, "
        f"{len(subs)} substitutions applied, no residuals)"
    )
    for value in broad_scan(blob, profiles):
        print(f"  note: broad scan matched a profile string: {value!r}")
    return 0


def _iter_files(paths: list[Path]):
    for path in paths:
        if path.is_dir():
            for child in sorted(path.rglob("*")):
                if child.is_file() and ".git" not in child.parts:
                    yield child
        elif path.is_file():
            yield path


def _cmd_audit(args: argparse.Namespace) -> int:
    profiles = load_profiles(args.secrets_dir)
    subs = base_table(args.secrets_dir, profiles)
    failures = 0
    notes = 0
    scanned = 0
    for path in _iter_files([Path(p) for p in args.paths]):
        try:
            blob = path.read_text(encoding="utf-8")
        except (UnicodeDecodeError, OSError):
            continue
        scanned += 1
        hits = [sub for sub in subs if sub.real and sub.real in blob]
        l10 = [sub for sub in hits if sub.kind in L10_KINDS]
        other = [sub for sub in hits if sub.kind not in L10_KINDS]
        if l10:
            failures += 1
            print(f"LEAK {path}: {sorted({sub.role for sub in l10})}")
        if other:
            notes += 1
            print(f"note {path}: other secret-derived value(s) {sorted({s.role for s in other})}")
        for value in broad_scan(blob, profiles):
            print(f"note {path}: broad scan matched a profile string of length {len(value)}")
    print(
        f"audited {scanned} file(s); {failures} carrying an appliance identifier "
        f"(haId/mac/serial/psk/iv), {notes} carrying another secrets-derived value"
    )
    return 1 if failures else 0


def _cmd_identifiers(args: argparse.Namespace) -> int:
    """Print the synthetic side of the table only. The real side is never printed."""
    subs = base_table(args.secrets_dir, load_profiles(args.secrets_dir))
    print(f"psk64  {SYNTHETIC_PSK64}")
    print(f"iv64   {SYNTHETIC_IV64}")
    for sub in sorted(subs, key=lambda s: s.role):
        print(f"{sub.role:<18} {sub.synthetic}")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--secrets-dir",
        type=Path,
        default=default_secrets_dir(),
        help="directory holding the appliance profile ZIPs (default: <repo>/secrets)",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    scrub_parser = sub.add_parser("scrub", help="raw capture -> golden corpus")
    scrub_parser.add_argument("-i", "--input", type=Path, required=True)
    scrub_parser.add_argument("-o", "--output", type=Path, required=True)
    scrub_parser.add_argument(
        "--skip-raw-verify",
        action="store_true",
        help="do not require the raw capture to verify under its own key first",
    )
    scrub_parser.set_defaults(func=_cmd_scrub)

    audit_parser = sub.add_parser("audit", help="scan files for real identifiers")
    audit_parser.add_argument("paths", nargs="+")
    audit_parser.set_defaults(func=_cmd_audit)

    ident_parser = sub.add_parser("identifiers", help="print the synthetic identifier set")
    ident_parser.set_defaults(func=_cmd_identifiers)

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
