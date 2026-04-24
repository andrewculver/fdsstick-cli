# FDS Stick Read Protocol

## Reading

The FDSStick returns raw pulse timing data packed 4 values per byte. The tool:

1. Communicates with the device via USB HID feature reports.
2. Downloads raw data via bulk transfer (~470+ blocks per side).
3. Unpacks the packed pulse widths (2-bit values, 4 per byte).
4. Decodes the pulse stream into FDS block data using MFM-style decoding.
5. Validates each block's CRC and assembles the standard `.fds` output.

## Background

Reverse-engineered from USB packet capture (`capture.json`, 2,470 packets, ~65 seconds) of the Windows tool (`fdsstick_20160926.exe`) reading a disk, then verified by building a working Rust implementation.

Device: FDS Stick (VID `0x16D0`, PID `0x0AAA`)

## Device Identity

| Field | Value |
|-------|-------|
| VID | `0x16D0` (MCS Electronics / GD Technik) |
| PID | `0x0AAA` |
| USB Version | 1.1 |
| Device Class | HID (Human Interface Device) |
| Firmware | v3.12 (`bcdDevice 0x0312`) |
| Max Packet Size | 64 bytes |
| Power | Bus-powered, 120mA max |
| Endpoints | EP0 (control) + EP1 IN (interrupt, 64B, 10ms interval) |

## Communication Method

All communication uses **HID Feature Reports** via USB control transfers on endpoint 0. No bulk or interrupt data transfers are used.

- **SET_REPORT** (`bRequest 0x09`, `bmRequestType 0x21`) = host → device
- **GET_REPORT** (`bRequest 0x01`, `bmRequestType 0xa1`) = device → host

On macOS (HIDAPI), `get_feature_report` preserves the report ID in `buf[0]` and includes it in the return count. The actual data starts at `buf[1]`.

## Protocol Sequence

### Phase 1: Initialization (No Fill)

```
SET Report 3:  [03, 01, 01, 01, 9F, 00×59]   → command 0x9F (prepare, no fill)
GET Report 2:  [64 bytes]                       → status
GET Report 33: [2 bytes]                        → device status
SET Report 3:  [03, 01, 01, 01, 05, 00×59]    → command 0x05 (read mode)
GET Report 2:  [64 bytes]                       → status
SET Report 3:  [03, 01, 01, 01, 15, 00×59]    → command 0x15 (confirm)
GET Report 2:  [64 bytes]                       → status
```

### Phase 2: Initialization (With Fill)

Same sequence repeated with fill byte `0x63`:

```
SET Report 3:  [03, 01, 01, 01, 9F, 63×59]    → command 0x9F (prepare, with fill)
GET Report 2:  [64 bytes]                       → status
GET Report 33: [2 bytes]                        → device status
SET Report 3:  [03, 01, 01, 01, 05, 00×59]    → command 0x05
GET Report 2:  [64 bytes]                       → status
SET Report 3:  [03, 01, 01, 01, 15, 00×59]    → command 0x15
GET Report 2:  [64 bytes]                       → status
```

### Phase 3: Verification Reads

```
GET Report 8:  [request 513 bytes, returns 1 byte]
GET Report 9:  [request 513 bytes, returns 1 byte]
```

Purpose unclear — possibly checking drive/disk readiness.

### Phase 4: Address Table Scan

Reads the device's internal SRAM directory. Scans addresses `0x01B0` to `0x04E0` in steps of `0x10` (52 iterations total). Each `set_address` call triggers a physical disk sector read (~93ms).

```
For each address:
  SET Report 6:  [06, addr_lo, addr_hi, D8]                     → set address
  SET Report 4:  [04, 01, 01, 03, 05, 63, 63, 63, 63]          → trigger read
  GET Report 1:  [256 bytes]                                     → block status
  SET Report 4:  [04, 00, 00, 00, 05, 63, 63, 63, 63]          → acknowledge
```

Address range: `0x01B0`, `0x01C0`, `0x01D0`, ..., `0x04E0`

