// One setting from `aocctl get --json`: a slider for {min, max}, a combo box for {options},
// or text when it's read-only or couldn't be read.
import QtQuick
import QtQuick.Layouts
import org.kde.plasma.components as PlasmaComponents3
import org.kde.kirigami as Kirigami

RowLayout {
    id: row

    property var setting
    readonly property var domain: setting.domain || ({})
    readonly property bool readable: setting.value !== undefined
    readonly property bool isRange: domain.max !== undefined
    readonly property bool isChoice: domain.options !== undefined
    readonly property bool editable: readable && setting.writable && (isRange || isChoice)

    spacing: Kirigami.Units.smallSpacing

    PlasmaComponents3.Label {
        Layout.preferredWidth: Kirigami.Units.gridUnit * 7
        text: row.setting.name || row.setting.id
        elide: Text.ElideRight
    }

    PlasmaComponents3.Slider {
        id: slider
        Layout.fillWidth: true
        visible: row.editable && row.isRange
        from: row.domain.min || 0
        // The monitor may report a lower maximum than the setting allows.
        to: Math.min(row.domain.max || 0, row.setting.max === undefined ? row.domain.max : row.setting.max)
        stepSize: 1
        value: row.setting.value || 0
        onMoved: writeTimer.restart()

        Timer {
            id: writeTimer
            interval: 300
            onTriggered: {
                if (slider.pressed) {
                    restart();
                } else if (slider.value !== row.setting.value) {
                    root.setValue(row.setting.id, slider.value);
                }
            }
        }
    }
    PlasmaComponents3.Label {
        Layout.preferredWidth: Kirigami.Units.gridUnit * 2
        horizontalAlignment: Text.AlignRight
        visible: slider.visible
        text: slider.value
    }

    PlasmaComponents3.ComboBox {
        Layout.fillWidth: true
        visible: row.editable && row.isChoice
        // The monitor may report fewer choices than the model's list.
        model: row.isChoice ? row.domain.options.slice(0, (row.setting.max ?? row.domain.options.length - 1) + 1) : []
        currentIndex: row.setting.value ?? -1
        onActivated: index => {
            if (index !== row.setting.value) {
                root.setValue(row.setting.id, index);
            }
        }
    }

    PlasmaComponents3.Label {
        Layout.fillWidth: true
        visible: !row.editable
        wrapMode: Text.Wrap
        opacity: 0.7
        text: !row.readable ? i18n("can't read: %1", row.setting.error)
            : (row.setting.label || row.setting.value) + "  " + i18n("(read-only)")
    }
}
