#!/usr/bin/env python3
"""Record a real Home Connect AES session as a tier-1 raw capture.

The session is driven by **hcpy**, unmodified, because hcpy is the reference implementation
and the point of the corpus is to pin *its* bytes. This script only observes: it wraps ``HCSocket.send``, ``HCSocket.encrypt`` and
``HCSocket.decrypt`` to record every frame, and re-derives the crypto chain independently so
that the recorded chain state is checked against hcpy's rather than copied from it.

Output goes to ``secrets/captures/<alias>/<timestamp>.json`` — **gitignored, tier 1, real key
material and real identifiers**. Run ``scrub-capture.py`` to produce the committed corpus.

  ⚠ **READ-ONLY BY CONSTRUCTION.** This project can physically start a cooktop. Every outbound frame is checked against
  ``ALLOWED_SENDS`` inside the ``encrypt`` wrapper — the last point at which the frame is
  still clear JSON, and the one gate no hcpy call path can route around. ``send`` re-checks
  earlier for a clearer error, but the guard does not depend on it. Anything not allowlisted
  aborts the capture. There is no flag to disable either check and no code path here that
  writes an appliance value. ``NOTIFY /ro/values`` frames arrive when a human physically
  touches the appliance or when it changes state on its own — never provoked by a write.

  The target address is pinned to the alias's entry in ``secrets/IP.md``; departing from it
  requires ``--allow-unpinned-host``.

Usage::

    PYTHONPATH=reference/hcpy scripts/capture.py --alias hob --duration 120

See ``scripts/capture.md`` for the full procedure.
"""

from __future__ import annotations

import argparse
import datetime as _dt
import json
import os
import re
import sys
import threading
import time
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import hc_aes  # noqa: E402
import hc_corpus  # noqa: E402

# ---------------------------------------------------------------------------------------
# Safety
# ---------------------------------------------------------------------------------------

#: The complete set of ``(action, resource)`` pairs hcpy's handshake emits, and the only
#: frames this harness will let onto the wire, taken from hcpy's ``HCDevice.reconnect``.
#: Everything here is a read or a fire-and-forget
#: notify; none of it changes appliance state.
ALLOWED_SENDS = frozenset(
    {
        ("RESPONSE", "/ei/initialValues"),
        ("GET", "/ci/services"),
        ("GET", "/ci/authentication"),
        ("GET", "/ci/info"),
        ("GET", "/iz/info"),
        ("NOTIFY", "/ei/deviceReady"),
        ("GET", "/ni/info"),
        ("GET", "/ro/allMandatoryValues"),
        ("GET", "/ro/allDescriptionChanges"),
    }
)

#: Actions that can change appliance state. Rejected unconditionally, before the allowlist,
#: so that adding a resource to ALLOWED_SENDS can never accidentally permit a write.
FORBIDDEN_ACTIONS = frozenset({"POST", "PUT", "DELETE"})


class UnsafeFrame(RuntimeError):
    """Raised when something tried to send a frame this harness must never send."""


def assert_send_is_safe(msg: dict) -> None:
    if not isinstance(msg, dict):
        raise UnsafeFrame(f"refusing to send a non-object frame of type {type(msg).__name__}")
    action = str(msg.get("action", ""))
    resource = str(msg.get("resource", ""))
    if action in FORBIDDEN_ACTIONS:
        raise UnsafeFrame(f"refusing to send a state-changing {action} to {resource}")
    if (action, resource) not in ALLOWED_SENDS:
        raise UnsafeFrame(f"refusing to send {action} {resource}: not in the read-only allowlist")


def assert_clear_frame_is_safe(clear_msg) -> None:
    """The same check, applied to the clear JSON *as it is handed to the cipher*.

    This is the narrowest choke point there is: ``encrypt`` is the last place the frame exists
    as clear JSON before it becomes bytes on the wire, so a check here cannot be routed around
    by any hcpy code path — present or future — that reaches the cipher without going through
    ``HCSocket.send``. The duplicate check on ``send`` is kept for the earlier, clearer error;
    this one is the one that has to hold.

    Fails closed: anything that cannot be decoded into a JSON object is refused rather than
    passed through, because an unparseable frame is a frame this harness cannot vouch for.
    """
    if isinstance(clear_msg, (bytes, bytearray)):
        try:
            clear_msg = bytes(clear_msg).decode("utf-8")
        except UnicodeDecodeError as exc:
            raise UnsafeFrame(f"refusing to encrypt undecodable frame bytes: {exc}") from exc
    if not isinstance(clear_msg, str):
        raise UnsafeFrame(
            f"refusing to encrypt a frame of unexpected type {type(clear_msg).__name__}"
        )
    try:
        msg = json.loads(clear_msg)
    except ValueError as exc:
        raise UnsafeFrame(f"refusing to encrypt a frame that is not JSON: {exc}") from exc
    assert_send_is_safe(msg)


