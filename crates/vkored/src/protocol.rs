//! Host-side framing helpers over the transport-neutral `kore-hostlink` codec.

pub use kore_hostlink::*;
use zerocopy::IntoBytes;

pub const HELLO_LEN: usize = core::mem::size_of::<HelloV1>();
pub const HELLO_ACK_LEN: usize = core::mem::size_of::<HelloAckV1>();
pub const ROUTE_OPEN_LEN: usize = core::mem::size_of::<RouteOpenV1>();

pub fn frame(envelope: &EnvelopeV1, payload: &[u8]) -> Vec<u8> {
    debug_assert_eq!(envelope.payload_len.get() as usize, payload.len());
    let mut bytes = Vec::with_capacity(ENVELOPE_LEN + payload.len());
    bytes.extend_from_slice(envelope.as_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

pub fn envelope(
    kind: u16,
    epoch: LinkEpoch,
    route: RouteId,
    generation: u64,
    sequence: u64,
    request: RequestId,
    payload_len: usize,
) -> EnvelopeV1 {
    link_envelope(
        kind,
        epoch,
        route,
        generation,
        sequence,
        request,
        payload_len,
    )
}
