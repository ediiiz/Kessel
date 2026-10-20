//! `hc-crypto` tests, driven against the golden corpus.
//!
//! The oracle is `fixtures/aes/hob-session.json`: 16 frames of a real hob session, scrubbed and
//! re-encrypted under a synthetic, published psk/iv. Where this crate and the corpus disagree,
//! this crate is wrong. The corpus is embedded with `include_str!` so the tests do no I/O.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic; library code may not"
    )
)]

use hc_crypto::{
    AesSession, CryptoError, Direction, Opener, Role, Sealer, SessionKeys, build_pad, pad_len,
    unpad,
};
use rand::{Rng as _, SeedableRng as _};
use serde::Deserialize;

/// The chain implementation's own source, for [`hmac_compare_is_constant_time`].
const CHAIN_SOURCE: &str = include_str!("../src/chain.rs");

/// `fixtures/aes/hob-session.json`, `schema: kessel.capture/1`, tier `golden`.
const CORPUS_JSON: &str = include_str!("../../../fixtures/aes/hob-session.json");

#[derive(Debug, Deserialize)]
struct Corpus {
    key: CorpusKey,
    frames: Vec<Frame>,
}

/// One captured frame. `plaintext_hex` is authoritative; `plaintext_utf8` is for readability and
/// the two are asserted to agree.
#[derive(Debug, Deserialize)]
struct Frame {
    seq: u32,
    /// `"rx"` = appliance → client (direction byte `0x43`), `"tx"` = client → appliance (`0x45`).
    direction: String,
    resource: Option<String>,
    wire_hex: String,
    ciphertext_hex: String,
    mac_hex: String,
    plaintext_hex: String,
    plaintext_utf8: String,
    pad_len: usize,
    pad_hex: String,
    chain: FrameChain,
}

/// Per-frame intermediate chain state, which localises a bug to one frame.
#[derive(Debug, Deserialize)]
struct FrameChain {
    direction_hmac_before: String,
    direction_hmac_after: String,
    cbc_iv_before: String,
    cbc_iv_after: String,
    /// Both directions' chain state as of this frame.
    tx_hmac_after: String,
    rx_hmac_after: String,
}

impl Frame {
    /// A label for assertion messages: `frame 4 rx /ci/services`.
    fn label(&self) -> String {
        format!(
            "frame {} {} {}",
            self.seq,
            self.direction,
            self.resource.as_deref().unwrap_or("-")
        )
    }

    fn wire(&self) -> Vec<u8> {
        hex::decode(&self.wire_hex).expect("wire_hex is hex")
    }

    /// The authoritative plaintext bytes, never the parsed JSON.
    fn clear(&self) -> Vec<u8> {
        hex::decode(&self.plaintext_hex).expect("plaintext_hex is hex")
    }

    /// The recorded pad, injected into the sealer rather than regenerated.
    fn pad(&self) -> Vec<u8> {
        hex::decode(&self.pad_hex).expect("pad_hex is hex")
    }
}

#[derive(Debug, Deserialize)]
struct CorpusKey {
    /// Synthetic and published; not appliance key material.
    psk64: String,
    iv64: String,
}

fn corpus() -> Corpus {
    serde_json::from_str(CORPUS_JSON).expect("the golden corpus parses")
}

/// The session keys of the golden corpus (synthetic and published).
fn corpus_keys() -> SessionKeys {
    let corpus = corpus();
    SessionKeys::from_base64(&corpus.key.psk64, &corpus.key.iv64)
        .expect("the synthetic psk64/iv64 decode and validate")
}

