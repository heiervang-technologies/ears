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
import QtQuick.Shapes
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

    // Smoothed level: rises fast with speech, settles slowly, like liquid.
    property real swell: 0
    property real clock: 0
    // Runs only while there is a wave to move; at rest nothing is redrawn.
    Timer {
        interval: 16
        repeat: true
        running: root.open && (root.level > 0.02 || root.swell > 0.005)
        onTriggered: {
            const target = root.level;
            root.swell += (target - root.swell) * (target > root.swell ? 0.35 : 0.06);
            if (root.swell < 0.005 && target <= 0.02)
                root.swell = 0;
            root.clock += 0.016 * (1 + 2 * root.swell);
        }
    }

    Variants {
        model: Quickshell.screens
        PanelWindow {
            id: edge
            required property var modelData
            screen: modelData
            anchors.bottom: true
            anchors.left: true
            anchors.right: true
            implicitHeight: 64
            color: "transparent"
            exclusionMode: ExclusionMode.Ignore
            WlrLayershell.layer: WlrLayer.Overlay
            WlrLayershell.namespace: "ears-led"
            WlrLayershell.keyboardFocus: WlrKeyboardFocus.None
            mask: Region {}

            // One wave: the top of a liquid surface along the bottom edge.
            // Sums of slow sines at different speeds, so it never repeats,
            // swelling in places that drift along the edge.
            function surface(layer) {
                const w = edge.width, h = edge.height, t = root.clock;
                const amp = root.swell * (h - 8) * [1.0, 0.75, 0.5][layer];
                const base = root.open ? 3 : 2;
                const pts = [Qt.point(0, h)];
                const n = 96;
                for (let i = 0; i <= n; i++) {
                    const x = w * i / n, u = i / n;
                    const swellAt = 0.55 + 0.45 * Math.sin(u * 5.1 + t * (0.7 + 0.2 * layer) + layer * 2.1);
                    const wave = 0.5 + 0.5 * (0.6 * Math.sin(u * (17 + 6 * layer) - t * (2.3 + layer))
                                             + 0.4 * Math.sin(u * (31 - 5 * layer) + t * (1.7 + 0.6 * layer)));
                    pts.push(Qt.point(x, h - base - amp * swellAt * wave));
                }
                pts.push(Qt.point(w, h));
                return pts;
            }

            Shape {
                anchors.fill: parent
                preferredRendererType: Shape.CurveRenderer
                ShapePath {
                    strokeWidth: 0
                    strokeColor: "transparent"
                    fillGradient: LinearGradient {
                        x1: 0; y1: 0; x2: 0; y2: edge.height
                        GradientStop { position: 0.0; color: Qt.alpha(root.tone, 0.10) }
                        GradientStop { position: 1.0; color: Qt.alpha(root.tone, 0.35) }
                    }
                    PathPolyline { path: { root.clock; root.open; edge.width; return edge.surface(2); } }
                }
                ShapePath {
                    strokeWidth: 0
                    strokeColor: "transparent"
                    fillGradient: LinearGradient {
                        x1: 0; y1: 0; x2: 0; y2: edge.height
                        GradientStop { position: 0.0; color: Qt.alpha(root.tone, 0.15) }
                        GradientStop { position: 1.0; color: Qt.alpha(root.tone, 0.45) }
                    }
                    PathPolyline { path: { root.clock; root.open; edge.width; return edge.surface(1); } }
                }
                ShapePath {
                    strokeWidth: root.open ? 2 : 1.5
                    strokeColor: Qt.alpha(root.tone, root.open ? 0.95 : 0.5)
                    fillGradient: LinearGradient {
                        x1: 0; y1: 0; x2: 0; y2: edge.height
                        GradientStop { position: 0.0; color: Qt.alpha(root.tone, 0.25) }
                        GradientStop { position: 1.0; color: Qt.alpha(root.tone, 0.6) }
                    }
                    PathPolyline { path: { root.clock; root.open; edge.width; return edge.surface(0); } }
                }
            }
        }
    }
}
