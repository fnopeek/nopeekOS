#!/bin/bash
# Test pictures for the jpeg decoder: sampling 4:2:2 (cameras), 4:2:0,
# 4:4:4, grey, restart intervals, odd sizes, and an MJPEG frame without
# DHT (ffmpeg, default tables, DHT cut out). Each .ppm is libjpeg-turbo's
# decode of the .jpg next to it.
set -e
cd "$(dirname "$0")"
src=src.png
magick -size 133x77 gradient:'#203080-#f0c040' -swirl 120 \( -size 133x77 plasma:fractal -blur 0x2 \) -compose blend -define compose:args=40 -composite $src
magick $src -quality 85 -sampling-factor 2x1 s422.jpg
magick $src -quality 85 -sampling-factor 2x2 s420.jpg
magick $src -quality 95 -sampling-factor 1x1 s444.jpg
magick $src -colorspace Gray -quality 85 grey.jpg
magick $src -quality 80 -sampling-factor 2x1 -define jpeg:restart-interval=3 restart.jpg
ffmpeg -loglevel error -y -i $src -vf scale=128:96,format=yuvj422p -c:v mjpeg -huffman default -q:v 4 -frames:v 1 mjpeg-dht.jpg
python3 - <<'PY'
d = open('mjpeg-dht.jpg', 'rb').read()
out = bytearray(d[:2]); p = 2
while p < len(d):
    m = d[p + 1]; ln = (d[p + 2] << 8) | d[p + 3]
    if m == 0xDA: out += d[p:]; break
    if m != 0xC4: out += d[p:p + 2 + ln]
    p += 2 + ln
open('mjpeg-nodht.jpg', 'wb').write(out)
PY
rm mjpeg-dht.jpg $src
for f in *.jpg; do magick "$f" -depth 8 "ppm:${f%.jpg}.ppm"; done

for f in *.jpg; do for n in 2 4 8; do djpeg -scale 1/$n -ppm "$f" > "${f%.jpg}.$n.ppm"; done; done