/// `HMAC_SHA256(psk, "ENC")` / `("MAC")`, full 32-byte digests, no truncation.
///
/// The expected vectors were computed independently with Python's `hmac`/`hashlib`.
#[test]
fn kdf_derives_enc_and_mac_keys() {
    let corpus = corpus();
    let keys = SessionKeys::from_base64(&corpus.key.psk64, &corpus.key.iv64)
        .expect("the synthetic psk64/iv64 decode and validate");

    assert_eq!(
        hex::encode(keys.enc_key().expose_secret()),
        "69ae08525eb4fb900494f72de805555edb8fd66f4eccd2f3155ac927e12d9adb",
        "enckey must be HMAC_SHA256(psk, \"ENC\") — aes-chain.md §3"
    );
    assert_eq!(
        hex::encode(keys.mac_key().expose_secret()),
        "db8ac26f8e639c90f35a228ffce89b37d528251a4fff749524cf294492bc1e65",
        "mackey must be HMAC_SHA256(psk, \"MAC\") — aes-chain.md §3"
    );
    assert_eq!(
        hex::encode(keys.iv().expose_secret()),
        "61e67b41fe8431a6e8f9137f2be6d656",
        "the iv is used raw, never derived — aes-chain.md §3"
    );
}

/// An empty plaintext takes a full 16-byte pad block; `pad_len` is never 0.
#[test]
fn pad_empty_is_16() {
    assert_eq!(
        pad_len(0),
        16,
        "0 % 16 == 0 -> 16 - 0 = 16 (aes-chain.md §5.1)"
    );
}

/// One byte of plaintext takes a 15-byte pad.
#[test]
fn pad_one_byte_is_15() {
    assert_eq!(
        pad_len(1),
        15,
        "1 % 16 == 1 -> 16 - 1 = 15 (aes-chain.md §5.1)"
    );
}

/// 16 bytes in, 32 bytes out.
#[test]
fn pad_sixteen_is_16() {
    assert_eq!(
        pad_len(16),
        16,
        "16 % 16 == 0 -> a full pad block, padded length 32 (aes-chain.md §5.1)"
    );
}

/// 15 bytes would take a 1-byte pad, which cannot hold both structural bytes, so it is bumped
/// to 17 and the frame grows to 32 bytes.
#[test]
fn pad_fifteen_is_17() {
    assert_eq!(
        pad_len(15),
        17,
        "15 % 16 == 15 -> 16 - 15 = 1, bumped to 17 (aes-chain.md §5.1)"
    );
}

proptest::proptest! {
    /// A pad is never 1 byte: it always carries a leading `0x00` and a trailing length byte.
    #[test]
    fn pad_len_never_one(clear_len in 0_usize..=64) {
        proptest::prop_assert_ne!(
            pad_len(clear_len),
            1,
            "pad_len must never be 1 — aes-chain.md §5.1"
        );
    }
}

/// The pad is `0x00 ‖ random(pad_len - 2) ‖ byte(pad_len)` and lands the frame on a block
/// boundary.
#[test]
fn pad_first_byte_is_zero_and_last_is_len() {
    let mut source = rand_chacha::ChaCha20Rng::seed_from_u64(0xD0);

    for clear_len in 0_usize..=64 {
        let expected_len = pad_len(clear_len);
        let pad = build_pad(clear_len, &mut source);

        assert_eq!(
            pad.len(),
            expected_len,
            "pad for {clear_len} clear bytes must be pad_len(clear_len) long"
        );
        assert_eq!(
            *pad.first().unwrap(),
            0x00,
            "the first pad byte is always 0x00 — aes-chain.md §5.1"
        );
        assert_eq!(
            usize::from(*pad.last().unwrap()),
            expected_len,
            "the last pad byte is always pad_len itself — aes-chain.md §5.1"
        );
        assert_eq!(
            (clear_len + pad.len()) % 16,
            0,
            "clear + pad is always a multiple of the block size — aes-chain.md §5.1"
        );
    }
}

