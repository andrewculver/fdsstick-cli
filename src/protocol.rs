use crate::device::{DeviceError, FdsStick};
use std::io::{self, Write};
use std::os::unix::io::AsRawFd;

const FILL_BYTE: u8 = 0x63;
const BULK_READS_PER_SIDE: usize = 512;

/// Send a 64-byte command via Report ID 3.
/// Format: [03, 01, 01, 01, cmd, fill × 59]
fn send_command(dev: &FdsStick, cmd: u8, fill: u8) -> Result<(), DeviceError> {
    let mut buf = [fill; 64];
    buf[0] = 0x03;
    buf[1] = 0x01;
    buf[2] = 0x01;
    buf[3] = 0x01;
    buf[4] = cmd;
    if fill == 0x00 {
        buf[5..].fill(0x00);
    }
    dev.set_report(&buf)
}

/// GET_REPORT with Report ID 2 (status, returns 64 bytes).
fn get_status(dev: &FdsStick) -> Result<Vec<u8>, DeviceError> {
    let mut buf = [0u8; 65];
    buf[0] = 0x02;
    let n = dev.get_report(&mut buf)?;
    Ok(buf[1..n].to_vec())
}

/// GET_REPORT with Report ID 33 (device status, returns 2 bytes).
fn get_device_status(dev: &FdsStick) -> Result<Vec<u8>, DeviceError> {
    let mut buf = [0u8; 65];
    buf[0] = 0x21;
    let n = dev.get_report(&mut buf)?;
    Ok(buf[1..n].to_vec())
}

/// Set memory address via Report ID 6.
/// The device physically reads the disk sector at this address (~93ms).
fn set_address(dev: &FdsStick, addr: u16) -> Result<(), DeviceError> {
    let buf = [0x06, (addr & 0xFF) as u8, (addr >> 8) as u8, 0xD8];
    dev.set_report(&buf)
}

/// SET_REPORT #4 with trigger pattern.
fn send_trigger(dev: &FdsStick) -> Result<(), DeviceError> {
    let buf = [0x04, 0x01, 0x01, 0x03, 0x05, FILL_BYTE, FILL_BYTE, FILL_BYTE, FILL_BYTE];
    dev.set_report(&buf)
}

/// SET_REPORT #4 with acknowledge pattern.
fn send_ack(dev: &FdsStick) -> Result<(), DeviceError> {
    let buf = [0x04, 0x00, 0x00, 0x00, 0x05, FILL_BYTE, FILL_BYTE, FILL_BYTE, FILL_BYTE];
    dev.set_report(&buf)
}

/// GET_REPORT with Report ID 1 (returns 2 bytes from address read).
fn read_block_status(dev: &FdsStick) -> Result<Vec<u8>, DeviceError> {
    let mut buf = [0u8; 257];
    buf[0] = 0x01;
    let n = dev.get_report(&mut buf)?;
    Ok(buf[1..n].to_vec())
}

/// GET_REPORT with Report ID 8 (verification, returns 1 byte).
fn verify_read_8(dev: &FdsStick) -> Result<Vec<u8>, DeviceError> {
    let mut buf = [0u8; 514];
    buf[0] = 0x08;
    let n = dev.get_report(&mut buf)?;
    Ok(buf[1..n].to_vec())
}

/// GET_REPORT with Report ID 9 (verification, returns 1 byte).
fn verify_read_9(dev: &FdsStick) -> Result<Vec<u8>, DeviceError> {
    let mut buf = [0u8; 514];
    buf[0] = 0x09;
    let n = dev.get_report(&mut buf)?;
    Ok(buf[1..n].to_vec())
}

/// Start bulk transfer via Report ID 16: [10, 00]
fn start_bulk(dev: &FdsStick) -> Result<(), DeviceError> {
    let buf = [0x10, 0x00];
    dev.set_report(&buf)
}

/// Read one bulk block via GET_REPORT ID 17 (256 bytes).
/// Returns (sequence_number, data_bytes).
/// Response format: [0x11=reportID, sequence, data×254]
fn read_bulk_block(dev: &FdsStick) -> Result<(u8, Vec<u8>), DeviceError> {
    let mut buf = [0u8; 257];
    buf[0] = 0x11;
    let n = dev.get_report(&mut buf)?;
    if n < 2 {
        return Ok((0, Vec::new()));
    }
    let seq = buf[1];
    Ok((seq, buf[2..n].to_vec()))
}

/// Init sequence: 9F command → status → device status → 05 → status → 15 → status
fn init_sequence(dev: &FdsStick, fill: u8) -> Result<(), DeviceError> {
    send_command(dev, 0x9F, fill)?;
    let _ = get_status(dev)?;
    let _ = get_device_status(dev)?;
    send_command(dev, 0x05, 0x00)?;
    let _ = get_status(dev)?;
    send_command(dev, 0x15, 0x00)?;
    let _ = get_status(dev)?;
    Ok(())
}

