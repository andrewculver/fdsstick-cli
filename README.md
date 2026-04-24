# fdsstick-cli

A cross-platform command-line tool for reading and writing disk images on Nintendo's [Famicom Disk System](https://en.wikipedia.org/wiki/Famicom_Disk_System) using [FDSStick](https://www.fdsstick.com).

## Requirements

### For Reading
- [Famicom Disk System (HVC-022)](https://en.wikipedia.org/wiki/Famicom_Disk_System)
- [FDSStick](https://www.fdsstick.com)
- [FDSStick Adapter Cable](https://www.ebay.com/itm/353731277861)
- [Rust toolchain (1.85+)](https://rustup.rs)

### For Writing
Nintendo introduced two anti-piracy measures on the Famicom Disk System, so to write full disks you need two things:
 1. FMD-POWER-01 or [modified FMD-POWER-02 through FMD-POWER-05](https://famicomworld.com/workshop/tech/fds-power-board-modifications/) power board
 2. FD7201 or [modified FD3206](https://famicomworld.com/workshop/tech/famicom-disk-system-fd3206-write-mod/) drive controller

## Installation

### Install via `cargo` (recommended)

1. Install Rust if you don't have it:
   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```
   On macOS you can also use `brew install rust`.

2. Install fdsstick-cli:
   ```sh
   cargo install --git https://github.com/andrewculver/fdsstick-cli
   ```

This builds the binary and places it in `~/.cargo/bin/`, which the Rust installer adds to your `PATH`.

### Build from source

```sh
git clone https://github.com/andrewculver/fdsstick-cli.git
cd fdsstick-cli
cargo install --path .
```

## Usage

### Dump a two-sided disk

```sh
fdsstick-cli dump output.fds
```

1. Insert the disk with **side A facing up**.
2. Run the command.
3. When prompted, if the game has two sides, flip the disk to side B and press <kbd>Return</kbd> or <kbd>Enter</kbd>. Otherwise hit <kbd>Escape</kbd> to dump a single side.
4. The tool reads both sides and writes the output file.

#### Dump a single-sided disk

To read only side A with no interactive prompt:

```sh
fdsstick-cli dump output.fds --sides 1
```

#### fwNES format

Add `--fwnes` to prepend the standard 16-byte fwNES header (`FDS\x1a`). The header's side count reflects the number of sides actually read.

```sh
fdsstick-cli dump output.fds --fwnes
```

### Write a disk

```sh
fdsstick-cli write image.fds
```

1. Insert the disk with **side A facing up**.
2. Run the command.
3. For 2-sided images, flip the disk when prompted and press <kbd>Enter</kbd>.

The tool auto-detects and strips fwNES headers. It accepts 1-sided (65,500 bytes) or 2-sided (131,000 bytes) images.

### Diagnostics

Check device communication without reading or writing a full disk:

```sh
fdsstick-cli diag
```

## Notes for Linux
I've never run this on Linux, but Claude says you need a udev rule for non-root access:

```
# /etc/udev/rules.d/99-fdsstick.rules
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="16d0", ATTRS{idProduct}=="0aaa", MODE="0666"
```

Then reload: `sudo udevadm control --reload-rules && sudo udevadm trigger`

## License
Released under the MIT License. See [LICENSE](LICENSE) for details.