/// Unpadding trusts the trailing length byte, but bounds it: `0`, over-long and empty buffers
/// are rejected, and a `pad_len` equal to the whole buffer yields an empty message.
#[test]
fn unpad_rejects_bogus_len() {
    assert_eq!(
        unpad(&[]).unwrap_err(),
        CryptoError::EmptyPlaintext,
        "an empty buffer cannot carry the two structural pad bytes — aes-chain.md §5.1"
    );

    let mut zero_terminated = vec![0x41_u8; 16];
    *zero_terminated.last_mut().unwrap() = 0x00;
    assert_eq!(
        unpad(&zero_terminated).unwrap_err(),
        CryptoError::PadLength {
            pad_len: 0,
            padded_len: 16
        },
        "pad_len 0 is rejected — aes-chain.md §6, the Go variant"
    );

    let mut overlong = vec![0x41_u8; 16];
    *overlong.last_mut().unwrap() = 32;
    assert_eq!(
        unpad(&overlong).unwrap_err(),
        CryptoError::PadLength {
            pad_len: 32,
            padded_len: 16
        },
        "a pad longer than the buffer is rejected — aes-chain.md §6"
    );

    let mut whole_block = vec![0x41_u8; 16];
    *whole_block.last_mut().unwrap() = 16;
    assert_eq!(
        unpad(&whole_block).unwrap(),
        b"",
        "a pad_len equal to the buffer is legal and yields an empty message"
    );

    let mut one_byte_message = vec![0x41_u8; 16];
    *one_byte_message.last_mut().unwrap() = 15;
    assert_eq!(
        unpad(&one_byte_message).unwrap(),
        b"A",
        "the message is everything before the last pad_len bytes — aes-chain.md §6"
    );
}

/// The minimum inbound frame is 32 bytes (one ciphertext block plus the MAC); shorter is a hard
/// error.
#[test]
fn short_frame_rejected() {
    let mut opener = Opener::new(corpus_keys(), Direction::ApplianceToClient);

    assert_eq!(
        opener.open(&[0_u8; 16]).unwrap_err(),
        CryptoError::FrameTooShort { len: 16 },
        "a 16-byte frame cannot carry ciphertext and a MAC — aes-chain.md §7"
    );
    assert_eq!(
        opener.open(&[]).unwrap_err(),
        CryptoError::FrameTooShort { len: 0 },
        "an empty frame is rejected on length, before any crypto — aes-chain.md §7"
    );
}

/// An inbound length that is not a multiple of 16 is a hard error.
#[test]
fn non_multiple_of_16_rejected() {
    let mut opener = Opener::new(corpus_keys(), Direction::ApplianceToClient);

    assert_eq!(
        opener.open(&[0_u8; 40]).unwrap_err(),
        CryptoError::FrameNotBlockAligned { len: 40 },
        "40 bytes clears the 32-byte minimum but is not block-aligned — aes-chain.md §7"
    );
    assert_eq!(
        opener.open(&[0_u8; 33]).unwrap_err(),
        CryptoError::FrameNotBlockAligned { len: 33 },
        "the alignment check is on the whole frame, MAC included — aes-chain.md §7"
    );
}

