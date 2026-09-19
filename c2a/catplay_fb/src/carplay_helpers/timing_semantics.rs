#[derive(Debug, PartialEq, Eq)]
pub(super) struct TimingSemantics {
    pub(super) num_units_in_tick: u32,
    pub(super) time_scale: u32,
    pub(super) fixed_frame_rate: u8,
}
