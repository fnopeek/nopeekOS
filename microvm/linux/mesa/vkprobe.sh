#!/bin/sh
# npk-vkprobe: whether the guest decodes video on the GPU through Vulkan
# Video (anv over native context), without the browser. Run it in place of
# the browser with `npkapp=npk-vkprobe` and `npklogs` on the guest command
# line; every line goes to the console as `[log] [vk] ...`.
#
# Per codec: a short clip is encoded in software, then decoded with
# `-hwaccel vulkan` and the frames kept in GPU memory, so a decoder that
# falls back to the CPU fails instead of passing.

say() { echo "[vk] $*"; }

say "vulkaninfo:"
vulkaninfo --summary 2>&1 | grep -E "deviceName|driverName|driverInfo|apiVersion|ERROR" | sed 's/^/[vk]   /'
vulkaninfo 2>/dev/null | grep -o "VK_KHR_video_decode_[a-z0-9_]*" | sort -u | sed 's/^/[vk]   /'

T=/tmp/vkprobe
mkdir -p "$T"
W=1920 H=1080 SEC=2

for c in h264:libx264 hevc:libx265 vp9:libvpx-vp9 av1:libsvtav1; do
    name=${c%%:*}
    enc=${c#*:}
    clip="$T/$name.mkv"
    if ! ffmpeg -nostdin -v error -f lavfi -i "testsrc2=size=${W}x${H}:rate=30" -t $SEC \
            -c:v "$enc" -pix_fmt yuv420p -y "$clip" 2>"$T/$name.enc.log"; then
        say "$name: no clip (encoder $enc): $(tail -1 "$T/$name.enc.log")"
        continue
    fi
    start=$(date +%s%N)
    if ffmpeg -nostdin -v warning -init_hw_device vulkan=vk -hwaccel vulkan -hwaccel_device vk \
            -hwaccel_output_format vulkan -i "$clip" -vf hwdownload,format=nv12 -f null - \
            2>"$T/$name.dec.log"; then
        ms=$(( ($(date +%s%N) - start) / 1000000 ))
        say "$name: decoded on the GPU, $((SEC * 30)) frames ${W}x${H} in ${ms} ms"
    else
        say "$name: NOT on the GPU:"
        tail -4 "$T/$name.dec.log" | sed 's/^/[vk]   /'
    fi
done
say "done"
