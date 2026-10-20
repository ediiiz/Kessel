//! The two continuous chains.
//!
//! One [`Sealer`] and one [`Opener`] per connection. Both are stateful for the whole connection:
//! the CBC IV for message *n+1* is the last ciphertext block of message *n*, and each direction
//! carries a rolling 16-byte HMAC that feeds the next frame's MAC input. TX and RX are fully
//! independent.

use aes::Aes256;
use aes::cipher::{BlockDecryptMut as _, BlockEncryptMut as _, KeyIvInit as _};
use hmac::{Hmac, Mac as _};
use sha2::Sha256;

use crate::error::CryptoError;
use crate::keys::SessionKeys;
use crate::pad::BLOCK_LEN;

type HmacSha256 = Hmac<Sha256>;
type Aes256CbcEnc = cbc::Encryptor<Aes256>;
type Aes256CbcDec = cbc::Decryptor<Aes256>;

/// Length of the MAC tag on the wire: the HMAC-SHA256 output truncated to its first 16 bytes.
pub const MAC_LEN: usize = 16;

/// The smallest legal inbound frame: 16 bytes of ciphertext plus the 16-byte MAC.
pub const MIN_FRAME_LEN: usize = 32;

/// Which way a frame is travelling.
///
/// A client seals with [`Direction::ClientToAppliance`] and opens with
/// [`Direction::ApplianceToClient`]; a harness playing the appliance inverts both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Client → appliance. Byte `0x45`, ASCII `E`.
    ClientToAppliance,
    /// Appliance → client. Byte `0x43`, ASCII `C`.
    ApplianceToClient,
}

impl Direction {
    /// The direction byte that goes into every HMAC input for this direction.
    #[must_use]
    pub const fn byte(self) -> u8 {
        match self {
            Self::ClientToAppliance => 0x45,
            Self::ApplianceToClient => 0x43,
        }
    }
}

/// The chain state a single frame moved through, for assertions and for the frame inspector.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ChainState {
    /// The direction's rolling HMAC before this frame — 16 zero bytes for the first frame.
    pub hmac_before: [u8; MAC_LEN],
    /// The direction's rolling HMAC after this frame; equals this frame's MAC.
    pub hmac_after: [u8; MAC_LEN],
    /// The CBC chaining block this frame was encrypted under — the `iv` only for the first frame.
    pub cbc_iv_before: [u8; BLOCK_LEN],
    /// The CBC chaining block for the next frame: this frame's last ciphertext block.
    pub cbc_iv_after: [u8; BLOCK_LEN],
}

impl core::fmt::Debug for ChainState {
    /// Renders the rolling HMACs, never the CBC chaining blocks.
    ///
    /// `cbc_iv_before` is the raw `iv` on the first frame of every connection, so neither CBC
    /// block is rendered. The HMACs are MAC tags already sent in clear on the wire.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ChainState")
            .field("hmac_before", &self.hmac_before)
            .field("hmac_after", &self.hmac_after)
            .field("cbc_iv_before", &crate::secret::REDACTED)
            .field("cbc_iv_after", &crate::secret::REDACTED)
            .finish()
    }
}

/// A frame produced by [`Sealer::seal_with_pad`].
#[derive(Clone)]
pub struct SealedFrame {
    /// The CBC ciphertext, without the MAC.
    pub ciphertext: Vec<u8>,
    /// The 16-byte truncated HMAC that suffixes the frame.
    pub mac: [u8; MAC_LEN],
    /// The pad that was appended to the plaintext.
    pub pad: Vec<u8>,
    /// The chain state this frame moved through.
    pub chain: ChainState,
}

impl core::fmt::Debug for SealedFrame {
    /// Ciphertext, MAC and pad are public on the wire; the chain state is delegated to
    /// [`ChainState`]'s impl, which withholds the CBC blocks (and so the `iv`).
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SealedFrame")
            .field("ciphertext", &self.ciphertext)
            .field("mac", &self.mac)
            .field("pad", &self.pad)
            .field("chain", &self.chain)
            .finish()
    }
}