/// Every appliance → client frame in the corpus, with its per-frame chain state.
///
/// One [`Opener`] for the whole session: the RX chain is continuous across the `rx` frames and
/// unaffected by the interleaved `tx` frames.
#[test]
fn rx_chain_decrypts_captured_frames() {
    let corpus = corpus();
    let mut rx_chain = Opener::new(corpus_keys(), Direction::ApplianceToClient);
    let mut opened_count = 0_usize;

    for frame in corpus.frames.iter().filter(|frame| frame.direction == "rx") {
        let label = frame.label();
        let opened = rx_chain
            .open(&frame.wire())
            .unwrap_or_else(|error| panic!("{label} must open: {error}"));

        assert_eq!(
            hex::encode(opened.plaintext.as_bytes()),
            frame.plaintext_hex,
            "{label}: plaintext bytes (the authoritative field — fixtures/README.md §4)"
        );
        assert_eq!(
            opened.plaintext, frame.plaintext_utf8,
            "{label}: plaintext_hex and plaintext_utf8 must agree"
        );
        assert_eq!(
            hex::encode(&opened.pad),
            frame.pad_hex,
            "{label}: the stripped pad, aes-chain.md §5.1"
        );
        assert_eq!(
            opened.pad.len(),
            frame.pad_len,
            "{label}: pad length must match the recorded pad_len"
        );
        assert_eq!(
            hex::encode(opened.mac),
            frame.mac_hex,
            "{label}: the 16-byte MAC suffix"
        );
        assert_eq!(
            hex::encode(opened.chain.hmac_before),
            frame.chain.direction_hmac_before,
            "{label}: rolling RX HMAC entering the frame"
        );
        assert_eq!(
            hex::encode(opened.chain.hmac_after),
            frame.chain.direction_hmac_after,
            "{label}: rolling RX HMAC leaving the frame"
        );
        assert_eq!(
            hex::encode(opened.chain.cbc_iv_before),
            frame.chain.cbc_iv_before,
            "{label}: CBC chaining block entering the frame"
        );
        assert_eq!(
            hex::encode(opened.chain.cbc_iv_after),
            frame.chain.cbc_iv_after,
            "{label}: CBC chaining block leaving the frame"
        );
        opened_count += 1;
    }

    assert_eq!(
        opened_count, 8,
        "the corpus carries eight rx frames; a change here means the fixture moved"
    );
}

/// A bad MAC is a hard error and leaves the RX chain exactly where it was.
///
/// Proven by replaying the untouched frame afterwards: it only opens against its recorded
/// `direction_hmac_before` if the failed attempt changed nothing.
#[test]
fn bad_hmac_returns_hard_error_and_does_not_advance_rx() {
    let corpus = corpus();
    let mut rx = corpus.frames.iter().filter(|frame| frame.direction == "rx");
    let first = rx.next().expect("the corpus has rx frames");
    let second = rx.next().expect("the corpus has a second rx frame");
    let third = rx.next().expect("the corpus has a third rx frame");

    let mut rx_chain = Opener::new(corpus_keys(), Direction::ApplianceToClient);
    rx_chain
        .open(&first.wire())
        .expect("the first rx frame opens");

    // (a) a flipped bit in the MAC suffix.
    let mut flipped_mac = second.wire();
    *flipped_mac.last_mut().unwrap() ^= 0x01;
    assert_eq!(
        rx_chain.open(&flipped_mac).unwrap_err(),
        CryptoError::HmacMismatch,
        "a flipped MAC bit must be a hard error — aes-chain.md §6 step 3"
    );

    let replayed = rx_chain
        .open(&second.wire())
        .expect("the untouched frame must still open: the failed attempt advanced nothing");
    assert_eq!(
        hex::encode(replayed.chain.hmac_before),
        second.chain.direction_hmac_before,
        "the rolling RX HMAC must be untouched by the rejected frame, or one bad frame desyncs the session"
    );
    assert_eq!(
        replayed.plaintext, second.plaintext_utf8,
        "and the CBC chain must be untouched too, or the plaintext would be garbage"
    );

    // (b) a flipped bit in the ciphertext, which the MAC also covers.
    let mut flipped_body = third.wire();
    *flipped_body.first_mut().unwrap() ^= 0x80;
    assert_eq!(
        rx_chain.open(&flipped_body).unwrap_err(),
        CryptoError::HmacMismatch,
        "the MAC is computed over the ciphertext, so a ciphertext bit flip is caught too"
    );
    assert_eq!(
        rx_chain
            .open(&third.wire())
            .expect("the untouched third frame opens")
            .plaintext,
        third.plaintext_utf8,
        "neither chain advanced on the ciphertext tamper either"
    );
}