/// Read the address table (0x01B0 to 0x04E0 step 0x10).
/// Each set_address call triggers a physical disk read (~93ms).
/// Sequence per address: set_address → trigger → read_status → ack
fn read_address_table(dev: &FdsStick) -> Result<(), DeviceError> {
    let start: u16 = 0x01B0;
    let end: u16 = 0x04E0;
    let total = ((end - start) / 0x10 + 1) as usize;
    let mut addr = start;
    let mut i = 0;
    while addr <= end {
        set_address(dev, addr)?;
        send_trigger(dev)?;
        let _ = read_block_status(dev)?;
        send_ack(dev)?;
        addr += 0x10;
        i += 1;
        if i % 10 == 0 {
            eprint!("\r  Reading disk: {}/{} sectors...", i, total);
        }
    }
    eprintln!("\r  Reading disk: {}/{} sectors done.    ", total, total);
    Ok(())
}

/// Read one side of the disk via bulk transfer.
fn read_side(dev: &FdsStick, side_label: &str) -> Result<Vec<u8>, DeviceError> {
    start_bulk(dev)?;

    let mut data = Vec::new();
    let mut expected_seq: u8 = 1;
    for i in 0..BULK_READS_PER_SIDE {
        let (seq, block) = read_bulk_block(dev)?;
        if block.is_empty() {
            eprintln!("\r  {side_label}: end of data at block {i}");
            break;
        }
        // Skip stale responses (wrong sequence on first block)
        if i == 0 && seq != 1 {
            eprintln!("  {side_label}: skipping stale response (seq=0x{seq:02X})");
            expected_seq = seq.wrapping_add(1);
            // Don't include stale data
            continue;
        }
        if seq != expected_seq {
            eprintln!("  {side_label}: sequence mismatch at block {i}: expected 0x{expected_seq:02X}, got 0x{seq:02X}");
        }
        expected_seq = seq.wrapping_add(1);
        data.extend_from_slice(&block);
        if (i + 1) % 50 == 0 {
            eprint!("\r  {side_label}: {:.0} KB read...", data.len() as f64 / 1024.0);
        }
    }
    eprintln!("\r  {side_label}: {} bytes read.      ", data.len());
    Ok(data)
}

/// Wait for user to flip disk. Returns true if they pressed Enter, false if Escape.
fn wait_for_flip() -> bool {
    eprintln!();
    eprint!("Flip disk to side B and press Enter (or Esc to skip)...");
    io::stderr().flush().unwrap();

    let stdin_fd = io::stdin().as_raw_fd();

    // Save and set raw terminal mode
    let mut old_termios: libc::termios = unsafe { std::mem::zeroed() };
    unsafe { libc::tcgetattr(stdin_fd, &mut old_termios) };
    let mut raw = old_termios;
    raw.c_lflag &= !(libc::ICANON | libc::ECHO);
    raw.c_cc[libc::VMIN] = 1;
    raw.c_cc[libc::VTIME] = 0;
    unsafe { libc::tcsetattr(stdin_fd, libc::TCSANOW, &raw) };

    let result = loop {
        let mut buf = [0u8; 1];
        let n = unsafe { libc::read(stdin_fd, buf.as_mut_ptr() as *mut _, 1) };
        if n == 1 {
            match buf[0] {
                0x1B => break false,
                0x0A | 0x0D => break true,
                _ => {}
            }
        }
    };

    // Restore terminal mode
    unsafe { libc::tcsetattr(stdin_fd, libc::TCSANOW, &old_termios) };
    eprintln!();
    result
}

/// Read disk (one or two sides).
/// If `sides` is 1, only side A is read. If 2, prompts for flip (Esc to skip).
pub fn read_disk(dev: &FdsStick, sides: usize) -> Result<(Vec<u8>, Option<Vec<u8>>), DeviceError> {
    // Phase 1: Init without fill
    eprintln!("Initializing device...");
    init_sequence(dev, 0x00)?;

    // Phase 2: Init with fill
    eprintln!("Initializing with fill pattern...");
    init_sequence(dev, FILL_BYTE)?;

    // Phase 3: Verification reads
    eprintln!("Verifying...");
    let _ = verify_read_8(dev)?;
    let _ = verify_read_9(dev)?;

    // Phase 4: Read disk sectors via address table
    eprintln!("Reading disk sectors...");
    read_address_table(dev)?;

    // Phase 5: Re-init with fill before bulk download
    eprintln!("Preparing for download...");
    init_sequence(dev, FILL_BYTE)?;

    // Phase 6: Bulk download side A
    eprintln!("Downloading side A...");
    let side_a = read_side(dev, "Side A")?;

    // Phase 7: Side B (if requested)
    if sides >= 2 {
        if wait_for_flip() {
            eprintln!("Downloading side B...");
            let side_b = read_side(dev, "Side B")?;
            return Ok((side_a, Some(side_b)));
        }
        eprintln!("Skipping side B.");
    }

    Ok((side_a, None))
}

