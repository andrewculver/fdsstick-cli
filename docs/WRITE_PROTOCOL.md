# FDS Stick Write Protocol

The tool reverses the process:

1. Parses the `.fds` file into FDS blocks (disk info, file count, file headers, file data).
2. Computes and appends CRC-16 to each block.
3. MFM-encodes each block using nibble-interleaved encoding.
4. Packs the encoded values (4 per byte) with gap and marker bytes.
5. Sends the packed data to the device via USB HID output reports.

## Background

Reverse-engineered from USB packet capture (`writing.json`) of the Windows tool (`fdsstick_20160926.exe`) writing `actual.fds` to disk.

Device: FDS Stick (VID `0x16D0`, PID `0x0AAA`)

## Protocol Sequence

### Phase 1: Initialization

Same init sequence as the read protocol:

```
SET Report 3: [03, 01, 01, 01, 9F, 00×59]   → command 0x9F (no fill)
GET Report 2: status (64 bytes)
GET Report 33: device status (2 bytes)
SET Report 3: [03, 01, 01, 01, 05, 00×59]   → command 0x05
GET Report 2: status
SET Report 3: [03, 01, 01, 01, 15, 00×59]   → command 0x15
GET Report 2: status
```

Then repeated with fill byte 0x63:

```
SET Report 3: [03, 01, 01, 01, 9F, 63×59]   → command 0x9F (with fill)
GET Report 2: status
GET Report 33: device status
SET Report 3: [03, 01, 01, 01, 05, 00×59]
GET Report 2: status
SET Report 3: [03, 01, 01, 01, 15, 00×59]
GET Report 2: status
```

Followed by verification reads:

```
GET Report 8: verification (1 byte)
GET Report 9: verification (1 byte)
```

### Phase 2: Address Table Scan

Same as read — scans addresses 0x01B0 to 0x04E0 in steps of 0x10:

```
For each address:
  SET Report 6: [06, addr_lo, addr_hi, D8]
  SET Report 4: [04, 01, 01, 03, 05, 63, 63, 63, 63]  → trigger
  GET Report 1: block status (256 bytes)
  SET Report 4: [04, 00, 00, 00, 05, 63, 63, 63, 63]  → ack
```

### Phase 3: Re-init

```
SET Report 3: [03, 01, 01, 01, 9F, 63×59]
GET Report 2: status
GET Report 33: device status
SET Report 3: [03, 01, 01, 01, 05, 00×59]
GET Report 2: status
SET Report 3: [03, 01, 01, 01, 15, 00×59]
GET Report 2: status
```

### Phase 4: Start Write (Side A)

```
SET Report 16: [10, 01]
```

Note: Read uses `[10, 00]`. The second byte `0x01` signals write mode.

### Phase 5: Bulk Write Data (Side A)

~577-578 packets sent via SET_REPORT, each using Report ID 18 (`0x12`):

```
SET Report 18: [12, data×254]   (255 bytes total per packet)
SET Report 18: [12, data×254]
... repeated ~577 times
```

Each packet carries 254 bytes of encoded disk data. Total raw data per side: ~146,000-147,000 bytes.

### Phase 6: Flip Disk

The Windows tool prompts the user to flip the disk to side B. No USB traffic during this pause.

### Phase 7: Start Write (Side B)

```
SET Report 16: [10, 01]
```

### Phase 8: Bulk Write Data (Side B)

Same format as side A — ~577-578 packets of Report 18.

### Phase 9: Finalize

```
SET Report 32: [20, 00]
```

This signals the device that the write operation is complete.

## Report ID Summary

| Report ID | Direction | Size | Purpose |
|-----------|-----------|------|---------|
| 0x03 | SET | 64 bytes | Command (0x9F, 0x05, 0x15) |
| 0x06 | SET | 4 bytes | Set address |
| 0x04 | SET | 9 bytes | Trigger / Ack |
| 0x10 | SET | 2 bytes | Start bulk (`00`=read, `01`=write) |
| 0x12 | SET | 255 bytes | **Bulk write data (NEW)** |
| 0x20 | SET | 2 bytes | **Finalize write (NEW)** |
| 0x01 | GET | 256 bytes | Block status |
| 0x02 | GET | 64 bytes | Status |
| 0x08 | GET | varies | Verification |
| 0x09 | GET | varies | Verification |
| 0x11 | GET | 256 bytes | Bulk read data (read only) |
| 0x21 | GET | 2 bytes | Device status |

