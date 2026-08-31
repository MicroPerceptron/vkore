//! Bounded, digest-checked inbound bulk-transfer lifecycle.

use kore_hostlink::{
    BULK_CHUNKS_MAX, BulkAbortV1, BulkChunkHeaderV1, BulkFinishV1, BulkId, BulkRefV1, RequestId,
    RouteId,
};
use zerocopy::FromBytes;

pub const BULK_TRANSFERS_PER_ROUTE_MAX: usize = 4;
pub const BULK_TRANSFERS_PER_LINK_MAX: usize = 12;
pub const HOST_BULK_OBJECT_MAX: usize = 8 * 1024 * 1024;
pub const HOST_BULK_STAGING_MAX: usize = 32 * 1024 * 1024;
/// Cumulative bulk reservations retained by one route until finish, abort, or
/// cancellation.  This is deliberately the per-route transfer count times the
/// frozen per-object chunk ceiling: chunk credits are staging reservations,
/// not an acknowledgement window.
pub const HOST_BULK_CHUNK_CREDITS_PER_ROUTE: u64 =
    (BULK_TRANSFERS_PER_ROUTE_MAX as u64) * (BULK_CHUNKS_MAX as u64);
pub const HOST_BULK_BYTE_CREDITS_PER_ROUTE: u64 = 16 * 1024 * 1024;
pub const BULK_DATA_MAX: usize =
    kore_hostlink::BULK_CHUNK_MAX - kore_hostlink::BULK_CHUNK_HEADER_LEN;