/// Start bulk write via Report ID 16: [10, 01]
fn start_bulk_write(dev: &FdsStick) -> Result<(), DeviceError> {
    let buf = [0x10, 0x01];
    dev.set_report(&buf)
}

/// Write one bulk block via Output Report ID 18 (256 bytes).
/// Format: [0x12, data×255]
/// Uses Output report type (not Feature), matching the device's HID descriptor.
fn write_bulk_block(dev: &FdsStick, data: &[u8]) -> Result<(), DeviceError> {
    let mut buf = [0u8; 256];
    buf[0] = 0x12;
    let len = data.len().min(255);
    buf[1..1 + len].copy_from_slice(&data[..len]);
    dev.write_output(&buf)
}

/// Finalize write via Report ID 32: [20, 00]
fn finalize_write(dev: &FdsStick) -> Result<(), DeviceError> {
    let buf = [0x20, 0x00];
    dev.set_report(&buf)
}

/// Minimum packets before a write error is treated as "disk full" (not a real error).
const MIN_PACKETS_FOR_FULL_DISK: usize = 400;

/// Write one side of the disk via bulk transfer.
fn write_side(dev: &FdsStick, packed_data: &[u8], side_label: &str) -> Result<(), DeviceError> {
    eprintln!("  {side_label}: starting bulk write...");
    start_bulk_write(dev)?;
    eprintln!("  {side_label}: bulk write started.");

    let chunk_size = 255; // 255 data bytes per 256-byte output report
    let total_packets = (packed_data.len() + chunk_size - 1) / chunk_size;

    for (i, chunk) in packed_data.chunks(chunk_size).enumerate() {
        match write_bulk_block(dev, chunk) {
            Ok(()) => {}
            Err(e) => {
                if i >= MIN_PACKETS_FOR_FULL_DISK {
                    // Disk rotation complete — device stopped accepting data
                    eprintln!(
                        "\r  {side_label}: disk full after {} packets (expected ~{}).      ",
                        i, total_packets
                    );
                    return Ok(());
                } else {
                    // Real error — too few packets sent
                    return Err(e);
                }
            }
        }
        if (i + 1) % 50 == 0 {
            eprint!("\r  {side_label}: {}/{} packets sent...", i + 1, total_packets);
        }
    }
    eprintln!("\r  {side_label}: {total_packets} packets sent.      ");
    Ok(())
}

/// Write disk image to FDS disk (one or two sides).
pub fn write_disk(
    dev: &FdsStick,
    side_a: &[u8],
    side_b: Option<&[u8]>,
) -> Result<(), DeviceError> {
    // Phase 1: Init without fill
    eprintln!("Initializing device...");
    init_sequence(dev, 0x00)?;

    // Phase 2: Init with fill
    eprintln!("Initializing with fill pattern...");
    init_sequence(dev, FILL_BYTE)?;

    // Phase 3: Verification reads
    eprintln!("Verifying...");
    let _ = verify_read_8(dev)?;
    let _ = verify_read_9(dev)?;

    // Phase 4: Address table scan
    eprintln!("Reading disk sectors...");
    read_address_table(dev)?;

    // Phase 5: Re-init with fill
    eprintln!("Preparing for write...");
    init_sequence(dev, FILL_BYTE)?;

    // Phase 6: Write side A
    eprintln!("Writing side A...");
    write_side(dev, side_a, "Side A")?;

    // Phase 7: Side B (if present)
    if let Some(b_data) = side_b {
        eprintln!();
        eprint!("Flip disk to side B and press Enter...");
        io::stderr().flush().unwrap();
        let mut input = String::new();
        io::stdin().read_line(&mut input).expect("Failed to read from stdin");

        eprintln!("Writing side B...");
        write_side(dev, b_data, "Side B")?;
    }

    // Phase 8: Finalize
    eprintln!("Finalizing...");
    finalize_write(dev)?;

    Ok(())
}

