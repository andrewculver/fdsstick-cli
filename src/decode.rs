/// FDS disk data decoder.
/// Converts packed raw03 pulse data from the FDS Stick into standard .fds format.
///
/// The FDS Stick device returns raw pulse widths packed 4 per byte:
///   bits 0-1: pulse width 0 (first in time)
///   bits 2-3: pulse width 1
///   bits 4-5: pulse width 2
///   bits 6-7: pulse width 3 (last in time)
/// Valid pulse widths: 0 (short), 1 (medium), 2 (long). Value 3 = invalid.
///
/// Pipeline: packed bytes → unpacked raw03 → decoded FDS data
///
/// Based on the holodnak/FDSStick reference implementation (fds.cpp).

const FDSSIZE: usize = 65500;
const MIN_GAP_SIZE: usize = 0x300; // minimum gap size in raw03 values

/// Unpack raw data from the FDS Stick: 4 pulse widths per byte → 1 per byte.
/// MSB-first: bits 6-7 are the first pulse width, bits 0-1 are last.
fn unpack_raw03(packed: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(packed.len() * 4);
    for &b in packed {
        raw.push((b >> 6) & 0x03); // bits 6-7 (first)
        raw.push((b >> 4) & 0x03); // bits 4-5
        raw.push((b >> 2) & 0x03); // bits 2-3
        raw.push(b & 0x03);        // bits 0-1 (last)
    }
    raw
}

/// Calculate FDS CRC-16.
/// Polynomial: 0x10810, initial value: 0x8000.
pub(crate) fn calc_crc(buf: &[u8], size: usize) -> u16 {
    let mut crc: u32 = 0x8000;
    for i in 0..size {
        crc |= (buf[i] as u32) << 16;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc ^= 0x10810;
            }
            crc >>= 1;
        }
    }
    crc as u16
}

/// Find the start of block 1 by matching a known bit pattern.
/// Returns the offset in the raw03 data, or None if not found.
fn find_first_block(raw: &[u8]) -> Option<usize> {
    let pattern: [u8; 32] = [
        1, 0, 1, 0, 0, 0, 0, 0, // gap end + first bit of 0x01
        0, 1, 2, 2, 1, 0, 1, 0, // 0x2A ('*')
        0, 1, 1, 2, 1, 1, 1, 1, // 0x4E ('N')
        1, 1, 0, 0, 1, 1, 1, 0, // 0x49 ('I')
    ];

    let search_limit = (0x2000 * 8).min(raw.len());
    let mut match_len = 0;
    for i in 0..search_limit {
        if raw[i] == pattern[match_len] {
            if match_len == pattern.len() - 1 {
                return Some(i - match_len);
            }
            match_len += 1;
        } else {
            let old_len = match_len;
            match_len = 0;
            if old_len > 0 && raw[i] == pattern[0] {
                match_len = 1;
            }
        }
    }
    None
}

/// Decode a single FDS block from raw03 pulse data.
fn block_decode(
    dst: &mut [u8],
    src: &[u8],
    in_pos: &mut usize,
    out_pos: &mut usize,
    src_size: usize,
    dst_size: usize,
    block_size: usize,
    block_type: u8,
) -> bool {
    if *out_pos + block_size + 2 > dst_size {
        return false;
    }

    let out_end = (*out_pos + block_size + 2) * 8;
    let mut out = *out_pos * 8;
    let mut inp = *in_pos;

    // Scan for gap end: a '1' preceded by at least MIN_GAP_SIZE zeros
    let mut zeros: usize = 0;
    loop {
        if inp >= src_size.saturating_sub(2) {
            return false;
        }
        if src[inp] == 1 && zeros >= MIN_GAP_SIZE {
            break;
        }
        if src[inp] == 0 {
            zeros += 1;
        } else {
            zeros = 0;
        }
        inp += 1;
    }

    let mut bitval: u8 = 1;
    inp += 1;

    loop {
        if inp >= src_size {
            return false;
        }
        let key = src[inp] | (bitval << 4);
        match key {
            0x11 => {
                out += 1;
                out += 1;
                bitval = 0;
            }
            0x00 => {
                out += 1;
                bitval = 0;
            }
            0x12 => {
                out += 1;
                dst[out / 8] |= 1 << (out & 7);
                out += 1;
                bitval = 1;
            }
            0x01 | 0x10 => {
                dst[out / 8] |= 1 << (out & 7);
                out += 1;
                bitval = 1;
            }
            _ => {
                out += 1;
                bitval = 0;
            }
        }
        inp += 1;
        if out >= out_end {
            break;
        }
    }

    if dst[*out_pos] != block_type {
        eprintln!(
            "  decode: wrong block type at 0x{:X}: found {}, expected {}",
            *out_pos, dst[*out_pos], block_type
        );
        return false;
    }

    let out_byte = out / 8 - 2;

    // CRC check
    if calc_crc(&dst[*out_pos..], block_size + 2) != 0 {
        let crc1 = (dst[out_byte + 1] as u16) << 8 | dst[out_byte] as u16;
        dst[out_byte] = 0;
        dst[out_byte + 1] = 0;
        let crc2 = calc_crc(&dst[*out_pos..], block_size + 2);
        eprintln!("  decode: bad CRC ({crc1:04X} != {crc2:04X})");
    }

    // Clear CRC bytes
    dst[out_byte] = 0;
    dst[out_byte + 1] = 0;
    if out_byte + 2 < dst.len() {
        dst[out_byte + 2] = 0;
    }

    *in_pos = inp;
    *out_pos = out_byte;
    true
}

