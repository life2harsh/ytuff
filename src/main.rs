mod core;
mod playback;
mod sources;
mod ui;

use anyhow::Result;
use clap::{Arg, Command};
use core::Core;

#[tokio::main]
async fn main() -> Result<()> {
    let matches = Command::new("rustplayer")
        .version("0.1.0")
        .author("RustPlayer Team")
        .about("A terminal-based music player with SoundCloud support")
        .arg(
            Arg::new("path")
                .short('p')
                .long("path")
                .value_name("DIR")
                .help("Directory to scan for music files")
                .default_value("."),
        )
        .arg(
            Arg::new("quality")
                .short('q')
                .long("quality")
                .value_name("QUALITY")
                .help("Audio quality preference (low, medium, high)")
                .default_value("high"),
        )
        .arg(
            Arg::new("soundcloud")
                .short('s')
                .long("soundcloud")
                .help("Enable SoundCloud integration")
                .action(clap::ArgAction::SetTrue),
        )
        .get_matches();

    let scan_path = matches.get_one::<String>("path").unwrap();
    let quality = matches.get_one::<String>("quality").unwrap();
    let soundcloud_enabled = matches.get_flag("soundcloud");

    println!("RustPlayer v0.1.0 - Terminal Music Player");
    println!("Scanning: {} | Quality: {} | SoundCloud: {}", scan_path, quality, soundcloud_enabled);

    let mut core = Core::new();
    core.scan_path(scan_path).await?;

    let playback_handle = playback::start_playback_thread(core.clone());

    ui::run_ui(core, playback_handle).await?;

    Ok(())
}
