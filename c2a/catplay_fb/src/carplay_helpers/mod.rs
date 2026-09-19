mod bit_reader;
mod bit_writer;
mod nal_rewriter;

use crate::H264FrameBufferError;

use bit_reader::BitReader;
use bit_writer::BitWriter;
pub(crate) use nal_rewriter::CarPlayNalRewriter;

type RewriteResult<T> = Result<T, H264FrameBufferError>;

#[cfg(test)]
mod bitstream_restriction_semantics;
#[cfg(test)]
mod idr_semantics;
#[cfg(test)]
mod pps_extension_semantics;
#[cfg(test)]
mod pps_semantics;
#[cfg(test)]
mod semantics_utils;
#[cfg(test)]
mod sps_semantics;
#[cfg(test)]
mod timing_semantics;
#[cfg(test)]
mod video_signal_semantics;
#[cfg(test)]
mod vui_semantics;

#[cfg(test)]
use idr_semantics::IdrSemantics;
#[cfg(test)]
use pps_semantics::PpsSemantics;
#[cfg(test)]
use sps_semantics::SpsSemantics;

#[cfg(test)]
mod tests;
