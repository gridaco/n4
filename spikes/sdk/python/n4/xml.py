"""Emit N4 visual sequence XML from an authored sequence mapping."""
import math
from xml.etree import ElementTree as ET


def _fields(mapping, allowed):
    extras = mapping.keys() - set(allowed.split())
    if extras:
        raise ValueError(f"Unrecognized fields in XML artwork: {sorted(extras)}")


def _number(value):
    if type(value) is int:
        return str(value)
    if type(value) is not float or not math.isfinite(value):
        raise ValueError(f"Expected a finite number, got {value!r}")
    return str(int(value)) if value.is_integer() else str(value)


def _numbers(values):
    return " ".join(_number(value) for value in values)


def _value(value):
    return _numbers(value) if isinstance(value, (tuple, list)) else _number(value)


def _color(values):
    if values is None:
        return "none"
    if (not isinstance(values, (tuple, list)) or len(values) not in (3, 4)
            or any(type(channel) is not int or not 0 <= channel <= 255 for channel in values)):
        raise ValueError(f"Expected RGB or RGBA channel bytes, got {values!r}")
    return "#" + bytes(values).hex()


def _attr(node, data, key, encode=str, name=None):
    if key in data:
        node.set(name or key.replace("_", "-"), encode(data[key]))


def _enum(value):
    return value.replace("_", "-")


def _ease(value):
    if isinstance(value, dict):
        _fields(value, "cubic")
        return "cubic(" + _numbers(value["cubic"]) + ")"
    return value


def _animate(node, name, keys):
    track = ET.SubElement(node, "animate", attributeName=name)
    _write_keys(track, keys)


def _write_keys(track, keys):
    for key in keys:
        _fields(key, "frame value ease")
        pose = ET.SubElement(track, "key", frame=_number(key["frame"]), value=_value(key["value"]))
        _attr(pose, key, "ease", _ease)


def _paint(node, content):
    _attr(node, content, "fill", _color)
    if "stroke" in content:
        stroke = content["stroke"]
        _fields(stroke, "rgba width cap join")
        node.set("stroke", _color(stroke["rgba"]))
        if isinstance(stroke.get("width"), list):
            _animate(node, "stroke-width", stroke["width"])
        else:
            _attr(node, stroke, "width", _number, "stroke-width")
        _attr(node, stroke, "cap", str, "stroke-linecap")
        _attr(node, stroke, "join", str, "stroke-linejoin")


def _path(node, content):
    _fields(content, "type d fill fill_rule stroke stroke_start stroke_end")
    _paint(node, content)
    _attr(node, content, "fill_rule", {"even_odd": "evenodd", "nonzero": "nonzero"}.__getitem__)
    for field in ("stroke_start", "stroke_end"):
        if isinstance(content.get(field), list):
            _animate(node, field.replace("_", "-"), content[field])
        else:
            _attr(node, content, field, _number)
    path = content["d"]
    if isinstance(path, str):
        node.set("d", path)
        return
    _fields(path, "closed keys")
    track = ET.SubElement(node, "animate", attributeName="d")
    _attr(track, path, "closed", lambda b: {True: "true", False: "false"}[b])
    for key in path["keys"]:
        _fields(key, "frame points ease")
        points = " ".join(",".join(_number(n) for n in pt) for pt in key["points"])
        pose = ET.SubElement(track, "key", frame=_number(key["frame"]), points=points)
        _attr(pose, key, "ease", _ease)


def _rect(node, content):
    _fields(content, "type x y width height corner_radius fill stroke")
    for field in ("x", "y", "width", "height", "corner_radius"):
        if field in content:
            v = content[field]
            if isinstance(v, list) and (not v or isinstance(v[0], dict)):
                _animate(node, field.replace("_", "-"), v)
            else:
                node.set(field.replace("_", "-"), _value(v))
    _paint(node, content)


def _text(node, content):
    _fields(content, "type text font size width rgb underline")
    for key in ("font", "size", "width"):
        node.set(key, str(content[key]))
    node.set("fill", _color(content["rgb"]))
    copy = ET.SubElement(node, "copy")
    if isinstance(content["text"], str):
        copy.text = content["text"]
    else:
        for part in content["text"]:
            _fields(part, "id text")
            span = ET.SubElement(copy, "span")
            _attr(span, part, "id")
            span.text = part["text"]
    if "underline" in content:
        line = content["underline"]
        _fields(line, "style rgba diameter spacing offset")
        element = ET.SubElement(node, "underline")
        for key in ("style", "diameter", "offset", "spacing"):
            _attr(element, line, key)
        element.set("fill", _color(line["rgba"]))


def _scalar_attrs(node, data, fields):
    for field in fields:
        if isinstance(data.get(field), list):
            _animate(node, field.replace("_", "-"), data[field])
        else:
            _attr(node, data, field, _number)


