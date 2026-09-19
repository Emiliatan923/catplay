use bytes::BytesMut;

use crate::YuvShadowBufferError;

pub trait H264Encoder {
    fn update_rgba(&mut self, rgba: &[u8], out: &mut BytesMut) -> Result<(), H264FrameBufferError>;
    fn get_headers(&mut self, out: &mut BytesMut) -> Result<(), H264FrameBufferError>;
}

#[derive(Debug, thiserror::Error)]
pub enum H264FrameBufferError {
    #[error("x264 EncoderEncode: {0:?}")]
    X264EncoderEncode(i32),
    #[error("x264 EncoderHeaders: {0:?}")]
    X264EncoderHeaders(i32),
    #[error("x264 EncoderOpenNull")]
    X264EncoderOpenNull,
    #[error("x264 DefaultPreset: {0:?}")]
    X264DefaultPreset(i32),
    #[error("x264 ApplyProfile: {0:?}")]
    X264ApplyProfile(i32),
    #[error("x264 bitstream rewrite: {0}")]
    X264BitstreamRewrite(&'static str),
    #[error("OpenH264: {0}")]
    #[cfg(feature = "openh264")]
    OpenH264(#[from] openh264::Error),
    #[error("Yuv error: {0}")]
    Yuv(#[from] YuvShadowBufferError),
}
