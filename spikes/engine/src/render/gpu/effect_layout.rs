use super::{Blend, SourceFeatures};

pub(super) enum EffectLayout {
    Inferred,
    Sampled(u32),
    History,
    ObjectMattes(Blend, SourceFeatures),
}

impl EffectLayout {
    pub(super) fn create(self, device: &wgpu::Device) -> Option<wgpu::PipelineLayout> {
        use wgpu::{BufferBindingType, TextureViewDimension as View};
        let texture = |dimension| wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: dimension,
            multisampled: false,
        };
        let buffer = |ty| wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let mut entries = Vec::new();
        let mut add = |binding, ty| {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty,
                count: None,
            })
        };
        match self {
            Self::Inferred => return None,
            Self::History => {
                add(0, texture(View::D2Array));
                add(1, buffer(BufferBindingType::Uniform));
            }
            Self::Sampled(count) => {
                for binding in 0..count {
                    add(binding, texture(View::D2));
                }
                add(count, buffer(BufferBindingType::Uniform));
            }
            Self::ObjectMattes(blend, features) => {
                // Use an explicit layout even if an authored effect ignores all its mattes.
                add(0, texture(View::D2));
                add(1, buffer(BufferBindingType::Uniform));
                add(2, texture(View::D2));
                if blend == Blend::Multiply {
                    add(3, texture(View::D2));
                }
                if features.blur {
                    add(4, texture(View::D2));
                }
                if features.grade {
                    add(5, buffer(BufferBindingType::Storage { read_only: true }));
                }
                add(6, texture(View::D2Array));
            }
        }
        let group = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("layer effect inputs"),
            entries: &entries,
        });
        Some(
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("layer effect inputs"),
                bind_group_layouts: &[Some(&group)],
                ..Default::default()
            }),
        )
    }
}
