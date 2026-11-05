# Waybar module

Your AOC monitor's brightness on the bar. Scroll on it to change brightness,
click it to pick one of your [presets](../../README.md#presets).

## Requirements

- `aocctl` on `PATH`, with access to your monitor's i2c bus (see the aocctl
  README). Check `aocctl get brightness` works first.
- `bash`, `jq`, `flock` (util-linux) and `pkill` (procps).
- For the preset menu: `fuzzel`, `wofi` or `rofi` (or any dmenu-style command
  in `AOCCTL_MENU`). `notify-send` shows errors, if installed.

## Install

```sh
install -Dm755 aocctl-waybar ~/.local/bin/aocctl-waybar
```

Add the `custom/aocctl` block from [`config.jsonc`](config.jsonc) to your
Waybar config, and `"custom/aocctl"` to one of the bar's `modules-left`,
`modules-center` or `modules-right` lists. [`style.css`](style.css) is
optional. Restart Waybar.

If `~/.local/bin` isn't on Waybar's `PATH`, use the script's full path in the
config.

## How it works

- Waybar runs `aocctl-waybar status` once a minute and whenever the script
  sends it the module's `signal` (8 by default). The script reads only
  brightness; the tooltip says what went wrong when aocctl can't (the monitor's
  bus is busy, no access, no AOC monitor).
- A scroll moves the shown value at once and writes it to the monitor about
  0.3 s later, so a quick run of scroll steps ends in one or two writes. Steps
  are 5%; the range comes from aocctl.
- A click lists the presets saved for your monitor's model and applies the one
  you pick. Presets that can't be applied (a bad value) aren't listed.

## Settings

Set these in the environment Waybar starts with (e.g. `environment.d`), or
prefix the commands in the config (`AOCCTL_STEP=10 aocctl-waybar up`):

| Variable | Default | Meaning |
| --- | --- | --- |
| `AOCCTL_DISPLAY` | | Connector (e.g. `DP-1`) or serial; needed with more than one AOC monitor |
| `AOCCTL_STEP` | `5` | Brightness change per scroll step |
| `AOCCTL_WAYBAR_SIGNAL` | `8` | Must match the module's `"signal"` |
| `AOCCTL_MENU` | | dmenu-style command for the preset menu, e.g. `tofi` |
| `AOCCTL` | `aocctl` | Path to aocctl |

For a second AOC monitor, add a second module (`custom/aocctl-2`) with its own
`signal`, and prefix its commands with `AOCCTL_DISPLAY=DP-2
AOCCTL_WAYBAR_SIGNAL=9`.
