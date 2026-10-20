"""The capture/corpus file format, and its end-to-end verifier.

Two tiers share one schema:

===========  ==========================  ==========  ==================================
tier         location                    committed?  content
===========  ==========================  ==========  ==================================
``raw``      ``secrets/captures/<dev>/``  **no**      exact bytes, real psk/iv, real JSON
``golden``   ``fixtures/aes/``            **yes**     identifiers rewritten, re-encrypted
===========  ==========================  ==========  ==================================

The tiers differ only in ``tier``, ``key`` and the identifier values inside the plaintexts.
A single verifier therefore checks both: the same code that says the real capture is
coherent says the scrubbed one is.

Per-frame layout:

``seq``
    position in the session, both directions interleaved in wire order.
``direction``
    ``"tx"`` = client -> appliance (direction byte ``0x45``);
    ``"rx"`` = appliance -> client (direction byte ``0x43``).
``wire_hex`` / ``ciphertext_hex`` / ``mac_hex``
    the raw bytes. ``wire_hex == ciphertext_hex + mac_hex``.
``plaintext_hex``
    **the load-bearing field.** The exact UTF-8 bytes of the decrypted message, so JSON key
    order survives; the firmware parses objects positionally, and a capture stored as parsed
    JSON would lose that order. Compare on these bytes, never on a parsed value.
``plaintext_utf8`` / ``json``
    readability only, stored *alongside* ``plaintext_hex`` and never instead of it.
``pad_len`` / ``pad_hex``
    the custom pad. Recorded so the frame is exactly reproducible.
``msg_id`` / ``s_id`` / ``resource`` / ``action`` / ``version`` / ``code``
    lifted from the JSON for convenience; ``null`` when absent.
``chain``
    ``direction_hmac_before``/``_after`` and ``cbc_iv_before``/``_after`` for this frame's own
    direction, plus ``tx_hmac_after`` and ``rx_hmac_after`` — **both** chain states as of this
    frame.
"""

from __future__ import annotations

import json
from typing import Any, Iterable

import hc_aes

SCHEMA = "kessel.capture/1"


def _lift(doc: Any, key: str) -> Any:
    return doc.get(key) if isinstance(doc, dict) else None


def make_frame_record(
    *,
    seq: int,
    direction: str,
    t_ms: float,
    frame: hc_aes.SealedFrame,
    tx_hmac_after: bytes,
    rx_hmac_after: bytes,
) -> dict:
    """Build one frame record from a sealed/opened frame plus the session-wide chain state."""
    if direction not in ("tx", "rx"):
        raise ValueError(f"direction must be 'tx' or 'rx', got {direction!r}")
    clear = frame.clear
    text = clear.decode("utf-8")
    doc = json.loads(text)
    return {
        "seq": seq,
        "direction": direction,
        "t_ms": round(t_ms, 3),
        "wire_hex": frame.wire.hex(),
        "ciphertext_hex": frame.ciphertext.hex(),
        "mac_hex": frame.mac.hex(),
        "plaintext_hex": clear.hex(),
        "plaintext_utf8": text,
        "json": doc,
        "pad_len": frame.pad_len,
        "pad_hex": frame.pad.hex(),
        "s_id": _lift(doc, "sID"),
        "msg_id": _lift(doc, "msgID"),
        "resource": _lift(doc, "resource"),
        "version": _lift(doc, "version"),
        "action": _lift(doc, "action"),
        "code": _lift(doc, "code"),
        "chain": {
            "direction_hmac_before": frame.hmac_before.hex(),
            "direction_hmac_after": frame.hmac_after.hex(),
            "cbc_iv_before": frame.cbc_iv_before.hex(),
            "cbc_iv_after": frame.cbc_iv_after.hex(),
            "tx_hmac_after": tx_hmac_after.hex(),
            "rx_hmac_after": rx_hmac_after.hex(),
        },
    }


