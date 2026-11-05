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
    property int brightness: -1
    property var brightnessDomain: ({ min: 0, max: 100 })
    // Why the last command failed, shown in the popup and the tooltip.
    property string error: ""
    property bool loading: false
    // Writes waiting to be sent: setting id -> value. One write runs at a time.
    property var queued: ({})
    property string writing: ""

    readonly property int step: 5

    Plasmoid.icon: "video-display"
    toolTipMainText: model || i18n("AOC monitor")
    toolTipSubText: error ? error
        : brightness >= 0 ? i18n("Brightness %1%. Scroll to change it.", brightness) : ""

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

    // Whether a change to the setting hasn't reached the monitor yet, so a read mustn't undo it on screen.
    function pending(id) {
        return writing === id || queued[id] !== undefined || (id === "brightness" && scrollTimer.running);
    }

    function refreshBrightness() {
        run(["get", "brightness"], (data, err) => {
            if (err) {
                error = err;
                brightness = -1;
                return;
            }
            const row = data.settings[0];
            display = data.display;
            if (row.error) {
                error = row.error;
            } else if (!pending("brightness")) {
                error = "";
                brightness = row.value;
                brightnessDomain = row.domain || { min: 0, max: row.max };
            }
        });
    }

    // Reads every setting and the presets (when the popup opens, and after each change,
    // since some modes change other settings).
    function refreshAll() {
        loading = true;
        run(["get"], (data, err) => {
            loading = false;
            if (err) {
                error = err;
                return;
            }
            error = "";
            display = data.display;
            settings = data.settings;
            const b = data.settings.find(s => s.id === "brightness");
            if (b && b.value !== undefined && !pending("brightness")) {
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

    // Queues a write; a newer value for the same setting replaces one not yet sent.
    function setValue(id, value) {
        const q = Object.assign({}, queued);
        q[id] = value;
        queued = q;
        if (id === "brightness") {
            brightness = value;
        }
        sendNext();
    }

    function sendNext() {
        if (writing) {
            return;
        }
        const ids = Object.keys(queued);
        if (ids.length === 0) {
            if (expanded) {
                refreshAll();
            } else {
                refreshBrightness();
            }
            return;
        }
        const id = ids[0];
        const value = queued[id];
        const q = Object.assign({}, queued);
        delete q[id];
        queued = q;
        writing = id;
        run(["set", id, String(value)], (data, err) => {
            writing = "";
            if (err) {
                error = err;
            } else if (data.status !== "verified") {
                error = data.status === "mismatch"
                    ? i18n("Sent %1 to %2, but it reads back %3", value, id, data.read_back)
                    : i18n("Sent %1 to %2, but couldn't read it back: %3", value, id, data.error);
            } else {
                error = "";
            }
            sendNext();
        });
    }

    // Scroll on the panel icon: steps from the newest value, written after a short pause.
    function nudgeBrightness(steps) {
        if (brightness < 0) {
            return;
        }
        const d = brightnessDomain;
        scrollTimer.value = Math.max(d.min, Math.min(d.max, brightness + steps * step));
        brightness = scrollTimer.value;
        scrollTimer.restart();
    }

    Timer {
        id: scrollTimer
        property int value: 0
        interval: 300
        onTriggered: root.setValue("brightness", value)
    }

    function applyPreset(name) {
        loading = true;
        run(["preset", "apply", name], (data, err) => {
            loading = false;
            if (err) {
                error = err;
            } else {
                const failed = data.settings.filter(s => s.status !== "unchanged" && s.status !== "verified");
                error = failed.length === 0 ? ""
                    : i18n("Preset %1 didn't fully apply: %2", name,
                           failed.map(s => s.id + " " + (s.error || s.status)).join(", "));
            }
            refreshAll();
        });
    }
}