# ---------------------------------------------------------------------------------------
# Profiles
# ---------------------------------------------------------------------------------------


def load_profile(secrets_dir: Path, alias: str) -> dict:
    """Find the appliance profile ZIP whose filename contains ``alias``."""
    matches = [p for p in sorted(secrets_dir.glob("*.zip")) if alias.lower() in p.name.lower()]
    if len(matches) != 1:
        raise SystemExit(
            f"expected exactly one profile ZIP matching {alias!r} in {secrets_dir}, "
            f"found {len(matches)}"
        )
    with zipfile.ZipFile(matches[0]) as archive:
        names = [n for n in archive.namelist() if n.lower().endswith(".json")]
        if len(names) != 1:
            raise SystemExit(f"expected exactly one profile JSON in {matches[0].name}")
        profile = json.loads(archive.read(names[0]))
    profile["_zip"] = matches[0].name
    return profile


def load_host(secrets_dir: Path, alias: str) -> str:
    """Read ``secrets/IP.md``, which maps ``<alias>: <ip>`` one per line."""
    ip_file = secrets_dir / "IP.md"
    if not ip_file.is_file():
        raise SystemExit(f"{ip_file} not found; pass --host explicitly")
    for line in ip_file.read_text(encoding="utf-8").splitlines():
        match = re.match(r"^\s*([A-Za-z0-9_-]+)\s*:\s*(\S+)\s*$", line)
        if match and match.group(1).lower() == alias.lower():
            return match.group(2)
    raise SystemExit(f"no entry for {alias!r} in {ip_file}; pass --host explicitly")


def lookup_host(secrets_dir: Path, alias: str) -> str | None:
    """``load_host`` without the exit: ``None`` when the alias cannot be pinned."""
    try:
        return load_host(secrets_dir, alias)
    except SystemExit:
        return None


def resolve_host(secrets_dir: Path, alias: str, override: str | None, unpinned_ok: bool) -> str:
    """Resolve the target address and refuse to depart from ``secrets/IP.md`` silently.

    Without this, ``--host <oven> --alias hob`` would open a socket to the wrong appliance
    with nothing objecting. The address is pinned to the alias's entry in ``secrets/IP.md``
    and departing from it takes an explicit ``--allow-unpinned-host``. This project can
    physically start a cooktop; which box you are talking to is not something to get right
    by accident.
    """
    pinned = lookup_host(secrets_dir, alias)
    if override is None:
        if pinned is None:
            raise SystemExit(
                f"no entry for {alias!r} in {secrets_dir / 'IP.md'}; pass --host together with "
                f"--allow-unpinned-host if you really mean to target an unlisted address"
            )
        return pinned
    if pinned is not None and override == pinned:
        return override
    if not unpinned_ok:
        listed = f"{pinned!r}" if pinned is not None else "no entry"
        raise SystemExit(
            f"--host {override!r} does not match {alias!r} in {secrets_dir / 'IP.md'} "
            f"({listed}). Refusing: this harness may only be pointed at the appliance it holds "
            f"a device lock on. Re-run with --allow-unpinned-host if the override is deliberate."
        )
    print(
        f"  ⚠ UNPINNED HOST: {override} is not {alias!r}'s entry in secrets/IP.md. "
        f"You asserted this deliberately with --allow-unpinned-host.",
        file=sys.stderr,
    )
    return override


# ---------------------------------------------------------------------------------------
# The recorder
# ---------------------------------------------------------------------------------------


