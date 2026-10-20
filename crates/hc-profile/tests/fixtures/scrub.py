#!/usr/bin/env python3
"""Turn a real Home Connect profile ZIP into a committable parser fixture.

Committed fixtures must carry no real identifier, while staying byte-shaped
identically to the real thing so that parser behaviour (string lengths, character
classes, XML structure, ZIP member names) is unchanged.

What gets substituted (every occurrence: in the JSON, in both XML files, and in
the ZIP member names):

  serialNumber   18 digits            -> 18 synthetic digits
  mac            AA-BB-CC-DD-EE-FF    -> synthetic hex, same separators
  mac (compact)  AABBCCDDEEFF         -> the same synthetic hex, unseparated
                                        (this is the tail of an AES `haId`)
  key  (psk64)   43 chars base64url   -> 43 synthetic base64url chars
  iv   (iv64)    22 chars base64url   -> 22 synthetic base64url chars
  haId                                -> only if it is not already covered by
                                         the serial / compact-MAC rewrites
  created        export timestamp     -> the fixed synthetic FIXED_CREATED

That haId rule keeps `derives_host_for_all_four_real_profiles` honest on the
fixtures: host derivation turns on the shape of `haId` (serial on TLS, composed
brand-VIB-MAC on AES), and both shapes survive because they are rewritten through
their components.

`brand`, `vib`/`model`, `type`, `connectionType` and every `uid`/`enid` are kept
verbatim: they are product metadata and protocol structure, not identifiers of a
particular appliance, and the entity model is built out of them.

`created` is not an identifier, but it is rewritten to `FIXED_CREATED` so that
regenerated fixtures are deterministic. It is the one field whose byte length is not
preserved; `hc-profile` keeps it as an opaque `Option<String>` and parses nothing
from it.

Nothing real is hard-coded here — the values to replace are discovered from the
archive itself, so this script is safe to commit.

Output is **byte-reproducible**: see `_zip_info` for the mtime, permission bits
and create-system that are pinned rather than taken from the clock or the host,
and note that every substitution is *idempotent* — `synth` depends only on the
role and on the template's length and per-position character class, all of which
a scrubbed value shares with the real one. Re-running the scrubber over its own
output is therefore a no-op, which is what makes regenerating the committed
fixtures possible without ever opening `secrets/`.

Usage:
    python3 scrub.py <in-dir> <out-dir>

`<in-dir>` may be the gitignored `secrets/` or, because the substitutions are
idempotent, this fixtures directory itself. Writing back over the inputs is not
supported — scrub into a scratch directory and move the results in.
"""

from __future__ import annotations

import hashlib
import json
import pathlib
import sys
import zipfile

# Deterministic, and written down so a regenerated fixture is byte-comparable. The label is a
# fixed seed: changing it changes every fixture.
SALT = b"kessel-C-1-profile-fixture-v1"

HEX = "0123456789ABCDEF"

# The synthetic export timestamp every fixture gets, so the fixtures do not churn when
# regenerated. Not a redaction.
FIXED_CREATED = "2020-01-01T00:00:00+00:00"

# ZIP metadata that must not come from the clock or the host, or the fixtures would differ on
# every run and every machine. 1980-01-01 00:00:00 is the earliest DOS timestamp a ZIP header
# can represent, the conventional "no meaningful mtime" value.
FIXED_MTIME = (1980, 1, 1, 0, 0, 0)
FIXED_MODE = 0o600
UNIX_CREATE_SYSTEM = 3
COMPRESSLEVEL = 9

# Real ZIP name -> fixture name, keyed on the appliance word so the mapping does
# not depend on the capture timestamp.
OUT_NAMES = {
    "hob": "hob-aes.zip",
    "dishwasher": "dishwasher-tls.zip",
    "oven": "oven-tls.zip",
    "fridge": "fridge-tls.zip",
}


def _stream(role: str, length: int) -> bytes:
    out = b""
    counter = 0
    while len(out) < length:
        out += hashlib.sha256(SALT + role.encode() + str(counter).encode()).digest()
        counter += 1
    return out


def synth(role: str, template: str) -> str:
    """Same length as `template`, same character class at every position."""
    src = _stream(role, len(template))
    out = []
    for byte, ch in zip(src, template):
        if ch.isdigit():
            out.append(str(byte % 10))
        elif ch.isupper():
            out.append(chr(ord("A") + byte % 26))
        elif ch.islower():
            out.append(chr(ord("a") + byte % 26))
        else:
            out.append(ch)  # separators: '-', '_', ':' … preserved verbatim
    return "".join(out)


def synth_hex(role: str, template: str) -> str:
    """Like `synth`, but hex-digit positions stay hex digits (MAC addresses)."""
    src = _stream(role, len(template))
    return "".join(
        HEX[byte % 16] if ch in HEX else ch for byte, ch in zip(src, template)
    )