def _source_effect(parent, data):
    kind = data["type"]
    if kind == "source_blur":
        _fields(data, "type sigma amount focus_band")
        node = ET.SubElement(parent, "source-blur")
        _scalar_attrs(node, data, ("sigma", "amount"))
        if "focus_band" in data:
            band = data["focus_band"]
            _fields(band, "center_y half_width feather")
            _scalar_attrs(ET.SubElement(node, "focus-band"), band, ("center_y", "half_width", "feather"))
    elif kind == "rgb_grade":
        _fields(data, "type gain offset")
        node = ET.SubElement(parent, "rgb-grade")
        for name in ("gain", "offset"):
            if name in data:
                value = data[name]
                if isinstance(value, list) and (not value or isinstance(value[0], dict)):
                    _animate(node, name, value)
                else:
                    node.set(name, _numbers(value))
    else:
        raise ValueError(f"Unsupported source effect {kind!r}")


def _scene3d(node, content):
    _fields(content, "type pixels_per_meter lights models planes")
    _attr(node, content, "pixels_per_meter", _number)
    for light in content.get("lights", []):
        _fields(light, "direction intensity color")
        child = ET.SubElement(node, "distant-light", direction=_numbers(light["direction"]))
        _attr(child, light, "intensity", _number)
        _attr(child, light, "color", _color)
    for model in content.get("models", []):
        _fields(model, "id source prim position rotation_x rotation_y rotation_z")
        child = ET.SubElement(node, "usd", source=model["source"], prim=model["prim"])
        _attr(child, model, "id")
        _attr(child, model, "position", _numbers)
        for field in ("rotation_x", "rotation_y", "rotation_z"):
            if isinstance(model.get(field), list):
                _animate(child, field.replace("_", "-"), model[field])
            else:
                _attr(child, model, field, _number)
    for plane in content.get("planes", []):
        _fields(plane, "id position text")
        child = ET.SubElement(node, "plane", position=_numbers(plane["position"]))
        _attr(child, plane, "id")
        _graphic(child, plane["text"])


def _graphic(parent, graphic):
    _fields(graphic, "name position span blend transform content mask effects text_anchor type_on text_animators motion channels filter")
    content = graphic["content"]
    kind = content["type"]
    tag = {"group": "g", "transform_group": "transform-group", "label": "text"}.get(kind, kind)
    node = ET.SubElement(parent, tag)
    _attr(node, graphic, "name")
    _attr(node, graphic, "position", _numbers)
    _attr(node, graphic, "span", _numbers)
    _attr(node, graphic, "blend", _enum)
    if "transform" in graphic:
        transform = graphic["transform"]
        _fields(transform, "pivot rotation_degrees scale scale_x scale_y")
        _attr(node, transform, "pivot", _numbers)
        _attr(node, transform, "rotation_degrees", _number, "rotation")
        for field in ("scale", "scale_x", "scale_y"):
            _attr(node, transform, field, _number)

    if kind == "scene3d":
        _scene3d(node, content)
    elif kind == "rect":
        _rect(node, content)
    elif kind == "path":
        _path(node, content)
    elif kind == "label":
        _text(node, content)
    elif kind in ("group", "transform_group"):
        _fields(content, "type children")
    elif kind in ("image", "svg"):
        _fields(content, "type source")
        node.set("source", content["source"])
    elif kind == "video":
        _fields(content, "type source source_in start duration size")
        node.set("source", content["source"])
        for field in ("start", "source_in", "duration"):
            _attr(node, content, field, _number)
        _attr(node, content, "size", _numbers)
    elif kind == "footage":
        _fields(content, "type history")
        if "history" in content:
            history = content["history"]
            _fields(history, "frames shader")
            ET.SubElement(node, "history", frames=_number(history["frames"]), source=history["shader"])
    elif kind == "backdrop":
        _fields(content, "type filter")
        effect = content["filter"]
        _fields(effect, "shader sample_radius_px sigma_px params")
        if ("sample_radius_px" in effect) == ("sigma_px" in effect):
            raise ValueError("A backdrop requires exactly one of sample_radius_px or sigma_px")
        node.set("source", effect["shader"])
        _attr(node, effect, "sample_radius_px", _number, "sample-radius")
        _attr(node, effect, "sigma_px", _number, "sigma")
        if "sigma_px" in effect and effect.get("params"):
            raise ValueError("Parameters require a 2D backdrop")
        for param in effect.get("params", []):
            _fields(param, "name value")
            control = ET.SubElement(node, "param", name=param["name"])
            if isinstance(param["value"], list):
                _write_keys(control, param["value"])
            else:
                control.set("value", _number(param["value"]))
    else:
        raise ValueError(f"Unsupported XML content type {kind!r}")
    for effect in graphic.get("effects", []):
        _source_effect(node, effect)
    if "filter" in graphic:
        effect = graphic["filter"]
        _fields(effect, "shader amount mattes")
        child = ET.SubElement(node, "filter", source=effect["shader"])
        _attr(child, effect, "amount", _number)
        for matte in effect.get("mattes", []):
            _fields(matte, "name object occluded_by coverage")
            element = ET.SubElement(child, "matte", name=matte["name"], object=matte["object"])
            _attr(element, matte, "occluded_by")
            _attr(element, matte, "coverage", _enum)
    if "mask" in graphic:
        mask = graphic["mask"]
        _fields(mask, "position content")
        child = ET.SubElement(node, "mask")
        _attr(child, mask, "position", _numbers)
        _graphic(child, {"content": mask["content"]})
    if "text_anchor" in graphic:
        anchor = graphic["text_anchor"]
        _fields(anchor, "target point")
        ET.SubElement(node, "text-anchor", target=anchor["target"], point=_enum(anchor["point"]))
    if "type_on" in graphic:
        reveal = ET.SubElement(node, "type-on")
        for frame, count in graphic["type_on"]:
            ET.SubElement(reveal, "key", frame=_number(frame), value="all" if count is None else _number(count))
    for animator in graphic.get("text_animators", []):
        _fields(animator, "target fill offset start end")
        child = ET.SubElement(node, "text-animator")
        _attr(child, animator, "target")
        _attr(child, animator, "fill", _color)
        _attr(child, animator, "offset", _numbers)
        for key in ("start", "end"):
            if key in animator:
                _animate(child, key, animator[key])
    if "motion" in graphic:
        track = ET.SubElement(node, "motion")
        for pose in graphic["motion"]:
            _fields(pose, "frame x y scale opacity")
            key = ET.SubElement(track, "key")
            for field in ("frame", "x", "y", "scale", "opacity"):
                _attr(key, pose, field, _number)
    if "channels" in graphic:
        channels = graphic["channels"]
        fields = ("x", "y", "scale", "scale_x", "scale_y", "rotation", "opacity")
        _fields(channels, " ".join(fields))
        for field in fields:
            if field in channels:
                name = "translate-" + field if field in ("x", "y") else field.replace("_", "-")
                _animate(node, name, channels[field])
    for child in content.get("children", []):
        _graphic(node, child)


