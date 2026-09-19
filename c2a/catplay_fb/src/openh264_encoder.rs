use bytes::BytesMut;
use catplay_tracing_macro::trace_time;
use openh264::OpenH264API;
use openh264::encoder::{Encoder, EncoderConfig, IntraFramePeriod, RateControlMode, SpsPpsStrategy, UsageType};

use crate::{H264Encoder, H264FrameBufferError, YuvShadowBuffer};

pub struct OpenH264FrameBuffer {
    encoder: Encoder,
    yuv: YuvShadowBuffer,
    headers: BytesMut,
}

unsafe impl Send for OpenH264FrameBuffer {}

impl OpenH264FrameBuffer {
    pub fn new(width: i32, height: i32, _fps: i32) -> Result<Self, H264FrameBufferError> {
        let config = EncoderConfig::new()
            // .usage_type(UsageType::ScreenContentRealTime)
            .rate_control_mode(RateControlMode::Off)
            .skip_frames(false)
            .scene_change_detect(false)
            .adaptive_quantization(false)
            .background_detection(false)
            .sps_pps_strategy(SpsPpsStrategy::ConstantId)
            .intra_frame_period(IntraFramePeriod::from_num_frames(1))
            .num_threads(1);

        let mut this = Self {
            encoder: Encoder::with_api_config(OpenH264API::from_source(), config)?,
            yuv: YuvShadowBuffer::i420(width as usize, height as usize),
            headers: BytesMut::new(),
        };

        this.prime_headers()?;
        Ok(this)
    }

    pub fn update_rgba(&mut self, rgba: &[u8], out: &mut BytesMut) -> Result<(), H264FrameBufferError> {
        <Self as H264Encoder>::update_rgba(self, rgba, out)
    }

    pub fn get_headers(&mut self, out: &mut BytesMut) -> Result<(), H264FrameBufferError> {
        <Self as H264Encoder>::get_headers(self, out)
    }

    fn prime_headers(&mut self) -> Result<(), H264FrameBufferError> {
        if !self.headers.is_empty() {
            return Ok(());
        }

        self.encoder.force_intra_frame();
        let bitstream = self.encoder.encode(&self.yuv)?;

        for layer_idx in 0..bitstream.num_layers() {
            let layer = bitstream.layer(layer_idx).unwrap();
            for nal_idx in 0..layer.nal_count() {
                let nal = layer.nal_unit(nal_idx).unwrap();
                if is_parameter_set_nal(nal) {
                    self.headers.extend_from_slice(nal);
                }
            }
        }

        Ok(())
    }
}

impl H264Encoder for OpenH264FrameBuffer {
    #[trace_time]
    fn update_rgba(&mut self, rgba: &[u8], out: &mut BytesMut) -> Result<(), H264FrameBufferError> {
        self.yuv.update(rgba)?;
        self.encoder.force_intra_frame();

        let bitstream = self.encoder.encode(&self.yuv)?;

        for layer_idx in 0..bitstream.num_layers() {
            let layer = bitstream.layer(layer_idx).unwrap();
            for nal_idx in 0..layer.nal_count() {
                let nal = layer.nal_unit(nal_idx).unwrap();
                if is_parameter_set_nal(nal) {
                    if self.headers.is_empty() {
                        self.headers.extend_from_slice(nal);
                    }
                    continue;
                }

                out.extend_from_slice(nal);
            }
        }

        Ok(())
    }

    fn get_headers(&mut self, out: &mut BytesMut) -> Result<(), H264FrameBufferError> {
        self.prime_headers()?;
        out.extend_from_slice(&self.headers);
        Ok(())
    }
}

fn is_parameter_set_nal(nal: &[u8]) -> bool {
    matches!(nal_type(nal), Some(7 | 8))
}

fn nal_type(nal: &[u8]) -> Option<u8> {
    let start = if nal.starts_with(&[0, 0, 0, 1]) {
        4
    } else if nal.starts_with(&[0, 0, 1]) {
        3
    } else {
        0
    };

    nal.get(start).map(|byte| byte & 0x1f)
}
