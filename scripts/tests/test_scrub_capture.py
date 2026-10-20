"""Tests for the capture scrubber (``scrub-capture.py``) and the capture harness's send guard.

Nothing here touches ``secrets/``. Every identifier below is a synthetic stand-in with the
same *shape* as a real profile field, built into a throwaway profile ZIP in ``tmp_path``.

Run with ``just test-py``, or::

    PYTHONPATH=scripts python3 -m pytest scripts/tests -q

These do not run under ``cargo test`` or ``just ci``.
"""

from __future__ import annotations

import importlib.util
import json
import sys
import zipfile
from pathlib import Path

import pytest

SCRIPTS_DIR = Path(__file__).resolve().parents[1]
if str(SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPTS_DIR))


def _load_hyphenated(module_name: str, filename: str):
    """Import a script whose filename is not a valid Python identifier."""
    spec = importlib.util.spec_from_file_location(module_name, SCRIPTS_DIR / filename)
    if spec is None or spec.loader is None:  # pragma: no cover - defensive
        raise ImportError(f"cannot load {filename}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    spec.loader.exec_module(module)
    return module


import hc_aes  # noqa: E402
import hc_corpus  # noqa: E402

scrub = _load_hyphenated("scrub_capture", "scrub-capture.py")


# --------------------------------------------------------------------------------------
# A fake appliance profile. Same shapes as a real Home Connect profile JSON, no real data.
# --------------------------------------------------------------------------------------

FAKE_MAC_COMPACT = "0011223344AA"
FAKE_MAC_COLON = "00:11:22:33:44:AA"
FAKE_HA_ID = f"FAKE-Z99ZZ9ZZ9-{FAKE_MAC_COMPACT}"
FAKE_SERIAL = "440000000000000017"
FAKE_MAC_HYPHEN = "00-11-22-33-44-AA"
FAKE_PSK64 = hc_aes.b64url_encode(bytes(range(32)))
FAKE_IV64 = hc_aes.b64url_encode(bytes(range(100, 116)))

# Present only in the capture, never in the profile ZIP.
FAKE_PHONE_NAME = "XY-Z000B"
FAKE_PHONE_ID = "a1b2c3d4e5f60718"
FAKE_SSID = "SomeHomeNetworkName"
FAKE_IPV4 = "10.99.0.55"
FAKE_GATEWAY = "10.99.0.1"
FAKE_IPV6 = "fd00:abcd:1234:0:0211:22ff:fe33:44aa"

FAKE_PROFILE = {
    "haId": FAKE_HA_ID,
    "type": "Hob",
    "serialNumber": FAKE_SERIAL,
    "brand": "FAKE",
    "vib": "Z99ZZ9ZZ9",
    "mac": FAKE_MAC_COLON,
    "featureMappingFileName": f"{FAKE_HA_ID}_FeatureMapping.xml",
    "deviceDescriptionFileName": f"{FAKE_HA_ID}_DeviceDescription.xml",
    "created": "2026-08-03T00:00:00.000000+00:00",
    "connectionType": "AES",
    "key": FAKE_PSK64,
    "iv": FAKE_IV64,
}


# Frames in wire order. Key order inside every object is deliberate: the firmware parses
# objects positionally, and ``deviceType, deviceName, deviceID`` is hcpy's order.
FAKE_SESSION: list[tuple[str, str]] = [
    (
        "rx",
        '{"sID":1234567,"msgID":42,"resource":"/ei/initialValues","version":2,'
        '"action":"POST","data":[{"edMsgID":43}]}',
    ),
    (
        "tx",
        '{"sID":1234567,"msgID":42,"resource":"/ei/initialValues","version":2,'
        '"action":"RESPONSE","data":[{"deviceType":"Application","deviceName":"hcpy",'
        '"deviceID":"0badcafe"}]}',
    ),
    (
        "tx",
        '{"sID":1234567,"msgID":43,"resource":"/ci/services","version":1,"action":"GET"}',
    ),
    (
        "rx",
        '{"sID":1234567,"msgID":43,"resource":"/ci/services","version":1,'
        '"action":"RESPONSE","data":[{"service":"ci","version":2},'
        '{"service":"ni","version":1},{"service":"ei","version":2}]}',
    ),
    (
        "rx",
        '{"sID":1234567,"msgID":44,"resource":"/ci/info","version":2,"action":"RESPONSE",'
        f'"data":[{{"haId":"{FAKE_HA_ID}","serialNumber":"{FAKE_SERIAL}",'
        f'"mac":"{FAKE_MAC_COLON}","hwVersion":"1.0","swVersion":"2.0",'
        f'"deviceID":"{FAKE_MAC_COMPACT}"}}]}}',
    ),
    # Identifiers that exist ONLY in the capture, never in the profile ZIP. The real hob
    # session contains both of these shapes: a previously-paired phone in
    # /ci/registeredDevices, and the home network in /ni/info.
    (
        "rx",
        '{"sID":1234567,"msgID":46,"resource":"/ci/registeredDevices","version":2,'
        '"action":"NOTIFY","data":[{"endDeviceID":0,"deviceType":"Application",'
        f'"deviceName":"{FAKE_PHONE_NAME}","deviceID":"{FAKE_PHONE_ID}","connected":false}},'
        '{"endDeviceID":1,"deviceType":"Application","deviceName":"hcpy",'
        '"deviceID":"0badcafe","connected":true}]}',
    ),
    (
        "rx",
        '{"sID":1234567,"msgID":47,"resource":"/ni/info","version":1,"action":"RESPONSE",'
        f'"data":[{{"interfaceID":0,"type":"WiFi","ssid":"{FAKE_SSID}","rssi":-61,'
        f'"euiAddress":"{FAKE_MAC_HYPHEN}",'
        f'"ipV4":{{"ipAddress":"{FAKE_IPV4}","prefixSize":16,"gateway":"{FAKE_GATEWAY}"}},'
        f'"ipV6":{{"ipAddress":"{FAKE_IPV6}","prefixSize":64}}}}]}}',
    ),
    (
        "rx",
        '{"sID":1234567,"msgID":45,"resource":"/ro/values","version":1,"action":"NOTIFY",'
        '"data":[{"uid":517,"value":0}]}',
    ),
]


@pytest.fixture()
def fake_secrets_dir(tmp_path: Path) -> Path:
    """A throwaway ``secrets/``-shaped directory holding one fake profile ZIP."""
    secrets_dir = tmp_path / "secrets"
    secrets_dir.mkdir()
    zip_path = secrets_dir / "homeconnectdirect-hob-fake_2026-08-03_00-00-00.zip"
    with zipfile.ZipFile(zip_path, "w") as zf:
        zf.writestr(f"{FAKE_HA_ID}.json", json.dumps(FAKE_PROFILE))
        zf.writestr(f"{FAKE_HA_ID}_DeviceDescription.xml", "<device/>")
    return secrets_dir


@pytest.fixture()
def raw_capture() -> dict:
    """A tier-1 raw capture built by sealing FAKE_SESSION under the fake profile key."""
    return hc_corpus.build_capture(
        psk64=FAKE_PSK64,
        iv64=FAKE_IV64,
        frames=FAKE_SESSION,
        tier="raw",
        device={"alias": "hob", "host": "10.0.0.1", "transport": "aes"},
        pad_rng=hc_aes.ShakeRng(b"test-pad-seed"),
    )


@pytest.fixture()
def substitutions(fake_secrets_dir: Path) -> list:
    return scrub.build_substitutions(scrub.load_profiles(fake_secrets_dir))


@pytest.fixture()
def scrubbed(raw_capture: dict, substitutions: list) -> dict:
    return scrub.scrub_capture(raw_capture, substitutions)


def _all_real_strings(subs) -> set[str]:
    return {sub.real for sub in subs}


# --------------------------------------------------------------------------------------
# 1. scrub_replaces_every_known_identifier
# --------------------------------------------------------------------------------------


def test_scrub_replaces_every_known_identifier(scrubbed: dict, substitutions: list) -> None:
    """No real identifier — in any recorded form — survives into the scrubbed corpus."""
    blob = json.dumps(scrubbed, indent=1, sort_keys=False)
    assert substitutions, "the substitution table must not be empty"
    leaked = sorted({real for real in _all_real_strings(substitutions) if real in blob})
    assert leaked == [], f"real identifiers leaked into the scrubbed corpus: {leaked}"

    # And the same check the way the committed-file audit does it: over the decoded
    # plaintext bytes of every frame, not just the JSON envelope.
    for frame in scrubbed["frames"]:
        clear = bytes.fromhex(frame["plaintext_hex"]).decode("utf-8")
        for real in _all_real_strings(substitutions):
            assert real not in clear, f"{real!r} survived in frame {frame['seq']}"

    # residual_scan is the scrubber's own audit hook; it must agree.
    assert scrub.residual_scan(blob, substitutions) == []


def test_scrub_replaces_capture_only_identifiers(scrubbed: dict) -> None:
    """Identifiers that exist only in the capture must be scrubbed too.

    The real hob session carries a previously-paired phone's name and ID in
    ``NOTIFY /ci/registeredDevices``, and the home SSID, LAN addresses and a MAC-derived
    EUI-64 IPv6 address in ``/ni/info``. None of that is in the profile ZIP, so a substitution
    table built from ``secrets/*.zip`` alone would miss all of it.
    """
    blob = json.dumps(scrubbed, ensure_ascii=False)
    for leaked in (
        FAKE_PHONE_NAME,
        FAKE_PHONE_ID,
        FAKE_SSID,
        FAKE_IPV4,
        FAKE_GATEWAY,
        FAKE_IPV6,
    ):
        assert leaked not in blob, f"capture-only identifier {leaked!r} survived"


def test_scrub_keeps_hcpy_public_constants(scrubbed: dict) -> None:
    """``hcpy`` / ``0badcafe`` are hcpy source constants and must NOT be rewritten.

    The corpus is used to check that Kessel emits these constants byte for byte against the
    captured ``/ei/initialValues`` RESPONSE. Scrubbing them would destroy that comparison,
    and they leak nothing: they are the same on every hcpy installation.
    """
    response = next(
        f
        for f in scrubbed["frames"]
        if f["resource"] == "/ei/initialValues" and f["action"] == "RESPONSE"
    )
    assert '"deviceName":"hcpy"' in response["plaintext_utf8"]
    assert '"deviceID":"0badcafe"' in response["plaintext_utf8"]


def test_scrub_substitutes_rather_than_deletes(scrubbed: dict, substitutions: list) -> None:
    """Identifiers are *replaced*, not stripped — the synthetic value must be present."""
    blob = json.dumps(scrubbed)
    # Several rows share a field (case variants); keep the first, which is the form the
    # session actually carries.
    by_field: dict = {}
    for sub in substitutions:
        by_field.setdefault(sub.field, sub)
    for field in ("haId", "serialNumber", "mac"):
        assert by_field[field].synthetic in blob, f"synthetic {field} missing from output"

    # And the published set describes what was actually substituted in this capture.
    published = {entry["synthetic"] for entry in scrubbed["synthetic_identifiers"]}
    assert by_field["haId"].synthetic in published
    assert by_field["serialNumber"].synthetic in published


# --------------------------------------------------------------------------------------
# 2. scrub_preserves_shape
# --------------------------------------------------------------------------------------


def test_scrub_preserves_shape(substitutions: list) -> None:
    """Every substitution has the same length and the same per-character class.

    Same length and character class means parser behaviour is unchanged.

    The ``key-material`` rows (the profile ``key``/``iv``) are length-checked but exempt from
    the character-class check: they map onto the *single published synthetic key* the whole
    corpus is encrypted under, so their replacement is a fixed constant and cannot track the
    real value's classes. See ``Substitution``'s docstring. Nothing in a plaintext frame is
    expected to be key material; those rows exist as defence in depth.
    """
    assert substitutions
    assert any(sub.kind == "identifier" for sub in substitutions)
    for sub in substitutions:
        assert len(sub.synthetic) == len(sub.real), (
            f"{sub.role}: length {len(sub.synthetic)} != {len(sub.real)}"
        )
        assert sub.synthetic != sub.real, f"{sub.role}: substitution is a no-op"
        if sub.kind != "identifier":
            continue
        real_classes = [scrub.char_class(c) for c in sub.real]
        synth_classes = [scrub.char_class(c) for c in sub.synthetic]
        assert synth_classes == real_classes, f"{sub.role}: character class drift"


def test_substitution_roles_are_unique_per_profile(substitutions: list) -> None:
    """Four appliances each have a ``haId``; roles must not collapse them onto one row."""
    roles = [sub.role for sub in substitutions]
    assert len(roles) == len(set(roles)), "duplicate substitution roles"


def test_scrub_preserves_hex_alphabet_for_hex_tokens(substitutions: list) -> None:
    """A hex-shaped identifier stays hex-shaped, so hex parsers behave identically."""
    mac = next(sub for sub in substitutions if sub.field == "mac" and ":" in sub.real)
    assert all(c in "0123456789ABCDEF:" for c in mac.synthetic), mac.synthetic


def test_scrub_preserves_frame_lengths(raw_capture: dict, scrubbed: dict) -> None:
    """Shape preservation implies the padded frame lengths are unchanged."""
    assert len(scrubbed["frames"]) == len(raw_capture["frames"])
    for before, after in zip(raw_capture["frames"], scrubbed["frames"]):
        assert len(after["plaintext_hex"]) == len(before["plaintext_hex"])
        assert after["pad_len"] == before["pad_len"]
        assert len(after["wire_hex"]) == len(before["wire_hex"])


# --------------------------------------------------------------------------------------
# 3. scrub_is_deterministic
# --------------------------------------------------------------------------------------


def test_scrub_is_deterministic(raw_capture: dict, fake_secrets_dir: Path) -> None:
    """Same input, same output, twice — including the random-looking pad bytes."""
    first = scrub.scrub_capture(
        raw_capture, scrub.build_substitutions(scrub.load_profiles(fake_secrets_dir))
    )
    second = scrub.scrub_capture(
        raw_capture, scrub.build_substitutions(scrub.load_profiles(fake_secrets_dir))
    )
    assert json.dumps(first, sort_keys=True) == json.dumps(second, sort_keys=True)


def test_synthetic_key_is_a_fixed_documented_constant(scrubbed: dict) -> None:
    """The synthetic key is a published constant, not derived from the real one."""
    assert scrubbed["key"]["psk64"] == scrub.SYNTHETIC_PSK64
    assert scrubbed["key"]["iv64"] == scrub.SYNTHETIC_IV64
    assert scrubbed["key"]["psk64"] != FAKE_PSK64
    assert len(hc_aes.b64url_decode(scrubbed["key"]["psk64"])) == 32
    assert len(hc_aes.b64url_decode(scrubbed["key"]["iv64"])) == 16


# --------------------------------------------------------------------------------------
# 4. scrubbed_corpus_chains_are_internally_consistent
# --------------------------------------------------------------------------------------


def test_scrubbed_corpus_chains_are_internally_consistent(scrubbed: dict) -> None:
    """The re-encrypted session verifies end to end under the synthetic key.

    ``verify_capture`` re-runs both directions from scratch: HMAC chain state, CBC
    continuity, frame lengths, the custom padding rules, and that every
    recorded plaintext is what the recorded wire bytes actually decrypt to.
    """
    problems = hc_corpus.verify_capture(scrubbed)
    assert problems == [], "\n".join(problems)


def test_raw_capture_also_verifies(raw_capture: dict) -> None:
    """Guard on the test fixture itself: the *unscrubbed* capture must verify too.

    Without this, test 4 could pass against a verifier that accepts anything.
    """
    assert hc_corpus.verify_capture(raw_capture) == []


def test_verify_capture_rejects_a_tampered_frame(scrubbed: dict) -> None:
    """Negative control for the verifier — flipping one ciphertext byte must be caught."""
    import copy

    broken = copy.deepcopy(scrubbed)
    wire = bytearray.fromhex(broken["frames"][0]["wire_hex"])
    wire[0] ^= 0x01
    broken["frames"][0]["wire_hex"] = wire.hex()
    assert hc_corpus.verify_capture(broken) != []


# --------------------------------------------------------------------------------------
# 5. scrub_preserves_json_key_order
# --------------------------------------------------------------------------------------


def test_scrub_preserves_json_key_order(raw_capture: dict, scrubbed: dict) -> None:
    """Key order in the stored plaintext bytes is byte-for-byte preserved.

    The firmware parses JSON objects positionally, so the corpus is compared on *bytes*; a
    scrubber that round-tripped through a dict could silently reorder keys.
    """
    for before, after in zip(raw_capture["frames"], scrubbed["frames"]):
        old = bytes.fromhex(before["plaintext_hex"]).decode("utf-8")
        new = bytes.fromhex(after["plaintext_hex"]).decode("utf-8")
        assert scrub.json_key_sequence(new) == scrub.json_key_sequence(old)

    response = next(
        f
        for f in scrubbed["frames"]
        if f["resource"] == "/ei/initialValues" and f["action"] == "RESPONSE"
    )
    clear = bytes.fromhex(response["plaintext_hex"]).decode("utf-8")
    # The top-level order every reference implementation emits.
    assert clear.index('"sID"') < clear.index('"msgID"') < clear.index('"resource"')
    assert clear.index('"resource"') < clear.index('"version"') < clear.index('"action"')
    assert clear.index('"action"') < clear.index('"data"')
    # hcpy's data[0] order — NOT Go's alphabetical deviceID/deviceName/deviceType.
    assert clear.index('"deviceType"') < clear.index('"deviceName"')
    assert clear.index('"deviceName"') < clear.index('"deviceID"')


def test_plaintext_bytes_are_stored_not_only_parsed_json(scrubbed: dict) -> None:
    """Raw plaintext bytes are mandatory; parsed JSON is optional."""
    for frame in scrubbed["frames"]:
        assert "plaintext_hex" in frame, "raw plaintext bytes must be stored"
        clear = bytes.fromhex(frame["plaintext_hex"])
        assert frame["plaintext_utf8"] == clear.decode("utf-8")
        # The convenience decode must agree with the bytes, never replace them.
        assert json.loads(frame["plaintext_utf8"]) == frame["json"]


# --------------------------------------------------------------------------------------
# The device-safety guard, at its choke point.
#
# `capture.py` enforces the read-only allowlist inside the `encrypt` wrapper rather than only
# on `send`, because `encrypt` is the last place the frame is still clear JSON: a check there
# cannot be routed around by any hcpy call path, present or future. These tests reach past
# `send` and call `encrypt` directly, which is exactly the bypass the placement exists to
# close. This project can physically start a cooktop.
# --------------------------------------------------------------------------------------

import capture  # noqa: E402


class _FakeSocket:
    """The surface `capture.instrument` wraps. Records whether the cipher was ever reached."""

    def __init__(self) -> None:
        self.encrypted: list[str] = []
        self.sent: list[dict] = []
        self.last_tx_hmac = bytes(16)
        self.last_rx_hmac = bytes(16)

    def send(self, msg):
        self.sent.append(msg)

    def encrypt(self, clear_msg):
        self.encrypted.append(clear_msg)
        return b"\x00" * 48

    def decrypt(self, buf):  # pragma: no cover - not exercised here
        return None


def _instrumented() -> _FakeSocket:
    socket_obj = _FakeSocket()
    recorder = capture.Recorder(bytes(range(32)), bytes(range(100, 116)))
    capture.instrument(socket_obj, recorder)
    return socket_obj


def test_post_reaching_encrypt_directly_is_refused() -> None:
    """A write that bypasses `send` entirely must still never reach the cipher."""
    socket_obj = _instrumented()
    frame = json.dumps(
        {"sID": 1, "msgID": 2, "resource": "/ro/values", "version": 1, "action": "POST",
         "data": [{"uid": 517, "value": 1}]},
        separators=(",", ":"),
    )

    with pytest.raises(capture.UnsafeFrame) as excinfo:
        socket_obj.encrypt(frame)

    assert "POST" in str(excinfo.value)
    # The decisive assertion: hcpy's real `encrypt` was never called, so nothing was ever
    # turned into bytes. Refusing *after* encryption would be no guard at all.
    assert socket_obj.encrypted == []


def test_encrypt_refuses_every_state_changing_action_and_unlisted_resource() -> None:
    """The unconditional action check runs first; the allowlist is not a fallthrough."""
    for action, resource in [
        ("POST", "/ro/values"),
        ("POST", "/ro/activeProgram"),
        ("PUT", "/ro/values"),
        ("DELETE", "/ro/activeProgram"),
        # Allowlisted *resource*, forbidden action: proves the action check is not skipped
        # for anything that happens to appear in ALLOWED_SENDS.
        ("POST", "/ei/initialValues"),
        # Permitted action, unlisted resource.
        ("GET", "/ro/values"),
    ]:
        socket_obj = _instrumented()
        frame = json.dumps({"action": action, "resource": resource}, separators=(",", ":"))
        with pytest.raises(capture.UnsafeFrame):
            socket_obj.encrypt(frame)
        assert socket_obj.encrypted == [], f"{action} {resource} reached the cipher"


def test_encrypt_fails_closed_on_anything_it_cannot_parse() -> None:
    """An unparseable frame is refused, not waved through."""
    for payload in ["not json", "[]", '"a string"', b"\xff\xfe", 12345]:
        socket_obj = _instrumented()
        with pytest.raises(capture.UnsafeFrame):
            socket_obj.encrypt(payload)
        assert socket_obj.encrypted == []


def test_encrypt_still_passes_an_allowlisted_read_through() -> None:
    """Negative control: the guard must not refuse the handshake it exists to record."""
    socket_obj = _instrumented()
    frame = json.dumps(
        {"sID": 1, "msgID": 2, "resource": "/ci/services", "version": 1, "action": "GET"},
        separators=(",", ":"),
    )
    with pytest.raises(ValueError):
        # The fake cipher returns a fixed 48-byte buffer that is not a valid frame, so the
        # recorder's mirror Opener rejects it. Reaching that failure is the proof the guard
        # let the frame through; `encrypted` below is the direct evidence.
        socket_obj.encrypt(frame)
    assert socket_obj.encrypted == [frame]
