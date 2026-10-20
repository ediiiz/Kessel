"""Unit tests for ``capture.resolve_host``, the host-pinning safety guard.

``resolve_host`` refuses to point the capture harness at any address other than the alias's
entry in ``secrets/IP.md`` unless ``--allow-unpinned-host`` is passed. This project can
physically start a cooktop; which appliance the harness talks to is not something to get
right by accident.

Nothing here touches ``secrets/`` or any appliance: a throwaway ``IP.md`` is written into
``tmp_path``. Every address below is from the RFC 5737 documentation range (``192.0.2.0/24``).

Run with ``just test-py``, or::

    PYTHONPATH=scripts python3 -m pytest scripts/tests -q

These do not run under ``cargo test`` or ``just ci``.
"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

SCRIPTS_DIR = Path(__file__).resolve().parents[1]
if str(SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPTS_DIR))

import capture  # noqa: E402

PINNED = "192.0.2.10"
OTHER = "192.0.2.99"


def _secrets_with_ip(tmp_path: Path, body: str) -> Path:
    (tmp_path / "IP.md").write_text(body, encoding="utf-8")
    return tmp_path


def test_pinned_alias_returns_its_address(tmp_path: Path) -> None:
    secrets = _secrets_with_ip(tmp_path, f"hob: {PINNED}\n")
    assert capture.resolve_host(secrets, "hob", None, False) == PINNED


def test_matching_override_is_allowed(tmp_path: Path) -> None:
    secrets = _secrets_with_ip(tmp_path, f"hob: {PINNED}\n")
    assert capture.resolve_host(secrets, "hob", PINNED, False) == PINNED


def test_mismatched_override_is_refused_without_flag(tmp_path: Path) -> None:
    # The load-bearing guard: --host <other> --alias hob must not open a socket.
    secrets = _secrets_with_ip(tmp_path, f"hob: {PINNED}\n")
    with pytest.raises(SystemExit):
        capture.resolve_host(secrets, "hob", OTHER, False)


def test_mismatched_override_allowed_with_flag(tmp_path: Path) -> None:
    secrets = _secrets_with_ip(tmp_path, f"hob: {PINNED}\n")
    assert capture.resolve_host(secrets, "hob", OTHER, True) == OTHER


def test_unpinned_alias_without_override_is_refused(tmp_path: Path) -> None:
    secrets = _secrets_with_ip(tmp_path, "oven: 192.0.2.20\n")
    with pytest.raises(SystemExit):
        capture.resolve_host(secrets, "hob", None, False)


def test_override_for_unpinned_alias_needs_the_flag(tmp_path: Path) -> None:
    secrets = _secrets_with_ip(tmp_path, "oven: 192.0.2.20\n")
    with pytest.raises(SystemExit):
        capture.resolve_host(secrets, "hob", OTHER, False)
    assert capture.resolve_host(secrets, "hob", OTHER, True) == OTHER
