# Numbered source cue sheet

`numbered.mkv` is original synthetic picture media for inspecting source selection, text layout and GPU readback. It has 90 frames at 30 fps, 641 × 360 square pixels, no audio, and lossless full-range RGB stored as FFV1 in Matroska. The fixture colors are authored as sRGB. The stream has a millisecond timestamp clock and frame zero starts at zero.

The three figures in the upper right display the zero-based source frame number. The green block starts at x = 12 and advances six pixels each frame. The seven black/white cells along the bottom carry the same number in binary. Read the red channel at **x = 16 + 16 × bit, y = 344**, for bit 0 through bit 6. White is one. At x = 640 every even row carries RGB (31, 187, 186) and every odd row (217, 147, 56). Both are opaque. The one-pixel edge exercises padded GPU row transfers. Keep overlays clear of these coordinates when checking raw source identity.

| Source frame | Time stamp (ms) | Digits | Cell values from left to right |
| --- | --- | --- | --- |
| 0 | 0 | 000 | 0000000 |
| 12 | 400 | 012 | 0011000 |
| 13 | 433 | 013 | 1011000 |
| 30 | 1000 | 030 | 0111100 |
| 60 | 2000 | 060 | 0011110 |
| 89 | 2967 | 089 | 1001101 |

Run `python3 make_numbered.py` from this directory to regenerate the movie with FFmpeg. Two runs with FFmpeg 9.0.1 on arm64 macOS produced SHA-256 `1711163134749ceecb853b3449f46b65173bc2020b15efd5fb5bd7cdf0327e4b`. The baseline was decoded and checked at all 90 frame IDs and all 360 right-edge rows. This short all-intra source verifies frame selection. Interframe seek performance, VFR playback, audio and general color conversion require separate fixtures.
