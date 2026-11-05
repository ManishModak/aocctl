// Popup: a slider per number setting, a choice per mode setting, and a button per preset.
// Sliders write after a short pause in dragging; choices and presets write at once.
import QtQuick
import QtQuick.Layouts
import org.kde.plasma.components as PlasmaComponents3
import org.kde.plasma.extras as PlasmaExtras
import org.kde.kirigami as Kirigami

PlasmaExtras.Representation {
    id: full

    Layout.minimumWidth: Kirigami.Units.gridUnit * 20
    Layout.minimumHeight: Kirigami.Units.gridUnit * 16
    Layout.preferredHeight: Kirigami.Units.gridUnit * 26

    header: PlasmaExtras.PlasmoidHeading {
        RowLayout {
            anchors.fill: parent
            Kirigami.Heading {
                Layout.fillWidth: true
                level: 3
                text: root.model ? i18n("%1 on %2", root.model, root.display) : i18n("AOC monitor")
                elide: Text.ElideRight
            }
            PlasmaComponents3.BusyIndicator {
                Layout.preferredHeight: Kirigami.Units.iconSizes.small
                Layout.preferredWidth: Kirigami.Units.iconSizes.small
                visible: root.loading || root.writing !== ""
            }
            PlasmaComponents3.ToolButton {
                icon.name: "view-refresh"
                onClicked: root.refreshAll()
                PlasmaComponents3.ToolTip.text: i18n("Read the monitor again")
                PlasmaComponents3.ToolTip.visible: hovered
            }
        }
    }

    PlasmaComponents3.ScrollView {
        id: scroll
        anchors.fill: parent
        PlasmaComponents3.ScrollBar.horizontal.policy: PlasmaComponents3.ScrollBar.AlwaysOff

        ColumnLayout {
            width: scroll.availableWidth
            spacing: Kirigami.Units.smallSpacing

            Kirigami.InlineMessage {
                Layout.fillWidth: true
                visible: root.error !== ""
                type: Kirigami.MessageType.Warning
                text: root.error
            }

            Repeater {
                model: root.settings
                delegate: SettingRow {
                    Layout.fillWidth: true
                    setting: modelData
                }
            }

            Kirigami.Heading {
                Layout.topMargin: Kirigami.Units.largeSpacing
                level: 4
                text: i18n("Presets")
                visible: root.settings.length > 0
            }
            PlasmaComponents3.Label {
                Layout.fillWidth: true
                visible: root.settings.length > 0 && root.presets.length === 0
                wrapMode: Text.Wrap
                opacity: 0.7
                text: i18n("No presets yet. Save one with: aocctl preset save <name>")
            }
            Flow {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Repeater {
                    model: root.presets
                    delegate: PlasmaComponents3.Button {
                        text: modelData.name
                        enabled: !modelData.error && !root.loading
                        onClicked: root.applyPreset(modelData.name)
                        PlasmaComponents3.ToolTip.text: modelData.error
                            || modelData.settings.map(s => s.id + " " + (s.label || s.value)).join(", ")
                        PlasmaComponents3.ToolTip.visible: hovered
                    }
                }
            }
        }
    }
}
