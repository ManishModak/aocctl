// Panel item: display icon and brightness. Click opens the popup, the wheel changes brightness.
import QtQuick
import QtQuick.Layouts
import org.kde.plasma.plasmoid
import org.kde.plasma.components as PlasmaComponents3
import org.kde.kirigami as Kirigami

MouseArea {
    id: compact

    property int wheelDelta: 0

    Layout.minimumWidth: row.implicitWidth
    acceptedButtons: Qt.LeftButton
    hoverEnabled: true
    onClicked: root.expanded = !root.expanded

    // One step per 120 units, so touchpads and high-resolution wheels don't race.
    onWheel: wheel => {
        wheelDelta += wheel.angleDelta.y || wheel.angleDelta.x;
        const steps = Math.trunc(wheelDelta / 120);
        if (steps !== 0) {
            wheelDelta -= steps * 120;
            root.nudgeBrightness(steps);
        }
    }

    RowLayout {
        id: row
        anchors.fill: parent
        spacing: Kirigami.Units.smallSpacing

        Kirigami.Icon {
            Layout.fillHeight: true
            Layout.preferredWidth: height
            source: root.brightness >= 0 ? "brightness-high" : "video-display"
            active: compact.containsMouse
        }
        PlasmaComponents3.Label {
            text: root.brightness >= 0 ? i18nc("brightness percent", "%1%", root.brightness) : "--"
            opacity: root.error ? 0.6 : 1
        }
    }
}
