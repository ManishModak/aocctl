// AOC Monitor plasmoid: brightness in the panel (scroll to change it), and a popup
// with every setting aocctl can write and the presets saved for the monitor's model.
// All monitor access goes through `aocctl ... --json`, run asynchronously by the
// executable data engine, so the shell never waits on the monitor.
import QtQuick
import org.kde.plasma.plasmoid
import org.kde.plasma.plasma5support as P5Support

PlasmoidItem {
    id: root

    // `aocctl get --json` rows: {id, name, value, max, label, writable, domain} or {id, name, domain, error}.
    property var settings: []
    // `aocctl preset list --json` presets: {name, settings, error}.
    property var presets: []
    property string model: ""
    property string display: ""
    // Brightness as last read from the monitor (-1: couldn't read it).
    property int brightness: -1
    property var brightnessDomain: ({ min: 0, max: 100 })
    // Brightness to show: the newest value asked for, else the last read.
    readonly property int shownBrightness: {
        const r = requested("brightness");
        return r !== undefined ? r : brightness;
    }
    // Why the last read, or the last change, failed. A change's error stays until a change succeeds.
    property string readError: ""
    property string writeError: ""
    readonly property string error: writeError || readError
    property bool loading: false
    // Slider and wheel moves waiting for a pause: setting id -> value.
    property var edits: ({})
    // Changes in the order asked for: {id, value} or {preset}. One runs at a time (`inFlight`).
    property var ops: []
    property var inFlight: null

    readonly property int step: 5

    Plasmoid.icon: "video-display"
    toolTipMainText: model || i18n("AOC monitor")
    toolTipSubText: error ? error
        : shownBrightness >= 0 ? i18n("Brightness %1%. Scroll to change it.", shownBrightness) : ""

    compactRepresentation: CompactRepresentation {}
    fullRepresentation: FullRepresentation {}

    onExpandedChanged: if (expanded) refreshAll()
    Component.onCompleted: refreshBrightness()

    // Brightness can also change from the monitor's own menu, so the panel reads it now and then.
    Timer {
        interval: 60000
        running: true
        repeat: true
        onTriggered: if (!root.expanded) root.refreshBrightness()
    }

    // Runs `aocctl <args> --json` and calls done(data, error): aocctl's JSON (also for a write
    // that didn't read back as sent) or null plus a message when the command couldn't run.
    P5Support.DataSource {
        id: executable
        engine: "executable"
        property var callbacks: ({})
        property int serial: 0

        onNewData: (source, data) => {
            const done = callbacks[source];
            delete callbacks[source];
            disconnectSource(source);
            if (!done) {
                return;
            }
            let parsed = null;
            try {
                parsed = JSON.parse(data.stdout);
            } catch (e) {
            }
            if (parsed && parsed.error) {
                done(null, parsed.error);
            } else if (parsed) {
                done(parsed, "");
            } else if (data["exit code"] === 127) {
                done(null, i18n("aocctl isn't installed (or not at \"%1\")", root.command()));
            } else {
                done(null, (data.stderr || "").trim() || i18n("aocctl failed (exit code %1)", data["exit code"]));
            }
        }
    }

    function command() {
        return Plasmoid.configuration.aocctlPath || "aocctl";
    }

    function quote(s) {
        return "'" + String(s).replace(/'/g, "'\\''") + "'";
    }

    function run(args, done) {
        let argv = args.concat(["--json"]);
        if (Plasmoid.configuration.display) {
            argv = argv.concat(["--display", Plasmoid.configuration.display]);
        }
        // The engine keys sources by command line; the trailing comment keeps calls apart.
        const cmd = quote(command()) + " " + argv.map(quote).join(" ") + " # " + executable.serial++;
        executable.callbacks[cmd] = done;
        executable.connectSource(cmd);
    }

    // The newest value asked for a setting that isn't confirmed yet, or undefined. Controls show
    // it over the last read, so a read that lands before a write doesn't move them back.
    function requested(id) {
        if (edits[id] !== undefined) {
            return edits[id];
        }
        for (let i = ops.length - 1; i >= 0; i--) {
            if (ops[i].id === id) {
                return ops[i].value;
            }
        }
        return inFlight && inFlight.id === id ? inFlight.value : undefined;
    }

    function refreshBrightness() {
        run(["get", "brightness"], (data, err) => {
            const row = data ? data.settings[0] : null;
            if (data) {
                display = data.display;
            }
            if (err || row.error) {
                readError = err || row.error;
                brightness = -1;
                return;
            }
            readError = "";
            brightness = row.value;
            brightnessDomain = row.domain || { min: 0, max: row.max };
        });
    }

    // Reads every setting and the presets (when the popup opens, and after each change,
    // since some modes change other settings).
    function refreshAll() {
        loading = true;
        run(["get"], (data, err) => {
            loading = false;
            if (err) {
                readError = err;
                return;
            }
            readError = "";
            display = data.display;
            settings = data.settings;
            const b = data.settings.find(s => s.id === "brightness");
            if (b && b.value !== undefined) {
                brightness = b.value;
                brightnessDomain = b.domain || { min: 0, max: b.max };
            }
        });
        run(["preset", "list"], (data, err) => {
            if (data) {
                presets = data.presets;
                model = data.model;
            }
        });
    }

    // Slider or wheel move: written after a short pause, so a drag or a quick scroll is one write.
    function edit(id, value) {
        const e = Object.assign({}, edits);
        e[id] = value;
        edits = e;
        editTimer.restart();
    }

    Timer {
        id: editTimer
        interval: 300
        onTriggered: root.flushEdits()
    }

    function flushEdits() {
        editTimer.stop();
        const e = edits;
        edits = {};
        for (const id in e) {
            enqueue({ id: id, value: e[id] });
        }
    }

    // A choice: written at once, after any moves made before it.
    function setValue(id, value) {
        flushEdits();
        enqueue({ id: id, value: value });
    }

    function applyPreset(name) {
        flushEdits();
        enqueue({ preset: name });
    }

    // Adds a change; a newer value replaces the last queued one when it's for the same setting.
    function enqueue(op) {
        const list = ops.slice();
        const last = list[list.length - 1];
        if (op.id !== undefined && last && last.id === op.id) {
            list[list.length - 1] = op;
        } else {
            list.push(op);
        }
        ops = list;
        sendNext();
    }

    function sendNext() {
        if (inFlight) {
            return;
        }
        if (ops.length === 0) {
            if (expanded) {
                refreshAll();
            } else {
                refreshBrightness();
            }
            return;
        }
        const op = ops[0];
        ops = ops.slice(1);
        inFlight = op;
        if (op.preset !== undefined) {
            run(["preset", "apply", op.preset], (data, err) => {
                const failed = data ? data.settings.filter(s => s.status !== "unchanged" && s.status !== "verified") : [];
                writeError = err ? err
                    : failed.length === 0 ? ""
                    : i18n("Preset %1 didn't fully apply: %2", op.preset,
                           failed.map(s => s.id + " " + (s.error || s.status)).join(", "));
                inFlight = null;
                sendNext();
            });
            return;
        }
        run(["set", op.id, String(op.value)], (data, err) => {
            writeError = err ? err
                : data.status === "verified" ? ""
                : data.status === "mismatch" ? i18n("Sent %1 to %2, but it reads back %3", op.value, op.id, data.read_back)
                : i18n("Sent %1 to %2, but couldn't read it back: %3", op.value, op.id, data.error);
            inFlight = null;
            sendNext();
        });
    }

    // Scroll on the panel icon: steps from the newest value.
    function nudgeBrightness(steps) {
        if (shownBrightness < 0) {
            return;
        }
        const d = brightnessDomain;
        edit("brightness", Math.max(d.min, Math.min(d.max, shownBrightness + steps * step)));
    }
}