/// The MAC comparison must use the `hmac` crate's constant-time verification, not a hand-rolled
/// equality.
///
/// Timing cannot be asserted usefully in a unit test, so this checks the choice of primitive in
/// the source. `verify_truncated_left` is the right one because the wire tag is truncated.
#[test]
fn hmac_compare_is_constant_time() {
    assert!(
        CHAIN_SOURCE.contains("verify_truncated_left("),
        "the frame MAC must be checked with hmac's own constant-time truncated verify; \
         verify_slice cannot be used because the wire tag is truncated to 16 of 32 bytes"
    );

    for hand_rolled in [
        "computed == mac",
        "mac == computed",
        "computed != mac",
        "mac != computed",
        "== received",
        "received ==",
    ] {
        assert!(
            !CHAIN_SOURCE.contains(hand_rolled),
            "the MAC must never be compared with `{hand_rolled}`: a byte-at-a-time equality is \
             the one place aes-chain.md §6 records hcpy as the weaker implementation"
        );
    }
}

/// Chain state is per connection, and a reset puts it back exactly where a new chain starts.
///
/// After a reset the session's first frame opens again, reporting the same state as a freshly
/// constructed [`Opener`].
#[test]
fn chain_state_resets_on_new_session() {
    let corpus = corpus();
    let rx: Vec<&Frame> = corpus
        .frames
        .iter()
        .filter(|frame| frame.direction == "rx")
        .collect();
    let first = rx.first().expect("the corpus has rx frames");

    let mut reused = Opener::new(corpus_keys(), Direction::ApplianceToClient);
    for frame in rx.iter().take(3) {
        reused
            .open(&frame.wire())
            .unwrap_or_else(|error| panic!("{} must open: {error}", frame.label()));
    }

    reused.reset();
    let after_reset = reused.open(&first.wire()).expect(
        "after a reset the session's first frame must open again: a reconnect starts a fresh chain",
    );

    assert_eq!(
        hex::encode(after_reset.chain.hmac_before),
        "00000000000000000000000000000000",
        "the rolling HMAC restarts at 16 zero bytes — aes-chain.md §4"
    );
    assert_eq!(
        hex::encode(after_reset.chain.cbc_iv_before),
        first.chain.cbc_iv_before,
        "the CBC chain restarts from the raw iv — aes-chain.md §4"
    );
    assert_eq!(
        after_reset.plaintext, first.plaintext_utf8,
        "and the frame decrypts to the same plaintext as on a fresh chain"
    );

    let mut fresh = Opener::new(corpus_keys(), Direction::ApplianceToClient);
    let on_fresh = fresh
        .open(&first.wire())
        .expect("a fresh chain opens the first frame");
    assert_eq!(
        after_reset.chain, on_fresh.chain,
        "a reset chain and a newly constructed one are indistinguishable — aes-chain.md §4"
    );
}

/// Every client → appliance frame in the corpus, byte-identical.
///
/// The pad is injected from `pad_hex`; the corpus's pads come from one session-global stream
/// that cannot be reproduced per frame. Everything else is this crate's.
#[test]
fn tx_chain_matches_captured_ciphertext() {
    let corpus = corpus();
    let mut tx_chain = Sealer::new(corpus_keys(), Direction::ClientToAppliance);
    let mut sealed_count = 0_usize;

    for frame in corpus.frames.iter().filter(|frame| frame.direction == "tx") {
        let label = frame.label();
        let sealed = tx_chain
            .seal_with_pad(&frame.clear(), &frame.pad())
            .unwrap_or_else(|error| panic!("{label} must seal: {error}"));

        assert_eq!(
            hex::encode(&sealed.ciphertext),
            frame.ciphertext_hex,
            "{label}: ciphertext"
        );
        assert_eq!(hex::encode(sealed.mac), frame.mac_hex, "{label}: MAC");
        assert_eq!(
            hex::encode(&sealed.pad),
            frame.pad_hex,
            "{label}: the injected pad is used verbatim"
        );
        assert_eq!(
            hex::encode(sealed.chain.hmac_before),
            frame.chain.direction_hmac_before,
            "{label}: rolling TX HMAC entering the frame"
        );
        assert_eq!(
            hex::encode(sealed.chain.hmac_after),
            frame.chain.direction_hmac_after,
            "{label}: rolling TX HMAC leaving the frame"
        );
        assert_eq!(
            hex::encode(sealed.chain.cbc_iv_before),
            frame.chain.cbc_iv_before,
            "{label}: CBC chaining block entering the frame"
        );
        assert_eq!(
            hex::encode(sealed.chain.cbc_iv_after),
            frame.chain.cbc_iv_after,
            "{label}: CBC chaining block leaving the frame"
        );
        assert_eq!(
            hex::encode(sealed.into_wire()),
            frame.wire_hex,
            "{label}: the wire frame is ct || mac — aes-chain.md §5.4"
        );
        sealed_count += 1;
    }

    assert_eq!(
        sealed_count, 8,
        "the corpus carries eight tx frames; a change here means the fixture moved"
    );
}

