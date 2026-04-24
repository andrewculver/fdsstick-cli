/// FDS disk data encoder.
/// Converts standard .fds format into packed write data for the FDS Stick device.
///
/// Pipeline: .fds bytes → parse blocks → MFM encode → pack → USB write stream
///
/// Write data uses packed format (4 two-bit values per byte, MSB-first),
/// with pulse values {1, 2, 3} (shifted +1 from read's {0, 1, 2}).
///
/// Each block is encoded as [0x80_marker] + [block_data] + [CRC] using
/// nibble-interleaved MFM encoding. The encoding processes byte pairs (B, B+1)
/// from a circular buffer, outputting hi-nibble of B then lo-nibble of B+1:
///
///   hi(B) bits 7,6,5,4: look-ahead within nibble; bit 4 looks to B's own bit 3
///   lo(B+1) bits 3,2,1,0: look-ahead within nibble; bit 0 looks to B's bit 7
///
/// MFM encoding rules (1 data bit → 1 raw03 value):
///   bit=1              → value 1 (short pulse)
///   bit=0, next_bit=0  → value 2 (medium pulse, clock between zeros)
///   bit=0, next_bit=1  → value 3 (long pulse, no clock)

use crate::decode::calc_crc;

const FDSSIZE: usize = 65500;
const LEAD_IN_PACKED: usize = 6750; // 27,000 values of 2, packed 4-per-byte
const INTER_BLOCK_GAP_PACKED: usize = 224; // 896 values of 2, packed
const PACKET_HEADER: [u8; 3] = [0xC0, 0x00, 0xAB];
const PACKET_DATA_SIZE: usize = 255; // 255 data bytes per output report

struct FdsBlock {
    data: Vec<u8>, // block content including type byte
}

/// Parse FDS side data (65,500 bytes) into a list of blocks.
fn parse_fds_blocks(fds_data: &[u8]) -> Vec<FdsBlock> {
    let mut blocks = Vec::new();
    let mut pos = 0;

    // Block 1: disk info, 56 bytes (type 0x01)
    if pos + 56 > fds_data.len() {
        return blocks;
    }
    blocks.push(FdsBlock {
        data: fds_data[pos..pos + 56].to_vec(),
    });
    pos += 56;

    // Block 2: file count, 2 bytes (type 0x02)
    if pos + 2 > fds_data.len() {
        return blocks;
    }
    blocks.push(FdsBlock {
        data: fds_data[pos..pos + 2].to_vec(),
    });
    let file_count = fds_data[pos + 1] as usize;
    pos += 2;

    // Blocks 3+4 pairs for each file
    for _ in 0..file_count {
        // Block 3: file header, 16 bytes (type 0x03)
        if pos + 16 > fds_data.len() {
            break;
        }
        blocks.push(FdsBlock {
            data: fds_data[pos..pos + 16].to_vec(),
        });
        let size_lo = fds_data[pos + 13] as usize;
        let size_hi = fds_data[pos + 14] as usize;
        let file_size = (size_hi << 8 | size_lo) + 1;
        pos += 16;

        // Block 4: file data, variable size (type 0x04)
        if pos + file_size > fds_data.len() {
            break;
        }
        blocks.push(FdsBlock {
            data: fds_data[pos..pos + file_size].to_vec(),
        });
        pos += file_size;
    }

    blocks
}

/// MFM-encode a block (with 0x80 marker prefix) into raw03 write values {1, 2, 3}.
///
/// Uses nibble-interleaved encoding: for each pair of bytes (B, B+1) in a
/// circular buffer, output hi-nibble(B) then lo-nibble(B+1). Each data bit
/// produces exactly one raw03 value.
///
/// Look-ahead rules:
///   hi-nibble bits 7,6,5: next bit within the nibble
///   hi-nibble bit 4: same byte's bit 3 (crosses to own lo-nibble)
///   lo-nibble bits 3,2,1: next bit within the nibble
///   lo-nibble bit 0: previous byte's bit 7 (crosses back to hi-nibble of B)
fn mfm_encode_block(data: &[u8]) -> Vec<u8> {
    let n = data.len();
    let mut output = Vec::with_capacity(n * 8);

    for pair_idx in 0..n {
        let b = data[pair_idx];
        let b_next = data[(pair_idx + 1) % n];

        // Hi nibble of B: bits 7, 6, 5, 4
        for pos in 0..4u8 {
            let bit_pos = 7 - pos;
            let data_bit = (b >> bit_pos) & 1;
            let next_bit = if pos < 3 {
                (b >> (bit_pos - 1)) & 1
            } else {
                // bit 4: look-ahead to same byte's bit 3
                (b >> 3) & 1
            };
            output.push(mfm_value(data_bit, next_bit));
        }

        // Lo nibble of B+1: bits 3, 2, 1, 0
        for pos in 0..4u8 {
            let bit_pos = 3 - pos;
            let data_bit = (b_next >> bit_pos) & 1;
            let next_bit = if pos < 3 {
                (b_next >> (bit_pos - 1)) & 1
            } else {
                // bit 0: look-ahead to B's bit 7
                (b >> 7) & 1
            };
            output.push(mfm_value(data_bit, next_bit));
        }
    }

    output
}

