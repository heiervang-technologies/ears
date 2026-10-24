// The ears LED: a small light at the bottom centre of every screen that
// shows whether ears VAD has the mic open, and glows with your voice.
//
//   off        grey, no glow: nothing is recorded
//   listening  cyan, glowing with the mic level: what you say is typed
//   standby    violet, glowing with the mic level: the mic is open, but
//              only the wake phrase is acted on
//
// Run: qs -p contrib/ears-led/shell.qml  (ears-led-feed must sit next to it)
// Click-through; it never takes input or focus.
import QtQuick
import Quickshell
import Quickshell.Io
import Quickshell.Wayland

ShellRoot {
    id: root
    property string state: "off"
    property real level: 0

    readonly property color tone: state === "listening" ? "#22d3ee"
                                : state === "standby" ? "#8b7bd8"
                                : "#6b7280"
    readonly property bool open: state !== "off"

    Process {
        id: feed
        command: ["python3", Qt.resolvedUrl("ears-led-feed").toString().replace("file://", "")]
        running: true
        stdout: SplitParser {
            onRead: line => {
                try {
                    const d = JSON.parse(line);
                    root.state = d.state;
                    root.level = d.level;
                } catch (e) {}
            }
        }
        // The feed only ends if it crashes: start it again.
        onRunningChanged: if (!running) restart.start()
    }
    Timer { id: restart; interval: 2000; onTriggered: feed.running = true }

    Variants {
        model: Quickshell.screens
        PanelWindow {
            required property var modelData
            screen: modelData
            anchors.bottom: true
            implicitWidth: 220
            implicitHeight: 30
            color: "transparent"
            exclusionMode: ExclusionMode.Ignore
            WlrLayershell.layer: WlrLayer.Overlay
            WlrLayershell.namespace: "ears-led"
            WlrLayershell.keyboardFocus: WlrKeyboardFocus.None
            mask: Region {}

            Item {
                id: led
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.bottom: parent.bottom
                anchors.bottomMargin: 5
                width: root.open ? 64 : 40
                height: 4
                Behavior on width { NumberAnimation { duration: 250; easing.type: Easing.OutCubic } }

                // Glow: soft rings around the light, brighter as you speak.
                property real glow: root.open ? 0.22 + 0.78 * root.level : 0
                Behavior on glow { NumberAnimation { duration: 90 } }

                Repeater {
                    model: [18, 14, 11, 8, 5, 3]
                    Rectangle {
                        required property int modelData
                        anchors.centerIn: parent
                        width: led.width + modelData * 2
                        height: led.height + modelData * 2
                        radius: height / 2
                        color: root.tone
                        opacity: led.glow * (0.06 + 0.25 / modelData)
                        visible: opacity > 0.01
                    }
                }
                Rectangle {
                    anchors.fill: parent
                    radius: height / 2
                    color: root.tone
                    opacity: root.open ? 0.75 + 0.25 * root.level : 0.55
                    Behavior on color { ColorAnimation { duration: 250 } }
                }
            }
        }
    }
}
