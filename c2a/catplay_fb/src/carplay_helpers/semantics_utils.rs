use super::{CarPlayNalRewriter, RewriteResult};
use crate::H264FrameBufferError;

pub(super) fn semantics_rbsp(nal: &[u8], expected_type: u8) -> RewriteResult<Vec<u8>> {
    let prefix_len = CarPlayNalRewriter::annexb_prefix_len(nal)?;
    let header = nal
        .get(prefix_len)
        .ok_or(H264FrameBufferError::X264BitstreamRewrite("missing semantic NAL header"))?;
    if header & 0x1f != expected_type {
        return Err(H264FrameBufferError::X264BitstreamRewrite("unexpected semantic NAL type"));
    }
    Ok(CarPlayNalRewriter::remove_emulation_prevention(&nal[prefix_len + 1..]))
}

pub(super) fn rbsp_data_bit_len(data: &[u8]) -> RewriteResult<usize> {
    let (last_index, last_byte) = data
        .iter()
        .enumerate()
        .rev()
        .find(|(_, byte)| **byte != 0)
        .ok_or(H264FrameBufferError::X264BitstreamRewrite("RBSP has no stop bit"))?;
    Ok(last_index * 8 + 7 - last_byte.trailing_zeros() as usize)
}