class SessionSealer:
    """Seals a whole session — both directions — under one key, tracking both chains.

    Used by the scrubber to re-run a capture under the synthetic key, and by the tests to
    synthesise a capture in the first place. The ``rx`` direction is sealed as the
    *appliance* (direction byte ``0x43``).
    """

    def __init__(self, psk: bytes, iv: bytes, rng) -> None:
        self.tx = hc_aes.Sealer(psk, iv, hc_aes.DIRECTION_TX, rng)
        self.rx = hc_aes.Sealer(psk, iv, hc_aes.DIRECTION_RX, rng)

    def seal(
        self, seq: int, direction: str, clear: bytes, *, t_ms: float = 0.0, pad: bytes | None = None
    ) -> dict:
        sealer = self.tx if direction == "tx" else self.rx
        frame = sealer.seal(clear, pad=pad)
        return make_frame_record(
            seq=seq,
            direction=direction,
            t_ms=t_ms,
            frame=frame,
            tx_hmac_after=self.tx.last_hmac,
            rx_hmac_after=self.rx.last_hmac,
        )


def build_capture(
    *,
    psk64: str,
    iv64: str,
    frames: Iterable[tuple[str, str]],
    tier: str,
    device: dict,
    pad_rng,
    meta: dict | None = None,
) -> dict:
    """Seal a list of ``(direction, plaintext_str)`` pairs into a capture document."""
    psk = hc_aes.b64url_decode(psk64)
    iv = hc_aes.b64url_decode(iv64)
    session = SessionSealer(psk, iv, pad_rng)
    records = [
        session.seal(seq, direction, clear.encode("utf-8"), t_ms=float(seq))
        for seq, (direction, clear) in enumerate(frames)
    ]
    doc = {
        "schema": SCHEMA,
        "tier": tier,
        "device": device,
        "key": {"psk64": psk64, "iv64": iv64},
        "frames": records,
    }
    if meta:
        doc.update(meta)
    return doc


# ---------------------------------------------------------------------------------------
# Verification
# ---------------------------------------------------------------------------------------


