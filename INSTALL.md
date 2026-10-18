# Installing Spektar

Spektar runs on Linux only. It captures audio through libpulse, so it needs either PulseAudio or PipeWire with `pipewire-pulse`.

## With Nix

The flake's dev shell brings in the Rust toolchain and every system library:

```bash
nix develop -c cargo run --release
```

With direnv, `direnv allow` loads the same shell when you enter the directory.

## Without Nix

You need:

- Rust 1.70 or newer
- PulseAudio, or PipeWire with its Pulse server
- `pkg-config` and the development packages for libpulse, Wayland, libxkbcommon, libGL/EGL, and X11 (xcb, Xcursor, Xrandr, Xi)

Then:

```bash
cargo run --release
```

## Troubleshooting

- **"Waiting for default sink audio…"** means Spektar is connected but hears silence. Check that something is playing on the default output (`pactl get-default-sink`).
- **It shows the wrong device.** Spektar always follows the default sink's monitor. Change the default with `pactl set-default-sink <name>` or your desktop's sound settings and restart it.
- **It opens through XWayland.** When both `DISPLAY` and `WAYLAND_DISPLAY` are set, Spektar uses X11 on purpose. Unset `DISPLAY` to run it natively on Wayland.
