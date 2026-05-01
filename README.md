# RustPlayer

**A high-performance terminal music player written in Rust with comprehensive local audio support and extensible architecture for streaming integration.**

RustPlayer is a modern, efficient music player designed for power users and developers who prefer terminal-based interfaces. Built with Rust's safety guarantees and performance characteristics, it provides robust audio playback with an intuitive TUI.

## Overview

RustPlayer delivers cross-platform audio playback with support for multiple audio formats, real-time progress tracking, and a clean command-line interface. The architecture is designed to support multiple audio sources, with local file playback currently implemented and SoundCloud integration planned for future releases.

## Features

### Current Capabilities
- **Local Audio Playback**: Support for MP3, FLAC, WAV, M4A, OGG, AAC, Opus, and WMA formats
- **Terminal User Interface**: Color-coded interface with real-time progress visualization
- **Efficient Playback Control**: Play, pause, resume, and track navigation
- **Metadata Extraction**: Automatic title, artist, and duration reading from audio files
- **Command-Line Interface**: Full CLI with configurable options and help documentation
- **Cross-Platform Support**: Windows, Linux, and macOS compatibility

### Planned Features
- SoundCloud API integration for streaming
- Playlist management and persistence
- Configuration file support (.rustplayer/config.toml)
- Audio quality selection
- Gapless playback
- Advanced navigation and search capabilities

## System Requirements

- **Rust**: 1.70 or later
- **Audio System**:
  - Windows: DirectSound-compatible audio device
  - Linux: ALSA or PulseAudio
  - macOS: CoreAudio
- **Memory**: Minimal (~50 MB runtime)
- **Disk**: Compact binary (~50 MB release build)

## Installation

### From Source

```bash
git clone https://github.com/yourusername/rustplayer.git
cd rustplayer
cargo build --release
./target/release/rustplayer --help
```

The release binary will be available at `target/release/rustplayer`.

## Usage

### Basic Playback

```bash
# Play from current directory
rustplayer

# Specify music directory
rustplayer --path "/path/to/music" --quality high

# Enable experimental features
rustplayer --path "/path/to/music" --soundcloud
```

### Command-Line Options

| Option | Short | Type | Default | Description |
|--------|-------|------|---------|-------------|
| `--path` | `-p` | DIR | `.` | Directory containing audio files to scan |
| `--quality` | `-q` | STRING | `high` | Audio quality preference (low/medium/high) |
| `--soundcloud` | `-s` | FLAG | disabled | Enable SoundCloud integration (experimental) |
| `--help` | `-h` | - | - | Display help information |
| `--version` | `-V` | - | - | Display version information |

### Interactive Controls

| Key(s) | Action |
|--------|--------|
| `Enter` | Play selected track |
| `Space` | Pause playback |
| `r` | Resume playback |
| `n` | Skip to next track |
| `j` / `k` or `↓` / `↑` | Navigate track list |
| `q` | Exit application |

### YouTube Music Auth (Optional)

Personalized home, playlists, history, and recommendations use browser cookies. You can import
cookies or a ytmusicapi-style headers file:

```bash
rustplayer auth cookie-file <cookies.txt>
rustplayer auth cookie-header "SID=...; SAPISID=..."
rustplayer auth headers-file <headers.json>
```

Artwork inline rendering now auto-detects the best available path in this order:
kitty protocol, `wimg` on sixel-capable terminals, then ANSI blocks.
You can still force a renderer manually:

```bash
RUSTPLAYER_ART=kitty
RUSTPLAYER_ART=blocks
RUSTPLAYER_ART=1
RUSTPLAYER_ART=wimg
RUSTPLAYER_ART=sixel
```

If the inline image looks too wide or too short in your terminal, you can tune the
assumed cell size with `RUSTPLAYER_ART_CELL_W` and `RUSTPLAYER_ART_CELL_H`.

**Track Indicators:**
- `L` = Local file
- `C` = Cloud source (SoundCloud)

## Architecture

RustPlayer follows a modular architecture with clear separation of concerns:

```
src/
├── core/
│   ├── mod.rs       # Core state management (Arc<Mutex<>> patterns)
│   └── track.rs     # Track model with multi-source support
├── sources/
│   ├── mod.rs
│   ├── local.rs     # Local filesystem scanning and metadata extraction
│   └── soundcloud.rs # SoundCloud API integration (framework)
├── playback/
│   └── mod.rs       # Audio engine with rodio and concurrent playback thread
├── ui/
│   └── mod.rs       # Terminal UI with ratatui
└── main.rs          # CLI entry point and application initialization
```

### Design Patterns

- **Thread-Safe State**: `Arc<Mutex<T>>` for shared state across playback and UI threads
- **Channel-Based Communication**: `mpsc` channels for command dispatch to playback thread
- **Async Runtime**: Tokio for future streaming support and concurrent operations
- **Modular Sources**: Extensible source abstraction for local and cloud audio

## Technology Stack

| Component | Library | Version | Purpose |
|-----------|---------|---------|---------|
| **Runtime** | Tokio | 1.0+ | Async task execution |
| **Audio Playback** | Rodio | 0.17 | Cross-platform audio output |
| **Terminal UI** | Ratatui | 0.20 | Rich terminal interface |
| **CLI Parsing** | Clap | 4.0 | Command-line argument handling |
| **HTTP Client** | Reqwest | 0.11 | API integration |
| **Metadata** | Lofty | 0.11 | Audio tag extraction |
| **File System** | Walkdir | 2.0 | Recursive directory traversal |
| **Serialization** | Serde/serde_json | 1.0 | Configuration and API data |

## Development

### Building

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# Run tests
cargo test

# Check code quality
cargo check
cargo clippy
```

### Project Structure

The codebase follows Rust best practices with clear module boundaries and responsibility. Each module is designed for independent testing and extension.

## Known Limitations

- **Seeking**: Not currently supported due to rodio audio stream constraints
- **SoundCloud**: Integration framework present but API methods are placeholders
- **Playlists**: Currently not persisted across sessions
- **Equalizer**: Audio processing not implemented

## Roadmap

### v0.2.0
- [ ] Basic SoundCloud API integration
- [ ] Configuration file support
- [ ] Playlist creation and loading

### v0.3.0
- [ ] Audio equalizer implementation
- [ ] Improved search and filtering
- [ ] Library database for large music collections

### v1.0.0
- [ ] Full feature parity with major music players
- [ ] Plugin architecture for extensions
- [ ] Remote control protocol

## Contributing

Contributions are welcome. Please follow these guidelines:

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/enhancement`)
3. Commit changes with clear messages
4. Push to branch and create a Pull Request

## License

This project is licensed under the GNU General Public License v3.0 - see the LICENSE file for details.

## Acknowledgments

Built with modern Rust tooling and designed for efficiency and reliability.
