# Contributing

## Layout

The workspace has three crates and a thin binary:

- `crates/spektar-audio`: finds the default sink's monitor and streams samples from it with libpulse
- `crates/spektar-spectrum`: FFT, log-spaced band mapping, auto-sensitivity, smoothing, and the response presets
- `crates/spektar-ui`: the eframe/egui window
- `src/main.rs`: picks the window backend and starts the UI

## Development

```bash
nix develop
cargo run
cargo clippy --all-targets
cargo fmt
```

Two environment variables help when tuning the response:

- `SPEKTAR_DEBUG=1` logs spectrum processing details
- `SPEKTAR_DIAGNOSTIC=1` prints per-frame timing and bar levels to stderr, then quits after five seconds

## README images

The images in `assets/screenshots/` are composed from the HTML layouts in `assets/screenshots/source/` (`style.css` plus one `.html` per image). They load raw captures from `assets/screenshots/source/shots/`, which is gitignored:

| Capture | Window size | Preset |
| --- | --- | --- |
| `main.png` | 1280 × 820 | Balanced |
| `monstercat.png` | 960 × 660 | Monstercat |
| `punchy.png` | 1280 × 820 | Punchy |
| `smooth.png` | 1280 × 820 | Smooth, lerp 0.60 |
| `noise.png` | 1280 × 820 | Balanced, playing pink noise |

To remake them:

1. Run Spektar at 2× scale with real audio playing and capture the window as PNG. A headless sway (`WLR_BACKENDS=headless`) with `grim`, plus a private PulseAudio null sink, keeps this away from your desktop.
2. Render each layout at 1600 × 1000 and 2× scale:

   ```bash
   cd assets/screenshots
   for n in hero punchy smooth noise; do
     chromium --headless=new --hide-scrollbars --force-device-scale-factor=2 \
       --window-size=1600,1000 --screenshot=/tmp/$n.png "file://$PWD/source/$n.html"
     cwebp -q 82 /tmp/$n.png -o $n.webp
   done
   ```