impl SealedFrame {
    /// `ct ‖ mac`: the bytes of the WebSocket binary frame.
    #[must_use]
    pub fn into_wire(mut self) -> Vec<u8> {
        self.ciphertext.extend_from_slice(&self.mac);
        self.ciphertext
    }
}

/// A frame consumed by [`Opener::open`].
#[derive(Clone)]
pub struct OpenedFrame {
    /// The message text; `as_bytes()` recovers the exact plaintext bytes, JSON key order intact.
    pub plaintext: String,
    /// The pad that was stripped.
    pub pad: Vec<u8>,
    /// The MAC received on the wire, which on success equals the recomputed one.
    pub mac: [u8; MAC_LEN],
    /// The chain state this frame moved through.
    pub chain: ChainState,
}

impl core::fmt::Debug for OpenedFrame {
    /// The plaintext is rendered on purpose: the frame inspector exists to show it. Message
    /// bodies can carry identifiers such as `deviceID`, `mac` and `serialNumber`, so redacting
    /// them is the caller's job at the point of emission.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("OpenedFrame")
            .field("plaintext", &self.plaintext)
            .field("pad", &self.pad)
            .field("mac", &self.mac)
            .field("chain", &self.chain)
            .finish()
    }
}

/// Seals frames for one direction: pad, encrypt, MAC, advance both chains.
pub struct Sealer {
    keys: SessionKeys,
    direction: Direction,
    encryptor: Aes256CbcEnc,
    last_hmac: [u8; MAC_LEN],
    cbc_iv: [u8; BLOCK_LEN],
}

impl core::fmt::Debug for Sealer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The rolling HMAC and the CBC chaining block derive from key material; not rendered.
        f.debug_struct("Sealer")
            .field("direction", &self.direction)
            .finish_non_exhaustive()
    }
}

impl Sealer {
    /// Starts a fresh TX chain: rolling HMAC at 16 zero bytes, CBC seeded with the `iv`.
    #[must_use]
    pub fn new(keys: SessionKeys, direction: Direction) -> Self {
        let encryptor = new_encryptor(&keys);
        let cbc_iv = *keys.iv().expose_secret();
        Self {
            keys,
            direction,
            encryptor,
            last_hmac: [0_u8; MAC_LEN],
            cbc_iv,
        }
    }

    /// Returns this chain to its start-of-connection state; see [`Opener::reset`].
    pub fn reset(&mut self) {
        self.encryptor = new_encryptor(&self.keys);
        self.last_hmac = [0_u8; MAC_LEN];
        self.cbc_iv = *self.keys.iv().expose_secret();
    }

    /// This direction's rolling HMAC; 16 zero bytes until the first frame is sealed.
    #[must_use]
    pub const fn last_hmac(&self) -> [u8; MAC_LEN] {
        self.last_hmac
    }

    /// Seals one frame, taking the pad's random middle from `source` (a CSPRNG in production).
    ///
    /// # Errors
    ///
    /// None in practice: [`crate::build_pad`] always produces a valid pad. The `Result` matches
    /// [`Sealer::seal_with_pad`]'s signature.
    pub fn seal(
        &mut self,
        clear: &[u8],
        source: &mut impl crate::pad::PadSource,
    ) -> Result<SealedFrame, CryptoError> {
        let pad = crate::pad::build_pad(clear.len(), source);
        self.seal_with_pad(clear, &pad)
    }