### Phase 5: Re-Init (With Fill)

Repeat of Phase 2 — re-initializes with fill pattern before bulk download:

```
SET Report 3:  [03, 01, 01, 01, 9F, 63×59]
GET Report 2:  status
GET Report 33: device status
SET Report 3:  [03, 01, 01, 01, 05, 00×59]
GET Report 2:  status
SET Report 3:  [03, 01, 01, 01, 15, 00×59]
GET Report 2:  status
```

### Phase 6: Bulk Download (Side A)

```
SET Report 16: [10, 00]                         → start bulk read
GET Report 17: [256 bytes] × ~495-496           → raw disk data
```

Each Report 17 response contains:
```
Byte 0:    0x11 (report ID, preserved by macOS HIDAPI)
Byte 1:    Sequence number (1, 2, 3, ..., wraps at 0xFF)
Bytes 2-255: 254 bytes of packed raw03 data
```

Total raw data per side: ~125,000-126,000 bytes (~495 × 254).

### Phase 7: Flip Disk

The tool prompts the user to flip the disk to side B and press Enter. No USB traffic during this pause.

### Phase 8: Bulk Download (Side B)

No re-initialization needed — just start another bulk transfer:

```
SET Report 16: [10, 00]                         → start bulk read
GET Report 17: [256 bytes] × ~495-496           → raw disk data
```

**Note:** The first response after starting side B may be a stale response left over from side A (wrong sequence number). This should be detected and discarded.

## Report ID Reference

| Report ID | Hex | Direction | Size | Purpose |
|-----------|-----|-----------|------|---------|
| 0 | 0x00 | SET_IDLE | 0 B | HID initialization |
| 1 | 0x01 | GET | 256 B | Block status (address scan response) |
| 2 | 0x02 | GET | 64 B | Status |
| 3 | 0x03 | SET | 64 B | Command (0x9F, 0x05, 0x15) |
| 4 | 0x04 | SET | 9 B | Trigger / Acknowledge |
| 6 | 0x06 | SET | 4 B | Set memory address |
| 8 | 0x08 | GET | 513 B | Verification read |
| 9 | 0x09 | GET | 513 B | Verification read |
| 16 | 0x10 | SET | 2 B | Start bulk transfer (`00`=read) |
| 17 | 0x11 | GET | 256 B | Bulk data read |
| 33 | 0x21 | GET | 2 B | Device status |

## Report Formats

### Report 3 — Command (SET, 64 bytes)

```
[03, 01, 01, 01, cmd, fill×59]
```

| Command | Fill | Purpose |
|---------|------|---------|
| 0x9F | 0x00 | Prepare (no fill) |
| 0x9F | 0x63 | Prepare (with fill pattern) |
| 0x05 | 0x00 | Read mode |
| 0x15 | 0x00 | Confirm |

### Report 6 — Set Address (SET, 4 bytes)

```
[06, addr_lo, addr_hi, D8]
```

Little-endian address. Range: `0x01B0`–`0x04E0`, step `0x10`.

### Report 4 — Trigger (SET, 9 bytes)

```
Trigger: [04, 01, 01, 03, 05, 63, 63, 63, 63]
Ack:     [04, 00, 00, 00, 05, 63, 63, 63, 63]
```

### Report 16 — Start Bulk (SET, 2 bytes)

```
[10, 00]
```

### Report 17 — Bulk Data (GET, 256 bytes)

```
Byte 0:      0x11 (report ID)
Byte 1:      Sequence number (1-255, wrapping)
Bytes 2-255: 254 bytes of packed raw03 data
```

## Raw Data Format

### Packed Raw03

The device returns pulse timing data packed 4 values per byte, MSB-first:

```
byte = (val0 << 6) | (val1 << 4) | (val2 << 2) | val3
```

Where `val0` (bits 6-7) is first in time, `val3` (bits 0-1) is last.

### Pulse Width Values

| Value | Meaning |
|-------|---------|
| 0 | Short pulse |
| 1 | Medium pulse |
| 2 | Long pulse |
| 3 | Invalid / glitch (never appears in good data) |