/// The two directions share key material and nothing else.
///
/// Drives the whole session in wire order and asserts both counters after every frame, so a
/// shared rolling HMAC, a shared CBC object, or a chain advanced by the wrong direction fails.
#[test]
fn tx_and_rx_chains_are_independent() {
    let corpus = corpus();
    let mut session = AesSession::new(corpus_keys(), Role::Client);

    for frame in &corpus.frames {
        let label = frame.label();
        match frame.direction.as_str() {
            "rx" => {
                let opened = session
                    .open(&frame.wire())
                    .unwrap_or_else(|error| panic!("{label} must open: {error}"));
                assert_eq!(
                    opened.plaintext, frame.plaintext_utf8,
                    "{label}: plaintext in wire order"
                );
            }
            "tx" => {
                let sealed = session
                    .seal_with_pad(&frame.clear(), &frame.pad())
                    .unwrap_or_else(|error| panic!("{label} must seal: {error}"));
                assert_eq!(
                    hex::encode(sealed.into_wire()),
                    frame.wire_hex,
                    "{label}: wire frame in wire order"
                );
            }
            other => panic!("{label}: unknown direction {other}"),
        }

        assert_eq!(
            hex::encode(session.seal_hmac()),
            frame.chain.tx_hmac_after,
            "{label}: the TX counter after this frame"
        );
        assert_eq!(
            hex::encode(session.open_hmac()),
            frame.chain.rx_hmac_after,
            "{label}: the RX counter after this frame"
        );
    }

    assert_eq!(
        corpus.frames.len(),
        16,
        "the corpus is 16 frames spanning both directions"
    );
}

/// Seal with a real pad source, open with the mirror ([`Role::Appliance`]), in both directions.
///
/// Payloads: the pad edges (0, 1, 15, 16 bytes), a multi-byte UTF-8 string (pad arithmetic is
/// over bytes) and 40 pseudo-random payloads up to 300 bytes.
#[test]
fn roundtrip_encrypt_decrypt() {
    let mut pads = rand_chacha::ChaCha20Rng::seed_from_u64(0x00C0_FFEE);
    let mut payloads = rand_chacha::ChaCha20Rng::seed_from_u64(0x5EED_5EED);

    let mut cases = vec![
        String::new(),
        "a".to_owned(),
        "0123456789abcde".to_owned(),
        "0123456789abcdef".to_owned(),
        "grüße, ⧫ 𝄞 — mehrbyte".to_owned(),
    ];
    for _ in 0_u32..40 {
        let len = payloads.random_range(0_usize..=300);
        cases.push(
            (0..len)
                .map(|_| char::from(payloads.random_range(b' '..=b'~')))
                .collect(),
        );
    }

    let mut client = AesSession::new(corpus_keys(), Role::Client);
    let mut appliance = AesSession::new(corpus_keys(), Role::Appliance);

    for payload in &cases {
        let sealed = client
            .seal(payload.as_bytes(), &mut pads)
            .expect("a pad from build_pad is always valid");
        let opened = appliance
            .open(&sealed.into_wire())
            .expect("the mirror opens what the client sealed");
        assert_eq!(
            &opened.plaintext, payload,
            "client -> appliance round trip must be lossless"
        );

        let back = appliance
            .seal(payload.as_bytes(), &mut pads)
            .expect("a pad from build_pad is always valid");
        let received = client
            .open(&back.into_wire())
            .expect("the client opens what the mirror sealed");
        assert_eq!(
            &received.plaintext, payload,
            "appliance -> client round trip must be lossless"
        );
    }

    assert_eq!(
        client.seal_hmac(),
        appliance.open_hmac(),
        "the client's TX counter and the mirror's RX counter must track each other exactly"
    );
    assert_eq!(
        client.open_hmac(),
        appliance.seal_hmac(),
        "and likewise in the other direction"
    );
}

