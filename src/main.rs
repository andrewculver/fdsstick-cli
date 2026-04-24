mod decode;
mod device;
mod encode;
mod protocol;

use clap::{Parser, Subcommand};
use std::fs::File;
use std::io::{Read, Write};
use std::process;

#[derive(Parser)]
#[command(name = "fdsstick-cli", about = "FDSStick CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Read a disk and save to file
    Dump {
        /// Output filename
        output: String,

        /// Use fwNES format (16-byte header)
        #[arg(long)]
        fwnes: bool,

        /// Number of sides to read (1 or 2, default: 2)
        #[arg(long, default_value_t = 2)]
        sides: usize,
    },
    /// Write a disk image to disk
    Write {
        /// Input .fds filename
        input: String,
    },
    /// Run diagnostics on the device
    Diag,
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Dump { output, fwnes, sides } => {
            if let Err(e) = dump(&output, fwnes, sides) {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
        Commands::Write { input } => {
            if let Err(e) = write_cmd(&input) {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
        Commands::Diag => {
            if let Err(e) = diag() {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
    }
}

fn dump(output: &str, fwnes: bool, sides: usize) -> Result<(), Box<dyn std::error::Error>> {
    if sides < 1 || sides > 2 {
        return Err("--sides must be 1 or 2".into());
    }

    eprintln!("Opening FDS Stick...");
    let dev = device::FdsStick::open().map_err(|e| format!("{e}"))?;
    eprintln!("Device found.");

    let (raw_a, raw_b) = protocol::read_disk(&dev, sides).map_err(|e| format!("{e}"))?;

    // Decode raw timing data to FDS format
    eprintln!("\nDecoding side A...");
    let side_a = decode::decode_side(&raw_a)
        .ok_or("Failed to decode side A")?;

    let side_b = if let Some(ref rb) = raw_b {
        eprintln!("\nDecoding side B...");
        Some(decode::decode_side(rb).ok_or("Failed to decode side B")?)
    } else {
        None
    };

    let side_count = if side_b.is_some() { 2u8 } else { 1u8 };

    // Write output file
    let mut file = File::create(output)?;

    if fwnes {
        let mut header = [0u8; 16];
        header[0] = b'F';
        header[1] = b'D';
        header[2] = b'S';
        header[3] = 0x1A;
        header[4] = side_count;
        file.write_all(&header)?;
    }

    file.write_all(&side_a)?;
    if let Some(ref sb) = side_b {
        file.write_all(sb)?;
    }

    let total = side_a.len() + side_b.as_ref().map_or(0, |b| b.len());
    eprintln!(
        "\nWrote {} bytes to {output} ({} side{}{})",
        if fwnes { total + 16 } else { total },
        side_count,
        if side_count > 1 { "s" } else { "" },
        if fwnes { ", fwNES format" } else { "" }
    );

    Ok(())
}

const FDSSIZE: usize = 65500;

fn write_cmd(input: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut data = Vec::new();
    File::open(input)?.read_to_end(&mut data)?;
    eprintln!("Read {} bytes from {input}", data.len());

    // Detect and strip fwNES header
    let fds_data = if data.len() >= 4 && &data[0..4] == b"FDS\x1a" {
        eprintln!("fwNES header detected, stripping 16 bytes.");
        &data[16..]
    } else {
        &data
    };

    // Determine number of sides
    let side_count = fds_data.len() / FDSSIZE;
    if fds_data.len() % FDSSIZE != 0 || side_count == 0 || side_count > 2 {
        return Err(format!(
            "Invalid FDS data size: {} bytes (expected {} or {})",
            fds_data.len(),
            FDSSIZE,
            FDSSIZE * 2
        )
        .into());
    }
    eprintln!("{side_count} side(s) detected.");

    // Encode
    eprintln!("\nEncoding side A...");
    let packed_a = encode::encode_side(&fds_data[..FDSSIZE]);

    let packed_b = if side_count == 2 {
        eprintln!("\nEncoding side B...");
        Some(encode::encode_side(&fds_data[FDSSIZE..FDSSIZE * 2]))
    } else {
        None
    };

    // Open device and write
    eprintln!("\nOpening FDS Stick...");
    let dev = device::FdsStick::open().map_err(|e| format!("{e}"))?;
    eprintln!("Device found.");

    protocol::write_disk(&dev, &packed_a, packed_b.as_deref()).map_err(|e| format!("{e}"))?;

    eprintln!("\nWrite complete!");
    Ok(())
}

fn diag() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("Opening FDS Stick...");
    let dev = device::FdsStick::open().map_err(|e| format!("{e}"))?;
    eprintln!("Device found.");
    protocol::run_diagnostics(&dev).map_err(|e| format!("{e}"))?;
    Ok(())
}
