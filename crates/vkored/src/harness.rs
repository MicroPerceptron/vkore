//! Frame construction and deliberate fault injection for conformance tests.

use crate::protocol;
use kore_hostlink::{EnvelopeV1, LinkEpoch};
use zerocopy::FromBytes;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    BadMagic,
    DeclaredLength(u32),
    Replay,
    StaleEpoch(LinkEpoch),
    StaleGeneration(u64),
}

#[derive(Default)]
pub struct FrameHarness {
    last: Option<Vec<u8>>,
}
impl FrameHarness {
    pub fn valid(&mut self, envelope: &EnvelopeV1, payload: &[u8]) -> Vec<u8> {
        let bytes = protocol::frame(envelope, payload);
        self.last = Some(bytes.clone());
        bytes
    }
    pub fn inject(&self, mut bytes: Vec<u8>, fault: Fault) -> Vec<u8> {
        match fault {
            Fault::BadMagic => bytes[..4].copy_from_slice(&0xdead_beefu32.to_le_bytes()),
            Fault::DeclaredLength(n) => bytes[80..84].copy_from_slice(&n.to_le_bytes()),
            Fault::Replay => {
                return self
                    .last
                    .clone()
                    .expect("construct a valid frame before replay");
            }
            Fault::StaleEpoch(epoch) => bytes[16..32].copy_from_slice(&epoch.0),
            Fault::StaleGeneration(generation) => {
                bytes[48..56].copy_from_slice(&generation.to_le_bytes())
            }
        }
        bytes
    }
    pub fn split<'a>(&self, bytes: &'a [u8], points: &[usize]) -> Vec<&'a [u8]> {
        let mut out = Vec::new();
        let mut start = 0;
        for &end in points {
            let end = end.min(bytes.len());
            if end >= start {
                out.push(&bytes[start..end]);
                start = end;
            }
        }
        out.push(&bytes[start..]);
        out
    }
}

pub fn decode_envelope(bytes: &[u8]) -> EnvelopeV1 {
    EnvelopeV1::read_from_bytes(&bytes[..kore_hostlink::ENVELOPE_LEN]).expect("complete envelope")
}