/// No public `Debug` may render the `iv`.
///
/// `ChainState.cbc_iv_before` is the raw `iv` on the first frame of every connection and is
/// reachable through [`hc_crypto::SealedFrame`] and [`hc_crypto::OpenedFrame`]. Both `{:?}` and
/// `{:#?}` are checked, as hex and as a decimal byte list, with whitespace squeezed out. The
/// plaintext is still rendered.
#[test]
fn public_debug_never_renders_the_iv() {
    /// The corpus's synthetic, published iv; not appliance key material.
    const IV_HEX: &str = "61e67b41fe8431a6e8f9137f2be6d656";

    fn squeeze(rendered: &str) -> String {
        rendered.chars().filter(|c| !c.is_whitespace()).collect()
    }

    let iv = hex::decode(IV_HEX).expect("IV_HEX is hex");
    let iv_decimal = iv.iter().map(u8::to_string).collect::<Vec<_>>().join(",");

    let corpus = corpus();
    let first_rx = corpus
        .frames
        .iter()
        .find(|frame| frame.direction == "rx")
        .expect("the corpus opens with an rx frame");

    let mut rx_chain = Opener::new(corpus_keys(), Direction::ApplianceToClient);
    let inbound = rx_chain.open(&first_rx.wire()).expect("frame 0 must open");

    let mut tx_chain = Sealer::new(corpus_keys(), Direction::ClientToAppliance);
    let pad = build_pad(5, &mut rand_chacha::ChaCha20Rng::seed_from_u64(0x1DED_1DED));
    let outbound = tx_chain
        .seal_with_pad(b"hello", &pad)
        .expect("build_pad always produces a valid pad");

    // Not vacuous: on the first frame the reported `cbc_iv_before` is the iv in both directions.
    assert_eq!(
        hex::encode(inbound.chain.cbc_iv_before),
        IV_HEX,
        "frame 0's CBC chaining block is the raw iv — aes-chain.md §4"
    );
    assert_eq!(
        hex::encode(outbound.chain.cbc_iv_before),
        IV_HEX,
        "a fresh Sealer's first frame is encrypted under the raw iv — aes-chain.md §4"
    );

    // Every public type that can reach a CBC chaining block belongs in this list. Only two
    // spellings are checked, so a `Debug` that base64s the bytes would slip through.
    let renderings = [
        ("OpenedFrame", format!("{inbound:?}")),
        ("OpenedFrame alternate", format!("{inbound:#?}")),
        ("ChainState (rx)", format!("{:?}", inbound.chain)),
        ("ChainState (rx) alternate", format!("{:#?}", inbound.chain)),
        ("SealedFrame", format!("{outbound:?}")),
        ("SealedFrame alternate", format!("{outbound:#?}")),
        ("ChainState (tx)", format!("{:?}", outbound.chain)),
        ("Sealer", format!("{tx_chain:?}")),
        ("Opener", format!("{rx_chain:?}")),
        (
            "AesSession",
            format!("{:?}", AesSession::new(corpus_keys(), Role::Client)),
        ),
    ];

    for (what, rendered) in &renderings {
        let squeezed = squeeze(rendered);
        assert!(
            !squeezed.contains(IV_HEX),
            "{what} renders the iv as hex: {rendered}"
        );
        assert!(
            !squeezed.contains(&iv_decimal),
            "{what} renders the iv as a byte list: {rendered}"
        );
    }

    assert!(
        format!("{inbound:?}").contains("/ei/initialValues"),
        "the plaintext stays visible: the frame inspector is a product feature"
    );
}