class Recorder:
    """Observes an ``HCSocket`` and accumulates tier-1 frame records.

    The chain state is re-derived here with :mod:`hc_aes` rather than read out of hcpy, and
    then cross-checked against hcpy's own ``last_tx_hmac``/``last_rx_hmac`` after every frame.
    A mismatch aborts: if our chain and hcpy's disagree, the capture is worthless.

    A mirror :class:`hc_aes.Opener` per direction recovers the padding bytes, which are
    otherwise unrecoverable — hcpy discards the pad on receive and never surfaces it on send.
    """

    def __init__(self, psk: bytes, iv: bytes) -> None:
        self.tx_opener = hc_aes.Opener(psk, iv, hc_aes.DIRECTION_TX)
        self.rx_opener = hc_aes.Opener(psk, iv, hc_aes.DIRECTION_RX)
        self.frames: list[dict] = []
        self.send_lock = threading.RLock()
        self._lock = threading.Lock()
        self._t0 = time.monotonic()
        self.errors: list[str] = []

    def record(self, direction: str, wire: bytes) -> None:
        opener = self.tx_opener if direction == "tx" else self.rx_opener
        with self._lock:
            frame = opener.open(wire)
            self.frames.append(
                hc_corpus.make_frame_record(
                    seq=len(self.frames),
                    direction=direction,
                    t_ms=(time.monotonic() - self._t0) * 1000.0,
                    frame=frame,
                    tx_hmac_after=self.tx_opener.last_hmac,
                    rx_hmac_after=self.rx_opener.last_hmac,
                )
            )

    def cross_check(self, socket_obj) -> None:
        """Our recomputed chains must equal hcpy's after every frame."""
        if self.tx_opener.last_hmac != socket_obj.last_tx_hmac:
            self.errors.append("tx chain diverged from hcpy")
        if self.rx_opener.last_hmac != socket_obj.last_rx_hmac:
            self.errors.append("rx chain diverged from hcpy")

    def summary(self) -> str:
        counts: dict[str, int] = {}
        for frame in self.frames:
            key = f"{frame['direction']} {frame['action']} {frame['resource']}"
            counts[key] = counts.get(key, 0) + 1
        return "\n".join(f"  {count:>3} x {key}" for key, count in sorted(counts.items()))


def instrument(socket_obj, recorder: Recorder) -> None:
    """Wrap send/encrypt/decrypt on a live ``HCSocket`` instance."""
    original_send = socket_obj.send
    original_encrypt = socket_obj.encrypt
    original_decrypt = socket_obj.decrypt

    def send(msg):
        # An earlier, clearer error on the frame as hcpy composed it. The check inside `encrypt`
        # below is the one that has to hold.
        assert_send_is_safe(msg)
        # hcpy has no send lock; encrypt+write must be atomic or the peer desyncs. Holding it here also makes the recorded order the wire order.
        with recorder.send_lock:
            return original_send(msg)

    def encrypt(clear_msg):
        # The choke point: nothing reaches the cipher, and therefore the wire, without passing
        # here, whatever hcpy's call graph looks like after an upgrade.
        assert_clear_frame_is_safe(clear_msg)
        wire = original_encrypt(clear_msg)
        recorder.record("tx", wire)
        recorder.cross_check(socket_obj)
        return wire

    def decrypt(buf):
        recorder.record("rx", buf)
        result = original_decrypt(buf)
        recorder.cross_check(socket_obj)
        return result

    socket_obj.send = send
    socket_obj.encrypt = encrypt
    socket_obj.decrypt = decrypt


