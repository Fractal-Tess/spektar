<p align="center">
  <img src="assets/logo.png" alt="Spektar" width="160" />
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-8b5cf6" alt="MIT license" /></a>
  <img src="https://img.shields.io/badge/platform-Linux-0b0816?logo=linux&logoColor=white" alt="Linux only" />
  <a href="flake.nix"><img src="https://img.shields.io/badge/nix-flake-5277C3?logo=nixos&logoColor=white" alt="Nix flake" /></a>
</p>

<h1 align="center">Spektar</h1>

<p align="center">
  <img src="assets/screenshots/hero.webp" alt="Two Spektar windows showing 40 lavender frequency bars for the same track, one on the Balanced preset and one on Monstercat, with the preset and slider controls below" width="100%" />
</p>

<p align="center">
  A real-time audio visualiser for Linux that draws whatever your speakers are playing as frequency bars.
</p>

Spektar listens to the monitor of your default output through PulseAudio (or PipeWire's Pulse layer), runs an FFT over it, and draws 40 bars in an egui window. It started as a way to learn audio capture in Rust and is modelled on [Cava](https://github.com/karlstav/cava), but runs as a desktop window instead of in a terminal.

- **Groups** frequencies into log-spaced bands, so bass and treble each get a fair share of the bars
- **Adjusts** sensitivity on its own, so quiet tracks still move and loud ones don't pin every bar
- **Tunes** the motion with four presets (Balanced, Monstercat, Punchy, Smooth) plus lerp, FPS and sampling sliders

## Quick start

```bash
git clone https://github.com/Fractal-Tess/spektar.git
cd spektar
nix develop -c cargo run --release
```

Play something. Spektar picks up the default sink on its own. Without Nix, see [INSTALL.md](INSTALL.md) for the system libraries it needs.

## A closer look

<table>
  <tr>
    <td><img src="assets/screenshots/punchy.webp" alt="The Punchy preset reacting to a drum hit, with tall bars in the low bands" /></td>
    <td><img src="assets/screenshots/smooth.webp" alt="The Smooth preset with the lerp slider at 0.60, giving softer, slower bars" /></td>
  </tr>
  <tr>
    <td colspan="2"><img src="assets/screenshots/noise.webp" alt="Pink noise lighting all 40 bands, tapering gently towards the treble" /></td>
  </tr>
</table>

## More

- [INSTALL.md](INSTALL.md): requirements and running without Nix
- [CONTRIBUTING.md](CONTRIBUTING.md): crate layout, debug flags, and how the README images are made

## License

[MIT](LICENSE)
