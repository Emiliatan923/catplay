use super::bitstream_restriction_semantics::BitstreamRestrictionSemantics;
use super::timing_semantics::TimingSemantics;
use super::video_signal_semantics::VideoSignalSemantics;
use super::{BitReader, RewriteResult};
use crate::H264FrameBufferError;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct VuiSemantics {
    pub(super) aspect_ratio: Option<(u32, Option<[u32; 2]>)>,
    pub(super) overscan_appropriate: Option<u8>,
    pub(super) video_signal: Option<VideoSignalSemantics>,
    pub(super) chroma_loc: Option<[u32; 2]>,
    pub(super) timing: Option<TimingSemantics>,
    pub(super) pic_struct_present: u8,
    pub(super) bitstream_restriction: Option<BitstreamRestrictionSemantics>,
}

impl VuiSemantics {
    pub(super) fn parse(reader: &mut BitReader<'_>) -> RewriteResult<Self> {
        let aspect_ratio = if reader.bit()? != 0 {
            let idc = reader.bits(8)?;
            let extended_sar = if idc == 255 {
                Some([reader.bits(16)?, reader.bits(16)?])
            } else {
                None
            };
            Some((idc, extended_sar))
        } else {
            None
        };
        let overscan_appropriate = (reader.bit()? != 0).then(|| reader.bit()).transpose()?;
        let video_signal = if reader.bit()? != 0 {
            let video_format = reader.bits(3)?;
            let full_range = reader.bit()?;
            let colour_description = if reader.bit()? != 0 {
                Some([reader.bits(8)?, reader.bits(8)?, reader.bits(8)?])
            } else {
                None
            };
            Some(VideoSignalSemantics {
                video_format,
                full_range,
                colour_description,
            })
        } else {
            None
        };
        let chroma_loc = if reader.bit()? != 0 {
            Some([reader.ue()?, reader.ue()?])
        } else {
            None
        };
        let timing = if reader.bit()? != 0 {
            Some(TimingSemantics {
                num_units_in_tick: reader.bits(32)?,
                time_scale: reader.bits(32)?,
                fixed_frame_rate: reader.bit()?,
            })
        } else {
            None
        };
        let nal_hrd_present = reader.bit()?;
        let vcl_hrd_present = reader.bit()?;
        if nal_hrd_present != 0 || vcl_hrd_present != 0 {
            return Err(H264FrameBufferError::X264BitstreamRewrite(
                "semantic SPS parser does not support HRD parameters",
            ));
        }
        let pic_struct_present = reader.bit()?;
        let bitstream_restriction = if reader.bit()? != 0 {
            Some(BitstreamRestrictionSemantics {
                motion_vectors_over_pic_boundaries: reader.bit()?,
                max_bytes_per_pic_denom: reader.ue()?,
                max_bits_per_mb_denom: reader.ue()?,
                log2_max_mv_length_horizontal: reader.ue()?,
                log2_max_mv_length_vertical: reader.ue()?,
                num_reorder_frames: reader.ue()?,
                max_dec_frame_buffering: reader.ue()?,
            })
        } else {
            None
        };

        Ok(Self {
            aspect_ratio,
            overscan_appropriate,
            video_signal,
            chroma_loc,
            timing,
            pic_struct_present,
            bitstream_restriction,
        })
    }
}
