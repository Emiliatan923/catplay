mod asset_cache;
mod canvas;
mod dirty_rect;
mod font_renderer;
mod keyframe_cache_file;
mod layout;
mod reclaimable_vec;
mod renderer;
mod surface;
mod yuv_shadow_buffer;

pub mod libyuv;

pub use asset_cache::*;
pub use canvas::*;
pub use dirty_rect::*;
pub use font_renderer::*;
pub use h264::*;
pub use keyframe_cache_file::*;
pub use layout::*;
pub use reclaimable_vec::*;
pub use renderer::*;
pub use surface::*;
pub use yuv_shadow_buffer::*;

mod h264;

#[cfg(feature = "x264")]
mod carplay_helpers;
#[cfg(feature = "x264")]
mod x264;
#[cfg(feature = "x264")]
pub use x264::*;

#[cfg(feature = "openh264")]
mod openh264_encoder;
#[cfg(feature = "openh264")]
pub use openh264_encoder::*;

#[cfg(feature = "prefer_openh264")]
pub type H264FrameBuffer = OpenH264FrameBuffer;
#[cfg(not(feature = "prefer_openh264"))]
pub type H264FrameBuffer = X264FrameBuffer;

// pub type H264FrameBuffer = OpenH264FrameBuffer;
