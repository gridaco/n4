"""Blender source for Field Unit R-01. Run with Blender --background --python device.py."""
import argparse
import math
from pathlib import Path
import sys
import bpy
from mathutils import Vector

ROOT = Path(__file__).resolve().parents[1]
argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
parser = argparse.ArgumentParser()
parser.add_argument("--revision", type=Path, help="export the blue housing revision to this path")
args = parser.parse_args(argv)
bpy.ops.wm.read_factory_settings(use_empty=True)
scene = bpy.context.scene
scene.unit_settings.system = "METRIC"


def mat(name, rgb, metallic=0, rough=.45):
    material=bpy.data.materials.new(name)
    material.use_nodes=True
    n=material.node_tree.nodes.get("Principled BSDF")
    n.inputs["Base Color"].default_value=(*rgb,1)
    n.inputs["Metallic"].default_value=metallic
    n.inputs["Roughness"].default_value=rough
    material.diffuse_color=(*rgb,1)
    return material


housing = mat("Anodized", (.072,.093,.092) if not args.revision else (.026,.076,.135), .66,.28)
rim = mat("Machined",(.30,.344,.317),.83,.22)
black = mat("Carbon", (.022,.027,.027),.26,.38)
deep = mat("Recess", (.008,.012,.012),.04,.46)
rubber = mat("Rubber",(.032,.033,.029),.03,.77)
orange = mat("Cerakote",(.92,.143,.020),.32,.28)
silver = mat("Steel",(.52,.54,.50),.92,.20)
cream = mat("Ivory",(.77,.75,.612),.14,.39)
display = mat("Display",(.089,.127,.134),0,.48)
nameplate = mat("Nameplate",(.69,.67,.51),.20,.42)
floor = mat("Cyclorama",(.35,.38,.35),0,.92)


def empty(name, parent=None, loc=(0,0,0)):
    o=bpy.data.objects.new(name,None)
    scene.collection.objects.link(o)
    o.parent=parent
    o.location=loc
    return o


def finish(o,name,loc,parent,material):
    o.name=name
    o.data.name=name+"Mesh"
    o.parent=parent
    o.location=loc
    o.data.materials.append(material)
    return o


def box(name,p,d,r,material,parent,segments=4):
    bpy.ops.mesh.primitive_cube_add(size=1)
    o=bpy.context.object
    for v in o.data.vertices:
        for i in range(3):
            v.co[i]*=d[i]
    if r:
        mod=o.modifiers.new("Machined edge","BEVEL"); mod.width=r; mod.segments=segments
        mod.harden_normals=True
        bpy.ops.object.modifier_apply(modifier=mod.name)
        for poly in o.data.polygons: poly.use_smooth=True
        mod=o.modifiers.new("Face normals","WEIGHTED_NORMAL"); mod.keep_sharp=True
        bpy.ops.object.modifier_apply(modifier=mod.name)
    return finish(o,name,p,parent,material)


def cylinder(name,p,r,h,material,parent,axis="Z",sides=48,bevel=.0005):
    bpy.ops.mesh.primitive_cylinder_add(vertices=sides,radius=r,depth=h)
    o=bpy.context.object
    for f in o.data.polygons:
        f.use_smooth=len(f.vertices)==4
    if bevel:
        b=o.modifiers.new("Edge break","BEVEL"); b.width=min(bevel,h*.29); b.segments=3
        b.harden_normals=True
        bpy.ops.object.modifier_apply(modifier=b.name)
        w=o.modifiers.new("End normals","WEIGHTED_NORMAL")
        bpy.ops.object.modifier_apply(modifier=w.name)
    o.rotation_euler=(0, math.pi/2,0) if axis=="X" else (math.pi/2,0,0) if axis=="Y" else (0,0,0)
    return finish(o,name,p,parent,material)


def quad(name,parent,coords,material):
    mesh=bpy.data.meshes.new(name+"Mesh")
    mesh.from_pydata(coords,[],[(0,1,2,3)])
    uv=mesh.uv_layers.new(name="st")
    for item,coords in zip(uv.data,[(0,0),(1,0),(1,1),(0,1)]): item.uv=coords
    obj=bpy.data.objects.new(name,mesh); scene.collection.objects.link(obj)
    obj.parent=parent; mesh.materials.append(material)
    return obj


def screw(name,x,y,z,parent,axis):
    cylinder(name,(x,y,z),.0028,.0008,silver,parent,axis=axis,sides=24,bevel=.00015)
    if axis=="Z":
        o=box(name+"Drive",(x,y,z+.00044),(.0033,.0007,.00012),0,black,parent)
        o.rotation_euler.z=math.radians(28)
    else:
        o=box(name+"Drive",(x,y-.00044,z),(.0033,.00012,.0007),0,black,parent)
        o.rotation_euler.y=math.radians(28)


