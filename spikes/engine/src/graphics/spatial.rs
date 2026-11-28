//! Shot-local spatial assets and native artwork. Positions use metres and a Y-up basis.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Scene3d {
    pub pixels_per_meter: Scalar,
    #[serde(default)]
    pub lights: Vec<DistantLight>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub studio: Option<StudioLook>,
    #[serde(default)]
    pub models: Vec<UsdInstance>,
    #[serde(default)]
    pub planes: Vec<TextPlane>,
    /// Area lights any renderer can reproduce. With at least one, the scene is lit by its rect
    /// lights and `ambient`, and the analytic studio environment is not used.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rect_lights: Vec<RectLight>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambient: Option<AmbientLight>,
    /// Ask for a path-traced Final. The rasterizer then supplies the Draft.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_trace: Option<PathTrace>,
}

/// A one-sided rectangle centred at `center`, facing `aim`, with its long edge horizontal.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RectLight {
    pub center: [Scalar; 3],
    pub aim: [Scalar; 3],
    /// Width and height in metres.
    pub size: [Scalar; 2],
    /// Emitted scene-linear radiance for white.
    pub intensity: Scalar,
    pub color: [u8; 3],
}

/// Constant radiance arriving from every direction not covered by a light.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AmbientLight {
    pub intensity: Scalar,
    pub color: [u8; 3],
}

/// Everything that fixes a path-traced frame's sampling. Preview and export both use it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PathTrace {
    pub samples: u32,
    pub bounces: u32,
    /// Upper bound on one indirect sample's contribution; zero disables it.
    pub clamp: Scalar,
    pub denoise: bool,
    pub seed: u32,
}

fn unit_scalar() -> Scalar {
    Scalar::try_from(1.).unwrap()
}

