// The ears LED: the bottom edge of every screen lights up to
// show whether ears VAD has the mic open, and glows with your voice.
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
            anchors.left: true
            anchors.right: true
            implicitHeight: 28
            color: "transparent"
            exclusionMode: ExclusionMode.Ignore
            WlrLayershell.layer: WlrLayer.Overlay
            WlrLayershell.namespace: "ears-led"
            WlrLayershell.keyboardFocus: WlrKeyboardFocus.None
            mask: Region {}

            // Glow: rises from the edge, brighter as you speak.
            property real glow: root.open ? 0.25 + 0.75 * root.level : 0
            Behavior on glow { NumberAnimation { duration: 90 } }

            Rectangle {
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                height: parent.height
                opacity: parent.glow
                visible: opacity > 0.01
                gradient: Gradient {
                    GradientStop { position: 0.0; color: "transparent" }
                    GradientStop { position: 0.7; color: Qt.alpha(root.tone, 0.18) }
                    GradientStop { position: 1.0; color: Qt.alpha(root.tone, 0.55) }
                }
            }
            // The light itself: the whole bottom edge.
            Rectangle {
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                height: root.open ? 3 : 2
                color: root.tone
                opacity: root.open ? 0.7 + 0.3 * root.level : 0.5
                Behavior on color { ColorAnimation { duration: 250 } }
            }
        }
    }
}
