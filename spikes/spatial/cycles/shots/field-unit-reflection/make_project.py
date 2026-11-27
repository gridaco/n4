"""Write an N4 project for the Field Unit reflection close-up into .local/examples.

A copy of the public Field Unit example with one 51-frame path-traced shot: the lid opens over
the deck under three rect lights, and the display is an emissive surface that lights the scene.

    python3 spikes/spatial/cycles/shots/field-unit-reflection/make_project.py [OUT]
    just inspect .local/examples/field-unit-reflection
"""
import math, shutil, sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[5]
EXAMPLE = REPO / "spikes/examples/field-unit"
OUT = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else REPO / ".local/examples/field-unit-reflection"


def srgb8(linear):
    c = 12.92 * linear if linear <= 0.0031308 else 1.055 * linear ** (1 / 2.4) - 0.055
    return max(0, min(255, round(c * 255)))


def light(tag, radiance, extra=""):
    peak = max(radiance)
    color = "#%02x%02x%02x" % tuple(srgb8(c / peak) for c in radiance)
    return f'    <{tag}{extra} intensity="{peak:g}" color="{color}"/>'


def look(position, target):
    d = [t - p for p, t in zip(position, target)]
    n = math.sqrt(sum(x * x for x in d))
    f = [x / n for x in d]
    return math.degrees(math.asin(f[1])), math.degrees(math.atan2(-f[0], -f[2]))


camera, target = [0.150, 0.118, 0.215], [-0.005, 0.048, -0.012]
pitch, yaw = look(camera, target)
rects = [
    ("-0.36 0.52 0.34", "0 0.05 0", "0.5 0.3", [11.0, 9.7, 7.9]),
    ("0.12 0.34 -0.40", "0 0.08 0", "0.52 0.05", [14.0, 12.6, 10.5]),
    ("0.34 0.20 0.22", "0 0.06 0", "0.5 0.2", [1.3, 1.5, 1.75]),
]
lights = "\n".join(light("rect-light", r, f' center="{c}" aim="{a}" size="{s}"') for c, a, s, r in rects)
shot = f'''<sequence name="Field Unit reflection" background="#0e1011" camera="close">
  <camera id="close" projection="perspective" position="{camera[0]} {camera[1]} {camera[2]}" rotation="{pitch:.4f} {yaw:.4f} 0" fov-y="27"/>
  <scene3d pixels-per-meter="3200">
    <studio shadow-center="0 0.061 0" shadow-extent="0.39" shadow-bias="0.0003" shadow-softness="4"/>
    <path-trace samples="64" bounces="8" clamp="10" denoise="true" seed="0"/>
{light("ambient-light", [0.045, 0.050, 0.056])}
{lights}
    <usd id="ground" source="assets/field.usdc" prim="/World/Studio"/>
    <usd id="recorder" source="assets/field.usdc" prim="/World/Recorder">
      <part prim="/World/Recorder/Hinge">
        <animate attributeName="rotation-x">
          <key frame="0" value="58" ease="cubic(0.4 0 0.3 1)"/>
          <key frame="38" value="14"/>
          <key frame="50" value="14"/>
        </animate>
      </part>
      <surface slot="/World/_materials/Display" role="emissive" glow="4" size="640 360">
        <video name="Live scope signal" source="assets/signal.mp4" start="0" duration="51" source-in="0" size="640 360"/>
        <text name="Display model name" position="26 7" font="assets/N4GridSans.ttf" size="42" width="505" fill="#f1eacb"><copy>FIELD UNIT</copy></text>
        <text name="Display input label" position="28 65" font="assets/N4GridSans.ttf" size="20" width="510" fill="#c1cabc"><copy>CH 01   /   INPUT MONITOR</copy></text>
      </surface>
      <surface slot="/World/_materials/Nameplate" role="base-color" size="440 112">
        <rect width="440" height="112" fill="#b4b09b"/>
        <text name="Printed model number" position="12 9" font="assets/N4GridSans.ttf" size="70" width="255" fill="#202624"><copy>R-01</copy></text>
        <rect x="287" y="22" width="7" height="66" fill="#202624"/>
        <rect x="310" y="22" width="12" height="66" fill="#202624"/>
        <rect x="354" y="22" width="9" height="66" fill="#202624"/>
        <rect x="389" y="22" width="13" height="66" fill="#202624"/>
      </surface>
    </usd>
  </scene3d>
</sequence>
'''
caption = '''<sequence>
  <text name="Caption" position="96 960" font="assets/N4GridSans.ttf" size="34" width="900" fill="#e8e2c8"><copy>PATH TRACED FINAL</copy></text>
</sequence>
'''
(OUT / "sequences").mkdir(parents=True, exist_ok=True)
(OUT / "assets").mkdir(exist_ok=True)
for name in ("field.usdc", "signal.mp4", "N4GridSans.ttf"):
    shutil.copy(EXAMPLE / "assets" / name, OUT / "assets" / name)
(OUT / "sequences/reflection.xml").write_text(shot)
(OUT / "sequences/caption.xml").write_text(caption)
(OUT / "project.proj").write_text("project field-unit-reflection fps 24/1 size 1920x1080\n[0,51) V1 reflection\n[0,51) V2 caption\n")
print(f"wrote {OUT}")