/// The reported CBC block must not go stale when a frame fails after decryption.
///
/// The real CBC state advances before unpad and UTF-8 decoding, so the reported block must too.
/// Driven through the UTF-8 arm: the frame authenticates, decrypts and unpads before failing.
#[test]
fn reported_cbc_iv_tracks_the_real_chain_after_a_failed_frame() {
    let mut appliance = Sealer::new(corpus_keys(), Direction::ApplianceToClient);
    let mut client = Opener::new(corpus_keys(), Direction::ApplianceToClient);

    let bad_utf8 = appliance
        .seal(
            b"\xff\xfe",
            &mut rand_chacha::ChaCha20Rng::seed_from_u64(0x0BAD_F00D),
        )
        .expect("build_pad always produces a valid pad");
    let first_cbc_iv_after = bad_utf8.chain.cbc_iv_after;
    let first_wire = bad_utf8.into_wire();

    let error = client
        .open(&first_wire)
        .expect_err("invalid UTF-8 is a hard error — aes-chain.md §8");
    assert!(
        matches!(error, CryptoError::Utf8 { .. }),
        "the frame authenticates and decrypts, so it must fail on the UTF-8 decode, not earlier: \
         {error:?}"
    );

    let good = appliance
        .seal(
            b"hello",
            &mut rand_chacha::ChaCha20Rng::seed_from_u64(0x600D_F00D),
        )
        .expect("build_pad always produces a valid pad");
    let opened = client
        .open(&good.into_wire())
        .expect("the next frame on the same chain still opens");

    assert_eq!(
        opened.plaintext, "hello",
        "decryption itself is unaffected by the failed frame"
    );
    assert_eq!(
        hex::encode(opened.chain.cbc_iv_before),
        hex::encode(first_cbc_iv_after),
        "the reported CBC block must be the failed frame's last ciphertext block, not the stale \
         value the success-path assignment left behind"
    );
}

/// An injected pad must start with `0x00`, end with its own length, and have
/// `pad_len ∈ {2..=16} ∪ {17}`; otherwise [`hc_crypto::CryptoError::InjectedPad`].
#[test]
fn injected_pad_must_have_the_documented_shape() {
    let chain = || Sealer::new(corpus_keys(), Direction::ClientToAppliance);

    // A well-formed pad from build_pad, as the positive control.
    let good = build_pad(16, &mut rand_chacha::ChaCha20Rng::seed_from_u64(0xF00D));
    assert!(
        chain().seal_with_pad(&[0x61; 16], &good).is_ok(),
        "a pad from build_pad is always §5.1-shaped"
    );

    // Block-aligned, ends with its own length, but 32 bytes long: outside {2..=16} ∪ {17}.
    let mut oversized = vec![0xAA_u8; 32];
    *oversized.last_mut().expect("non-empty") = 32;
    assert!(
        matches!(
            chain().seal_with_pad(&[0x61; 16], &oversized),
            Err(CryptoError::InjectedPad { .. })
        ),
        "pad_len 32 is outside §5.1's {{2..=16}} ∪ {{17}}"
    );

    // Right length, right trailing byte, but the leading structural 0x00 is missing.
    let mut wrong_first = vec![0x00_u8; 16];
    *wrong_first.first_mut().expect("non-empty") = 0xAA;
    *wrong_first.last_mut().expect("non-empty") = 16;
    assert!(
        matches!(
            chain().seal_with_pad(b"", &wrong_first),
            Err(CryptoError::InjectedPad { .. })
        ),
        "§5.1: the first pad byte is always 0x00"
    );
}