def substitutions(meta: dict) -> list[tuple[str, str]]:
    # Scope every role to the appliance, so the four fixtures get four distinct
    # synthetic identities rather than one shared one.
    scope = f"{meta.get('brand', '')}/{meta.get('type', '')}/"
    subs: list[tuple[str, str]] = []

    serial = meta.get("serialNumber")
    if serial:
        subs.append((serial, synth(scope + "serial", serial)))

    mac = meta.get("mac")
    if mac:
        fake_mac = synth_hex(scope + "mac", mac)
        subs.append((mac, fake_mac))
        subs.append((mac.replace("-", ""), fake_mac.replace("-", "")))

    for field, role in (("key", "psk"), ("iv", "iv")):
        value = meta.get(field)
        if value:
            subs.append((value, synth(scope + role, value)))

    # Longest-first, so the compact MAC never partially rewrites the dashed one.
    subs.sort(key=lambda pair: -len(pair[0]))

    ha_id = meta.get("haId")
    if ha_id and not any(real in ha_id for real, _ in subs):
        # Neither the serial nor the MAC appears in it; rewrite it wholesale.
        #
        # The test is containment, not `apply(ha_id, subs) != ha_id`: on an already-scrubbed
        # profile every component rewrite is a no-op, and the latter would wrongly rewrite the
        # haId wholesale, breaking the TLS haId == serial and AES BRAND-VIB-MAC shapes.
        # Containment keeps the scrub idempotent.
        subs.append((ha_id, synth(scope + "haid", ha_id)))
        subs.sort(key=lambda pair: -len(pair[0]))

    return subs


def timestamp_substitutions(meta: dict) -> list[tuple[str, str]]:
    """The `created` rewrite, kept out of `substitutions`.

    Two reasons it is a separate list. It is not a redaction, so it does not belong
    in the identifier set the leak check below scans for — and once a fixture has
    been scrubbed its `created` already *is* `FIXED_CREATED`, so feeding it to that
    check would report a leak for a value the scrubber itself wrote.
    """
    created = meta.get("created")
    if not created or created == FIXED_CREATED:
        return []
    return [(created, FIXED_CREATED)]


def apply(text: str, subs: list[tuple[str, str]]) -> str:
    for real, fake in subs:
        text = text.replace(real, fake)
    return text


def _zip_info(name: str) -> zipfile.ZipInfo:
    """A ZIP entry header with nothing in it that varies between runs or hosts.

    `ZipFile.writestr(str, …)` stamps `time.localtime()` and the host's create_system
    into the header, which would make every regeneration a fresh binary diff — the
    committed fixtures would churn on every run and differ per machine. Pinning the
    mtime, the permission bits and the create system leaves the entry bytes a pure
    function of the name and the body. Entry *order* is deterministic already: it is
    the input archive's `namelist()` order, preserved.
    """
    info = zipfile.ZipInfo(filename=name, date_time=FIXED_MTIME)
    info.compress_type = zipfile.ZIP_DEFLATED
    info.create_system = UNIX_CREATE_SYSTEM
    info.external_attr = FIXED_MODE << 16
    return info


def scrub(src: pathlib.Path, dst: pathlib.Path) -> None:
    if dst.resolve() == src.resolve():
        raise SystemExit(f"refusing to scrub {src} onto itself; use a scratch out-dir")
    with zipfile.ZipFile(src) as zin:
        names = zin.namelist()
        json_name = next(n for n in names if n.endswith(".json"))
        meta = json.loads(zin.read(json_name))
        subs = substitutions(meta)
        all_subs = subs + timestamp_substitutions(meta)

        dst.parent.mkdir(parents=True, exist_ok=True)
        with zipfile.ZipFile(dst, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as zout:
            for name in names:
                body = zin.read(name).decode("utf-8")
                zout.writestr(
                    _zip_info(apply(name, all_subs)),
                    apply(body, all_subs),
                    compresslevel=COMPRESSLEVEL,
                )

    # Fail loudly rather than emit a fixture that still leaks.
    #
    # The leak check iterates `subs`, not `all_subs`, so `created` is never leak-checked (it is
    # not an identifier). `all_subs` is not re-sorted longest-first after the timestamp pair is
    # appended; that is harmless because no timestamp can contain or be contained in an
    # identifier.
    with zipfile.ZipFile(dst) as check:
        blob = "".join(check.read(n).decode("utf-8") + n for n in check.namelist())
    # A pair whose replacement equals its input was already scrubbed. Its survival is not a
    # leak, but the exemption is announced rather than applied silently, because it is the one
    # place a genuine leak could hide behind a no-op.
    already = [real for real, fake in subs if real == fake]
    if already:
        print(
            f"    {dst.name}: {len(already)}/{len(subs)} identifier fields were already "
            f"synthetic (idempotent re-scrub); leak check covers the remaining "
            f"{len(subs) - len(already)}"
        )
    for real, fake in subs:
        if real != fake and real in blob:
            raise SystemExit(f"scrub failed: {dst.name} still contains a real identifier")


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    src_dir, out_dir = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
    for zip_path in sorted(src_dir.glob("*.zip")):
        for word, out_name in OUT_NAMES.items():
            if word in zip_path.name:
                scrub(zip_path, out_dir / out_name)
                print(f"{zip_path.name} -> {out_name}")
                break
        else:
            print(f"skipped (unrecognised appliance): {zip_path.name}")


if __name__ == "__main__":
    main()
