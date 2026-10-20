"""The Home Connect app-layer AES chain, in Python.

Used by ``scrub-capture.py`` to re-run a captured session end to end under a synthetic key,
and by ``capture.py`` to record chain state alongside each frame. Not part of the shipped
product; the Rust ``hc-crypto`` crate is the real implementation.

Dependencies: ``pycryptodome`` (AES-CBC). Everything else is stdlib.
"""

from __future__ import annotations

import base64
import hashlib
import hmac as _hmac
from dataclasses import dataclass

from Crypto.Cipher import AES

#: Client -> appliance. ASCII ``E``.
DIRECTION_TX = b"\x45"
#: Appliance -> client. ASCII ``C``.
DIRECTION_RX = b"\x43"

#: The smallest legal inbound frame: 16 bytes of ciphertext + 16 of MAC.
MINIMUM_MESSAGE_LENGTH = 32
#: HMAC-SHA256 truncated to its first 16 bytes.
MAC_LENGTH = 16
BLOCK_SIZE = 16

PSK_LENGTH = 32
IV_LENGTH = 16


# ---------------------------------------------------------------------------------------
# base64url, padding-indifferent
# ---------------------------------------------------------------------------------------


def b64url_decode(value: str) -> bytes:
    """Decode URL-safe base64 with optional padding, the way all three references do."""
    return base64.urlsafe_b64decode(value + "===")


def b64url_encode(value: bytes) -> str:
    """Encode URL-safe base64 **without** padding — the shape real profiles use."""
    return base64.urlsafe_b64encode(value).decode("ascii").rstrip("=")


# ---------------------------------------------------------------------------------------
# KDF
# ---------------------------------------------------------------------------------------


def derive_keys(psk: bytes) -> tuple[bytes, bytes]:
    """``(enckey, mackey)`` = two independent single-shot HMACs over ``"ENC"`` / ``"MAC"``."""
    if len(psk) != PSK_LENGTH:
        raise ValueError(f"psk must be {PSK_LENGTH} bytes, got {len(psk)}")
    enckey = _hmac.new(psk, b"ENC", hashlib.sha256).digest()
    mackey = _hmac.new(psk, b"MAC", hashlib.sha256).digest()
    return enckey, mackey


# ---------------------------------------------------------------------------------------
# Padding: custom, NOT PKCS#7
# ---------------------------------------------------------------------------------------


def pad_length(clear_len: int) -> int:
    """``16 - (len % 16)``, bumped to 17 when it would be 1 (there are always >= 2 pad bytes)."""
    pad_len = BLOCK_SIZE - (clear_len % BLOCK_SIZE)
    if pad_len == 1:
        pad_len += BLOCK_SIZE
    return pad_len


def build_pad(pad_len: int, rng) -> bytes:
    """``0x00 || random(pad_len - 2) || pad_len``."""
    if pad_len < 2:
        raise ValueError(f"pad_len must be >= 2, got {pad_len}")
    return b"\x00" + rng(pad_len - 2) + bytes([pad_len])


def unpad(padded: bytes) -> bytes:
    """Trust the trailing length byte, bounded (the strict variant of unpadding)."""
    if not padded:
        raise ValueError("empty plaintext")
    pad_len = padded[-1]
    if pad_len < 1 or pad_len > len(padded):
        raise ValueError(f"bogus pad length {pad_len} for {len(padded)} bytes")
    return padded[: len(padded) - pad_len]


class ShakeRng:
    """A deterministic byte source, so the corpus is reproducible byte for byte.

    SHAKE-256 is used purely as a seekable, seedable stream; no security property is claimed.

    ``__call__`` re-digests the whole prefix each time, so producing *n* bytes across *k*
    calls is O(n·k). That is fine for a session of a few hundred pad bytes; buffer the
    stream in chunks if a corpus ever grows large enough to matter.
    """

    def __init__(self, seed: bytes) -> None:
        self.seed = seed
        self._offset = 0

    def __call__(self, n: int) -> bytes:
        if n == 0:
            return b""
        end = self._offset + n
        stream = hashlib.shake_256(self.seed).digest(end)
        out = stream[self._offset : end]
        self._offset = end
        return out


# ---------------------------------------------------------------------------------------
# The two chains
# ---------------------------------------------------------------------------------------


@dataclass(frozen=True)
class SealedFrame:
    clear: bytes
    pad: bytes
    ciphertext: bytes
    mac: bytes
    hmac_before: bytes
    hmac_after: bytes
    cbc_iv_before: bytes
    cbc_iv_after: bytes

    @property
    def wire(self) -> bytes:
        return self.ciphertext + self.mac

    @property
    def pad_len(self) -> int:
        return self.pad[-1]