# ---------------------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------------------


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--alias", default="hob", help="appliance alias, matched against the ZIP")
    parser.add_argument(
        "--host",
        default=None,
        help="override the address from secrets/IP.md; must match the alias's entry unless "
        "--allow-unpinned-host is also given",
    )
    parser.add_argument(
        "--allow-unpinned-host",
        action="store_true",
        help="permit --host to differ from the alias's entry in secrets/IP.md. Do not use this "
        "to point the harness at an appliance you do not hold the device lock for.",
    )
    parser.add_argument(
        "--secrets-dir",
        type=Path,
        default=Path(os.environ.get("KESSEL_SECRETS_DIR", "secrets")),
    )
    parser.add_argument("--hcpy", type=Path, default=Path("reference/hcpy"))
    parser.add_argument(
        "--duration",
        type=float,
        default=120.0,
        help="seconds to stay connected after the handshake settles",
    )
    parser.add_argument("--out", type=Path, default=None)
    args = parser.parse_args(argv)

    sys.path.insert(0, str(args.hcpy.resolve()))
    from HCDevice import HCDevice  # noqa: E402
    from HCSocket import HCSocket  # noqa: E402

    profile = load_profile(args.secrets_dir, args.alias)
    if profile.get("connectionType") != "AES":
        raise SystemExit(
            f"{args.alias} has connectionType {profile.get('connectionType')!r}; this harness "
            "captures the AES transport only"
        )
    host = resolve_host(args.secrets_dir, args.alias, args.host, args.allow_unpinned_host)

    print("=" * 78)
    print(f"  READ-ONLY capture of {args.alias} at {host}")
    print(f"  {len(ALLOWED_SENDS)} outbound frames are permitted; every write action is refused.")
    print("=" * 78)

    psk = hc_aes.b64url_decode(profile["key"])
    iv = hc_aes.b64url_decode(profile["iv"])
    recorder = Recorder(psk, iv)

    socket_obj = HCSocket(host, profile["key"], profile["iv"], debug=False)
    device = HCDevice(socket_obj, {"name": args.alias, "features": {}}, debug=False)

    stop = threading.Event()
    failure: list[BaseException] = []

    def on_open(_ws):
        print("connected; handshake starting")

    def on_close(_ws, code, message):
        print(f"closed: {code} {message}")
        stop.set()

    def on_message(_values):
        pass

    def run():
        try:
            # HCSocket.reset() runs at the top of run_forever, so the instrumentation has to
            # be installed before that and our openers are already at the zero state.
            instrument(socket_obj, recorder)
            device.run_forever(on_message=on_message, on_open=on_open, on_close=on_close)
        except BaseException as exc:  # noqa: BLE001 - reported, then re-raised on the main thread
            failure.append(exc)
        finally:
            stop.set()

    worker = threading.Thread(target=run, daemon=True)
    worker.start()

    deadline = time.monotonic() + args.duration
    try:
        while time.monotonic() < deadline and not stop.is_set():
            time.sleep(0.5)
    except KeyboardInterrupt:
        print("\ninterrupted; closing")
    finally:
        try:
            socket_obj.close()
        except Exception:  # pragma: no cover - best effort
            pass
        worker.join(timeout=10)

    if failure:
        print(f"capture thread failed: {failure[0]!r}", file=sys.stderr)
    if recorder.errors:
        print("CHAIN CROSS-CHECK FAILED:", file=sys.stderr)
        for error in sorted(set(recorder.errors)):
            print(f"  {error}", file=sys.stderr)
        return 3
    if not recorder.frames:
        print("no frames captured", file=sys.stderr)
        return 4

    stamp = _dt.datetime.now(_dt.timezone.utc).strftime("%Y-%m-%dT%H-%M-%SZ")
    capture = {
        "schema": hc_corpus.SCHEMA,
        "tier": "raw",
        "device": {
            "alias": args.alias,
            "host": host,
            "transport": "aes",
            "connection_type": profile.get("connectionType"),
            "brand": profile.get("brand"),
            "vib": profile.get("vib"),
            "type": profile.get("type"),
            "profile_zip": profile.get("_zip"),
        },
        "client": {
            "name": "hcpy",
            "commit": _git_rev(args.hcpy),
            "device_name": device.device_name,
            "device_id": device.device_id,
        },
        "captured_at": _dt.datetime.now(_dt.timezone.utc).isoformat(),
        "key": {"psk64": profile["key"], "iv64": profile["iv"]},
        "frames": recorder.frames,
    }

    problems = hc_corpus.verify_capture(capture)
    if problems:
        print("capture does not verify against its own key:", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 5

    out = args.out or (args.secrets_dir / "captures" / args.alias / f"{stamp}.json")
    out.parent.mkdir(parents=True, exist_ok=True)
    hc_corpus.dump(capture, out)
    os.chmod(out, 0o600)

    print(f"\n{len(recorder.frames)} frames, verified, written to {out}")
    print(recorder.summary())
    notifies = sum(
        1 for f in recorder.frames if f["action"] == "NOTIFY" and f["resource"] == "/ro/values"
    )
    print(f"\nlive NOTIFY /ro/values frames: {notifies}")
    if notifies == 0:
        print(
            "  none arrived. That is expected on an idle appliance and is NOT a reason to "
            "write to it. See scripts/capture.md."
        )
    return 0


def _git_rev(path: Path) -> str | None:
    import subprocess

    try:
        return subprocess.run(
            ["git", "-C", str(path), "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
    except Exception:  # pragma: no cover - reference checkout may not be a git repo
        return None


if __name__ == "__main__":
    raise SystemExit(main())