## Data Encoding

### Packing Format

Same as the read protocol: 4 two-bit values per byte, MSB-first:

```
byte = (val0 << 6) | (val1 << 4) | (val2 << 2) | val3
```

Where `val0` (bits 6-7) is first in time, `val3` (bits 0-1) is last.

### Pulse Width Values

Write data uses values **{1, 2, 3}** (shifted by +1 compared to read's {0, 1, 2}):

| Value | Meaning | Physical |
|-------|---------|----------|
| 1 | Short pulse | Data bit = 1 |
| 2 | Medium pulse | Data bit = 0, next bit = 0 (clock pulse between two zeros) |
| 3 | Long pulse | Data bit = 0, next bit = 1 (no clock needed) |

This is MFM (Modified Frequency Modulation) encoding. The choice between value 2 and 3 for a zero bit depends on a look-ahead to the next data bit.

### MFM Encoding Rules

For each data bit being encoded:

- **Data bit = 1** → always produces value **1** (short pulse)
- **Data bit = 0, next bit = 0** → produces value **2** (medium pulse, with clock)
- **Data bit = 0, next bit = 1** → produces value **3** (long pulse, no clock)

The "next bit" context follows MSB-first bit ordering within each byte, and at byte boundaries wraps to the MSB of the next byte.

### Nibble Interleaving

Data bytes are encoded in an interleaved nibble order rather than sequentially. For each pair of data bytes (B, B+1):

- Positions 0-3: Encode the **high nibble** (bits 7,6,5,4) of byte B
- Positions 4-7: Encode the **low nibble** (bits 3,2,1,0) of byte B+1

This produces the pattern: hi(0), lo(1), hi(1), lo(2), hi(2), lo(3), ...

The look-ahead context at each position:

| Position | Data Bit | Context (Next) Bit |
|----------|----------|-------------------|
| 0 | bit 7 of B | bit 6 of B |
| 1 | bit 6 of B | bit 5 of B |
| 2 | bit 5 of B | bit 4 of B |
| 3 | bit 4 of B | bit 3 of B (same byte's low nibble) |
| 4 | bit 3 of B+1 | bit 2 of B+1 |
| 5 | bit 2 of B+1 | bit 1 of B+1 |
| 6 | bit 1 of B+1 | bit 0 of B+1 |
| 7 | bit 0 of B+1 | bit 7 of B (wraps to high nibble) |

## Block Structure

Each side's data stream is composed of:

### 1. Packet Header (first packet only)

```
[0xC0, 0x00, 0xAB]
```

3 bytes at the start of the first Report 18 packet for each side.

### 2. Lead-In Gap

27,000 packed values (6,750 bytes) — all value 2 (medium pulse). This creates the initial gap the FDS drive uses to synchronize.

### 3. FDS Blocks

Each block is encoded as:

```
[0x80]              → gap-end marker (1 byte)
[block data]        → MFM-encoded block content
[CRC lo] [CRC hi]  → 2-byte CRC, little-endian
```

FDS block types and sizes:

| Block | Type Byte | Content Size | Description |
|-------|-----------|-------------|-------------|
| 1 | 0x01 | 56 bytes | Disk info header |
| 2 | 0x02 | 2 bytes | File count |
| 3 | 0x03 | 16 bytes | File header (repeated per file) |
| 4 | 0x04 | variable | File data (repeated per file) |

### 4. Inter-Block Gap

112 zero bytes (producing 896 packed values, all value 2) between each block.

### 5. CRC Calculation

Same CRC as the read protocol:
- Polynomial: `0x10810`
- Initial value: `0x8000`
- Computed over block content bytes + 2 zero placeholder bytes
- Result stored little-endian after the block content

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

## Comparison: Read vs Write

| Aspect | Read | Write |
|--------|------|-------|
| Start bulk | `[10, 00]` | `[10, 01]` |
| Bulk data | GET Report 17 (0x11) | SET Report 18 (0x12) |
| Finalize | (none) | SET Report 32 `[20, 00]` |
| Pulse values | {0, 1, 2} | {1, 2, 3} |
| Packet size | 256 bytes (with seq) | 255 bytes (report ID + 254 data) |
| Packets/side | ~495-496 | ~577-578 |
| Encoding | Packed raw03 | Packed MFM with nibble interleave |

## Verified

The encoding was verified against the full capture with a **100% match** on both sides:
- Side A: 447,560 values matched
- Side B: 445,888 values matched