fn is_unit(value: &Scalar) -> bool {
    f32::from(*value) == 1.
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DistantLight {
    pub direction: [Scalar; 3],
    pub intensity: Scalar,
    pub color: [u8; 3],
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UsdInstance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub source: PathBuf,
    pub prim: String,
    #[serde(default)]
    pub position: [Scalar; 3],
    #[serde(default)]
    pub rotation_x: AnimatedScalar<Scalar>,
    #[serde(default)]
    pub rotation_y: AnimatedScalar<Scalar>,
    #[serde(default)]
    pub rotation_z: AnimatedScalar<Scalar>,
    #[serde(default)]
    pub parts: Vec<RigidPart>,
    #[serde(default)]
    pub surfaces: Vec<LiveSurface>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RigidPart {
    pub prim: String,
    #[serde(default)]
    pub rotation_x: AnimatedScalar<Scalar>,
    #[serde(default)]
    pub rotation_y: AnimatedScalar<Scalar>,
    #[serde(default)]
    pub rotation_z: AnimatedScalar<Scalar>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LiveSurface {
    pub slot: String,
    pub role: SurfaceRole,
    pub size: [u32; 2],
    pub layers: Vec<Graphic>,
    /// For an emissive surface: emitted radiance per unit of artwork, as the camera sees it.
    #[serde(default = "unit_scalar", skip_serializing_if = "is_unit")]
    pub emission: Scalar,
    /// For an emissive surface: extra gain on the light it casts on the rest of the scene.
    #[serde(default = "unit_scalar", skip_serializing_if = "is_unit")]
    pub glow: Scalar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SurfaceRole {
    /// Shown as authored. It lights nothing.
    Unlit,
    BaseColor,
    /// A screen: shown as authored times `emission`, and in a path-traced frame it lights and
    /// reflects in the scene. The rasterizer approximates it with one rect light.
    Emissive,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StudioLook {
    pub environment_rotation: AnimatedScalar<Scalar>,
    pub environment_intensity: AnimatedScalar<Scalar>,
    pub exposure: AnimatedScalar<Scalar>,
    pub shadow_center: [Scalar; 3],
    pub shadow_extent: Scalar,
    pub shadow_bias: Scalar,
    pub shadow_softness: Scalar,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextPlane {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub position: [Scalar; 3],
    pub text: Box<Graphic>,
}

/// A sampled screen-space mask exported to the scene's WGSL image filter.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectMatte {
    pub name: String,
    pub object: String,
    #[serde(default, skip_serializing_if = "MatteCoverage::is_projected")]
    pub coverage: MatteCoverage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occluded_by: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatteCoverage {
    #[default]
    Projected,
    Visible,
}

impl MatteCoverage {
    fn is_projected(&self) -> bool {
        *self == Self::Projected
    }
}

impl ObjectMatte {
    /// The object that must own the visible sample, or any owner for a full projection.
    pub fn owner(&self) -> Option<&str> {
        match self.coverage {
            MatteCoverage::Projected => self.occluded_by.as_deref(),
            MatteCoverage::Visible => Some(&self.object),
        }
    }
}

fn contains_primary_footage(layers: &[Graphic]) -> bool {
    layers.iter().any(|layer| match &layer.content {
        Content::Footage { .. } => true,
        Content::Group { children } | Content::TransformGroup { children } => {
            contains_primary_footage(children)
        }
        _ => false,
    })
}

impl Scene3d {
    pub(crate) fn validate(
        &self,
        duration: u32,
        timeline_end: u32,
        mattes: &[ObjectMatte],
    ) -> Result<()> {
        self.validate_mattes(mattes)?;
        ensure!(
            f32::from(self.pixels_per_meter) > 0.,
            "scene3d pixels-per-meter must be positive"
        );
        if let Some(look) = &self.studio {
            look.environment_rotation
                .range("studio environment rotation")?;
            ensure!(
                look.environment_intensity
                    .range("studio environment intensity")?[0]
                    >= 0.,
                "environment intensity must be nonnegative"
            );
            ensure!(
                look.exposure.range("studio exposure")?[1]
                    .exp2()
                    .is_finite(),
                "studio exposure exceeds the supported linear gain"
            );
            ensure!(
                f32::from(look.shadow_extent) > 0.
                    && f32::from(look.shadow_bias) >= 0.
                    && f32::from(look.shadow_softness) >= 0.,
                "invalid studio shadow extent, bias or softness"
            );
        }
        for light in &self.lights {
            ensure!(
                light.direction.iter().any(|&v| f32::from(v) != 0.)
                    && f32::from(light.intensity) >= 0.,
                "distant-light requires a nonzero direction and nonnegative intensity"
            );
        }
        for light in &self.rect_lights {
            let offset: [f32; 3] =
                std::array::from_fn(|i| f32::from(light.aim[i]) - f32::from(light.center[i]));
            ensure!(
                offset.iter().any(|&v| v != 0.)
                    && light.size.iter().all(|&v| f32::from(v) > 0.)
                    && f32::from(light.intensity) >= 0.,
                "rect-light requires an aim away from its center, a positive size and nonnegative intensity"
            );
        }
        if let Some(ambient) = &self.ambient {
            ensure!(
                f32::from(ambient.intensity) >= 0.,
                "ambient-light intensity must be nonnegative"
            );
            ensure!(
                !self.rect_lights.is_empty(),
                "ambient-light applies to a scene lit by rect-light; the studio environment lights other scenes"
            );
        }
        if let Some(trace) = &self.path_trace {
            ensure!(
                (1..=65_536).contains(&trace.samples) && trace.bounces <= 64,
                "path-trace requires 1 to 65536 samples and at most 64 bounces"
            );
            ensure!(
                f32::from(trace.clamp) >= 0.,
                "path-trace clamp must be nonnegative"
            );
            ensure!(
                !self.rect_lights.is_empty() && self.lights.is_empty() && self.planes.is_empty(),
                "a path-traced scene is lit by rect-light and ambient-light and holds USD models; distant-light and text planes are not yet path traced"
            );
        }
        for model in &self.models {
            ensure!(
                model.prim.starts_with('/') && !model.source.as_os_str().is_empty(),
                "usd requires a source and absolute prim path"
            );
            for channel in [&model.rotation_x, &model.rotation_y, &model.rotation_z] {
                channel.range("USD instance rotation")?;
            }
            let mut paths = HashSet::new();
            for part in &model.parts {
                ensure!(
                    part.prim.starts_with('/') && paths.insert(&part.prim),
                    "expected unique absolute USD part path: {}",
                    part.prim
                );
                for channel in [&part.rotation_x, &part.rotation_y, &part.rotation_z] {
                    channel.range("USD part rotation")?;
                }
            }
            let mut slots = HashSet::new();
            for surface in &model.surfaces {
                ensure!(
                    surface.slot.starts_with('/') && slots.insert(&surface.slot),
                    "expected unique absolute material slot: {}",
                    surface.slot
                );
                ensure!(
                    surface
                        .size
                        .iter()
                        .all(|v| (1..=crate::MAX_SURFACE_DIMENSION).contains(v)),
                    "invalid live surface dimensions"
                );
                ensure!(
                    f32::from(surface.emission) > 0. && f32::from(surface.glow) >= 0.,
                    "surface {} requires positive emission and nonnegative glow",
                    surface.slot
                );
                ensure!(
                    surface.role == SurfaceRole::Emissive
                        || (is_unit(&surface.emission) && is_unit(&surface.glow)),
                    "emission and glow apply to an emissive surface: {}",
                    surface.slot
                );
                ensure!(
                    !crate::camera::contains_scene3d(&surface.layers)
                        && !contains_primary_footage(&surface.layers),
                    "surface {} must use independent artwork or video; sampling the containing scene or its footage creates a dependency cycle",
                    surface.slot
                );
                super::validate_scope(&surface.layers, duration, timeline_end, true, [0; 2], None)?;
            }
        }
        for plane in &self.planes {
            let label = &*plane.text;
            ensure!(
                matches!(label.content, Content::Label { .. })
                    && label.blend == Blend::SourceOver
                    && !label.has_spatial_transform()
                    && label.channels.is_none()
                    && label.text_anchor.is_none()
                    && label.filter.is_none()
                    && label.mask.is_none()
                    && label.wipe.is_none()
                    && label.effects.is_empty()
                    && label.underlay.is_none(),
                "a spatial plane accepts editable text, spans and text animators; apply spatial placement on the plane and image effects on the scene"
            );
            super::validate(std::slice::from_ref(label), duration)?;
        }
        Ok(())
    }

    fn validate_mattes(&self, mattes: &[ObjectMatte]) -> Result<()> {
        let mut ids = HashSet::new();
        for id in self
            .models
            .iter()
            .filter_map(|m| m.id.as_deref())
            .chain(self.planes.iter().filter_map(|p| p.id.as_deref()))
        {
            ensure!(
                !id.is_empty()
                    && id
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_')),
                "invalid scene object ID {id:?}"
            );
            ensure!(ids.insert(id), "duplicate scene object ID {id:?}");
        }
        ensure!(
            mattes.len() <= 8,
            "a scene filter supports at most eight object mattes"
        );
        let mut names = HashSet::new();
        for matte in mattes {
            let name = &matte.name;
            ensure!(
                name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
                    && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                    && !name.contains("__"),
                "invalid matte name {name:?}; expected an ASCII shader identifier starting with a letter"
            );
            ensure!(names.insert(name), "duplicate matte name {name:?}");
            ensure!(
                ids.contains(matte.object.as_str()),
                "matte {name:?} references unknown object {:?}",
                matte.object
            );
            if let Some(occluder) = &matte.occluded_by {
                ensure!(
                    matte.coverage == MatteCoverage::Projected,
                    "matte {name:?} cannot combine visible coverage with occluded-by"
                );
                ensure!(
                    occluder != &matte.object,
                    "matte {name:?} cannot use the same object as its occluder"
                );
                ensure!(
                    ids.contains(occluder.as_str()),
                    "matte {name:?} references unknown occluder {occluder:?}"
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn clipped_scene_keeps_video_in_its_owning_shot_clock() -> Result<()> {
        let g: Graphic = serde_json::from_value(json!({
            "span":[3,5],
            "content":{"type":"scene3d","pixels_per_meter":100,"models":[{
                "source":"card.usda","prim":"/Card","surfaces":[{
                    "slot":"/Display","role":"unlit","size":[32,32],"layers":[{
                        "content":{"type":"video","source":"display.mp4","source_in":11,
                                   "start":3,"duration":2,"size":[32,32]}
                    }]
                }]
            }]}
        }))?;
        validate(std::slice::from_ref(&g), 5)?;
        let mut clips = Vec::new();
        crate::project::collect_video_clips(std::slice::from_ref(&g), Some(2), &mut clips);
        assert!(clips.is_empty());
        crate::project::collect_video_clips(std::slice::from_ref(&g), Some(3), &mut clips);
        assert_eq!(clips[0].source_frame(3), Some(11));
        Ok(())
    }

    #[test]
    fn matte_references_resolve_within_one_scene_and_keep_their_binding_order() -> Result<()> {
        let mut layer = json!({
            "content": {"type":"scene3d", "pixels_per_meter":100,
                "models":[{"id":"ribbon", "prim":"/Mesh", "source":"prop.usda"}],
                "planes":[{"id":"word", "position":[0,0,0], "text":{"content":{"type":"label",
                    "font":"type.ttf","width":80,"size":16,"rgb":[0,0,0],"text":"PHASE"}}}]},
            "filter":{"shader":"look.wgsl","amount":100, "mattes":[
                {"name":"coverage","object":"word"},
                {"name":"hidden","object":"word","occluded_by":"ribbon"},
                {"name":"visible","object":"word","coverage":"visible"}]}
        });
        let decode = |value| -> Result<Graphic> { Ok(serde_json::from_value(value)?) };
        let graphic = decode(layer.clone())?;
        assert_eq!(
            graphic
                .filter
                .as_ref()
                .unwrap()
                .mattes
                .iter()
                .map(ObjectMatte::owner)
                .collect::<Vec<_>>(),
            [None, Some("ribbon"), Some("word")]
        );
        validate(&[graphic.clone(), graphic], 1)?;
        let invalid = |value, expected: &str| -> Result<()> {
            let error = validate(&[decode(value)?], 1).unwrap_err();
            assert!(format!("{error:#}").contains(expected), "{error:#}");
            Ok(())
        };
        for (field, value, reason) in [
            ("name", "hidden", "duplicate matte name"),
            ("name", "a);", "invalid matte name"),
            ("name", "a__b", "invalid matte name"),
            ("object", "elsewhere", "unknown object"),
            ("occluded_by", "word", "same object"),
            ("occluded_by", "elsewhere", "unknown occluder"),
        ] {
            let mut candidate = layer.clone();
            candidate["filter"]["mattes"][0][field] = json!(value);
            invalid(candidate, reason)?;
        }
        let mut ambiguous = layer.clone();
        ambiguous["filter"]["mattes"][1]["coverage"] = json!("visible");
        invalid(ambiguous, "cannot combine visible coverage")?;
        let mut too_many = layer.clone();
        too_many["filter"]["mattes"] = json!(
            (0..9)
                .map(|i| json!({"name":format!("m{i}"),"object":"word"}))
                .collect::<Vec<_>>()
        );
        invalid(too_many, "at most eight")?;
        let mut outside = layer.clone();
        outside["content"] = json!({"type":"group","children":[]});
        invalid(outside, "require a filter on a scene3d")?;
        layer["content"]["planes"][0]["id"] = json!("ribbon");
        invalid(layer, "duplicate scene object ID")?;
        Ok(())
    }

    #[test]
    fn a_path_traced_scene_declares_lights_any_renderer_can_reproduce() -> Result<()> {
        let scene = json!({"type":"scene3d","pixels_per_meter":100,
            "rect_lights":[{"center":[0,1,0],"aim":[0,0,0],"size":[0.5,0.3],"intensity":11,"color":[255,240,220]}],
            "ambient":{"intensity":0.05,"color":[255,255,255]},
            "path_trace":{"samples":64,"bounces":8,"clamp":10,"denoise":true,"seed":0},
            "models":[{"source":"card.usda","prim":"/Card","surfaces":[{
                "slot":"/Display","role":"emissive","glow":4,"size":[32,32],
                "layers":[{"content":{"type":"video","source":"display.mp4","source_in":0,
                           "start":0,"duration":1,"size":[32,32]}}]}]}]});
        let decode = |content: &serde_json::Value| -> Result<Graphic> {
            Ok(serde_json::from_value(json!({"content": content}))?)
        };
        let graphic = decode(&scene)?;
        validate(std::slice::from_ref(&graphic), 1)?;
        // Defaults are not written back, so documents without these features keep their bytes.
        let Content::Scene3d(decoded) = &graphic.content else {
            unreachable!()
        };
        let written = serde_json::to_value(&decoded.models[0].surfaces[0])?;
        assert_eq!(written["glow"], json!(4.0));
        assert!(written.get("emission").is_none());
        for (path, value, reason) in [
            (
                "/rect_lights/0/aim",
                json!([0, 1, 0]),
                "aim away from its center",
            ),
            ("/rect_lights/0/size", json!([0, 1]), "positive size"),
            ("/path_trace/samples", json!(0), "1 to 65536 samples"),
            ("/models/0/surfaces/0/glow", json!(-1), "nonnegative glow"),
            (
                "/models/0/surfaces/0/role",
                json!("unlit"),
                "apply to an emissive surface",
            ),
        ] {
            let mut candidate = scene.clone();
            *candidate.pointer_mut(path).unwrap() = value;
            let error = validate(&[decode(&candidate)?], 1).unwrap_err();
            assert!(format!("{error:#}").contains(reason), "{path}: {error:#}");
        }
        let mut unlit = scene.clone();
        unlit["rect_lights"] = json!([]);
        let error = validate(&[decode(&unlit)?], 1).unwrap_err();
        assert!(
            format!("{error:#}").contains("lit by rect-light"),
            "{error:#}"
        );
        Ok(())
    }
}
