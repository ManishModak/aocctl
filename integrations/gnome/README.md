# GNOME Shell extension

A brightness slider for your AOC monitor in Quick Settings. The slider's menu
lists your [presets](../../README.md#presets); pick one to apply it.

## Requirements

- GNOME Shell 46 to 49.
- `aocctl` on `PATH`, with access to your monitor's i2c bus (see the aocctl
  README). Check `aocctl get brightness` works first.
- One AOC monitor. With more than one, aocctl needs `--display`, and the
  slider shows aocctl's message instead.

## Install

```sh
cp -r aocctl@manishmodak.github.io ~/.local/share/gnome-shell/extensions/
```

Log out and back in (on Wayland, GNOME loads new extensions only at login),
then:

```sh
gnome-extensions enable aocctl@manishmodak.github.io
```

## Usage

- The slider reads the monitor's brightness each time you open Quick
  Settings. Moving it writes the new value after a short pause, so a drag is
  one or two writes.
- The arrow next to the slider opens its menu: the monitor's model and your
  presets. Presets with a bad value are greyed out.
- When aocctl can't do something (the monitor's bus is busy, no access, a
  value didn't read back), the menu shows aocctl's message under the title.

The extension runs only `aocctl`: `get brightness` when Quick Settings opens,
`preset list` when the slider's menu opens, and `set` or `preset apply` when
you change something.
