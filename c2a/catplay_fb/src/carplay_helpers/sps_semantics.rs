use super::semantics_utils::{rbsp_data_bit_len, semantics_rbsp};
use super::vui_semantics::VuiSemantics;
use super::{BitReader, RewriteResult};
use crate::H264FrameBufferError;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct SpsSemantics {
    profile_idc: u32,
    constraints: u32,
    level_idc: u32,
    sps_id: u32,
    chroma_format_idc: u32,
    bit_depth_luma_minus8: u32,
    bit_depth_chroma_minus8: u32,
    log2_max_frame_num_minus4: u32,
    pic_order_cnt_type: u32,
    log2_max_pic_order_cnt_lsb_minus4: u32,
    max_num_ref_frames: u32,
    gaps_in_frame_num_value_allowed: u8,
    pic_width_in_mbs_minus1: u32,
    pic_height_in_map_units_minus1: u32,
    frame_mbs_only: u8,
    direct_8x8_inference: u8,
    crop: Option<[u32; 4]>,
    vui: VuiSemantics,
}

impl SpsSemantics {
    pub(super) fn parse(nal: &[u8]) -> RewriteResult<Self> {
        let rbsp = semantics_rbsp(nal, 7)?;
        let data_end = rbsp_data_bit_len(&rbsp)?;
        let mut reader = BitReader::new(&rbsp);

        let profile_idc = reader.bits(8)?;
        let constraints = reader.bits(8)?;
        let level_idc = reader.bits(8)?;
        let sps_id = reader.ue()?;
        if profile_idc != 100 {
            return Err(H264FrameBufferError::X264BitstreamRewrite(
                "semantic SPS parser expects High Profile",
            ));
        }
        let chroma_format_idc = reader.ue()?;
        if chroma_format_idc == 3 {
            reader.bit()?;
        }
        let bit_depth_luma_minus8 = reader.ue()?;
        let bit_depth_chroma_minus8 = reader.ue()?;
        reader.bit()?;
        if reader.bit()? != 0 {
            return Err(H264FrameBufferError::X264BitstreamRewrite(
                "semantic SPS parser does not support scaling matrices",
            ));
        }
        let log2_max_frame_num_minus4 = reader.ue()?;
        let pic_order_cnt_type = reader.ue()?;
        if pic_order_cnt_type != 0 {
            return Err(H264FrameBufferError::X264BitstreamRewrite(
                "semantic SPS parser expects pic_order_cnt_type 0",
            ));
        }
        let log2_max_pic_order_cnt_lsb_minus4 = reader.ue()?;
        let max_num_ref_frames = reader.ue()?;
        let gaps_in_frame_num_value_allowed = reader.bit()?;
        let pic_width_in_mbs_minus1 = reader.ue()?;
        let pic_height_in_map_units_minus1 = reader.ue()?;
        let frame_mbs_only = reader.bit()?;
        if frame_mbs_only == 0 {
            reader.bit()?;
        }
        let direct_8x8_inference = reader.bit()?;
        let crop = if reader.bit()? != 0 {
            Some([reader.ue()?, reader.ue()?, reader.ue()?, reader.ue()?])
        } else {
            None
        };
        if reader.bit()? == 0 {
            return Err(H264FrameBufferError::X264BitstreamRewrite("semantic SPS parser expects VUI"));
        }
        let vui = VuiSemantics::parse(&mut reader)?;
        if reader.pos != data_end {
            return Err(H264FrameBufferError::X264BitstreamRewrite("unparsed SPS data"));
        }

        Ok(Self {
            profile_idc,
            constraints,
            level_idc,
            sps_id,
            chroma_format_idc,
            bit_depth_luma_minus8,
            bit_depth_chroma_minus8,
            log2_max_frame_num_minus4,
            pic_order_cnt_type,
            log2_max_pic_order_cnt_lsb_minus4,
            max_num_ref_frames,
            gaps_in_frame_num_value_allowed,
            pic_width_in_mbs_minus1,
            pic_height_in_map_units_minus1,
            frame_mbs_only,
            direct_8x8_inference,
            crop,
            vui,
        })
    }
}
