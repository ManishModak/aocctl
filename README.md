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
check on that model. Settings that haven't are read-only until someone with
that monitor runs [`aocctl verify`](#verify-a-setting) on them.

## Install

### Arch Linux (AUR)

```sh
paru -S aocctl      # latest release; aocctl-git builds the latest commit
```

The package sets up permissions (see below) and loads `i2c-dev`; no reboot
needed.

### Prebuilt binary (any distribution, x86_64 or aarch64)

Static binaries are attached to each [release](https://github.com/ManishModak/aocctl/releases):

```sh
v=0.1.0 arch=$(uname -m)
base=https://github.com/ManishModak/aocctl/releases/download/v$v
curl -fL -O "$base/aocctl-$v-$arch-linux.tar.gz" -O "$base/SHA256SUMS"
sha256sum -c --ignore-missing SHA256SUMS
tar xzf "aocctl-$v-$arch-linux.tar.gz" && cd "aocctl-$v-$arch-linux"
sudo install -Dm755 aocctl /usr/local/bin/aocctl
sudo install -Dm755 aocctl-ddc-bus /usr/lib/udev/aocctl-ddc-bus
sudo install -Dm644 60-aocctl-ddc-i2c.rules /etc/udev/rules.d/60-aocctl-ddc-i2c.rules
sudo install -Dm644 aocctl.conf /etc/modules-load.d/aocctl.conf
sudo modprobe i2c-dev && sudo udevadm control --reload && sudo udevadm trigger --subsystem-match=i2c-dev
```

### From source (cargo)

With Rust 1.85 or newer:

```sh
cargo install --git https://github.com/ManishModak/aocctl
```

Then set up permissions with the files in the repository's `dist/` folder, as in
the binary install above (`dist/udev/` and `dist/modules-load.d/`).

### Permissions

aocctl needs read/write access to the `/dev/i2c-N` device of your monitor's
connection. The packages and the steps above install a udev rule that gives the
user logged in at the computer (not over SSH) access to monitor buses only: the
DisplayPort AUX bus and the DDC bus of each external DP, HDMI, DVI or VGA
connector, which are the buses aocctl uses. Other i2c buses, such as the GPU's
power-management buses ("AMDGPU SMU"), RAM and motherboard SMBus, and sensors,
stay root-only. The `i2c-dev` kernel module must be loaded; `aocctl.conf` loads
it at boot.

The rule relies on systemd-logind's `uaccess`. Without systemd, or to use
aocctl from an SSH session, use the `i2c` group instead: install your
distribution's `i2c-tools` package, which creates the group and gives it the
i2c devices, then `sudo usermod -aG i2c $USER` and log in again. That grants
every i2c bus, as does the udev rule that `ddcutil` packages install. If `ddcutil detect`
works for your user, aocctl works too.

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

aocctl preset save gaming           # save the monitor's current settings as a preset
aocctl preset save night brightness=10 low-blue=office   # or give the values
aocctl preset apply night
aocctl preset list
```

Every command takes `--json` for scripts and desktop widgets. JSON output
carries `"schema": 1`; a failed command exits non-zero and prints
`{"schema": 1, "error": "..."}`. After a write, aocctl reads the value back and
reports `verified`, `mismatch` or `unknown`; only `verified` exits 0.

`get --json` lists each setting as `{"id", "name", "value", "max", "label",
"writable", "domain"}` (with `error` in place of the reading when it fails).
`domain` is `{"min", "max"}` for a number or `{"options": [...]}` for a choice
(the value is the option's index), so a widget can build its controls for any
model.

When more than one AOC monitor is connected, `--display` is required.

### Presets

A preset is a named set of values for your monitor model. `aocctl preset`
takes `save`, `list`, `show`, `apply` and `delete`:

- `preset save <name>` captures the monitor's current values of every setting
  aocctl can write (or only `--only brightness,low-blue`), replacing that
  preset's values. Values aocctl can't set back are left out and reported
  (on the 24G4, Eco reads a DCR-only value while DCR is on).
- `preset save <name> setting=value ...` doesn't read the monitor: it adds
  those values to the preset, creating it if needed.
- `preset apply <name>` writes only the values that differ, mode settings
  (DCR, Eco, HDR mode, Low Blue) first because they change other settings,
  then reads every value back. Each setting reports `unchanged`, `verified`,
  `mismatch`, `unknown` or `error`; only `unchanged` and `verified` exit 0.

Presets live in `~/.config/aocctl/presets.toml` (`$XDG_CONFIG_HOME` is
respected). You can edit it by hand, and aocctl keeps your comments:

```toml
[[preset]]
name = "Night"
mfg = "AOC"
model = "24G4"
product-code = "b401"

[preset.settings]
brightness = 10
low-blue = "office"
```

A preset belongs to one model (EDID manufacturer, product code and model
name) and is never listed for or applied to another. Every value is checked
like `aocctl set` checks it; a preset with a bad value is listed with its
`error` and never applied.

With `--json`, `list` prints `{"schema": 1, "presets": [{"name", "settings":
[{"id", "value", "label"}], "error"}]}`, and `apply` prints each setting's
`status`, `read_back` and `error`.

### Notes on settings

- `hdr-mode` is the monitor's own HDR-style picture processing. It doesn't turn
  on HDR output from your desktop.
- Changing `shadow-control` or `game-color` switches the monitor out of its FPS
  and racing game modes. That is the monitor's behaviour; aocctl doesn't switch
  the mode back. Applying a preset leaves values that are already set alone,
  so it only does this when the preset changes one of them.
- While `hdr-mode` is on, the 24G4 doesn't accept changes to shadow control,
  game color or Low Blue, so a preset that turns HDR mode on and also sets
  them reports `mismatch`.

## Desktop widgets

- **Noctalia**: a bar widget with the monitor's brightness (scroll to change
  it) and a panel with every setting and your presets. Copy
  [`integrations/noctalia/aocctl`](integrations/noctalia/aocctl) to
  `~/.local/share/noctalia/plugins/aocctl`, run
  `noctalia msg plugins enable manishmodak/aocctl`, then add the `brightness`
  widget to your bar. Its [README](integrations/noctalia/aocctl/README.md)
  has the details.

## Add your monitor

If your AOC model isn't supported yet:

1. Run `aocctl probe --aoc-items` and save the output. It only reads values, and
   it leaves out your serial number.
2. Open a [model report](https://github.com/ManishModak/aocctl/issues/new?template=model-report.yml)
   and paste it.

`--aoc-items` reads the AOC vendor items aocctl already knows from other models.
It's experimental: it only reads, and only on AOC monitors.

From the report and your monitor's menu, a maintainer adds your model with its
settings read-only. You can then make them writable with `aocctl verify`.

## Verify a setting

A setting shown as `(read-only)` in `aocctl get` hasn't been write-tested on
your model yet. To test one:

```sh
aocctl verify shadow-control --confirm-write
```

It moves the setting one step from its current value (down if it can, else up),
reads it back, then writes your original value back and reads that too. Your
screen may change for a moment. It prints a JSON report (without your serial
number) and exits 0 only if both values read back. Paste the report into a
[write verification](https://github.com/ManishModak/aocctl/issues/new?template=write-verification.yml)
issue, so the setting can be made writable for your model.

- It only tests settings aocctl already knows for your model, plus brightness
  and contrast, and never goes outside the setting's known values or the
  maximum your monitor reports.
- Ctrl-C waits until the original value is back. If the power goes or the
  cable is pulled mid-test, or the report says it couldn't confirm the restore,
  set the setting back in the monitor's on-screen menu; the report's `before`
  value is what it was.
- Like `set`, testing `shadow-control` or `game-color` can switch the monitor
  out of its FPS and racing game modes; the report shows the game mode
  before and after.

## How it stays safe

- It only opens i2c buses that belong to a connected external monitor's
  connector, never built-in laptop panels or other system buses.
- AOC vendor commands go only to monitors whose EDID says AOC.
- It writes only settings verified for that exact model (EDID product code and
  model name), checks every value against the setting's allowed values and the
  monitor's reported maximum, and never resends a write. The one exception is
  `aocctl verify --confirm-write`, which tests one step on a known setting and
  restores it.
- It locks the i2c device while it works, the same way ddcutil does, so the two
  don't interleave commands.

## License

MIT OR Apache-2.0, at your option.
