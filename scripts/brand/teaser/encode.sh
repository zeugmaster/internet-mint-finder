#!/usr/bin/env bash
# Encode the teaser's rendered frames (teaser.py --anim) to an H.264 MP4 for
# X, Nostr and Reels, tagged BT.709 so players don't shift the colours, and
# print what came out. Also writes a contact sheet, one frame a second.
#   scripts/brand/teaser/encode.sh preview|final
set -euo pipefail

cd "$(dirname "$0")/../../.."
MODE="${1:-preview}"
DIR="renders/teaser/$MODE"
CRF=$([ "$MODE" = final ] && echo 16 || echo 22)
OUT="$DIR/imf-teaser-$MODE.mp4"

ffmpeg -hide_banner -loglevel error -y -framerate 30 -start_number 0 -i "$DIR/frames/%04d.png" \
  -vf "scale=out_color_matrix=bt709:out_range=tv,format=yuv420p" \
  -c:v libx264 -preset slow -crf "$CRF" -tune film -profile:v high \
  -colorspace bt709 -color_primaries bt709 -color_trc bt709 -color_range tv \
  -movflags +faststart -an "$OUT"

ffmpeg -hide_banner -loglevel error -y -i "$OUT" \
  -vf "select='not(mod(n\,30))',scale=270:-1,tile=8x1" -frames:v 1 "$DIR/contact-sheet.png"

ffprobe -v error -select_streams v:0 -count_frames \
  -show_entries stream=codec_name,width,height,pix_fmt,r_frame_rate,nb_read_frames,color_space \
  -show_entries format=duration -of default=nw=1 "$OUT"
echo "$OUT"
