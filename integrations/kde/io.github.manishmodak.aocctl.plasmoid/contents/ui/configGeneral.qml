// Settings page: where aocctl is and which monitor to control.
import QtQuick
import QtQuick.Controls as QQC2
import org.kde.kirigami as Kirigami
import org.kde.kcmutils as KCM

KCM.SimpleKCM {
    property alias cfg_aocctlPath: pathField.text
    property alias cfg_display: displayField.text

    Kirigami.FormLayout {
        QQC2.TextField {
            id: pathField
            Kirigami.FormData.label: i18n("aocctl command:")
            placeholderText: "aocctl"
        }
        QQC2.TextField {
            id: displayField
            Kirigami.FormData.label: i18n("Monitor:")
            placeholderText: i18n("DP-1 or serial; empty for the only AOC monitor")
        }
    }
}