class Sealer:
    """Produces frames for one direction: pad, encrypt, MAC, advance both chains.

    ``direction`` is ``DIRECTION_TX`` when acting as the *client* and ``DIRECTION_RX`` when
    acting as the *appliance*. The scrubber needs both, because it regenerates the
    appliance's frames too.
    """

    def __init__(self, psk: bytes, iv: bytes, direction: bytes, rng) -> None:
        if len(iv) != IV_LENGTH:
            raise ValueError(f"iv must be {IV_LENGTH} bytes, got {len(iv)}")
        self.enckey, self.mackey = derive_keys(psk)
        self.iv = iv
        self.direction = direction
        self.rng = rng
        self._aes = AES.new(self.enckey, AES.MODE_CBC, iv)
        self.last_hmac = bytes(MAC_LENGTH)
        self.cbc_iv = iv

    def seal(self, clear: bytes, pad: bytes | None = None) -> SealedFrame:
        if pad is None:
            pad = build_pad(pad_length(len(clear)), self.rng)
        padded = clear + pad
        # A caller-supplied pad is trusted for its *content* (the scrubber replays the pad
        # bytes recovered from the real capture, and the verifier checks their shape) but not
        # for its length: a misaligned pad would make PyCryptodome raise from inside
        # the cipher with no clue as to which frame, and a corrupt-but-aligned pad would
        # silently produce a corpus that cannot be unpadded. Fail here, where the frame is
        # still identifiable.
        if len(padded) % BLOCK_SIZE != 0:
            raise ValueError(
                f"clear+pad must be a multiple of {BLOCK_SIZE}, got {len(padded)} "
                f"({len(clear)} clear + {len(pad)} pad)"
            )
        if len(pad) < 2:
            raise ValueError(f"pad must be >= 2 bytes, got {len(pad)}")
        hmac_before = self.last_hmac
        cbc_iv_before = self.cbc_iv
        ciphertext = self._aes.encrypt(padded)
        mac = _hmac.new(
            self.mackey,
            self.iv + self.direction + hmac_before + ciphertext,
            hashlib.sha256,
        ).digest()[:MAC_LENGTH]
        self.last_hmac = mac
        self.cbc_iv = ciphertext[-BLOCK_SIZE:]
        return SealedFrame(
            clear=clear,
            pad=pad,
            ciphertext=ciphertext,
            mac=mac,
            hmac_before=hmac_before,
            hmac_after=mac,
            cbc_iv_before=cbc_iv_before,
            cbc_iv_after=self.cbc_iv,
        )


class Opener:
    """Consumes frames for one direction: length checks, MAC-then-decrypt, unpad."""

    def __init__(self, psk: bytes, iv: bytes, direction: bytes) -> None:
        if len(iv) != IV_LENGTH:
            raise ValueError(f"iv must be {IV_LENGTH} bytes, got {len(iv)}")
        self.enckey, self.mackey = derive_keys(psk)
        self.iv = iv
        self.direction = direction
        self._aes = AES.new(self.enckey, AES.MODE_CBC, iv)
        self.last_hmac = bytes(MAC_LENGTH)
        self.cbc_iv = iv

    def open(self, wire: bytes) -> SealedFrame:
        if len(wire) < MINIMUM_MESSAGE_LENGTH:
            raise ValueError(f"frame shorter than {MINIMUM_MESSAGE_LENGTH} bytes: {len(wire)}")
        if len(wire) % BLOCK_SIZE != 0:
            raise ValueError(f"frame length {len(wire)} is not a multiple of {BLOCK_SIZE}")
        ciphertext = wire[:-MAC_LENGTH]
        received_mac = wire[-MAC_LENGTH:]
        hmac_before = self.last_hmac
        cbc_iv_before = self.cbc_iv
        expected = _hmac.new(
            self.mackey,
            self.iv + self.direction + hmac_before + ciphertext,
            hashlib.sha256,
        ).digest()[:MAC_LENGTH]
        if not _hmac.compare_digest(received_mac, expected):
            # The chain must not advance on failure.
            raise ValueError(
                f"HMAC mismatch: received {received_mac.hex()}, computed {expected.hex()}"
            )
        self.last_hmac = received_mac
        padded = self._aes.decrypt(ciphertext)
        clear = unpad(padded)
        self.cbc_iv = ciphertext[-BLOCK_SIZE:]
        return SealedFrame(
            clear=clear,
            pad=padded[len(clear) :],
            ciphertext=ciphertext,
            mac=received_mac,
            hmac_before=hmac_before,
            hmac_after=received_mac,
            cbc_iv_before=cbc_iv_before,
            cbc_iv_after=self.cbc_iv,
        )
