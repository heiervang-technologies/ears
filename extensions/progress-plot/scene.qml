// ears-responsiveness: HAIos desktop progress plot comparing live ears
// voice-to-action latency against the v1.2.0 baseline.
// Styled strictly with the HAIos design system (LiveTheme / Theme / ProgressRail / HaiButton).
import QtQuick
import QtQuick.Controls
import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import Quickshell.Hyprland

ShellRoot {
    id: root

    readonly property string runtimeDir: Quickshell.env("XDG_RUNTIME_DIR") || "/run/user/1000"
    readonly property string dataPath: Quickshell.env("EARS_METRICS_OUT") || (runtimeDir + "/ears/responsiveness.json")
    readonly property string metricsBin: Quickshell.env("EARS_METRICS_BIN") || "ears-metrics.py"

    property var payload: null
    property string metricMode: "e2e"       // "e2e" | "proc"
    property string kindFilter: "all"       // "all" | "dictation" | "command"
    property int hoveredIdx: -1
    property string snapStatus: ""

    readonly property var base: payload ? payload.baseline : null
    readonly property var live: payload ? payload.live : null
    readonly property var health: payload ? payload.health : null
    readonly property var allEvents: payload && payload.events ? payload.events : []
    readonly property var filteredEvents: {
        if (kindFilter === "all") return allEvents;
        return allEvents.filter(e => e.kind === kindFilter);
    }

    function fmtMs(v) {
        if (v === null || v === undefined || isNaN(v)) return "—";
        if (v < 10) return v.toFixed(1) + " ms";
        return Math.round(v) + " ms";
    }

    function fmtX(v) {
        if (v === null || v === undefined || isNaN(v)) return "—";
        return v.toFixed(2) + "×";
    }

    // Keep responsiveness.json continuously fresh while this overlay is open
    Process {
        id: watcher
        command: ["python3", root.metricsBin, "--watch"]
        running: true
    }

    Process {
        id: snapshotProc
        command: ["python3", root.metricsBin, "snapshot"]
        running: false
        onExited: (code) => {
            root.snapStatus = code === 0 ? "SNAPSHOT SAVED" : "SNAPSHOT FAILED";
            snapClearTimer.restart();
            feed.reload();
        }
    }
    Timer { id: snapClearTimer; interval: 2600; onTriggered: root.snapStatus = "" }

    FileView {
        id: feed
        path: root.dataPath
        watchChanges: true
        blockLoading: true
        printErrors: false
        onFileChanged: reload()
        onLoaded: {
            try {
                const parsed = JSON.parse(text());
                if (parsed && parsed.live) {
                    root.payload = parsed;
                }
            } catch (e) { /* atomic replace race: keep previous frame */ }
        }
        onLoadFailed: retryTimer.start()
    }
    Timer { id: retryTimer; interval: 1000; onTriggered: feed.reload() }

    // Reusable hairline glass tile
    component StatTile: Rectangle {
        property string eyebrow: ""
        property string value: "—"
        property string sub: ""
        property color tone: LiveTheme.cyan
        radius: LiveTheme.radiusSmall + 2
        color: LiveTheme.glass2
        border.width: LiveTheme.borderThin
        border.color: LiveTheme.cyanLine
        implicitHeight: 82

        Column {
            anchors { fill: parent; margins: 11 }
            spacing: 4
            Text {
                text: parent.parent.eyebrow
                color: LiveTheme.muted
                font { family: LiveTheme.fontMono; pixelSize: 10; weight: Font.Bold; letterSpacing: 1.4 }
                elide: Text.ElideRight; width: parent.width
            }
            Text {
                text: parent.parent.value
                color: parent.parent.tone
                font { family: LiveTheme.fontMono; pixelSize: 22; weight: Font.Bold }
                elide: Text.ElideRight; width: parent.width
            }
            Text {
                text: parent.parent.sub
                color: LiveTheme.faint
                font { family: LiveTheme.fontMono; pixelSize: 11 }
                elide: Text.ElideRight; width: parent.width
            }
        }
    }

    component ModeChip: Rectangle {
        id: chip
        property string label: ""
        property bool active: false
        property color activeTone: LiveTheme.cyan
        signal clicked()
        implicitWidth: chipTxt.implicitWidth + 18
        implicitHeight: 26
        radius: LiveTheme.radiusSmall
        color: active ? activeTone : (chipMouse.containsMouse ? LiveTheme.cyanTint : "transparent")
        border.width: LiveTheme.borderThin
        border.color: active ? activeTone : LiveTheme.cyanLine
        Text {
            id: chipTxt
            anchors.centerIn: parent
            text: chip.label
            color: chip.active ? LiveTheme.bg : LiveTheme.ink2
            font { family: LiveTheme.fontMono; pixelSize: 11; weight: Font.Bold; letterSpacing: 1.0 }
        }
        MouseArea {
            id: chipMouse
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: chip.clicked()
        }
    }

    PanelWindow {
        id: win
        readonly property string wantScreen: Quickshell.env("HAIOS_UI_SCREEN") || (Hyprland.focusedMonitor ? Hyprland.focusedMonitor.name : "HDMI-A-1")
        screen: Quickshell.screens.find(s => s.name === wantScreen) ?? Quickshell.screens[0]
        WlrLayershell.layer: WlrLayer.Top
        WlrLayershell.namespace: "haios-ears-progress"
        WlrLayershell.keyboardFocus: WlrKeyboardFocus.OnDemand
        exclusionMode: ExclusionMode.Ignore
        color: "transparent"
        implicitWidth: 960
        implicitHeight: 684

        Rectangle {
            id: card
            anchors.fill: parent
            radius: LiveTheme.radiusCard
            color: LiveTheme.glass
            border.width: LiveTheme.borderCard
            border.color: LiveTheme.cyan
            focus: true
            Keys.onEscapePressed: Qt.quit()

            // Subtle HAIos horizontal accent highlight along top border
            Rectangle {
                anchors { top: parent.top; left: parent.left; right: parent.right; margins: 2 }
                height: 2
                radius: 1
                gradient: Gradient {
                    orientation: Gradient.Horizontal
                    GradientStop { position: 0.0; color: "transparent" }
                    GradientStop { position: 0.25; color: LiveTheme.cyan }
                    GradientStop { position: 0.75; color: LiveTheme.ok }
                    GradientStop { position: 1.0; color: "transparent" }
                }
            }

            Column {
                id: mainCol
                anchors { fill: parent; margins: 20 }
                spacing: 13

                // ---- 1. Header bar ----
                Item {
                    width: parent.width
                    height: 44

                    Column {
                        anchors.left: parent.left
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 3
                        Row {
                            spacing: 10
                            Rectangle {
                                width: 8; height: 8; radius: 4
                                anchors.verticalCenter: parent.verticalCenter
                                color: root.health && root.health.asr_state === "ok" ? LiveTheme.ok : LiveTheme.cyan
                                SequentialAnimation on opacity {
                                    running: true; loops: Animation.Infinite
                                    NumberAnimation { from: 1.0; to: 0.35; duration: 1100; easing.type: Easing.InOutSine }
                                    NumberAnimation { from: 0.35; to: 1.0; duration: 1100; easing.type: Easing.InOutSine }
                                }
                            }
                            Text {
                                text: "EARS // RESPONSIVENESS PROGRESS PLOT"
                                color: LiveTheme.cyan
                                font { family: LiveTheme.fontMono; pixelSize: 13; weight: Font.Bold; letterSpacing: 2.0 }
                            }
                            Rectangle {
                                visible: root.live !== null
                                anchors.verticalCenter: parent.verticalCenter
                                radius: LiveTheme.radiusPill
                                color: Qt.rgba(LiveTheme.ok.r, LiveTheme.ok.g, LiveTheme.ok.b, 0.16)
                                border.width: 1; border.color: Qt.rgba(LiveTheme.ok.r, LiveTheme.ok.g, LiveTheme.ok.b, 0.55)
                                implicitWidth: badgeTxt.implicitWidth + 14; implicitHeight: 20
                                Text {
                                    id: badgeTxt
                                    anchors.centerIn: parent
                                    text: root.live ? ("−" + root.live.e2e_reduction_pct.toFixed(0) + "% E2E · " + root.fmtX(root.live.e2e_speedup) + " FASTER") : ""
                                    color: LiveTheme.ok
                                    font { family: LiveTheme.fontMono; pixelSize: 11; weight: Font.Bold; letterSpacing: 0.8 }
                                }
                            }
                        }
                        Text {
                            text: {
                                if (!root.live) return "Loading live telemetry from debug.log…";
                                const dev = root.health && root.health.device ? root.health.device.replace(/^alsa_input\.usb-/, "").slice(0, 34) : "pipewire";
                                const path = root.health && root.health.asr_path ? root.health.asr_path.toUpperCase() : "STREAM";
                                const extra = root.snapStatus ? ("  ·  " + root.snapStatus) : "";
                                return "LIVE " + path + "  ·  " + root.live.n_utterances + " utterances (" + root.live.n_dictation + " dict, " + root.live.n_command + " cmd)  ·  " + dev + extra;
                            }
                            color: LiveTheme.muted
                            font { family: LiveTheme.fontMono; pixelSize: 11 }
                        }
                    }

                    Row {
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 7
                        ModeChip { label: "E2E"; active: root.metricMode === "e2e"; onClicked: { root.metricMode = "e2e"; root.hoveredIdx = -1; plotCanvas.requestPaint(); } }
                        ModeChip { label: "POST-SPEECH"; active: root.metricMode === "proc"; activeTone: LiveTheme.ok; onClicked: { root.metricMode = "proc"; root.hoveredIdx = -1; plotCanvas.requestPaint(); } }
                        Rectangle { width: 1; height: 20; color: LiveTheme.cyanLine; anchors.verticalCenter: parent.verticalCenter }
                        ModeChip { label: "ALL"; active: root.kindFilter === "all"; onClicked: { root.kindFilter = "all"; root.hoveredIdx = -1; plotCanvas.requestPaint(); } }
                        ModeChip { label: "DICT"; active: root.kindFilter === "dictation"; onClicked: { root.kindFilter = "dictation"; root.hoveredIdx = -1; plotCanvas.requestPaint(); } }
                        ModeChip { label: "CMD"; active: root.kindFilter === "command"; activeTone: LiveTheme.neural; onClicked: { root.kindFilter = "command"; root.hoveredIdx = -1; plotCanvas.requestPaint(); } }
                        Rectangle { width: 1; height: 20; color: LiveTheme.cyanLine; anchors.verticalCenter: parent.verticalCenter }
                        ModeChip { label: "+ SNAP"; active: false; onClicked: { snapshotProc.running = true; } }
                        ModeChip { label: "×"; active: false; activeTone: LiveTheme.bad; onClicked: Qt.quit() }
                    }
                }

                // ---- 2. Progress Rails toward Instant Floor ----
                Row {
                    width: parent.width
                    spacing: 16
                    ProgressRail {
                        width: (parent.width - 16) / 2
                        k: 0.92
                        tone: LiveTheme.cyan
                        label: "E2E REACTION SCORE (TO 150ms FLOOR)"
                        detail: root.live ? (Math.round(root.live.baseline_combined_e2e_ms) + " → " + Math.round(root.live.current_combined_e2e_ms) + " ms") : ""
                        value: root.live ? Math.max(0, Math.min(1, root.live.responsiveness_score / 100)) : 0
                    }
                    ProgressRail {
                        width: (parent.width - 16) / 2
                        k: 0.92
                        tone: LiveTheme.ok
                        label: "POST-SPEECH ENGINE SCORE (TO 5ms FLOOR)"
                        detail: root.live ? (Math.round(root.live.baseline_combined_proc_ms) + " → " + root.fmtMs(root.live.current_combined_proc_ms) + " · " + root.fmtX(root.live.proc_speedup)) : ""
                        value: root.live ? Math.max(0, Math.min(1, root.live.processing_score / 100)) : 0
                    }
                }

                // ---- 3. KPI Strip ----
                Row {
                    width: parent.width
                    spacing: 10
                    readonly property real tileW: (width - 30) / 4

                    StatTile {
                        width: parent.tileW
                        eyebrow: "DICTATION POST-SPEECH"
                        tone: LiveTheme.cyan
                        value: root.live ? root.fmtMs(root.live.dictation_proc_med_ms) : "—"
                        sub: root.live && root.base ? ("base " + Math.round(root.base.dictation_proc_med_ms) + " ms  ·  E2E " + Math.round(root.live.dictation_e2e_med_ms) + " ms") : ""
                    }
                    StatTile {
                        width: parent.tileW
                        eyebrow: "COMMAND ACTION"
                        tone: LiveTheme.neural
                        value: root.live ? root.fmtMs(root.live.command_proc_med_ms) : "—"
                        sub: root.live && root.base ? ("base " + Math.round(root.base.command_proc_med_ms) + " ms  ·  key " + Math.round(root.live.command_exec_med_ms || 0) + " ms") : ""
                    }
                    StatTile {
                        width: parent.tileW
                        eyebrow: "SUB-MS PARTIAL COMMITS"
                        tone: LiveTheme.ok
                        value: root.live ? (root.live.partial_commit_pct.toFixed(0) + "%") : "—"
                        sub: root.live ? ("p95 ASR " + Math.round(root.live.asr_p95_ms || 0) + " ms  (base 262 ms)") : ""
                    }
                    StatTile {
                        width: parent.tileW
                        eyebrow: "LIVE HOT-PATH PROBE"
                        tone: LiveTheme.ember
                        value: (root.live && root.live.micro && root.live.micro.hypr_ipc_ms !== null && root.live.micro.hypr_ipc_ms !== undefined)
                            ? (root.live.micro.hypr_ipc_ms.toFixed(2) + " ms")
                            : "—"
                        sub: {
                            if (!root.live || !root.live.micro) return "probing…";
                            const m = root.live.micro;
                            const steer = m.steer_resident ? "steer resident" : "steer oneshot";
                            const chunk = (m.capture_chunk_ms !== null && m.capture_chunk_ms !== undefined)
                                ? (Math.round(m.capture_chunk_ms) + "ms cap")
                                : "cap n/a";
                            return "hypr ipc · " + steer + " · " + chunk;
                        }
                    }
                }

                // ---- 4. Main Progress Plot ----
                Rectangle {
                    width: parent.width
                    height: 274
                    radius: LiveTheme.radiusSmall + 2
                    color: Qt.rgba(LiveTheme.bg.r, LiveTheme.bg.g, LiveTheme.bg.b, 0.72)
                    border.width: LiveTheme.borderThin
                    border.color: LiveTheme.cyanLine

                    Canvas {
                        id: plotCanvas
                        anchors { fill: parent; topMargin: 10; leftMargin: 12; rightMargin: 14; bottomMargin: 28 }
                        property var watchPayload: root.payload
                        property string watchMode: root.metricMode
                        property string watchFilter: root.kindFilter
                        property int watchHover: root.hoveredIdx
                        onWatchPayloadChanged: requestPaint()
                        onWatchModeChanged: requestPaint()
                        onWatchFilterChanged: requestPaint()
                        onWatchHoverChanged: requestPaint()

                        function chartBounds() {
                            return { left: 54, right: width - 16, top: 20, bottom: height - 24 };
                        }

                        function yMaxVal() {
                            const isProc = root.metricMode === "proc";
                            let mx = isProc ? 420 : 1150;
                            const evs = root.filteredEvents;
                            for (let i = 0; i < evs.length; i++) {
                                const v = isProc ? evs[i].proc_ms : evs[i].e2e_ms;
                                if (v > mx) mx = v;
                            }
                            return Math.ceil(mx / 100) * 100;
                        }

                        function ptCoords(idx, total, val, b, yMax) {
                            const spanX = Math.max(1, b.right - b.left);
                            // Reserve slot 0 for Baseline M0, slots 1..total for live events
                            const denom = Math.max(1, total);
                            const x = b.left + (idx / denom) * spanX;
                            const y = b.bottom - (Math.min(yMax, Math.max(0, val)) / yMax) * (b.bottom - b.top);
                            return { x: x, y: y };
                        }

                        onPaint: {
                            const g = getContext("2d");
                            g.reset();
                            if (!root.live || !root.base) return;

                            const isProc = root.metricMode === "proc";
                            const b = chartBounds();
                            const yMax = yMaxVal();
                            const evs = root.filteredEvents;
                            const n = evs.length;

                            const baseVal = isProc
                                ? (root.kindFilter === "dictation" ? root.base.dictation_proc_med_ms
                                   : root.kindFilter === "command" ? root.base.command_proc_med_ms
                                   : root.live.baseline_combined_proc_ms)
                                : (root.kindFilter === "dictation" ? root.base.dictation_e2e_med_ms
                                   : root.kindFilter === "command" ? root.base.command_e2e_med_ms
                                   : root.live.baseline_combined_e2e_ms);
                            const floorVal = isProc ? root.live.instant_floor_proc_ms : root.live.instant_floor_e2e_ms;

                            const Y = v => b.bottom - (Math.min(yMax, Math.max(0, v)) / yMax) * (b.bottom - b.top);

                            // Horizontal grid + Y labels
                            const steps = 4;
                            g.font = "10.5px '" + LiveTheme.fontMono + "'";
                            for (let s = 0; s <= steps; s++) {
                                const v = (yMax / steps) * s;
                                const y = Y(v);
                                g.strokeStyle = Qt.rgba(LiveTheme.cyan.r, LiveTheme.cyan.g, LiveTheme.cyan.b, 0.11);
                                g.lineWidth = 1;
                                g.beginPath(); g.moveTo(b.left, y); g.lineTo(b.right, y); g.stroke();
                                g.fillStyle = LiveTheme.faint;
                                g.textAlign = "right";
                                g.fillText(Math.round(v) + "ms", b.left - 8, y + 3.5);
                            }

                            // Instant target floor line (green dashed)
                            const yFloor = Y(floorVal);
                            g.save();
                            g.setLineDash([4, 4]);
                            g.strokeStyle = Qt.rgba(LiveTheme.ok.r, LiveTheme.ok.g, LiveTheme.ok.b, 0.55);
                            g.lineWidth = 1.2;
                            g.beginPath(); g.moveTo(b.left, yFloor); g.lineTo(b.right, yFloor); g.stroke();
                            g.restore();
                            g.fillStyle = LiveTheme.ok;
                            g.textAlign = "right";
                            g.fillText("TARGET FLOOR " + Math.round(floorVal) + "ms", b.right, yFloor - 5);

                            // Baseline reference line (ember dashed)
                            const yBase = Y(baseVal);
                            g.save();
                            g.setLineDash([6, 4]);
                            g.strokeStyle = Qt.rgba(LiveTheme.ember.r, LiveTheme.ember.g, LiveTheme.ember.b, 0.72);
                            g.lineWidth = 1.5;
                            g.beginPath(); g.moveTo(b.left, yBase); g.lineTo(b.right, yBase); g.stroke();
                            g.restore();
                            g.fillStyle = LiveTheme.ember;
                            g.textAlign = "left";
                            g.fillText("BASELINE M0 (" + Math.round(baseVal) + "ms)", b.left + 8, yBase - 6);

                            // Build rolling trend path from Baseline M0 (slot 0) across live utterances (slots 1..n)
                            if (n > 0) {
                                const trendPts = [{ x: b.left, y: yBase }];
                                for (let i = 0; i < n; i++) {
                                    const rv = isProc ? evs[i].rolling_proc_ms : evs[i].rolling_e2e_ms;
                                    trendPts.push(ptCoords(i + 1, n, rv, b, yMax));
                                }

                                // Shaded improvement area between Baseline line and rolling curve
                                g.save();
                                g.beginPath();
                                g.moveTo(trendPts[0].x, yBase);
                                for (let i = 0; i < trendPts.length; i++) g.lineTo(trendPts[i].x, trendPts[i].y);
                                g.lineTo(trendPts[trendPts.length - 1].x, yBase);
                                g.closePath();
                                const areaCol = isProc ? LiveTheme.ok : LiveTheme.cyan;
                                const grad = g.createLinearGradient(0, yBase, 0, b.bottom);
                                grad.addColorStop(0, Qt.rgba(areaCol.r, areaCol.g, areaCol.b, 0.22));
                                grad.addColorStop(1, Qt.rgba(areaCol.r, areaCol.g, areaCol.b, 0.03));
                                g.fillStyle = grad;
                                g.fill();
                                g.restore();

                                // Crisp rolling median trend line
                                g.strokeStyle = isProc ? LiveTheme.ok : LiveTheme.cyan;
                                g.lineWidth = 2.4;
                                g.lineJoin = "round";
                                g.beginPath();
                                for (let i = 0; i < trendPts.length; i++) {
                                    if (i === 0) g.moveTo(trendPts[i].x, trendPts[i].y);
                                    else g.lineTo(trendPts[i].x, trendPts[i].y);
                                }
                                g.stroke();

                                // Individual utterance markers
                                for (let i = 0; i < n; i++) {
                                    const e = evs[i];
                                    const v = isProc ? e.proc_ms : e.e2e_ms;
                                    const p = ptCoords(i + 1, n, v, b, yMax);
                                    const isHov = (i === root.hoveredIdx);

                                    if (isHov) {
                                        g.strokeStyle = Qt.rgba(LiveTheme.ink.r, LiveTheme.ink.g, LiveTheme.ink.b, 0.35);
                                        g.lineWidth = 1;
                                        g.beginPath(); g.moveTo(p.x, b.top); g.lineTo(p.x, b.bottom); g.stroke();
                                    }

                                    const r = isHov ? 6.5 : 4.2;
                                    if (e.kind === "command") {
                                        // Neural violet diamond
                                        g.fillStyle = LiveTheme.neural;
                                        g.beginPath();
                                        g.moveTo(p.x, p.y - r);
                                        g.lineTo(p.x + r, p.y);
                                        g.lineTo(p.x, p.y + r);
                                        g.lineTo(p.x - r, p.y);
                                        g.closePath();
                                        g.fill();
                                    } else {
                                        // Cyan circle, emerald core if sub-ms partial commit
                                        g.fillStyle = e.partial_commit ? LiveTheme.ok : LiveTheme.cyan;
                                        g.beginPath();
                                        g.arc(p.x, p.y, r, 0, Math.PI * 2);
                                        g.fill();
                                    }
                                    if (isHov) {
                                        g.strokeStyle = LiveTheme.ink;
                                        g.lineWidth = 1.6;
                                        g.beginPath(); g.arc(p.x, p.y, r + 3, 0, Math.PI * 2); g.stroke();
                                    }
                                }
                            }

                            // Baseline M0 anchor dot
                            g.fillStyle = LiveTheme.ember;
                            g.beginPath(); g.arc(b.left, yBase, 5.5, 0, Math.PI * 2); g.fill();
                            g.strokeStyle = LiveTheme.bg; g.lineWidth = 1.5; g.stroke();

                            // Axis title & X labels
                            g.fillStyle = LiveTheme.muted;
                            g.textAlign = "left";
                            g.fillText(isProc ? "POST-SPEECH PROCESSING LATENCY (ms · LOWER = FASTER)" : "END-TO-END REACTION LATENCY (SILENCE + ASR + EXEC · LOWER = FASTER)", b.left, 11);

                            g.fillStyle = LiveTheme.faint;
                            g.textAlign = "left";
                            g.fillText("M0 BASELINE", b.left - 4, b.bottom + 15);
                            g.textAlign = "right";
                            g.fillText("NOW (#" + n + ")  ·  ● dict  ● partial(<1ms)  ◆ cmd", b.right, b.bottom + 15);
                        }

                        MouseArea {
                            anchors.fill: parent
                            hoverEnabled: true
                            onExited: root.hoveredIdx = -1
                            onPositionChanged: (mouse) => {
                                const evs = root.filteredEvents;
                                if (!evs.length) { root.hoveredIdx = -1; return; }
                                const b = plotCanvas.chartBounds();
                                const yMax = plotCanvas.yMaxVal();
                                const isProc = root.metricMode === "proc";
                                let best = -1, bestDist = 26;
                                for (let i = 0; i < evs.length; i++) {
                                    const v = isProc ? evs[i].proc_ms : evs[i].e2e_ms;
                                    const p = plotCanvas.ptCoords(i + 1, evs.length, v, b, yMax);
                                    const dx = mouse.x - p.x, dy = mouse.y - p.y;
                                    const d = Math.sqrt(dx * dx + dy * dy);
                                    if (d < bestDist) { bestDist = d; best = i; }
                                }
                                root.hoveredIdx = best;
                            }
                        }
                    }

                    // Bottom hover / latest utterance readout strip
                    Text {
                        anchors { left: parent.left; right: parent.right; bottom: parent.bottom; leftMargin: 14; rightMargin: 14; bottomMargin: 7 }
                        elide: Text.ElideRight
                        color: root.hoveredIdx >= 0 ? LiveTheme.ink : LiveTheme.muted
                        font { family: LiveTheme.fontMono; pixelSize: 11 }
                        text: {
                            const evs = root.filteredEvents;
                            if (!evs.length) return "Speak a phrase or command to plot live points…";
                            const e = root.hoveredIdx >= 0 && root.hoveredIdx < evs.length ? evs[root.hoveredIdx] : evs[evs.length - 1];
                            const prefix = root.hoveredIdx >= 0 ? "INSPECT" : "LATEST ";
                            return prefix + " #" + e.idx + " [" + e.kind.toUpperCase() + "/" + e.path + "]  "
                                + Math.round(e.silence_ms) + "ms sil + " + root.fmtMs(e.asr_ms) + " asr + "
                                + root.fmtMs(e.post_ms) + " post = " + root.fmtMs(e.e2e_ms)
                                + "   ·  \"" + e.label + "\"";
                        }
                    }
                }

                // ---- 5. Before vs After Stage Breakdown Waterfall ----
                Rectangle {
                    width: parent.width
                    height: 118
                    radius: LiveTheme.radiusSmall + 2
                    color: LiveTheme.glass2
                    border.width: LiveTheme.borderThin
                    border.color: LiveTheme.cyanLine

                    Column {
                        anchors { fill: parent; margins: 12 }
                        spacing: 7

                        Row {
                            width: parent.width
                            Text {
                                text: "STAGE BREAKDOWN // BEFORE vs LIVE (MEDIAN ms)"
                                color: LiveTheme.muted
                                font { family: LiveTheme.fontMono; pixelSize: 11; weight: Font.Bold; letterSpacing: 1.4 }
                            }
                            Item { width: 20; height: 1 }
                            Row {
                                spacing: 12
                                Row { spacing: 4; Rectangle { width: 8; height: 8; radius: 2; color: LiveTheme.cyan; anchors.verticalCenter: parent.verticalCenter } Text { text: "silence"; color: LiveTheme.faint; font { family: LiveTheme.fontMono; pixelSize: 10 } } }
                                Row { spacing: 4; Rectangle { width: 8; height: 8; radius: 2; color: LiveTheme.neural; anchors.verticalCenter: parent.verticalCenter } Text { text: "asr"; color: LiveTheme.faint; font { family: LiveTheme.fontMono; pixelSize: 10 } } }
                                Row { spacing: 4; Rectangle { width: 8; height: 8; radius: 2; color: LiveTheme.ember; anchors.verticalCenter: parent.verticalCenter } Text { text: "steer/ipc/key"; color: LiveTheme.faint; font { family: LiveTheme.fontMono; pixelSize: 10 } } }
                            }
                        }

                        Repeater {
                            model: root.base && root.live ? [
                                { name: "DICT BASE", sil: root.base.dictation_silence_med_ms, asr: root.base.dictation_asr_med_ms, post: root.base.dictation_post_med_ms, tot: root.base.dictation_e2e_med_ms, hi: false },
                                { name: "DICT LIVE", sil: root.live.dictation_silence_med_ms || 0, asr: root.live.dictation_asr_med_ms || 0, post: root.live.dictation_post_med_ms || 0, tot: root.live.dictation_e2e_med_ms || 0, hi: true },
                                { name: "CMD  BASE", sil: root.base.command_silence_med_ms, asr: root.base.command_asr_med_ms, post: root.base.command_exec_med_ms, tot: root.base.command_e2e_med_ms, hi: false },
                                { name: "CMD  LIVE", sil: root.live.command_silence_med_ms || 0, asr: root.live.command_asr_med_ms || 0, post: root.live.command_exec_med_ms || 0, tot: root.live.command_e2e_med_ms || 0, hi: true }
                            ] : []

                            Row {
                                required property var modelData
                                width: parent.width
                                height: 15
                                spacing: 10
                                readonly property real barArea: width - 170
                                readonly property real scaleMax: 820.0

                                Text {
                                    width: 78
                                    text: modelData.name
                                    color: modelData.hi ? LiveTheme.ink : LiveTheme.faint
                                    font { family: LiveTheme.fontMono; pixelSize: 11; weight: modelData.hi ? Font.Bold : Font.Normal }
                                    anchors.verticalCenter: parent.verticalCenter
                                }

                                Row {
                                    width: parent.barArea
                                    height: 10
                                    anchors.verticalCenter: parent.verticalCenter
                                    spacing: 1
                                    Rectangle {
                                        width: Math.max(0, (modelData.sil / parent.parent.scaleMax) * parent.width)
                                        height: parent.height; radius: 2
                                        color: LiveTheme.cyan; opacity: modelData.hi ? 0.92 : 0.35
                                    }
                                    Rectangle {
                                        width: Math.max(modelData.asr > 0.5 ? 2 : 0, (modelData.asr / parent.parent.scaleMax) * parent.width)
                                        height: parent.height; radius: 2
                                        color: LiveTheme.neural; opacity: modelData.hi ? 0.95 : 0.42
                                    }
                                    Rectangle {
                                        width: Math.max(modelData.post > 0.5 ? 2 : 0, (modelData.post / parent.parent.scaleMax) * parent.width)
                                        height: parent.height; radius: 2
                                        color: LiveTheme.ember; opacity: modelData.hi ? 0.95 : 0.52
                                    }
                                }

                                Text {
                                    width: 70
                                    horizontalAlignment: Text.AlignRight
                                    text: Math.round(modelData.tot) + " ms"
                                    color: modelData.hi ? LiveTheme.ok : LiveTheme.faint
                                    font { family: LiveTheme.fontMono; pixelSize: 11; weight: modelData.hi ? Font.Bold : Font.Normal }
                                    anchors.verticalCenter: parent.verticalCenter
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