const _: () = assert!(HOST_BULK_CHUNK_CREDITS_PER_ROUTE == 1024);
const _: () = assert!(
    HOST_BULK_BYTE_CREDITS_PER_ROUTE >= (BULK_TRANSFERS_PER_ROUTE_MAX * 1024 * 1024) as u64
);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BulkError {
    InvalidLength,
    InvalidShape,
    TooLarge,
    TooMany,
    ArithmeticOverflow,
    Duplicate,
    OutOfOrder,
    Unknown,
    Digest,
    RequestMismatch,
    Backpressure,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletedBulk {
    pub route: RouteId,
    pub request: RequestId,
    pub bulk_id: BulkId,
    pub content_kind: u16,
    pub bytes: Vec<u8>,
}

struct Transfer {
    route: RouteId,
    request: RequestId,
    reference: BulkRefV1,
    next_offset: u32,
    chunks: u16,
    hasher: blake3::Hasher,
    bytes: Vec<u8>,
    opened_request: bool,
}

#[derive(Default)]
pub struct BulkManager {
    transfers: Vec<Transfer>,
    reserved: usize,
}

impl BulkManager {
    #[must_use]
    pub fn active(&self) -> usize {
        self.transfers.len()
    }

    pub fn reset(&mut self) {
        self.transfers.clear();
        self.reserved = 0;
    }

    pub fn begin(
        &mut self,
        route: RouteId,
        request: RequestId,
        reference: BulkRefV1,
        opened_request: bool,
    ) -> Result<(), BulkError> {
        if request.is_zero() || !reference.shape_is_valid() {
            return Err(BulkError::InvalidShape);
        }
        let total = reference.total_len.get() as usize;
        if total > HOST_BULK_OBJECT_MAX {
            return Err(BulkError::TooLarge);
        }
        if self.transfers.len() == BULK_TRANSFERS_PER_LINK_MAX
            || self
                .transfers
                .iter()
                .filter(|transfer| transfer.route == route)
                .count()
                == BULK_TRANSFERS_PER_ROUTE_MAX
        {
            return Err(BulkError::TooMany);
        }
        if self.transfers.iter().any(|transfer| {
            transfer.route == route && transfer.reference.bulk_id == reference.bulk_id
        }) {
            return Err(BulkError::Duplicate);
        }
        let next = self
            .reserved
            .checked_add(total)
            .ok_or(BulkError::ArithmeticOverflow)?;
        if next > HOST_BULK_STAGING_MAX {
            return Err(BulkError::Backpressure);
        }
        self.reserved = next;
        self.transfers.push(Transfer {
            route,
            request,
            reference,
            next_offset: 0,
            chunks: 0,
            hasher: blake3::Hasher::new(),
            bytes: Vec::new(),
            opened_request,
        });
        Ok(())
    }

    pub fn chunk(
        &mut self,
        route: RouteId,
        request: RequestId,
        payload: &[u8],
    ) -> Result<usize, BulkError> {
        if payload.len() <= kore_hostlink::BULK_CHUNK_HEADER_LEN {
            return Err(BulkError::InvalidLength);
        }
        let header =
            BulkChunkHeaderV1::read_from_bytes(&payload[..kore_hostlink::BULK_CHUNK_HEADER_LEN])
                .map_err(|_| BulkError::InvalidShape)?;
        let bulk_id = header.bulk_id;
        let offset = header.offset.get();
        let expected_digest = header.digest_blake3;
        let data = &payload[kore_hostlink::BULK_CHUNK_HEADER_LEN..];
        if data.len() > BULK_DATA_MAX {
            return Err(BulkError::TooLarge);
        }
        let transfer = self.find_mut(route, bulk_id)?;
        if transfer.request != request {
            return Err(BulkError::RequestMismatch);
        }
        if offset < transfer.next_offset {
            return Err(BulkError::Duplicate);
        }
        if offset > transfer.next_offset {
            return Err(BulkError::OutOfOrder);
        }
        if transfer.chunks == transfer.reference.chunk_count.get()
            || transfer.chunks == BULK_CHUNKS_MAX
        {
            return Err(BulkError::TooMany);
        }
        let next = (offset as usize)
            .checked_add(data.len())
            .ok_or(BulkError::ArithmeticOverflow)?;
        if next > transfer.reference.total_len.get() as usize {
            return Err(BulkError::TooLarge);
        }
        if *blake3::hash(data).as_bytes() != expected_digest {
            return Err(BulkError::Digest);
        }
        transfer.bytes.extend_from_slice(data);
        transfer.hasher.update(data);
        transfer.next_offset = u32::try_from(next).map_err(|_| BulkError::ArithmeticOverflow)?;
        transfer.chunks += 1;
        Ok(data.len())
    }

    pub fn finish(
        &mut self,
        route: RouteId,
        request: RequestId,
        payload: &[u8],
    ) -> Result<(CompletedBulk, u64, u64, bool), BulkError> {
        if payload.len() != kore_hostlink::BULK_FINISH_LEN {
            return Err(BulkError::InvalidLength);
        }
        let finish = BulkFinishV1::read_from_bytes(payload).map_err(|_| BulkError::InvalidShape)?;
        let id = finish.bulk_id;
        let final_total = finish.final_total.get();
        let final_digest = finish.digest_blake3;
        let index = self.index(route, id)?;
        let transfer = &self.transfers[index];
        if transfer.request != request {
            return Err(BulkError::RequestMismatch);
        }
        if final_total != transfer.reference.total_len.get()
            || transfer.next_offset != final_total
            || transfer.chunks != transfer.reference.chunk_count.get()
        {
            return Err(BulkError::InvalidLength);
        }
        if final_digest != transfer.reference.digest_blake3
            || final_digest != *transfer.hasher.clone().finalize().as_bytes()
        {
            return Err(BulkError::Digest);
        }
        let transfer = self.transfers.swap_remove(index);
        self.reserved -= transfer.reference.total_len.get() as usize;
        let bytes = transfer.reference.total_len.get() as u64;
        let chunks = transfer.reference.chunk_count.get() as u64;
        Ok((
            CompletedBulk {
                route,
                request,
                bulk_id: id,
                content_kind: transfer.reference.content_kind.get(),
                bytes: transfer.bytes,
            },
            bytes,
            chunks,
            transfer.opened_request,
        ))
    }

    pub fn abort(
        &mut self,
        route: RouteId,
        request: RequestId,
        payload: &[u8],
    ) -> Result<(BulkId, u64, u64, bool), BulkError> {
        if payload.len() != kore_hostlink::BULK_ABORT_LEN {
            return Err(BulkError::InvalidLength);
        }
        let abort = BulkAbortV1::read_from_bytes(payload).map_err(|_| BulkError::InvalidShape)?;
        let reason = abort.reason.get();
        if reason > crate::protocol::RefusalCode::Internal as u16 {
            return Err(BulkError::InvalidShape);
        }
        let id = abort.bulk_id;
        let index = self.index(route, id)?;
        if self.transfers[index].request != request {
            return Err(BulkError::RequestMismatch);
        }
        Ok(self.remove(index))
    }

    pub fn cancel_request(
        &mut self,
        route: RouteId,
        request: RequestId,
    ) -> Vec<(BulkId, u64, u64, bool)> {
        let mut retired = Vec::new();
        let mut index = 0;
        while index < self.transfers.len() {
            if self.transfers[index].route == route && self.transfers[index].request == request {
                retired.push(self.remove(index));
            } else {
                index += 1;
            }
        }
        retired
    }

    fn remove(&mut self, index: usize) -> (BulkId, u64, u64, bool) {
        let transfer = self.transfers.swap_remove(index);
        self.reserved -= transfer.reference.total_len.get() as usize;
        (
            transfer.reference.bulk_id,
            transfer.next_offset as u64,
            transfer.chunks as u64,
            transfer.opened_request,
        )
    }

    fn index(&self, route: RouteId, id: BulkId) -> Result<usize, BulkError> {
        self.transfers
            .iter()
            .position(|transfer| transfer.route == route && transfer.reference.bulk_id == id)
            .ok_or(BulkError::Unknown)
    }

    fn find_mut(&mut self, route: RouteId, id: BulkId) -> Result<&mut Transfer, BulkError> {
        let index = self.index(route, id)?;
        Ok(&mut self.transfers[index])
    }
}
