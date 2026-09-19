#[derive(Debug, PartialEq, Eq)]
pub(super) struct PpsExtensionSemantics {
    pub(super) transform_8x8_mode: u8,
    pub(super) second_chroma_qp_index_offset: i32,
}