    /// Seals one frame with a caller-supplied pad, so recorded sessions can be replayed
    /// byte-exactly. Production code wants [`Sealer::seal`].
    ///
    /// # Errors
    ///
    /// [`CryptoError::InjectedPad`] if the pad does not have the required shape.
    pub fn seal_with_pad(&mut self, clear: &[u8], pad: &[u8]) -> Result<SealedFrame, CryptoError> {
        if !is_valid_pad(clear.len(), pad) {
            return Err(CryptoError::InjectedPad {
                clear_len: clear.len(),
                pad_len: pad.len(),
            });
        }

        // `padded = clear ‖ pad`, always a multiple of the block size.
        let mut padded = Vec::with_capacity(clear.len() + pad.len());
        padded.extend_from_slice(clear);
        padded.extend_from_slice(pad);

        let hmac_before = self.last_hmac;
        let cbc_iv_before = self.cbc_iv;

        // Continuous CBC: the encryptor belongs to the connection, so this frame's IV is the
        // previous frame's last ciphertext block.
        for block in padded.chunks_exact_mut(BLOCK_LEN) {
            self.encryptor.encrypt_block_mut(block.into());
        }
        let ciphertext = padded;

        // The chain advances to the newly computed MAC, unconditionally.
        let mut mac = [0_u8; MAC_LEN];
        let digest = mac_state(&self.keys, self.direction, &hmac_before)
            .chain_update(&ciphertext)
            .finalize()
            .into_bytes();
        if let Some(head) = digest.get(..MAC_LEN) {
            mac.copy_from_slice(head);
        }
        self.last_hmac = mac;
        self.cbc_iv = last_block(&ciphertext);

        Ok(SealedFrame {
            ciphertext,
            mac,
            pad: pad.to_vec(),
            chain: ChainState {
                hmac_before,
                hmac_after: mac,
                cbc_iv_before,
                cbc_iv_after: self.cbc_iv,
            },
        })
    }
}

/// Opens frames for one direction: length checks, MAC-then-decrypt, unpad.
pub struct Opener {
    keys: SessionKeys,
    direction: Direction,
    decryptor: Aes256CbcDec,
    last_hmac: [u8; MAC_LEN],
    cbc_iv: [u8; BLOCK_LEN],
}

impl core::fmt::Debug for Opener {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Opener")
            .field("direction", &self.direction)
            .finish_non_exhaustive()
    }
}

impl Opener {
    /// Starts a fresh RX chain: rolling HMAC at 16 zero bytes, CBC seeded with the `iv`.
    #[must_use]
    pub fn new(keys: SessionKeys, direction: Direction) -> Self {
        let decryptor = new_decryptor(&keys);
        let cbc_iv = *keys.iv().expose_secret();
        Self {
            keys,
            direction,
            decryptor,
            last_hmac: [0_u8; MAC_LEN],
            cbc_iv,
        }
    }

    /// Returns this chain to its start-of-connection state.
    ///
    /// Chain state is never carried across a reconnect. Call this under the same lock that
    /// guards sending, so an in-flight frame for the dead connection cannot advance the new chain.
    pub fn reset(&mut self) {
        self.decryptor = new_decryptor(&self.keys);
        self.last_hmac = [0_u8; MAC_LEN];
        self.cbc_iv = *self.keys.iv().expose_secret();
    }

    /// This direction's rolling HMAC; 16 zero bytes until the first frame is opened.
    #[must_use]
    pub const fn last_hmac(&self) -> [u8; MAC_LEN] {
        self.last_hmac
    }

