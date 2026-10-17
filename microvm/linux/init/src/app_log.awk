# The app's output on its way to /tmp/app.log, read as it is written.
#   all=1      every line to the console as `[log]` (plain `npklogs`)
#   p=REGEX    only the matching lines (`npklogs=REGEX`)
#   frames=1   the video decode times Firefox logs with FFmpegVideo:4
#              (`npkframes`): one `[frames]` line a second, and every frame
#              over FRAME_LATE_MS at once as `[frame] SLOW`, with the guest's
#              uptime, so it can be laid next to the host's log.

function out(s) {
    print "<0>" s > "/dev/kmsg"
    fflush("/dev/kmsg")
}

function uptime(   u, a) {
    getline u < "/proc/uptime"
    close("/proc/uptime")
    split(u, a, " ")
    return a[1] + 0
}

function report(t) {
    if (n > 0)
        out(sprintf("[frames] %d frames, avg %.1f ms, max %.1f ms, over 20 ms %d, over %d ms %d",
                    n, sum / n, max, over20, FRAME_LATE_MS, late))
    n = 0; sum = 0; max = 0; over20 = 0; late = 0; since = t
}

BEGIN { FRAME_LATE_MS = 40 }

all { out("[log] " $0) }

!all && p != "" && $0 ~ p { out("[log] " $0) }

frames && index($0, "Frame decode takes ") {
    v = substr($0, index($0, "Frame decode takes ") + 19) + 0
    t = uptime()
    if (since == 0) since = t
    n++; sum += v
    if (v > max) max = v
    if (v > 20) over20++
    if (v > FRAME_LATE_MS) {
        late++
        out(sprintf("[frame] SLOW %.1f ms at %.2f s", v, t))
    }
    if (t - since >= 1) report(t)
    next
}

frames && ($0 ~ /HW decoding is slow|software codec|hardware codec/) {
    s = substr($0, index($0, "FFVPX:") + 6)
    sub(/^ +/, "", s)
    out("[frame] " s)
}
