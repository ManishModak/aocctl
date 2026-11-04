// AOC Monitor: a brightness slider in Quick Settings for an AOC monitor, with the
// presets saved for its model in the slider's menu. Everything goes through
// `aocctl ... --json`, run as an async subprocess so the shell never waits on the monitor.
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import Gio from 'gi://Gio';

import {Extension, gettext as _} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import {QuickSlider, SystemIndicator} from 'resource:///org/gnome/shell/ui/quickSettings.js';

Gio._promisify(Gio.Subprocess.prototype, 'communicate_utf8_async');

// A slider write waits this long after the last move, so a drag ends in one or two writes.
const WRITE_DELAY_MS = 300;

// Runs `aocctl <args> --json` and returns its JSON. Throws with aocctl's message when the
// command couldn't run; a write that didn't read back as sent still returns its JSON.
async function aocctl(args, cancellable) {
    let proc;
    try {
        proc = Gio.Subprocess.new(['aocctl', ...args, '--json'],
            Gio.SubprocessFlags.STDOUT_PIPE | Gio.SubprocessFlags.STDERR_PIPE);
    } catch {
        throw new Error(_('aocctl isn’t installed'));
    }
    const [stdout, stderr] = await proc.communicate_utf8_async(null, cancellable);
    let data;
    try {
        data = JSON.parse(stdout);
    } catch {
        throw new Error(stderr.trim() || _('aocctl failed'));
    }
    if (data.error)
        throw new Error(data.error);
    return data;
}

const BrightnessSlider = GObject.registerClass(
class BrightnessSlider extends QuickSlider {
    _init() {
        super._init({iconName: 'display-brightness-symbolic', menuEnabled: true});
        this._cancellable = new Gio.Cancellable();
        this._range = {min: 0, max: 100};
        this._value = undefined;
        this._title = _('AOC Monitor');
        this._target = null;
        this._writing = false;
        this._timeoutId = 0;

        this.slider.accessible_name = _('AOC monitor brightness');
        this._sliderId = this.slider.connect('notify::value', () => this._onMoved());

        this._setError('');
        this._presets = new PopupMenu.PopupMenuSection();
        this.menu.addMenuItem(this._presets);
        this.menu.connect('open-state-changed', (_menu, open) => {
            if (open)
                this._loadPresets().catch(logError);
        });

        this.connect('destroy', () => {
            this._cancellable.cancel();
            if (this._timeoutId)
                GLib.source_remove(this._timeoutId);
            this._timeoutId = 0;
        });
    }

    // Shows an error under the menu's title; '' clears it. The slider stays greyed out
    // until brightness has been read once.
    _setError(message) {
        this.menu.setHeader('video-display-symbolic', this._title, message);
        this.slider.reactive = this._value !== undefined;
    }

    // Reads brightness from the monitor, unless a change of ours hasn't reached it yet.
    async refresh() {
        if (this._writing || this._target !== null)
            return;
        try {
            const row = (await aocctl(['get', 'brightness'], this._cancellable)).settings[0];
            if (row.error)
                throw new Error(row.error);
            if (this._writing || this._target !== null)
                return;
            const domain = row.domain ?? {min: 0, max: row.max};
            this._range = {min: domain.min, max: Math.min(domain.max, row.max ?? domain.max)};
            this._value = row.value;
            this.slider.block_signal_handler(this._sliderId);
            this.slider.value = (row.value - this._range.min) / Math.max(1, this._range.max - this._range.min);
            this.slider.unblock_signal_handler(this._sliderId);
            this._setError('');
        } catch (e) {
            if (!e.matches?.(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED))
                this._setError(e.message);
        }
    }

    _onMoved() {
        const {min, max} = this._range;
        this._target = Math.round(min + this.slider.value * (max - min));
        if (this._timeoutId)
            GLib.source_remove(this._timeoutId);
        this._timeoutId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, WRITE_DELAY_MS, () => {
            this._timeoutId = 0;
            this._write().catch(logError);
            return GLib.SOURCE_REMOVE;
        });
    }

    // Sends the newest target; one write at a time, then the next target if the slider moved on.
    async _write() {
        if (this._writing || this._target === null)
            return;
        const value = this._target;
        this._target = null;
        this._writing = true;
        try {
            const r = await aocctl(['set', 'brightness', String(value)], this._cancellable);
            if (r.status === 'verified') {
                this._value = value;
                this._setError('');
            }
            else {
                this._setError(_('Sent %d%%, but the monitor reads %s').format(value, String(r.read_back ?? '?')));
            }
        } catch (e) {
            if (e.matches?.(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED))
                return;
            this._setError(e.message);
        } finally {
            this._writing = false;
        }
        if (this._target !== null && !this._timeoutId)
            await this._write();
    }

    async _loadPresets() {
        this._presets.removeAll();
        let data;
        try {
            data = await aocctl(['preset', 'list'], this._cancellable);
        } catch (e) {
            if (!e.matches?.(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED))
                this._setError(e.message);
            return;
        }
        this._title = `${data.model} (${data.display})`;
        this._setError('');
        this._presets.removeAll();
        if (data.presets.length === 0) {
            const item = new PopupMenu.PopupMenuItem(_('No presets yet. Save one with “aocctl preset save”.'));
            item.setSensitive(false);
            this._presets.addMenuItem(item);
        }
        for (const preset of data.presets) {
            const item = new PopupMenu.PopupMenuItem(preset.name);
            if (preset.error)
                item.setSensitive(false);
            item.connect('activate', () => this._apply(preset.name).catch(logError));
            this._presets.addMenuItem(item);
        }
    }

    async _apply(name) {
        try {
            const r = await aocctl(['preset', 'apply', name], this._cancellable);
            const failed = r.settings.filter(s => s.status !== 'unchanged' && s.status !== 'verified');
            this._setError(failed.length === 0 ? ''
                : _('%s didn’t fully apply: %s').format(name,
                    failed.map(s => `${s.id} ${s.error ?? s.status}`).join(', ')));
        } catch (e) {
            if (e.matches?.(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED))
                return;
            this._setError(e.message);
        }
        await this.refresh();
    }
});

export default class AocMonitorExtension extends Extension {
    enable() {
        this._slider = new BrightnessSlider();
        this._indicator = new SystemIndicator();
        this._indicator.quickSettingsItems.push(this._slider);
        Main.panel.statusArea.quickSettings.addExternalIndicator(this._indicator, 2);

        // Brightness can change from the monitor's own menu, so read it whenever Quick Settings opens.
        this._menuId = Main.panel.statusArea.quickSettings.menu.connect('open-state-changed', (_menu, open) => {
            if (open)
                this._slider.refresh().catch(logError);
        });
        this._slider.refresh().catch(logError);
    }

    disable() {
        Main.panel.statusArea.quickSettings.menu.disconnect(this._menuId);
        this._indicator.quickSettingsItems.forEach(item => item.destroy());
        this._indicator.destroy();
        this._indicator = null;
        this._slider = null;
    }
}
