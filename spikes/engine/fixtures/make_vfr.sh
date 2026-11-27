#!/bin/sh
# Make the presentation index independently predictable while retaining B frames.
set -eu
cd "$(dirname "$0")"
ffmpeg -nostdin -v error -i numbered.mkv -an \
    -vf 'crop=640:360,settb=1/30,setpts=N+floor(N/7),scale=out_color_matrix=bt709:out_range=tv,format=yuv420p,setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709' \
    -fps_mode passthrough -enc_time_base 1/30 -c:v libx264 -g 24 -bf 3 \
    -x264-params scenecut=0:b-adapt=0:open-gop=0 -threads 1 \
    -color_range tv -color_trc bt709 -colorspace bt709 -color_primaries bt709 \
    -output_ts_offset 0.7 -video_track_timescale 30000 -y numbered-vfr.mp4
