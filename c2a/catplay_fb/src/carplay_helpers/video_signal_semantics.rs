#[derive(Debug, PartialEq, Eq)]
pub(super) struct VideoSignalSemantics {
    pub(super) video_format: u32,
    pub(super) full_range: u8,
    pub(super) colour_description: Option<[u32; 3]>,
}