def _indent(node, depth=0):
    # <copy> is a lossless text island. Its whitespace, including a standalone
    # space in a span, is always source copy, never pretty-printer indentation.
    if not len(node) or node.tag == "copy":
        return
    prefix = "\n" + "  " * depth
    node.text = prefix + "  "
    for child in node:
        _indent(child, depth + 1)
        child.tail = prefix + "  "
    node[-1].tail = prefix


def _camera(root, view):
    projection = view.get("projection", "orthographic")
    fields = {"orthographic": ("center_x", "center_y", "zoom", "roll"),
              "perspective": ("fov_y",)}
    if projection not in fields:
        raise ValueError(f"Unknown camera projection {projection!r}")
    allowed = ["id", "projection", *fields[projection]]
    if projection == "perspective":
        allowed.extend(["position", "rotation"])
    _fields(view, " ".join(allowed))
    node = ET.SubElement(root, "camera", id=view["id"])
    if projection == "perspective":
        node.set("projection", projection)
        for prop in ("position", "rotation"):
            if prop in view:
                values = view[prop]
                if len(values) != 3:
                    raise ValueError(f"Expected three axes for {prop}")
                if any(isinstance(axis, list) for axis in values):
                    for axis, value in zip("xyz", values):
                        key = f"{prop}-{axis}"
                        if isinstance(value, list):
                            _animate(node, key, value)
                        else:
                            node.set(key, _number(value))
                else:
                    node.set(prop, _numbers(values))
    for prop in fields[projection]:
        value = view.get(prop)
        if value is not None:
            name = prop.replace("_", "-")
            if isinstance(value, list):
                _animate(node, name, value)
            else:
                node.set(name, _number(value))


def encode_sequence(document):
    """Return canonical UTF-8 visual sequence XML. Unknown fields fail before saving."""
    _fields(document, "name source source_in background graphics camera cameras")
    root = ET.Element("sequence")
    _attr(root, document, "name")
    _attr(root, document, "camera")
    _attr(root, document, "source")
    _attr(root, document, "source_in", _number)
    if "background" in document:
        _fields(document["background"], "color")
        root.set("background", _color(document["background"]["color"]))
    for view in document.get("cameras", []):
        _camera(root, view)
    for graphic in document.get("graphics", []):
        _graphic(root, graphic)
    _indent(root)
    return ET.tostring(root, encoding="utf-8").replace(b"\r", b"&#13;") + b"\n"