/// Map a data bit and its look-ahead context to an MFM write value.
#[inline]
fn mfm_value(data_bit: u8, next_bit: u8) -> u8 {
    if data_bit == 1 {
        1
    } else if next_bit == 0 {
        2
    } else {
        3
    }
}

/// Pack raw03 write values (4 per byte, MSB-first).
fn pack_raw03(values: &[u8]) -> Vec<u8> {
    let mut packed = Vec::with_capacity((values.len() + 3) / 4);
    for chunk in values.chunks(4) {
        let mut byte = 0u8;
        for (i, &val) in chunk.iter().enumerate() {
            byte |= val << (6 - i * 2);
        }
        packed.push(byte);
    }
    packed
}

/// Encode one FDS side (65,500 bytes) into packed write data for USB transmission.
pub fn encode_side(fds_data: &[u8]) -> Vec<u8> {
    assert!(
        fds_data.len() == FDSSIZE,
        "FDS side data must be {} bytes",
        FDSSIZE
    );

    let blocks = parse_fds_blocks(fds_data);
    eprintln!("  {} blocks parsed", blocks.len());

    let mut packed: Vec<u8> = Vec::new();

    // Packet header
    packed.extend_from_slice(&PACKET_HEADER);

    // Lead-in gap: 6,750 packed bytes of 0xAA (27,000 values of 2)
    packed.extend(std::iter::repeat_n(0xAAu8, LEAD_IN_PACKED));

    for (i, block) in blocks.iter().enumerate() {
        // Inter-block gap (between blocks, not before first)
        if i > 0 {
            packed.extend(std::iter::repeat_n(0xAAu8, INTER_BLOCK_GAP_PACKED));
        }

        // Compute CRC: append 2 zero bytes, compute, replace with CRC
        let mut block_with_crc = block.data.clone();
        block_with_crc.push(0x00);
        block_with_crc.push(0x00);
        let crc = calc_crc(&block_with_crc, block_with_crc.len());
        let len = block_with_crc.len();
        block_with_crc[len - 2] = (crc & 0xFF) as u8;
        block_with_crc[len - 1] = (crc >> 8) as u8;

        // Prepend 0x80 gap-end marker for nibble-interleaved encoding.
        // The marker byte is part of the circular encoding buffer — its
        // hi-nibble produces the gap-end pattern [1,2,2,2] and its
        // lo-nibble produces the block-end pattern [2,2,2,3].
        let mut marker_and_block = Vec::with_capacity(1 + block_with_crc.len());
        marker_and_block.push(0x80);
        marker_and_block.extend_from_slice(&block_with_crc);

        // MFM-encode (nibble-interleaved) and pack (4 values → 1 byte)
        let encoded_values = mfm_encode_block(&marker_and_block);
        let encoded_packed = pack_raw03(&encoded_values);
        packed.extend_from_slice(&encoded_packed);

        let block_type = block.data[0];
        eprintln!(
            "  Block {} (type {}): {} data bytes → {} MFM values → {} packed bytes",
            i + 1,
            block_type,
            block.data.len(),
            encoded_values.len(),
            encoded_packed.len()
        );
    }

    // Pad to fill a full packet at the end (no extra trailing gap —
    // the device stops accepting data when the disk rotation completes)
    let remainder = packed.len() % PACKET_DATA_SIZE;
    if remainder != 0 {
        packed.extend(std::iter::repeat_n(0xAAu8, PACKET_DATA_SIZE - remainder));
    }

    eprintln!(
        "  Total packed: {} bytes ({} packets of {})",
        packed.len(),
        packed.len() / PACKET_DATA_SIZE,
        PACKET_DATA_SIZE
    );
    packed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pack_raw03() {
        assert_eq!(pack_raw03(&[1, 1, 1, 1]), vec![0x55]);
        assert_eq!(pack_raw03(&[2, 2, 2, 2]), vec![0xAA]);
        assert_eq!(pack_raw03(&[3, 3, 3, 3]), vec![0xFF]);
        // (1<<6)|(2<<4)|(3<<2)|1 = 64+32+12+1 = 0x6D
        assert_eq!(pack_raw03(&[1, 2, 3, 1]), vec![0x6D]);
    }

    #[test]
    fn test_mfm_encode_produces_valid_values() {
        let data = [0x80, 0x01, 0x2A]; // marker + 2 data bytes
        let values = mfm_encode_block(&data);
        for &v in &values {
            assert!(v >= 1 && v <= 3, "invalid write value: {v}");
        }
        // 3 bytes × 8 values = 24 values
        assert_eq!(values.len(), 24);
    }

    #[test]
    fn test_mfm_encode_size() {
        // Each byte should produce exactly 8 MFM values
        let data = vec![0u8; 100];
        let values = mfm_encode_block(&data);
        assert_eq!(values.len(), 800);
    }

    #[test]
    fn test_mfm_encode_gap_end_marker() {
        // 0x80 marker + 0x01 (first byte of disk info block)
        // The hi-nibble of 0x80 (=1000) should produce the gap-end pattern [1,2,2,2]
        let data = [0x80, 0x01];
        let values = mfm_encode_block(&data);
        // First 4 values = hi-nibble of 0x80: bits 7,6,5,4 = 1,0,0,0
        // bit7=1 -> 1
        // bit6=0, next=bit5=0 -> 2
        // bit5=0, next=bit4=0 -> 2
        // bit4=0, next=bit3(same byte 0x80)=0 -> 2
        assert_eq!(&values[0..4], &[1, 2, 2, 2]);
    }

    #[test]
    fn test_mfm_encode_nibble_interleave() {
        // Verify the nibble interleaving pattern for a known byte
        // 0x80, 0x2A
        // Pair 0: hi(0x80)=[1,2,2,2], lo(0x2A)=[1,3,1,2]
        // Pair 1: hi(0x2A)=[2,3,1,3], lo(0x80)=[2,2,2,3]
        let data = [0x80, 0x2A];
        let values = mfm_encode_block(&data);
        assert_eq!(values.len(), 16);
        // hi(0x80): bits 7654=1000, bit4 look-ahead to 0x80's bit3=0
        assert_eq!(&values[0..4], &[1, 2, 2, 2]);
        // lo(0x2A): 0x2A = 0b00101010, lo nibble bits 3,2,1,0 = 1,0,1,0
        // bit3=1->1, bit2=0 next=bit1=1->3, bit1=1->1, bit0=0 next=B[7]=0x80[7]=1->3
        assert_eq!(&values[4..8], &[1, 3, 1, 3]);
    }

    #[test]
    fn test_crc_roundtrip() {
        let block_data = vec![0x01, 0x00, 0x00];
        let mut buf = block_data.clone();
        buf.push(0x00);
        buf.push(0x00);
        let crc = calc_crc(&buf, buf.len());
        let n = buf.len();
        buf[n - 2] = (crc & 0xFF) as u8;
        buf[n - 1] = (crc >> 8) as u8;
        assert_eq!(calc_crc(&buf, buf.len()), 0);
    }

    #[test]
    fn test_parse_fds_blocks_minimal() {
        let mut fds = vec![0u8; FDSSIZE];
        fds[0] = 0x01;
        fds[56] = 0x02;
        fds[57] = 0x00;

        let blocks = parse_fds_blocks(&fds);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].data[0], 0x01);
        assert_eq!(blocks[0].data.len(), 56);
        assert_eq!(blocks[1].data[0], 0x02);
        assert_eq!(blocks[1].data.len(), 2);
    }

    #[test]
    fn test_parse_fds_blocks_with_files() {
        let mut fds = vec![0u8; FDSSIZE];
        fds[0] = 0x01;
        fds[56] = 0x02;
        fds[57] = 0x01; // 1 file

        fds[58] = 0x03;
        fds[71] = 0x03; // size_lo
        fds[72] = 0x00; // size_hi → file_size = 4

        fds[74] = 0x04;

        let blocks = parse_fds_blocks(&fds);
        assert_eq!(blocks.len(), 4);
        assert_eq!(blocks[2].data.len(), 16);
        assert_eq!(blocks[3].data.len(), 4);
    }

    #[test]
    fn test_encode_side_output_size() {
        let mut fds = vec![0u8; FDSSIZE];
        fds[0] = 0x01;
        fds[56] = 0x02;
        fds[57] = 0x00;

        let packed = encode_side(&fds);
        // Should be aligned to PACKET_DATA_SIZE
        assert_eq!(packed.len() % PACKET_DATA_SIZE, 0);
        assert!(packed.len() > 6750, "packed output too small: {}", packed.len());
    }
}
