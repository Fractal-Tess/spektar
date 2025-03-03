# Spektar

A Rust-based audio spectrum visualizer that displays real-time frequency analysis of the system's audio input.

## Features

- Real-time audio spectrum visualization
- Vertical bar graph representation of audio frequencies
- Color gradients representing intensity levels
- Historical visualization with fading effect
- Logarithmic frequency bands for better audio perception

## Requirements

- Rust (1.77.0 or newer)
- Linux system with ALSA/PulseAudio
- OpenGL support

## Setup with Nix/Direnv

This project uses Nix flakes and direnv for reproducible development environments:

1. Ensure you have Nix with flakes enabled and direnv installed
2. Run `direnv allow` in the project directory
3. The development environment will be automatically loaded

## Manual Setup

If not using Nix:

1. Install system dependencies:
   - ALSA development libraries
   - PulseAudio development libraries
   - X11 and OpenGL libraries

2. Build and run the project:
   ```
   cargo build --release
   cargo run --release
   ```

## Usage

1. Run the application:
   ```
   cargo run --release
   ```

2. The application will automatically connect to your default audio input device and start visualizing the audio spectrum.

3. Make some noise or play audio to see the visualization respond.

## License

MIT

## Credits

Built with:
- cpal - Cross-platform audio library
- spectrum-analyzer - Audio frequency analysis
- egui/eframe - GUI framework
- ring_buffer - Efficient circular buffer implementation