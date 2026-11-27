"""Build the native frosted-caption project and its resolution-matched controls."""
import json
import re
from pathlib import Path

HERE = Path(__file__).resolve().parent
FONT = "../fonts/N4GridSans.ttf"


def project(scale=1.0, card_size=None, sigma_px=8.0):
    """Return a 180-frame edit. An optional card_size is in final output pixels."""
    def px(value):
        return round(value * scale)

    def label(value, x, y, size, width, rgb):
        return {
            "content": {"type": "label", "text": value, "font": FONT, "size": px(size),
                        "width": px(width), "rgb": rgb},
            "position": [x, y],
        }

    width, height = px(960), px(540)
    if card_size is None:
        card_x, card_y = px(474), px(242)
        card_w, card_h = px(330), px(96)
    else:
        card_w, card_h = card_size
        card_x, card_y = (width - card_w) // 2, (height - card_h) // 2
    # The animated shader cuts a smaller moving silhouette out of this static path.
    mask_d = f"M0 0 H{card_w} V{card_h} H0 Z"
    trace_d = "M425 323 C488 323 509 244 563 251 S660 337 715 307 S814 241 871 259"
    trace_d = re.sub(r"[0-9]+", lambda m: str(px(int(m[0]))), trace_d)
    trace = {
        "content": {"type": "path", "d": trace_d, "fill": None,
                    "stroke": {"rgba": [255, 184, 120, 255], "width": 4.0 * scale,
                               "cap": "round", "join": "round"}},
        "wipe": [8, 155],
    }
    adjustment = {
        "content": {"type": "backdrop", "filter": {"shader": "gaussian-blur.wgsl",
                                                     "sigma_px": sigma_px}},
        "position": [card_x, card_y],
        "mask": {"content": {"type": "path", "d": mask_d},
                 "shader": {"shader": "frost-mask.wgsl", "period_frames": 90}},
    }
    text_x, text_y = card_x + px(31), card_y + card_h // 2
    layers = [
        {"content": {"type": "footage"}},
        trace,
        adjustment,
        label("Room tone", text_x, text_y - px(33), 25, 260, [252, 250, 246]),
        label("60 Hz  /  Harmonic noise", text_x + px(1), text_y + px(3), 15, 270,
              [248, 247, 243]),
        label("Audio restoration", px(42), px(108), 16, 280, [193, 188, 234]),
        label("Reduce electrical noise from a room recording in one switch.",
              px(42), px(306), 20, 272, [226, 226, 237]),
    ]
    return {
        "size": [width, height], "fps": [30, 1],
        "shots": [{"source": "take-b.mp4", "source_in": 0, "duration": 180}],
        "title": {"text": "Keep the take.\nLose the hum.", "font": FONT, "font_size": px(45),
                  "position": [px(40), px(151)], "width": px(329), "start": 0, "fade_frames": 8},
        "graphics": layers,
        "audio": [{"source": "take-b.wav", "start_sample": 0, "source_in_sample": 0,
                   "samples": 288_000, "gain": 0.9}],
    }


if __name__ == "__main__":
    (HERE / "frost.json").write_text(json.dumps(project(), indent=2) + "\n")
