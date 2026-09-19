use log::debug;

use crate::video::NalType;

const START_CODE: [u8; 4] = [0, 0, 0, 1];

#[derive(Debug, PartialEq, Clone)]
pub struct AvccConfig {
    pub nal_size_len: usize,
    pub sps_pps: Vec<u8>,
}

fn find_annexb_start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut pos = from;
    while pos + 3 <= data.len() {
        if data[pos..].starts_with(&[0, 0, 0, 1]) {
            return Some((pos, 4));
        }
        if data[pos..].starts_with(&[0, 0, 1]) {
            return Some((pos, 3));
        }
        pos += 1;
    }
    None
}

pub fn avcc_config_deserialize(data: &[u8]) -> Option<AvccConfig> {
    if data.len() < 6 || data[0] != 1 {
        return None;
    }

    let nal_size_len = ((data[4] & 0x03) + 1) as usize;

    let mut pos = 5;
    let sps_count = (data[pos] & 0x1f) as usize;
    pos += 1;

    let mut header = Vec::new();

    // SPS
    for _ in 0..sps_count {
        if pos + 2 > data.len() {
            return None;
        }
        let len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2;
        if pos + len > data.len() {
            return None;
        }
        header.extend_from_slice(&START_CODE);
        header.extend_from_slice(&data[pos..pos + len]);
        pos += len;
    }

    // PPS
    if pos >= data.len() {
        return None;
    }
    let pps_count = data[pos] as usize;
    pos += 1;

    for _ in 0..pps_count {
        if pos + 2 > data.len() {
            return None;
        }
        let len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2;
        if pos + len > data.len() {
            return None;
        }
        header.extend_from_slice(&START_CODE);
        header.extend_from_slice(&data[pos..pos + len]);
        pos += len;
    }

    debug!(
        "Parsed AVCC header: {:?}",
        AvccConfig {
            nal_size_len,
            sps_pps: header.clone(),
            // next_nal_offset: pos,
        }
    );

    Some(AvccConfig {
        nal_size_len,
        sps_pps: header,
        // next_nal_offset: pos,
    })
}

pub fn avcc_config_serialize(avcc: &AvccConfig) -> Option<Vec<u8>> {
    let mut sps_list = Vec::new();
    let mut pps_list = Vec::new();

    let mut pos = 0;
    while let Some((prefix_start, prefix_len)) = find_annexb_start_code(&avcc.sps_pps, pos) {
        let start = prefix_start + prefix_len;
        let end = find_annexb_start_code(&avcc.sps_pps, start)
            .map(|(next_start, _)| next_start)
            .unwrap_or(avcc.sps_pps.len());
        let nal = &avcc.sps_pps[start..end];
        if !nal.is_empty() {
            match NalType::from_byte(nal[0]) {
                NalType::Sps => sps_list.push(nal.to_vec()),
                NalType::Pps => pps_list.push(nal.to_vec()),
                _ => {}
            }
        }
        pos = end;
    }

    if sps_list.is_empty() {
        return None;
    }

    let sps = &sps_list[0];
    if sps.len() < 4 {
        return None;
    }

    let profile = sps[1];
    let compat = sps[2];
    let level = sps[3];

    let mut out = vec![
        1,                                      // configurationVersion
        profile,                                // AVCProfileIndication
        compat,                                 // profile_compatibility
        level,                                  // AVCLevelIndication
        0xFC | ((avcc.nal_size_len - 1) as u8), // reserved(111111) + lengthSizeMinusOne
        0xE0 | (sps_list.len() as u8),          // reserved(111) + numOfSPS
    ];

    for sps in &sps_list {
        out.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        out.extend_from_slice(sps);
    }

    out.push(pps_list.len() as u8);
    for pps in &pps_list {
        out.extend_from_slice(&(pps.len() as u16).to_be_bytes());
        out.extend_from_slice(pps);
    }

    Some(out)
}

#[cfg(test)]
mod tests {

    use crate::video::{AvccConfig, avcc_config_deserialize, avcc_config_serialize};

    #[test]
    fn test_avcc_config_annexb_roundtrip() {
        let avcc: Vec<u8> = vec![
            0x01, // version
            0x42, // profile (Baseline)
            0xE0, // compatibility
            0x1E, // level_idc
            0xFF, // reserved + nal_size_len (4 bytes)
            0xE1, // reserved + numOfSPS=1
            0x00, 0x06, // SPS length
            0x67, 0x42, 0xE0, 0x1E, 0x89, 0x8B, // SPS payload
            0x01, // numOfPPS
            0x00, 0x02, // PPS length
            0x68, 0xCE, // PPS payload
        ];

        let header1 = avcc_config_deserialize(&avcc).expect("avcc->annexb failed");
        println!("{header1:?}");

        let avcc2 = avcc_config_serialize(&header1).expect("annexb->avcc failed");

        let header2 = avcc_config_deserialize(&avcc2).expect("avcc2->annexb failed");
        println!("{header2:?}");

        assert_eq!(header1.nal_size_len, header2.nal_size_len);
        assert_eq!(header1.sps_pps, header2.sps_pps);
        assert_eq!(header1, header2);
        assert_eq!(
            header1,
            AvccConfig {
                nal_size_len: 4,
                sps_pps: vec![
                    0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0xE0, 0x1E, 0x89, 0x8B, 0x00, 0x00, 0x00, 0x01, 0x68, 0xCE,
                ],
            }
        );
    }

    #[test]
    fn serialize_ignores_three_byte_prefixed_sei_after_pps() {
        let config = AvccConfig {
            nal_size_len: 4,
            sps_pps: vec![
                0, 0, 0, 1, 0x67, 0x64, 0x00, 0x1f, // SPS
                0, 0, 0, 1, 0x68, 0xe9, 0x20, 0xfc, 0xb0, // PPS
                0, 0, 1, 0x06, 0x05, 0xff, // SEI
            ],
        };

        let serialized = avcc_config_serialize(&config).expect("annexb->avcc failed");
        let decoded = avcc_config_deserialize(&serialized).expect("avcc->annexb failed");

        assert_eq!(
            decoded.sps_pps,
            vec![
                0, 0, 0, 1, 0x67, 0x64, 0x00, 0x1f, // SPS
                0, 0, 0, 1, 0x68, 0xe9, 0x20, 0xfc, 0xb0, // PPS
            ]
        );
    }
}
