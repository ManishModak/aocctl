# aocctl

Control AOC monitors from Linux, including the AOC-only settings that normally
need AOC's Windows app (G-Menu): shadow control, game color, overdrive, Low Blue,
Eco mode and more.

aocctl talks to the monitor over DDC/CI, the same channel ddcutil uses. It
also speaks AOC's vendor command page, which standard tools don't. It is an
independent project, not made or endorsed by AOC.

## Supported monitors

| Model | Settings |
|---|---|
| AOC 24G4 | brightness, contrast, shadow-control, game-color, overdrive, low-blue, eco, hdr-mode, dcr, clear-vision, dial-point, frame-counter |
| Other AOC models | brightness, contrast (vendor settings once the model is added; see below) |

Every setting listed for a model has passed a write, read-back and restore
check on that model. Settings that haven't are left out or read-only.

## Install

From source, with Rust 1.85 or newer:

```sh
cargo install --git https://github.com/ManishModak/aocctl
```

### Permissions

aocctl needs read/write access to the `/dev/i2c-N` device of your monitor's
connection: the same access ddcutil needs. On most distributions:

1. Load the `i2c-dev` kernel module (`sudo modprobe i2c-dev`; to load it at
   boot, add `i2c-dev` to a file in `/etc/modules-load.d/`).
2. Get access to the device, either through the udev rule your distribution's
   `ddcutil` package installs, or by adding yourself to the `i2c` group
   (`sudo usermod -aG i2c $USER`, then log in again).

Having that access doesn't by itself mean your monitor, GPU or connection
supports DDC/CI; `aocctl list` shows what answers. NVIDIA's proprietary driver
is known to have trouble with DDC/CI over DisplayPort.

## Usage

```sh
aocctl list                         # connected monitors and what aocctl can do with them
aocctl get                          # all settings of the monitor
aocctl get overdrive shadow-control
aocctl set overdrive medium         # values by name or number
aocctl set shadow-control 55
aocctl set low-blue office --display DP-1   # pick a monitor by connector or serial
```

Every command takes `--json` for scripts and desktop widgets. JSON output
carries `"schema": 1`; a failed command exits non-zero and prints
`{"schema": 1, "error": "..."}`. After a write, aocctl reads the value back and
reports `verified`, `mismatch` or `unknown`; only `verified` exits 0.

When more than one AOC monitor is connected, `--display` is required.

### Notes on settings

- `hdr-mode` is the monitor's own HDR-style picture processing. It doesn't turn
  on HDR output from your desktop.
- Changing `shadow-control` or `game-color` switches the monitor out of its FPS
  and racing game modes. That is the monitor's behaviour; aocctl doesn't switch
  the mode back.

## Add your monitor

If your AOC model isn't supported yet:

1. Run `aocctl probe --aoc-items` and save the output. It only reads values, and
   it leaves out your serial number.
2. Open a [model report](https://github.com/ManishModak/aocctl/issues/new?template=model-report.yml)
   and paste it.

`--aoc-items` reads the AOC vendor items aocctl already knows from other models.
It's experimental: it only reads, and only on AOC monitors.

## How it stays safe

- It only opens i2c buses that belong to a connected external monitor's
  connector, never built-in laptop panels or other system buses.
- AOC vendor commands go only to monitors whose EDID says AOC.
- It writes only settings verified for that exact model (EDID product code and
  model name), checks every value against the setting's allowed values and the
  monitor's reported maximum, and never resends a write.
- It locks the i2c device while it works, the same way ddcutil does, so the two
  don't interleave commands.

## License

MIT OR Apache-2.0, at your option.