    /// Verifies, decrypts and unpads one inbound frame.
    ///
    /// # Errors
    ///
    /// Any [`CryptoError`] is a hard error: the caller must close the socket and reconnect.
    pub fn open(&mut self, wire: &[u8]) -> Result<OpenedFrame, CryptoError> {
        if wire.len() < MIN_FRAME_LEN {
            return Err(CryptoError::FrameTooShort { len: wire.len() });
        }
        if !wire.len().is_multiple_of(BLOCK_LEN) {
            return Err(CryptoError::FrameNotBlockAligned { len: wire.len() });
        }

        // The MAC is the 16-byte suffix; the length checks above guarantee both halves.
        let (ciphertext, received) = wire.split_at(wire.len() - MAC_LEN);
        let mac: [u8; MAC_LEN] = received
            .try_into()
            .map_err(|_ignored| CryptoError::FrameTooShort { len: wire.len() })?;

        let hmac_before = self.last_hmac;
        let cbc_iv_before = self.cbc_iv;

        // Verify the MAC over the ciphertext, in constant time, before decrypting. The wire tag
        // is the leftmost 16 bytes of the HMAC, which is what `verify_truncated_left` checks.
        if mac_state(&self.keys, self.direction, &hmac_before)
            .chain_update(ciphertext)
            .verify_truncated_left(&mac)
            .is_err()
        {
            // The chain must not advance on failure: return before every assignment below.
            return Err(CryptoError::HmacMismatch);
        }

        // Store the received MAC, which on success equals the recomputed one.
        self.last_hmac = mac;

        // Continuous CBC: the decryptor belongs to the connection.
        let mut plain = ciphertext.to_vec();
        for block in plain.chunks_exact_mut(BLOCK_LEN) {
            self.decryptor.decrypt_block_mut(block.into());
        }
        // Assigned here, not on the success path: the unpad and UTF-8 checks below can fail
        // after decryption has already advanced the real CBC state.
        self.cbc_iv = last_block(ciphertext);

        // Trust the trailing pad-length byte, bounded, then decode as UTF-8.
        let clear = crate::pad::unpad(&plain)?;
        let pad = plain.get(clear.len()..).unwrap_or_default().to_vec();
        let plaintext = String::from_utf8(clear.to_vec()).map_err(|error| CryptoError::Utf8 {
            valid_up_to: error.utf8_error().valid_up_to(),
        })?;

        Ok(OpenedFrame {
            plaintext,
            pad,
            mac,
            chain: ChainState {
                hmac_before,
                hmac_after: self.last_hmac,
                cbc_iv_before,
                cbc_iv_after: self.cbc_iv,
            },
        })
    }
}

/// The last ciphertext block, which becomes the next frame's CBC chaining block.
fn last_block(ciphertext: &[u8]) -> [u8; BLOCK_LEN] {
    let mut block = [0_u8; BLOCK_LEN];
    if let Some(tail) = ciphertext.get(ciphertext.len().saturating_sub(BLOCK_LEN)..)
        && let Some(slot) = block.get_mut(..tail.len())
    {
        slot.copy_from_slice(tail);
    }
    block
}

/// An HMAC primed with `iv ‖ direction ‖ last_hmac`, ready for the ciphertext. The order of the
/// parts matters.
#[expect(
    clippy::expect_used,
    reason = "HMAC accepts a key of any length, so new_from_slice cannot fail"
)]
fn mac_state(keys: &SessionKeys, direction: Direction, last_hmac: &[u8; MAC_LEN]) -> HmacSha256 {
    let mut mac = HmacSha256::new_from_slice(keys.mac_key().expose_secret())
        .expect("HMAC accepts a key of any length");
    mac.update(keys.iv().expose_secret());
    mac.update(&[direction.byte()]);
    mac.update(last_hmac);
    mac
}

/// Whether an injected pad is valid: a length in `{2..=16} ∪ {17}`, a leading `0x00`, a trailing
/// byte equal to its own length, and a total that lands on a block boundary.
fn is_valid_pad(clear_len: usize, pad: &[u8]) -> bool {
    let pad_len = pad.len();
    let documented_range = (2..=BLOCK_LEN).contains(&pad_len) || pad_len == BLOCK_LEN + 1;
    documented_range
        && (clear_len + pad_len).is_multiple_of(BLOCK_LEN)
        && pad.first() == Some(&0x00)
        && pad.last().is_some_and(|&byte| usize::from(byte) == pad_len)
}

/// The AES-256-CBC encryptor for a fresh connection, seeded with the raw `iv`.
fn new_encryptor(keys: &SessionKeys) -> Aes256CbcEnc {
    Aes256CbcEnc::new(
        keys.enc_key().expose_secret().into(),
        keys.iv().expose_secret().into(),
    )
}

