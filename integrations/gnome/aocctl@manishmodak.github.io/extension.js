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

const isCancelled = e => e.matches?.(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED);

const BrightnessSlider = GObject.registerClass(
class BrightnessSlider extends QuickSlider {
    _init() {
        super._init({iconName: 'display-brightness-symbolic', menuEnabled: true});
        this._cancellable = new Gio.Cancellable();
        this._range = {min: 0, max: 100};
        this._value = undefined;
        this._title = _('AOC Monitor');
        // Why the last read, or the last change, failed. A change's error stays until a change succeeds.
        this._readError = '';
        this._writeError = '';
        // Changes (slider writes, presets) run one at a time, in the order asked for.
        this._queue = Promise.resolve();
        this._queued = 0;
        this._destroyed = false;
        // The slider's newest value, until it's queued.
        this._target = null;
        this._timeoutId = 0;

        this.slider.accessible_name = _('AOC monitor brightness');
        this._sliderId = this.slider.connect('notify::value', () => this._onMoved());

        this._showStatus();
        this._presets = new PopupMenu.PopupMenuSection();
        this.menu.addMenuItem(this._presets);
        this.menu.connect('open-state-changed', (_menu, open) => {
            if (open)
                this._loadPresets().catch(logError);
        });

        this.connect('destroy', () => {
            this._destroyed = true;
            this._cancellable.cancel();
            if (this._timeoutId)
                GLib.source_remove(this._timeoutId);
            this._timeoutId = 0;
        });
    }

    // Shows the current error under the menu's title. The slider stays greyed out until
    // brightness has been read once.
    _showStatus() {
        this.menu.setHeader('video-display-symbolic', this._title, this._writeError || this._readError);
        this.slider.reactive = this._value !== undefined;
    }

    // Reads brightness from the monitor, unless a change of ours hasn't reached it yet.
    async refresh() {
        if (this._queued > 0 || this._target !== null)
            return;
        await this._read(0);
    }

    // `own`: queued changes that belong to the caller (a preset reading back its result).
    async _read(own) {
        try {
            const row = (await aocctl(['get', 'brightness'], this._cancellable)).settings[0];
            if (row.error)
                throw new Error(row.error);
            this._readError = '';
            // A change queued meanwhile wins over this reading.
            if (this._queued <= own && this._target === null) {
                const domain = row.domain ?? {min: 0, max: row.max};
                this._range = {min: domain.min, max: Math.min(domain.max, row.max ?? domain.max)};
                this._value = row.value;
                this.slider.block_signal_handler(this._sliderId);
                this.slider.value = (row.value - this._range.min) / Math.max(1, this._range.max - this._range.min);
                this.slider.unblock_signal_handler(this._sliderId);
            }
        } catch (e) {
            if (isCancelled(e))
                return;
            this._readError = e.message;
        }
        this._showStatus();
    }

    // Runs fn after the changes already queued. Changes still queued when the slider is
    // destroyed are dropped; one already running finishes.
    _enqueue(fn) {
        this._queued++;
        this._queue = this._queue
            .then(() => this._destroyed ? null : fn())
            .catch(logError)
            .then(() => this._queued--);
    }

    // Queues a write of the slider's newest value, in order with presets.
    _queueWrite() {
        if (this._target === null)
            return;
        const value = this._target;
        this._target = null;
        this._enqueue(() => this._write(value));
    }

    _onMoved() {
        const {min, max} = this._range;
        this._target = Math.round(min + this.slider.value * (max - min));
        if (this._timeoutId)
            GLib.source_remove(this._timeoutId);
        this._timeoutId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, WRITE_DELAY_MS, () => {
            this._timeoutId = 0;
            this._queueWrite();
            return GLib.SOURCE_REMOVE;
        });
    }

    // Queues the slider's newest value now instead of after the pause.
    _flushMove() {
        if (!this._timeoutId)
            return;
        GLib.source_remove(this._timeoutId);
        this._timeoutId = 0;
        this._queueWrite();
    }

    async _write(value) {
        try {
            const r = await aocctl(['set', 'brightness', String(value)], this._cancellable);
            if (r.status === 'verified') {
                this._value = value;
                this._writeError = '';
            } else {
                this._writeError = _('Sent %d%%, but the monitor reads %s').format(value, String(r.read_back ?? '?'));
            }
        } catch (e) {
            if (isCancelled(e))
                return;
            this._writeError = e.message;
        }
        this._showStatus();
    }

    async _loadPresets() {
        this._presets.removeAll();
        let data;
        try {
            data = await aocctl(['preset', 'list'], this._cancellable);
        } catch (e) {
            if (!isCancelled(e)) {
                this._readError = e.message;
                this._showStatus();
            }
            return;
        }
        this._title = `${data.model} (${data.display})`;
        this._readError = '';
        this._showStatus();
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
            item.connect('activate', () => this._apply(preset.name));
            this._presets.addMenuItem(item);
        }
    }

    // Applies a preset after any slider move made before it.
    _apply(name) {
        this._flushMove();
        this._enqueue(async () => {
            try {
                const r = await aocctl(['preset', 'apply', name], this._cancellable);
                const failed = r.settings.filter(s => s.status !== 'unchanged' && s.status !== 'verified');
                this._writeError = failed.length === 0 ? ''
                    : _('%s didn’t fully apply: %s').format(name,
                        failed.map(s => `${s.id} ${s.error ?? s.status}`).join(', '));
            } catch (e) {
                if (isCancelled(e))
                    return;
                this._writeError = e.message;
            }
            // Read back what the preset did, unless more changes are waiting.
            if (this._queued === 1 && this._target === null)
                await this._read(1);
            else
                this._showStatus();
        });
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