rec=empty("Recorder")
base=empty("Base",rec)
hinge=empty("Hinge",rec,(0,.059,.041))
# Three gasket-separated shells, floating on rubber skids.
box("Sole",(0,-.008,.009),(.245,.142,.013),.005,black,base,6)
box("Gasket",(0,-.008,.017),(.262,.157,.007),.003,rubber,base)
box("Deck",(0,-.008,.027),(.265,.159,.018),.0055,housing,base,6)
for x in [-.095,.095]:
    box("SkidL" if x<0 else "SkidR",(x,-.007,.0027),(.018,.12,.0054),.0022,rubber,base)
# Inlay and rectangular printed inventory plaque.
box("PlaqueRecess",(-.062,-.038,.0359),(.116,.032,.0026),.00125,deep,base)
plate=quad("Plaque",base,[(-.117,-.052,.0374),(-.007,-.052,.0374),(-.007,-.024,.0374),(-.117,-.024,.0374)],nameplate)
# Hardware encoder, radially knurled crown and inset directional position.
for x,crown in [(.086,True),(.044,False)]:
    r=.019 if crown else .014
    cylinder("EncoderWell",(x,-.033,.0372),r+.002,.0022,deep,base)
    cylinder("Encoder",(x,-.033,.041),r,.008,orange if crown else black,base)
    cylinder("EncoderFace",(x,-.033,.0454),r-.0024,.0013,housing,base)
    box("EncoderIndex",(x,-.033-r*.55,.0462),(.0016,r*.51,.00035),.00012,cream,base)
    for i in range(24 if crown else 16):
        a=i*2*math.pi/(24 if crown else 16)
        o=box("Knurl",(x+r*math.cos(a),-.033+r*math.sin(a),.0406),(.00065,.0011,.0037),.00018,black,base,2)
        o.rotation_euler.z=a
# Twelve slots opening into the rear loudspeaker recess.
for i in range(12):
    box("SpeakerSlot",(-.102+i*.009,.018,.0360),(.0033,.028,.0015),.0007,deep,base)
for i in range(4):
    x=.032+i*.019
    box("KeyWell",(x,.019,.0362),(.016,.015,.0024),.001,black,base)
    box("Keycap",(x,.019,.0385),(.012,.011,.004),.0017,orange if i==0 else rim,base)
for x in [-.119,.119]:
    for y in [-.074,.046]:
        screw("DeckFastener",x,y,.036,base,"Z")
for i in range(6):
    box("InputVent",(-.052+i*.011,-.08765,.026),(.006,.0005,.005),.00022,deep,base)
if args.revision:
    # The touring housing adds replaceable corner bumpers while retaining hinge and slot paths.
    for x in [-.125,.125]:
        box("TourBumper",(x,-.071,.024),(.020,.037,.027),.0045,rubber,base)
# The hinge shaft has stationary end-bearing blocks and rotating central sleeves.
cylinder("Axle",(0,.059,.041),.006,.247,silver,base,"X")
for x in [-.114,.114]:
    cylinder("Bearing",(x,.059,.041),.010,.023,orange,base,"X")
for x in [-.065,.065]:
    cylinder("Bushing",(x,0,0),.0082,.064,black,hinge,"X")
# Rear anodized shell, bright machined edge and a recessed glass display.
box("BackCover",(0,.002,.0695),(.255,.011,.138),.0045,housing,hinge,6)
box("EdgeBand",(0,-.00385,.0695),(.250,.0023,.134),.0011,rim,hinge)
box("FrontBezel",(0,-.0063,.0695),(.247,.0045,.130),.0021,black,hinge)
box("DisplayWell",(0,-.00885,.072),(.213,.0015,.122),.0007,deep,hinge)
w,h=.2016,.1134
panel=quad("Monitor",hinge,[(-w/2,-.0098,.072-h/2),(w/2,-.0098,.072-h/2),
                             (w/2,-.0098,.072+h/2),(-w/2,-.0098,.072+h/2)],display)
# Physical glass guard: where it crosses the left edge, it must occlude live pixels.
for x in [-.104,.104]:
    box("GlassRail",(x,-.0109,.072),(.0068,.0033,.118),.00145,rim,hinge)
for x in [-.115,.115]:
    for z in [.017,.123]:
        screw("LidFastener",x,-.00855,z,hinge,"Y")
box("MonitorFlag",(-.107,-.0109,.090),(.024,.0029,.0048),.0009,orange,hinge)
for i in range(3):
    box("WitnessMark",(.112,-.0087,.046+i*.005),(.006,.0008,.0016),.00025,cream,hinge)
# Separate floor prim is selectable for shadow and lighting tests.
ground=empty("Studio")
quad("Floor",ground,[(-24,-24,-.0002),(24,-24,-.0002),(24,24,-.0002),(-24,24,-.0002)],floor)

target=args.revision or ROOT/"assets/field.usdc"
bpy.ops.wm.usd_export(filepath=str(target),root_prim_path="/World",export_animation=False,
                          export_subdivision="TESSELLATE",triangulate_meshes=True,export_uvmaps=True,
                          rename_uvmaps=True,export_mesh_colors=False,export_lights=False,export_cameras=False,
                          export_custom_properties=False,convert_world_material=False,generate_materialx_network=False,
                          convert_orientation=False,export_materials=True,xform_op_mode="TRS")