/// Run diagnostics to check device communication and data format.
pub fn run_diagnostics(dev: &FdsStick) -> Result<(), DeviceError> {
    fn hex_line(data: &[u8], max: usize) -> String {
        data.iter()
            .take(max)
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    // Test 1: Check get_report buffer behavior
    eprintln!("=== Test 1: GET_REPORT buffer behavior ===");
    {
        // Fill buffer with 0xDD sentinel to see which bytes get overwritten
        let mut buf = [0xDDu8; 257];
        buf[0] = 0x02; // Report ID 2 (status)
        let n = dev.get_report(&mut buf)?;
        eprintln!("  Report 2: n={n}");
        eprintln!("  buf[0..8]: {}", hex_line(&buf[..8], 8));
        eprintln!("  buf[0] (report ID or data?): 0x{:02X}", buf[0]);
        if buf[0] == 0x02 {
            eprintln!("  -> buf[0] preserved as report ID");
        } else {
            eprintln!("  -> buf[0] overwritten with data (0x{:02X})", buf[0]);
        }
        // Check how many bytes were written
        let last_dd = buf.iter().rposition(|&b| b != 0xDD).unwrap_or(0);
        eprintln!("  Last non-sentinel byte at index {last_dd}");
    }

    // Test 2: Init + one bulk read to check data format
    eprintln!("\n=== Test 2: Init sequence ===");
    {
        send_command(dev, 0x9F, 0x00)?;
        let status = get_status(dev)?;
        eprintln!("  Status after 9F: {} bytes: {}", status.len(), hex_line(&status, 16));
        let dev_status = get_device_status(dev)?;
        eprintln!("  Device status: {} bytes: {}", dev_status.len(), hex_line(&dev_status, 16));
        send_command(dev, 0x05, 0x00)?;
        let status2 = get_status(dev)?;
        eprintln!("  Status after 05: {} bytes: {}", status2.len(), hex_line(&status2, 16));
        send_command(dev, 0x15, 0x00)?;
        let status3 = get_status(dev)?;
        eprintln!("  Status after 15: {} bytes: {}", status3.len(), hex_line(&status3, 16));
    }

    // Test 3: Init with fill + verify + address scan (first 3 addresses)
    eprintln!("\n=== Test 3: Init with fill + partial address scan ===");
    {
        send_command(dev, 0x9F, FILL_BYTE)?;
        let _ = get_status(dev)?;
        let _ = get_device_status(dev)?;
        send_command(dev, 0x05, 0x00)?;
        let _ = get_status(dev)?;
        send_command(dev, 0x15, 0x00)?;
        let _ = get_status(dev)?;

        let v8 = verify_read_8(dev)?;
        eprintln!("  Verify read 8: {} bytes: {}", v8.len(), hex_line(&v8, 16));
        let v9 = verify_read_9(dev)?;
        eprintln!("  Verify read 9: {} bytes: {}", v9.len(), hex_line(&v9, 16));

        // Read first 3 address table entries
        for addr in (0x01B0..=0x01D0).step_by(0x10) {
            set_address(dev, addr)?;
            send_trigger(dev)?;
            let block = read_block_status(dev)?;
            eprintln!("  Addr 0x{addr:04X}: {} bytes: {}", block.len(), hex_line(&block, 16));
            send_ack(dev)?;
        }
    }

    // Test 4: Re-init + bulk read (first 5 blocks, showing raw buffer)
    eprintln!("\n=== Test 4: Bulk read (raw buffer analysis) ===");
    {
        send_command(dev, 0x9F, FILL_BYTE)?;
        let _ = get_status(dev)?;
        let _ = get_device_status(dev)?;
        send_command(dev, 0x05, 0x00)?;
        let _ = get_status(dev)?;
        send_command(dev, 0x15, 0x00)?;
        let _ = get_status(dev)?;

        start_bulk(dev)?;
        eprintln!("  Bulk started");

        for i in 0..5 {
            let mut buf = [0xDDu8; 257];
            buf[0] = 0x11;
            let n = dev.get_report(&mut buf)?;
            eprintln!("  Block {i}: n={n}, buf[0..16]: {}", hex_line(&buf[..16], 16));
            eprintln!("           buf[0]=0x{:02X} buf[1]=0x{:02X} buf[2]=0x{:02X}",
                      buf[0], buf[1], buf[2]);
            // Find last non-DD byte
            let last = buf.iter().rposition(|&b| b != 0xDD).unwrap_or(0);
            eprintln!("           last non-sentinel at index {last}");
            // Count non-zero in data area
            let data_start = if buf[0] == 0x11 { 2 } else { 1 };
            let nz = buf[data_start..n.min(257)].iter().filter(|&&b| b != 0).count();
            eprintln!("           non-zero data bytes: {nz}/{}", n.saturating_sub(data_start));
        }
    }

    eprintln!("\n=== Diagnostics complete ===");
    Ok(())
}
