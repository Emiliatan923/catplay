#[derive(Debug, PartialEq, Eq)]
pub(super) struct BitstreamRestrictionSemantics {
    pub(super) motion_vectors_over_pic_boundaries: u8,
    pub(super) max_bytes_per_pic_denom: u32,
    pub(super) max_bits_per_mb_denom: u32,
    pub(super) log2_max_mv_length_horizontal: u32,
    pub(super) log2_max_mv_length_vertical: u32,
    pub(super) num_reorder_frames: u32,
    pub(super) max_dec_frame_buffering: u32,
}
