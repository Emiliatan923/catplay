use super::pps_extension_semantics::PpsExtensionSemantics;
use super::semantics_utils::{rbsp_data_bit_len, semantics_rbsp};
use super::{BitReader, RewriteResult};
use crate::H264FrameBufferError;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct PpsSemantics {
    pps_id: u32,
    sps_id: u32,
    cabac: u8,
    bottom_field_pic_order_in_frame_present: u8,
    num_ref_idx_l0_default_active_minus1: u32,
    num_ref_idx_l1_default_active_minus1: u32,
    weighted_pred: u8,
    weighted_bipred_idc: u32,
    pic_init_qp_minus26: i32,
    pic_init_qs_minus26: i32,
    chroma_qp_index_offset: i32,
    deblocking_filter_control_present: u8,
    constrained_intra_pred: u8,
    redundant_pic_cnt_present: u8,
    extension: Option<PpsExtensionSemantics>,
}

impl PpsSemantics {
    pub(super) fn parse(nal: &[u8]) -> RewriteResult<Self> {
        let rbsp = semantics_rbsp(nal, 8)?;
        let data_end = rbsp_data_bit_len(&rbsp)?;
        let mut reader = BitReader::new(&rbsp);
        let pps_id = reader.ue()?;
        let sps_id = reader.ue()?;
        let cabac = reader.bit()?;
        let bottom_field_pic_order_in_frame_present = reader.bit()?;
        if reader.ue()? != 0 {
            return Err(H264FrameBufferError::X264BitstreamRewrite(
                "semantic PPS parser does not support slice groups",
            ));
        }
        let num_ref_idx_l0_default_active_minus1 = reader.ue()?;
        let num_ref_idx_l1_default_active_minus1 = reader.ue()?;
        let weighted_pred = reader.bit()?;
        let weighted_bipred_idc = reader.bits(2)?;
        let pic_init_qp_minus26 = reader.se()?;
        let pic_init_qs_minus26 = reader.se()?;
        let chroma_qp_index_offset = reader.se()?;
        let deblocking_filter_control_present = reader.bit()?;
        let constrained_intra_pred = reader.bit()?;
        let redundant_pic_cnt_present = reader.bit()?;
        let extension = if reader.pos < data_end {
            let transform_8x8_mode = reader.bit()?;
            if reader.bit()? != 0 {
                return Err(H264FrameBufferError::X264BitstreamRewrite(
                    "semantic PPS parser does not support scaling matrices",
                ));
            }
            Some(PpsExtensionSemantics {
                transform_8x8_mode,
                second_chroma_qp_index_offset: reader.se()?,
            })
        } else {
            None
        };
        if reader.pos != data_end {
            return Err(H264FrameBufferError::X264BitstreamRewrite("unparsed PPS data"));
        }

        Ok(Self {
            pps_id,
            sps_id,
            cabac,
            bottom_field_pic_order_in_frame_present,
            num_ref_idx_l0_default_active_minus1,
            num_ref_idx_l1_default_active_minus1,
            weighted_pred,
            weighted_bipred_idc,
            pic_init_qp_minus26,
            pic_init_qs_minus26,
            chroma_qp_index_offset,
            deblocking_filter_control_present,
            constrained_intra_pred,
            redundant_pic_cnt_present,
            extension,
        })
    }
}