/// Decode raw03 pulse data to standard .fds format (65500 bytes).
fn raw03_to_fds(raw: &[u8]) -> Option<Vec<u8>> {
    let mut fds = vec![0u8; FDSSIZE + 16];
    let raw_size = raw.len();

    let block_start = find_first_block(raw)?;
    let mut inp = block_start.saturating_sub(MIN_GAP_SIZE);
    let mut out: usize = 0;

    // Block 1: disk info (0x38 = 56 bytes)
    if !block_decode(&mut fds, raw, &mut inp, &mut out, raw_size, FDSSIZE + 2, 0x38, 1) {
        eprintln!("  decode: failed to decode block 1");
        return None;
    }
    eprintln!("  Block 1, 0000-{:04X}: disk info", out - 1);

    // Block 2: file count (2 bytes)
    if !block_decode(&mut fds, raw, &mut inp, &mut out, raw_size, FDSSIZE + 2, 2, 2) {
        eprintln!("  decode: failed to decode block 2");
        return None;
    }
    let file_count = fds[0x39];
    eprintln!("  Block 2, 0038-{:04X}: {} files", out - 1, file_count);

    // Blocks 3+4: file headers + data
    let mut file_num = 0;
    loop {
        let block3_start = out;
        if !block_decode(&mut fds, raw, &mut inp, &mut out, raw_size, FDSSIZE + 2, 16, 3) {
            break;
        }

        let size_lo = fds[out - 16 + 13] as usize;
        let size_hi = fds[out - 16 + 14] as usize;
        let file_size = (size_hi << 8 | size_lo) + 1;
        let file_type = match fds[out - 16 + 15] {
            0 => "PRG",
            1 => "CHR",
            2 => "NT",
            _ => "???",
        };
        let load_addr = fds[out - 16 + 11] as u16 | ((fds[out - 16 + 12] as u16) << 8);

        eprintln!(
            "  Block 3, {:04X}-{:04X}: File {}, {} @ {:04X}({:X})",
            block3_start,
            out - 1,
            file_num,
            file_type,
            load_addr,
            file_size
        );

        let block4_start = out;
        if !block_decode(&mut fds, raw, &mut inp, &mut out, raw_size, FDSSIZE + 2, file_size, 4) {
            eprintln!("  decode: failed to decode file {} data", file_num);
            break;
        }
        eprintln!("  Block 4, {:04X}-{:04X}: data", block4_start, out - 1);

        file_num += 1;
    }

    eprintln!("  {} files read", file_num);
    fds.truncate(FDSSIZE);
    Some(fds)
}

/// Decode packed raw03 data from the FDS Stick into standard .fds format.
/// Takes packed data from one side of the disk.
/// Returns 65,500 bytes of decoded FDS data, or None on failure.
pub fn decode_side(packed_data: &[u8]) -> Option<Vec<u8>> {
    eprintln!("  Packed data: {} bytes", packed_data.len());

    // Step 1: Unpack (4 pulse widths per byte → 1 per byte)
    let raw = unpack_raw03(packed_data);
    eprintln!("  Unpacked: {} raw03 values", raw.len());

    // Count distribution
    let mut counts = [0usize; 4];
    for &b in &raw {
        counts[b as usize] += 1;
    }
    eprintln!(
        "  Pulse widths: 0={} 1={} 2={} 3(glitch)={}",
        counts[0], counts[1], counts[2], counts[3]
    );

    // Step 2: Find first block
    match find_first_block(&raw) {
        Some(pos) => eprintln!("  First block at raw03 offset 0x{:X}", pos),
        None => {
            eprintln!("  ERROR: Could not find block 1 pattern!");
            return None;
        }
    }

    // Step 3: Decode
    raw03_to_fds(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unpack_raw03() {
        // MSB-first: bits 6-7 first, 0-1 last
        // 0x55 = 01|01|01|01 → [1,1,1,1]
        assert_eq!(unpack_raw03(&[0x55]), vec![1, 1, 1, 1]);
        // 0xAA = 10|10|10|10 → [2,2,2,2]
        assert_eq!(unpack_raw03(&[0xAA]), vec![2, 2, 2, 2]);
        // 0x00 → [0,0,0,0]
        assert_eq!(unpack_raw03(&[0x00]), vec![0, 0, 0, 0]);
        // 0x1B = 00|01|10|11 → [0,1,2,3]
        assert_eq!(unpack_raw03(&[0x1B]), vec![0, 1, 2, 3]);
    }

    #[test]
    fn test_calc_crc() {
        let data = [0u8; 4];
        let crc = calc_crc(&data, 4);
        assert_ne!(crc, 0);
    }
}
