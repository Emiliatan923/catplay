use super::semantics_utils::semantics_rbsp;
use super::{BitReader, RewriteResult};
use crate::H264FrameBufferError;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct IdrSemantics {
    first_mb_in_slice: u32,
    slice_type: u32,
    pps_id: u32,
    frame_num: u32,
    idr_pic_id: u32,
    pic_order_cnt_lsb: u32,
    no_output_of_prior_pics: u8,
    long_term_reference: u8,
    slice_qp_delta: i32,
    disable_deblocking_filter_idc: u32,
    deblock_offsets: Option<[i32; 2]>,
    cabac_payload_present: bool,
}

impl IdrSemantics {
    pub(super) fn parse(nal: &[u8]) -> RewriteResult<Self> {
        let rbsp = semantics_rbsp(nal, 5)?;
        let mut reader = BitReader::new(&rbsp);
        let first_mb_in_slice = reader.ue()?;
        let slice_type = reader.ue()?;
        let pps_id = reader.ue()?;
        let frame_num = reader.bits(12)?;
        let idr_pic_id = reader.ue()?;
        let pic_order_cnt_lsb = reader.bits(13)?;
        let no_output_of_prior_pics = reader.bit()?;
        let long_term_reference = reader.bit()?;
        let slice_qp_delta = reader.se()?;
        let disable_deblocking_filter_idc = reader.ue()?;
        let deblock_offsets = if disable_deblocking_filter_idc != 1 {
            Some([reader.se()?, reader.se()?])
        } else {
            None
        };
        while reader.pos % 8 != 0 {
            if reader.bit()? != 1 {
                return Err(H264FrameBufferError::X264BitstreamRewrite("invalid CABAC alignment"));
            }
        }

        Ok(Self {
            first_mb_in_slice,
            slice_type,
            pps_id,
            frame_num,
            idr_pic_id,
            pic_order_cnt_lsb,
            no_output_of_prior_pics,
            long_term_reference,
            slice_qp_delta,
            disable_deblocking_filter_idc,
            deblock_offsets,
            cabac_payload_present: reader.pos / 8 < rbsp.len(),
        })
    }
}
