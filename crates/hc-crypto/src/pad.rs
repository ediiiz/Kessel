//! The custom padding. **This is not PKCS#7.**
//!
//! ```text
//! pad_len = 16 - (len(clear) % 16)
//! if pad_len == 1 { pad_len += 16 }                    // => pad_len ∈ {2..16} ∪ {17}
//! padded  = clear ‖ 0x00 ‖ random(pad_len - 2) ‖ byte(pad_len)
//! ```
//!
//! The first pad byte is always `0x00`, the last is always `pad_len` itself, and the bytes
//! between are cryptographically random and carry no information. `pad_len` is never 0 (a
//! plaintext that is already block-aligned gets a full 16-byte pad block) and never 1 (there are
//! always at least the two structural bytes, which is why a would-be 1 becomes 17).

use crate::error::CryptoError;

/// The AES block size, in bytes.
pub const BLOCK_LEN: usize = 16;

/// Where the random middle of a pad comes from.
///
/// Injected so tests can be deterministic; production passes an OS CSPRNG. Recorded pads are
/// replayed through [`crate::Sealer::seal_with_pad`] instead.
///
/// Blanket-implemented for every [`rand::RngCore`].
pub trait PadSource {
    /// Fills `out` with the bytes that become the pad's random middle.
    fn fill_pad(&mut self, out: &mut [u8]);
}

impl<R: rand::RngCore> PadSource for R {
    fn fill_pad(&mut self, out: &mut [u8]) {
        self.fill_bytes(out);
    }
}

/// Builds the pad for a plaintext of `clear_len` bytes: `0x00 ‖ random(pad_len - 2) ‖ pad_len`.
///
/// Taking the plaintext length makes an out-of-range pad unrepresentable: the result always
/// brings `clear_len` up to a block boundary.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "pad_len() returns 2..=17 by construction, so it fits in a u8"
)]
pub fn build_pad(clear_len: usize, source: &mut impl PadSource) -> Vec<u8> {
    let pad_len = pad_len(clear_len);
    let mut pad = vec![0x00_u8; pad_len];

    // `pad_len >= 2`, so both splits succeed without any indexing that could panic.
    if let Some((last, head)) = pad.split_last_mut() {
        *last = pad_len as u8;
        if let Some((first, middle)) = head.split_first_mut() {
            *first = 0x00;
            source.fill_pad(middle);
        }
    }
    pad
}

/// How many pad bytes `clear_len` bytes of plaintext attract.
#[must_use]
pub const fn pad_len(clear_len: usize) -> usize {
    let pad_len = BLOCK_LEN - (clear_len % BLOCK_LEN);
    if pad_len == 1 {
        pad_len + BLOCK_LEN
    } else {
        pad_len
    }
}

/// Strips the pad from a decrypted buffer, trusting the trailing length byte.
///
/// # Errors
///
/// [`CryptoError::EmptyPlaintext`] for an empty buffer, and [`CryptoError::PadLength`] when the
/// trailing byte is `0` or larger than the buffer.
pub fn unpad(padded: &[u8]) -> Result<&[u8], CryptoError> {
    let &pad_len = padded.last().ok_or(CryptoError::EmptyPlaintext)?;
    let keep = padded
        .len()
        .checked_sub(usize::from(pad_len))
        .filter(|_| pad_len > 0)
        .ok_or(CryptoError::PadLength {
            pad_len,
            padded_len: padded.len(),
        })?;
    padded.get(..keep).ok_or(CryptoError::PadLength {
        pad_len,
        padded_len: padded.len(),
    })
}
