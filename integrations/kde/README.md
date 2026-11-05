# KDE Plasma widget

A Plasma 6 widget for your AOC monitor: brightness in the panel (scroll on it
to change it), and a popup with every setting aocctl can write and your
[presets](../../README.md#presets).

## Requirements

- KDE Plasma 6.
- `aocctl` on `PATH`, with access to your monitor's i2c bus (see the aocctl
  README). Check `aocctl get` works first.

## Install

```sh
kpackagetool6 --type Plasma/Applet --install io.github.manishmodak.aocctl.plasmoid
```

(`--upgrade` instead of `--install` for a newer copy.) Then right-click the
panel, choose Add Widgets, and add "AOC Monitor". To remove it:
`kpackagetool6 --type Plasma/Applet --remove io.github.manishmodak.aocctl.plasmoid`.

## Usage

- The panel shows the monitor's brightness. Scroll on it to change it in 5%
  steps; the write goes out after a short pause, so a quick scroll is one
  write.
- Click it for the popup: a slider for each number setting, a choice for each
  mode setting, and a button per preset (hover one to see its values). Sliders
  write when you stop moving them; choices and presets write at once. After
  each change the popup reads every setting again, since some modes change
  others. Settings aocctl can't write yet on your model are shown as
  read-only.
- When aocctl can't do something (the monitor's bus is busy, no access, a
  value didn't read back), the popup shows aocctl's message.

In the widget's settings you can set the aocctl command and, with more than
one AOC monitor, which one to control (connector such as `DP-1`, or serial).
Add the widget twice for two monitors.

The widget runs only `aocctl`: `get brightness` once a minute, `get` and
`preset list` when the popup opens, and `set` or `preset apply` when you
change something.
