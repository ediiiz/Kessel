//! Shared corpus loader for the `hc-proto` integration tests.
//!
//! The corpus is `fixtures/aes/hob-session.json`: 16 frames of a real hob handshake, scrubbed and
//! re-encrypted under a synthetic, published psk/iv. It is embedded with `include_str!` so the
//! tests do no filesystem I/O.

#![allow(
    dead_code,
    unreachable_pub,
    reason = "each integration test file compiles this module independently and uses a subset; \
              the items are pub for cross-file use but each test binary reaches only some"
)]

use hc_crypto::SessionKeys;
use serde::Deserialize;
use serde_json::Value;

/// `fixtures/aes/hob-session.json`, `schema: kessel.capture/1`, tier `golden`.
const CORPUS_JSON: &str = include_str!("../../../../fixtures/aes/hob-session.json");

#[derive(Debug, Deserialize)]
pub struct Corpus {
    pub key: CorpusKey,
    pub client: CorpusClient,
    pub frames: Vec<Frame>,
}

#[derive(Debug, Deserialize)]
pub struct CorpusKey {
    /// Synthetic and published; not appliance key material.
    pub psk64: String,
    pub iv64: String,
}

#[derive(Debug, Deserialize)]
pub struct CorpusClient {
    /// The client's configured `deviceName` — `"hcpy"` in this capture.
    pub device_name: String,
    /// The client's configured `deviceID` — `"0badcafe"` in this capture.
    pub device_id: String,
}

/// One captured frame. `plaintext_hex` is the authoritative plaintext; `plaintext_utf8` is stored
/// alongside it for readability.
#[derive(Debug, Deserialize)]
pub struct Frame {
    pub seq: u32,
    /// `"rx"` = appliance → client, `"tx"` = client → appliance.
    pub direction: String,
    pub resource: Option<String>,
    pub wire_hex: String,
    pub plaintext_hex: String,
    pub plaintext_utf8: String,
    pub pad_hex: String,
    pub pad_len: usize,
    pub s_id: u64,
    pub msg_id: u64,
    pub action: String,
    #[serde(default)]
    pub json: Value,
}

impl Frame {
    /// The full WebSocket binary-frame bytes (`ct ‖ mac`).
    #[must_use]
    pub fn wire(&self) -> Vec<u8> {
        hex::decode(&self.wire_hex).expect("wire_hex is hex")
    }

    /// The authoritative plaintext bytes, never the parsed JSON.
    #[must_use]
    pub fn plaintext(&self) -> Vec<u8> {
        hex::decode(&self.plaintext_hex).expect("plaintext_hex is hex")
    }

    /// The pad's random middle: the bytes between the leading `0x00` and the trailing length byte.
    /// This is what `hc_crypto::PadSource::fill_pad` supplies, so feeding these reproduces the
    /// corpus ciphertext.
    #[must_use]
    pub fn pad_middle(&self) -> Vec<u8> {
        let pad = hex::decode(&self.pad_hex).expect("pad_hex is hex");
        pad.get(1..pad.len().saturating_sub(1))
            .expect("pad has a leading and trailing byte")
            .to_vec()
    }

    #[must_use]
    pub fn is_tx(&self) -> bool {
        self.direction == "tx"
    }
}

#[must_use]
pub fn corpus() -> Corpus {
    serde_json::from_str(CORPUS_JSON).expect("the golden corpus parses")
}

impl Corpus {
    #[must_use]
    pub fn frame(&self, seq: u32) -> &Frame {
        self.frames
            .iter()
            .find(|frame| frame.seq == seq)
            .expect("frame seq present")
    }

    #[must_use]
    pub fn keys(&self) -> SessionKeys {
        SessionKeys::from_base64(&self.key.psk64, &self.key.iv64)
            .expect("the synthetic psk64/iv64 decode and validate")
    }
}
