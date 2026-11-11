use eframe::{egui, egui_wgpu, wgpu};
use leviathan_core::CvFrame;
use leviathan_gpu::{FeedbackSurface, VisualUniforms, FEEDBACK_SHADER_WGSL};

const GPU_WIDTH: u32 = 640;
const GPU_HEIGHT: u32 = 360;
const CPU_FALLBACK_WIDTH: usize = 160;
const CPU_FALLBACK_HEIGHT: usize = 90;

struct GpuPingPong {
    render_state: egui_wgpu::RenderState,
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    _textures: [wgpu::Texture; 2],
    views: [wgpu::TextureView; 2],
    bind_groups: [wgpu::BindGroup; 2],
    texture_id: egui::TextureId,
    front_idx: usize,
    width: u32,
    height: u32,
}

impl GpuPingPong {
    fn new(rs: &egui_wgpu::RenderState, width: u32, height: u32) -> Self {
        let tex_format = wgpu::TextureFormat::Rgba8Unorm;

        let shader = rs.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("leviathan_feedback_wgsl"),
            source: wgpu::ShaderSource::Wgsl(FEEDBACK_SHADER_WGSL.into()),
        });

        let uniform_buf = rs.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("leviathan_visual_uniforms"),
            size: std::mem::size_of::<VisualUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let make_target = |label: &'static str| -> (wgpu::Texture, wgpu::TextureView) {
            let tex = rs.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: tex_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            (tex, view)
        };

        let (tex_a, view_a) = make_target("leviathan_feedback_ping");
        let (tex_b, view_b) = make_target("leviathan_feedback_pong");

        let mut init_encoder =
            rs.device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("leviathan_feedback_init_clear"),
                });
        for view in [&view_a, &view_b] {
            let _pass = init_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("leviathan_clear_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }
        rs.queue.submit(std::iter::once(init_encoder.finish()));

        let sampler = rs.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("leviathan_feedback_sampler"),
            address_mode_u: wgpu::AddressMode::MirrorRepeat,
            address_mode_v: wgpu::AddressMode::MirrorRepeat,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let bind_group_layout =
            rs.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("leviathan_feedback_bgl"),
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Uniform,
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 2,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                    ],
                });

        let make_bg = |label: &'static str, src_view: &wgpu::TextureView| {
            rs.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(src_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            })
        };

        let bg_a = make_bg("leviathan_feedback_bg_ping", &view_a);
        let bg_b = make_bg("leviathan_feedback_bg_pong", &view_b);

        let pipeline_layout =
            rs.device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("leviathan_feedback_pl"),
                    bind_group_layouts: &[&bind_group_layout],
                    push_constant_ranges: &[],
                });

        let pipeline = rs
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("leviathan_feedback_pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: tex_format,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                multiview: None,
                cache: None,
            });

        let texture_id = rs.renderer.write().register_native_texture(
            &rs.device,
            &view_a,
            wgpu::FilterMode::Linear,
        );

        Self {
            render_state: rs.clone(),
            pipeline,
            uniform_buf,
            _textures: [tex_a, tex_b],
            views: [view_a, view_b],
            bind_groups: [bg_a, bg_b],
            texture_id,
            front_idx: 0,
            width,
            height,
        }
    }

    fn step(&mut self, uniforms: &VisualUniforms) -> egui::TextureId {
        self.render_state
            .queue
            .write_buffer(&self.uniform_buf, 0, uniforms.as_bytes());

        let src = self.front_idx;
        let dst = 1 - src;

        let mut encoder =
            self.render_state
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("leviathan_feedback_step"),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("leviathan_feedback_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.views[dst],
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_groups[src], &[]);
            pass.draw(0..3, 0..1);
        }
        self.render_state
            .queue
            .submit(std::iter::once(encoder.finish()));

        self.render_state
            .renderer
            .write()
            .update_egui_texture_from_wgpu_texture(
                &self.render_state.device,
                &self.views[dst],
                wgpu::FilterMode::Linear,
                self.texture_id,
            );

        self.front_idx = dst;
        self.texture_id
    }
}

enum Backend {
    Hardware(Box<GpuPingPong>),
    Software {
        surface: FeedbackSurface,
        preview_tex: Option<egui::TextureHandle>,
    },
}

pub struct FeedbackViewport {
    backend: Backend,
    frame_index: u32,
    status_label: String,
}

impl FeedbackViewport {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        if let Some(rs) = cc.wgpu_render_state.as_ref() {
            let gpu = GpuPingPong::new(rs, GPU_WIDTH, GPU_HEIGHT);
            Self {
                backend: Backend::Hardware(Box::new(gpu)),
                frame_index: 0,
                status_label: format!("wgpu ping-pong {GPU_WIDTH}x{GPU_HEIGHT}"),
            }
        } else {
            Self::new_software()
        }
    }

    pub fn new_software() -> Self {
        Self {
            backend: Backend::Software {
                surface: FeedbackSurface::new(CPU_FALLBACK_WIDTH, CPU_FALLBACK_HEIGHT),
                preview_tex: None,
            },
            frame_index: 0,
            status_label: format!("CPU fallback {CPU_FALLBACK_WIDTH}x{CPU_FALLBACK_HEIGHT}"),
        }
    }

    pub fn status_label(&self) -> &str {
        &self.status_label
    }

    pub fn render_frame(&mut self, ctx: &egui::Context, cv: &CvFrame) -> egui::TextureId {
        let frame = self.frame_index;
        self.frame_index = self.frame_index.wrapping_add(1);

        match &mut self.backend {
            Backend::Hardware(gpu) => {
                let uniforms = VisualUniforms::from_cv(cv, frame, gpu.width, gpu.height);
                gpu.step(&uniforms)
            }
            Backend::Software {
                surface,
                preview_tex,
            } => {
                let surf_w = surface.width;
                let surf_h = surface.height;
                let uniforms = VisualUniforms::from_cv(cv, frame, surf_w as u32, surf_h as u32);
                let rgba = surface.step(&uniforms);
                let color_img =
                    egui::ColorImage::from_rgba_unmultiplied([surf_w, surf_h], rgba);
                let tex = preview_tex.get_or_insert_with(|| {
                    ctx.load_texture(
                        "feedback_surface_cpu",
                        color_img.clone(),
                        egui::TextureOptions::LINEAR,
                    )
                });
                tex.set(color_img, egui::TextureOptions::LINEAR);
                tex.id()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn software_viewport_steps_headlessly_and_reports_dimensions() {
        let mut vp = FeedbackViewport::new_software();
        assert!(vp.status_label().contains("CPU fallback"));

        let ctx = egui::Context::default();
        let mut cv = CvFrame::default();
        cv.gate_v[0] = 10.0;
        cv.feedback_v = 5.0;

        let id0 = vp.render_frame(&ctx, &cv);
        let id1 = vp.render_frame(&ctx, &cv);
        assert_eq!(id0, id1);
    }
}