Expected distribution for a healthy disk read: ~74% zeros, ~19% ones, ~7% twos, ~0% threes.

### Unpacking Example

```
0x55 = 01|01|01|01 → [1, 1, 1, 1]
0xAA = 10|10|10|10 → [2, 2, 2, 2]
0x1B = 00|01|10|11 → [0, 1, 2, 3]
```

## Decoding Pipeline

```
Packed bytes → Unpacked raw03 → Decoded FDS data
```

### Step 1: Unpack

Expand each byte to 4 pulse width values (see above).

### Step 2: Find Block 1

Scan the raw03 stream for the block 1 start pattern — a `1` bit preceded by a gap of at least `0x300` zeros, followed by the MFM-encoded bytes `0x01`, `0x2A` ('*'), `0x4E` ('N'), `0x49` ('I') — the start of the "*NINTENDO-HVC*" disk header.

The 32-value pattern to match:

```
1, 0, 1, 0, 0, 0, 0, 0,   ← gap end + first bit of 0x01
0, 1, 2, 2, 1, 0, 1, 0,   ← 0x2A ('*')
0, 1, 1, 2, 1, 1, 1, 1,   ← 0x4E ('N')
1, 1, 0, 0, 1, 1, 1, 0,   ← 0x49 ('I')
```

### Step 3: Decode Blocks

MFM-style decoding using a state machine. The decoder tracks a `bitval` (previous bit state) and interprets each raw03 value combined with the previous state:

```
key = raw03_value | (bitval << 4)

0x11 → two 0-bits                (bitval = 0)
0x00 → one 0-bit                 (bitval = 0)
0x12 → one 0-bit, then one 1-bit (bitval = 1)
0x01 → one 1-bit                 (bitval = 1)
0x10 → one 1-bit                 (bitval = 1)
else → one 0-bit                 (bitval = 0)
```

Output bits are packed LSB-first into bytes.

### Step 4: Validate CRC

Each block has a 2-byte CRC appended. CRC parameters:
- Polynomial: `0x10810`
- Initial value: `0x8000`

```rust
fn calc_crc(buf: &[u8], size: usize) -> u16 {
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
```

CRC is computed over block data + 2 CRC bytes. Result should be 0 for a valid block.

## FDS Block Structure

Each side of a disk contains these blocks in sequence, separated by gaps:

| Block | Type Byte | Content Size | Description |
|-------|-----------|-------------|-------------|
| 1 | 0x01 | 56 bytes (0x38) | Disk info — contains "*NINTENDO-HVC*" header, game title, disk side, etc. |
| 2 | 0x02 | 2 bytes | File count |
| 3 | 0x03 | 16 bytes | File header — filename, load address, file size, file type |
| 4 | 0x04 | variable | File data |

Blocks 3 and 4 repeat as pairs for each file on the disk. Between each block is a gap (stream of zeros, minimum `0x300` raw03 values).

### File Header (Block 3) Layout

| Offset | Size | Field |
|--------|------|-------|
| 0 | 1 | Block type (0x03) |
| 1 | 1 | File number |
| 2 | 1 | File ID |
| 3 | 8 | Filename (ASCII) |
| 11 | 2 | Load address (little-endian) |
| 13 | 2 | File size (little-endian, actual size = value + 1) |
| 15 | 1 | File type: 0=PRG, 1=CHR, 2=NT |

## Output Format

Standard `.fds` format: **65,500 bytes per side**. For a 2-side disk, the output is 131,000 bytes (raw) or 131,016 bytes with fwNES header.

### fwNES Header (optional, 16 bytes)

```
Bytes 0-2:  "FDS"
Byte 3:     0x1A
Byte 4:     Number of sides (typically 2)
Bytes 5-15: 0x00
```

## Verified

The read protocol was verified by building a working Rust implementation (`fdsstick-cli`) that produces output matching the Windows tool's known-good dump at 99.995% accuracy (65,497/65,500 bytes identical per side, with differences only in unused padding area at end of data).