def verify_capture(capture: dict) -> list[str]:
    """Re-run both chains from scratch and report every inconsistency found.

    Checks:

    * framing: every frame is >= 32 bytes and a multiple of 16.
    * the recorded MAC is the one the rolling HMAC chain produces, per direction.
    * CBC continuity: each frame's CBC input block is the previous frame's last
      ciphertext block in the *same* direction, and the ``iv`` only for the first.
    * the recorded ``wire_hex`` actually decrypts to the recorded ``plaintext_hex``.
    * the recorded pad obeys the custom padding rules.
    * the recorded chain states (``chain.*``) match the recomputation.
    * ``plaintext_utf8`` and ``json`` agree with ``plaintext_hex``.

    Returns an empty list when the capture is coherent.
    """
    problems: list[str] = []
    try:
        psk = hc_aes.b64url_decode(capture["key"]["psk64"])
        iv = hc_aes.b64url_decode(capture["key"]["iv64"])
    except Exception as exc:  # pragma: no cover - malformed document
        return [f"key material is unusable: {exc}"]

    openers = {
        "tx": hc_aes.Opener(psk, iv, hc_aes.DIRECTION_TX),
        "rx": hc_aes.Opener(psk, iv, hc_aes.DIRECTION_RX),
    }
    last_after = {"tx": bytes(hc_aes.MAC_LENGTH), "rx": bytes(hc_aes.MAC_LENGTH)}

    for index, record in enumerate(capture.get("frames", [])):
        tag = f"frame {record.get('seq', index)}"
        direction = record.get("direction")
        if direction not in openers:
            problems.append(f"{tag}: bad direction {direction!r}")
            continue
        try:
            wire = bytes.fromhex(record["wire_hex"])
        except Exception as exc:
            problems.append(f"{tag}: wire_hex is not hex: {exc}")
            continue

        if len(wire) < hc_aes.MINIMUM_MESSAGE_LENGTH:
            problems.append(f"{tag}: wire length {len(wire)} < {hc_aes.MINIMUM_MESSAGE_LENGTH}")
            continue
        if len(wire) % hc_aes.BLOCK_SIZE != 0:
            problems.append(f"{tag}: wire length {len(wire)} is not a multiple of 16")
            continue
        if record.get("ciphertext_hex", "") + record.get("mac_hex", "") != record["wire_hex"]:
            problems.append(f"{tag}: ciphertext_hex + mac_hex != wire_hex")

        opener = openers[direction]
        chain = record.get("chain", {})
        expected_before = opener.last_hmac.hex()
        expected_cbc_before = opener.cbc_iv.hex()

        try:
            opened = opener.open(wire)
        except ValueError as exc:
            problems.append(f"{tag}: {exc}")
            # Once a chain's position is uncertain, everything after it is unverifiable. Stop rather than emit a cascade of derived failures.
            problems.append(f"{tag}: chain desynchronised, stopping verification here")
            break

        if chain.get("direction_hmac_before") != expected_before:
            problems.append(
                f"{tag}: chain.direction_hmac_before {chain.get('direction_hmac_before')} "
                f"!= {expected_before}"
            )
        if chain.get("direction_hmac_after") != opened.hmac_after.hex():
            problems.append(f"{tag}: chain.direction_hmac_after does not match recomputation")
        if chain.get("cbc_iv_before") != expected_cbc_before:
            problems.append(
                f"{tag}: chain.cbc_iv_before {chain.get('cbc_iv_before')} "
                f"!= {expected_cbc_before} (CBC continuity broken)"
            )
        if chain.get("cbc_iv_after") != opened.cbc_iv_after.hex():
            problems.append(f"{tag}: chain.cbc_iv_after does not match recomputation")

        last_after[direction] = opened.hmac_after
        if chain.get("tx_hmac_after") != last_after["tx"].hex():
            problems.append(f"{tag}: chain.tx_hmac_after does not match recomputation")
        if chain.get("rx_hmac_after") != last_after["rx"].hex():
            problems.append(f"{tag}: chain.rx_hmac_after does not match recomputation")

        if opened.clear.hex() != record.get("plaintext_hex"):
            problems.append(f"{tag}: decrypted plaintext does not match plaintext_hex")
        if opened.pad.hex() != record.get("pad_hex"):
            problems.append(f"{tag}: recovered pad does not match pad_hex")

        pad = opened.pad
        if record.get("pad_len") != pad[-1]:
            problems.append(f"{tag}: pad_len {record.get('pad_len')} != trailing byte {pad[-1]}")
        if pad[0] != 0x00:
            problems.append(f"{tag}: first pad byte is {pad[0]:#04x}, expected 0x00")
        if len(pad) != pad[-1]:
            problems.append(f"{tag}: pad is {len(pad)} bytes but declares {pad[-1]}")
        if pad[-1] == 1 or not (2 <= pad[-1] <= 17):
            problems.append(f"{tag}: pad_len {pad[-1]} outside the legal set {{2..16}} u {{17}}")
        if pad[-1] != hc_aes.pad_length(len(opened.clear)):
            problems.append(
                f"{tag}: pad_len {pad[-1]} != {hc_aes.pad_length(len(opened.clear))} "
                f"required for a {len(opened.clear)}-byte plaintext"
            )

        try:
            text = opened.clear.decode("utf-8")
        except UnicodeDecodeError as exc:
            problems.append(f"{tag}: plaintext is not valid UTF-8: {exc}")
            continue
        if record.get("plaintext_utf8") != text:
            problems.append(f"{tag}: plaintext_utf8 disagrees with plaintext_hex")
        try:
            if record.get("json") != json.loads(text):
                problems.append(f"{tag}: json disagrees with plaintext_hex")
        except json.JSONDecodeError as exc:
            problems.append(f"{tag}: plaintext is not valid JSON: {exc}")

    return problems


def load(path) -> dict:
    with open(path, "r", encoding="utf-8") as handle:
        return json.load(handle)


def dump(capture: dict, path) -> None:
    """Write a capture. ``sort_keys`` is deliberately off — insertion order is the record."""
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(capture, handle, indent=1, ensure_ascii=False, sort_keys=False)
        handle.write("\n")