/// The AES-256-CBC decryptor for a fresh connection, seeded with the raw `iv`.
fn new_decryptor(keys: &SessionKeys) -> Aes256CbcDec {
    Aes256CbcDec::new(
        keys.enc_key().expose_secret().into(),
        keys.iv().expose_secret().into(),
    )
}

/// Which side of the connection this crate is playing.
///
/// A client signs with `E` and verifies with `C`; a harness playing the appliance inverts both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Kessel's own role: seal with `0x45`, open with `0x43`.
    Client,
    /// The mirror, for tests and for a fake appliance: seal with `0x43`, open with `0x45`.
    Appliance,
}

impl Role {
    /// The direction this role seals in.
    #[must_use]
    pub const fn seal_direction(self) -> Direction {
        match self {
            Self::Client => Direction::ClientToAppliance,
            Self::Appliance => Direction::ApplianceToClient,
        }
    }

    /// The direction this role opens.
    #[must_use]
    pub const fn open_direction(self) -> Direction {
        match self {
            Self::Client => Direction::ApplianceToClient,
            Self::Appliance => Direction::ClientToAppliance,
        }
    }
}

/// One connection's worth of app-layer crypto: a [`Sealer`] and an [`Opener`] that share key
/// material and **nothing else**.
///
/// # This type is for single-task use
///
/// Every method takes `&mut self`, so sharing one between a reader and a writer would serialise
/// reads against sends. A concurrent transport should build [`Sealer`] and [`Opener`] directly
/// (see [`Role::seal_direction`] / [`Role::open_direction`]) behind separate locks. The send lock
/// must then span encrypt + write, and [`Sealer::reset`] / [`Opener::reset`] must be called
/// inside those locks on reconnect.
#[derive(Debug)]
pub struct AesSession {
    sealer: Sealer,
    opener: Opener,
}

impl AesSession {
    /// Starts a connection in the given role, both chains at their start-of-connection state.
    #[must_use]
    pub fn new(keys: SessionKeys, role: Role) -> Self {
        Self {
            sealer: Sealer::new(keys.clone(), role.seal_direction()),
            opener: Opener::new(keys, role.open_direction()),
        }
    }

    /// Resets both chains.
    pub fn reset(&mut self) {
        self.sealer.reset();
        self.opener.reset();
    }

    /// Seals one outbound frame, drawing the pad's random middle from `source`. See
    /// [`Sealer::seal`].
    ///
    /// # Errors
    ///
    /// As [`Sealer::seal`].
    pub fn seal(
        &mut self,
        clear: &[u8],
        source: &mut impl crate::pad::PadSource,
    ) -> Result<SealedFrame, CryptoError> {
        self.sealer.seal(clear, source)
    }

    /// Seals one outbound frame with an injected pad. See [`Sealer::seal_with_pad`].
    ///
    /// # Errors
    ///
    /// [`CryptoError::InjectedPad`] if the pad does not have the required shape.
    pub fn seal_with_pad(&mut self, clear: &[u8], pad: &[u8]) -> Result<SealedFrame, CryptoError> {
        self.sealer.seal_with_pad(clear, pad)
    }

    /// Opens one inbound frame. See [`Opener::open`].
    ///
    /// # Errors
    ///
    /// As [`Opener::open`]; every one is a hard error.
    pub fn open(&mut self, wire: &[u8]) -> Result<OpenedFrame, CryptoError> {
        self.opener.open(wire)
    }

    /// The rolling HMAC of the sealing direction, as of the last frame sealed.
    #[must_use]
    pub const fn seal_hmac(&self) -> [u8; MAC_LEN] {
        self.sealer.last_hmac()
    }

    /// The rolling HMAC of the opening direction, as of the last frame opened.
    #[must_use]
    pub const fn open_hmac(&self) -> [u8; MAC_LEN] {
        self.opener.last_hmac()
    }
}
