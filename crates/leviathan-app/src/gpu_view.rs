use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::SystemTime;

use eframe::{egui, egui_wgpu, wgpu};
use leviathan_core::CvFrame;
use leviathan_gpu::{
    is_wgsl_filename, validate_wgsl_contract, AudioScopeSnapshot, FeedbackSurface, VisualUniforms,
    AUDIO_TEX_BYTES, AUDIO_TEX_HEIGHT, AUDIO_TEX_WIDTH, FEEDBACK_SHADER_WGSL,
    MAX_CUSTOM_WGSL_BYTES, STARTER_SHADER_TEMPLATE_WGSL,
};

use crate::project::CustomShader;

const GPU_WIDTH: u32 = 640;
const GPU_HEIGHT: u32 = 360;
const CPU_FALLBACK_WIDTH: usize = 160;
const CPU_FALLBACK_HEIGHT: usize = 90;
const HOT_RELOAD_POLL_SECS: f64 = 0.5;
const DEFAULT_STARTER_PATH: &str = "custom_feedback.wgsl";

fn is_wgsl_drop(name: &str, path: Option<&Path>) -> bool {
    is_wgsl_filename(name)
        || path
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .is_some_and(is_wgsl_filename)
}

struct NoopWake;

impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
    fn wake_by_ref(self: &Arc<Self>) {}
}

fn block_on_wgpu_error(
    device: &wgpu::Device,
    fut: impl Future<Output = Option<wgpu::Error>>,
) -> Option<wgpu::Error> {
    let waker = Waker::from(Arc::new(NoopWake));
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        if let Poll::Ready(err) = fut.as_mut().poll(&mut cx) {
            return err;
        }
        let _ = device.poll(wgpu::Maintain::Wait);
    }
}

struct GpuPingPong {
    render_state: egui_wgpu::RenderState,
    pipeline_layout: wgpu::PipelineLayout,
    tex_format: wgpu::TextureFormat,
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    _textures: [wgpu::Texture; 2],
    views: [wgpu::TextureView; 2],
    bind_groups: [wgpu::BindGroup; 2],
    audio_tex: wgpu::Texture,
    audio_bind_group: wgpu::BindGroup,
    audio_rgba: Box<[u8; AUDIO_TEX_BYTES]>,
    texture_id: egui::TextureId,
    front_idx: usize,
    width: u32,
    height: u32,
}

impl GpuPingPong {
    fn compile_pipeline(
        rs: &egui_wgpu::RenderState,
        pipeline_layout: &wgpu::PipelineLayout,
        tex_format: wgpu::TextureFormat,
        wgsl_source: &str,
    ) -> Result<wgpu::RenderPipeline, String> {
        validate_wgsl_contract(wgsl_source)?;

        rs.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = rs.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("leviathan_feedback_wgsl"),
            source: wgpu::ShaderSource::Wgsl(wgsl_source.into()),
        });

        let pipeline = rs
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("leviathan_feedback_pipeline"),
                layout: Some(pipeline_layout),
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

        let err_fut = rs.device.pop_error_scope();
        if let Some(err) = block_on_wgpu_error(&rs.device, err_fut) {
            return Err(format!("WGSL validation error: {err}"));
        }

        Ok(pipeline)
    }

    fn new(rs: &egui_wgpu::RenderState, width: u32, height: u32) -> Self {
        let tex_format = wgpu::TextureFormat::Rgba8Unorm;

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
                            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
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

        let bg_a = rs.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("leviathan_bg_ping"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view_b),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let bg_b = rs.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("leviathan_bg_pong"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view_a),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let audio_tex = rs.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("leviathan_audio_scope_tex"),
            size: wgpu::Extent3d {
                width: AUDIO_TEX_WIDTH,
                height: AUDIO_TEX_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let audio_tex_view = audio_tex.create_view(&wgpu::TextureViewDescriptor::default());

        let mut audio_rgba = Box::new([0u8; AUDIO_TEX_BYTES]);
        AudioScopeSnapshot::default().pack_rgba8(&mut audio_rgba);
        rs.queue.write_texture(
            audio_tex.as_image_copy(),
            &*audio_rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * AUDIO_TEX_WIDTH),
                rows_per_image: Some(AUDIO_TEX_HEIGHT),
            },
            wgpu::Extent3d {
                width: AUDIO_TEX_WIDTH,
                height: AUDIO_TEX_HEIGHT,
                depth_or_array_layers: 1,
            },
        );

        let audio_bind_group_layout =
            rs.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("leviathan_audio_tex_bgl"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    }],
                });

        let audio_bind_group = rs.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("leviathan_audio_tex_bg"),
            layout: &audio_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&audio_tex_view),
            }],
        });

        let pipeline_layout = rs
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("leviathan_feedback_pl"),
                bind_group_layouts: &[&bind_group_layout, &audio_bind_group_layout],
                push_constant_ranges: &[],
            });

        let pipeline =
            Self::compile_pipeline(rs, &pipeline_layout, tex_format, FEEDBACK_SHADER_WGSL)
                .expect("built-in FEEDBACK_SHADER_WGSL must compile cleanly");

        let texture_id = rs.renderer.write().register_native_texture(
            &rs.device,
            &view_a,
            wgpu::FilterMode::Linear,
        );

        Self {
            render_state: rs.clone(),
            pipeline_layout,
            tex_format,
            pipeline,
            uniform_buf,
            _textures: [tex_a, tex_b],
            views: [view_a, view_b],
            bind_groups: [bg_a, bg_b],
            audio_tex,
            audio_bind_group,
            audio_rgba,
            texture_id,
            front_idx: 0,
            width,
            height,
        }
    }

    fn replace_shader(&mut self, wgsl_source: &str) -> Result<(), String> {
        let next = Self::compile_pipeline(
            &self.render_state,
            &self.pipeline_layout,
            self.tex_format,
            wgsl_source,
        )?;
        self.pipeline = next;
        Ok(())
    }

    fn step(&mut self, uniforms: &VisualUniforms, scope: &AudioScopeSnapshot) -> egui::TextureId {
        self.render_state
            .queue
            .write_buffer(&self.uniform_buf, 0, uniforms.as_bytes());

        scope.pack_rgba8(&mut self.audio_rgba);
        self.render_state.queue.write_texture(
            self.audio_tex.as_image_copy(),
            &*self.audio_rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * AUDIO_TEX_WIDTH),
                rows_per_image: Some(AUDIO_TEX_HEIGHT),
            },
            wgpu::Extent3d {
                width: AUDIO_TEX_WIDTH,
                height: AUDIO_TEX_HEIGHT,
                depth_or_array_layers: 1,
            },
        );

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
            // bind_groups[i] samples views[1 - i], so the group that reads the
            // front texture (views[src]) while we render into views[dst] is [dst].
            pass.set_bind_group(0, &self.bind_groups[dst], &[]);
            pass.set_bind_group(1, &self.audio_bind_group, &[]);
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
    active_shader: Option<CustomShader>,
    watched_path: Option<PathBuf>,
    watched_mtime: Option<SystemTime>,
    last_watch_poll_secs: f64,
    shader_path_input: String,
    shader_status: String,
    shader_error: Option<String>,
    latest_scope: AudioScopeSnapshot,
    show_audio_scope: bool,
}

impl FeedbackViewport {
    fn from_backend(backend: Backend, status_label: String) -> Self {
        Self {
            backend,
            frame_index: 0,
            status_label,
            active_shader: None,
            watched_path: None,
            watched_mtime: None,
            last_watch_poll_secs: 0.0,
            shader_path_input: DEFAULT_STARTER_PATH.to_string(),
            shader_status: "Shader: Built-in Möbius/PDE + audio_tex".to_string(),
            shader_error: None,
            latest_scope: AudioScopeSnapshot::default(),
            show_audio_scope: true,
        }
    }

    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        if let Some(rs) = cc.wgpu_render_state.as_ref() {
            let gpu = GpuPingPong::new(rs, GPU_WIDTH, GPU_HEIGHT);
            Self::from_backend(
                Backend::Hardware(Box::new(gpu)),
                format!("wgpu ping-pong {GPU_WIDTH}x{GPU_HEIGHT}"),
            )
        } else {
            Self::new_software()
        }
    }

    pub fn new_software() -> Self {
        Self::from_backend(
            Backend::Software {
                surface: FeedbackSurface::new(CPU_FALLBACK_WIDTH, CPU_FALLBACK_HEIGHT),
                preview_tex: None,
            },
            format!("CPU fallback {CPU_FALLBACK_WIDTH}x{CPU_FALLBACK_HEIGHT}"),
        )
    }

    pub fn status_label(&self) -> &str {
        &self.status_label
    }

    pub fn custom_shader(&self) -> Option<&CustomShader> {
        self.active_shader.as_ref()
    }

    pub fn restore_custom_shader(&mut self, shader: Option<CustomShader>) {
        match shader {
            Some(cs) => {
                let _ = self.apply_custom_shader_source(cs.name, cs.wgsl, None);
            }
            None => self.reset_builtin_shader(),
        }
    }

    fn apply_custom_shader_source(
        &mut self,
        name: String,
        wgsl: String,
        watch_path: Option<PathBuf>,
    ) -> Result<(), String> {
        let mtime = watch_path
            .as_deref()
            .and_then(|p| fs::metadata(p).ok()?.modified().ok());

        let compile_res = match &mut self.backend {
            Backend::Hardware(gpu) => gpu.replace_shader(&wgsl),
            Backend::Software { .. } => validate_wgsl_contract(&wgsl),
        };

        match compile_res {
            Ok(()) => {
                self.watched_path = watch_path;
                self.watched_mtime = mtime;
                let hot = if self.watched_path.is_some() {
                    " (live hot-reload)"
                } else {
                    ""
                };
                self.shader_status = format!("Shader: {name}{hot}");
                self.shader_error = None;
                self.active_shader = Some(CustomShader { name, wgsl });
                Ok(())
            }
            Err(err) => {
                if let Some(path) = watch_path {
                    self.watched_path = Some(path);
                    self.watched_mtime = mtime;
                }
                self.shader_error = Some(err.clone());
                Err(err)
            }
        }
    }

    fn load_shader_path(&mut self, path: &Path) -> Result<(), String> {
        let read_res: Result<(String, String), String> = (|| {
            let meta = fs::metadata(path)
                .map_err(|e| format!("Cannot read `{}`: {e}", path.display()))?;
            if meta.len() > MAX_CUSTOM_WGSL_BYTES as u64 {
                return Err(format!(
                    "Shader `{}` exceeds {} bytes",
                    path.display(),
                    MAX_CUSTOM_WGSL_BYTES
                ));
            }
            let bytes = fs::read(path)
                .map_err(|e| format!("Failed reading `{}`: {e}", path.display()))?;
            let wgsl = String::from_utf8(bytes)
                .map_err(|_| format!("Shader `{}` is not valid UTF-8", path.display()))?;
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("custom.wgsl")
                .to_string();
            Ok((name, wgsl))
        })();

        match read_res {
            Ok((name, wgsl)) => {
                self.shader_path_input = path.display().to_string();
                self.apply_custom_shader_source(name, wgsl, Some(path.to_path_buf()))
            }
            Err(err) => {
                self.shader_error = Some(err.clone());
                Err(err)
            }
        }
    }

    fn reset_builtin_shader(&mut self) {
        if let Backend::Hardware(gpu) = &mut self.backend {
            let _ = gpu.replace_shader(FEEDBACK_SHADER_WGSL);
        }
        self.active_shader = None;
        self.watched_path = None;
        self.watched_mtime = None;
        self.shader_error = None;
        self.shader_status = "Shader: Built-in Möbius/PDE + audio_tex".to_string();
    }

    fn export_starter_template(path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed creating `{}`: {e}", parent.display()))?;
            }
        }
        fs::write(path, STARTER_SHADER_TEMPLATE_WGSL.as_bytes())
            .map_err(|e| format!("Failed writing `{}`: {e}", path.display()))
    }

    fn resolved_input_path(&mut self) -> PathBuf {
        let trimmed = self.shader_path_input.trim();
        if trimmed.is_empty() {
            self.shader_path_input = DEFAULT_STARTER_PATH.to_string();
            PathBuf::from(DEFAULT_STARTER_PATH)
        } else {
            PathBuf::from(trimmed)
        }
    }

    pub fn poll_shader_drop(&mut self, ctx: &egui::Context) -> bool {
        let hovering_wgsl = ctx.input(|i| {
            i.raw
                .hovered_files
                .iter()
                .any(|f| is_wgsl_drop("", f.path.as_deref()))
        });
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());

        for df in &dropped {
            if !is_wgsl_drop(&df.name, df.path.as_deref()) {
                continue;
            }
            if let Some(path) = df.path.as_deref() {
                let _ = self.load_shader_path(path);
            } else if let Some(bytes) = df.bytes.as_deref() {
                let label = if df.name.trim().is_empty() {
                    "dropped.wgsl".to_string()
                } else {
                    df.name.clone()
                };
                match std::str::from_utf8(bytes) {
                    Ok(src) => {
                        let _ = self.apply_custom_shader_source(label, src.to_string(), None);
                    }
                    Err(_) => {
                        self.shader_error =
                            Some(format!("Dropped shader `{label}` is not valid UTF-8"));
                    }
                }
            } else {
                self.shader_error = Some("Dropped .wgsl file had no path or bytes".to_string());
            }
        }

        hovering_wgsl
    }

    pub fn poll_hot_reload(&mut self, now_secs: f64) {
        let Some(path) = self.watched_path.clone() else {
            return;
        };
        if now_secs - self.last_watch_poll_secs < HOT_RELOAD_POLL_SECS {
            return;
        }
        self.last_watch_poll_secs = now_secs;

        if let Ok(modified) = fs::metadata(&path).and_then(|m| m.modified()) {
            if self.watched_mtime != Some(modified) {
                self.watched_mtime = Some(modified);
                let _ = self.load_shader_path(&path);
            }
        }
    }

    fn draw_audio_scope_canvas(&self, ui: &mut egui::Ui) {
        egui::Frame::canvas(ui.style()).show(ui, |ui| {
            let (resp, painter) =
                ui.allocate_painter(egui::vec2(340.0, 42.0), egui::Sense::hover());
            let rect = resp.rect;
            let band_w = rect.width() * 0.25;
            for (b_idx, &lvl) in self.latest_scope.bands.iter().enumerate() {
                let clamped = if lvl.is_finite() {
                    lvl.clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let bar_h = clamped * (rect.height() - 4.0);
                if bar_h > 0.5 {
                    let x0 = rect.left() + b_idx as f32 * band_w + 2.0;
                    let x1 = x0 + band_w - 4.0;
                    let bar_rect = egui::Rect::from_min_max(
                        egui::pos2(x0, rect.bottom() - 2.0 - bar_h),
                        egui::pos2(x1, rect.bottom() - 2.0),
                    );
                    painter.rect_filled(
                        bar_rect,
                        2.0,
                        egui::Color32::from_rgba_unmultiplied(45, 115, 135, 90),
                    );
                }
            }

            let mid_y = rect.center().y;
            painter.line_segment(
                [
                    egui::pos2(rect.left(), mid_y),
                    egui::pos2(rect.right(), mid_y),
                ],
                egui::Stroke::new(0.75_f32, egui::Color32::from_gray(55)),
            );

            let half_h = rect.height() * 0.44;
            let stroke_l = egui::Stroke::new(1.25_f32, egui::Color32::from_rgb(95, 225, 250));
            let stroke_r = egui::Stroke::new(1.10_f32, egui::Color32::from_rgb(235, 130, 215));
            let step = 2usize;
            let count = 256 / step;
            for i in 0..(count - 1) {
                let s0 = i * step;
                let s1 = (i + 1) * step;
                let x0 = rect.left() + (s0 as f32 / 255.0) * rect.width();
                let x1 = rect.left() + (s1 as f32 / 255.0) * rect.width();

                let yl0 = mid_y - ((self.latest_scope.wave_l[s0] as f32 / 255.0) * 2.0 - 1.0) * half_h;
                let yl1 = mid_y - ((self.latest_scope.wave_l[s1] as f32 / 255.0) * 2.0 - 1.0) * half_h;
                painter.line_segment([egui::pos2(x0, yl0), egui::pos2(x1, yl1)], stroke_l);

                let yr0 = mid_y - ((self.latest_scope.wave_r[s0] as f32 / 255.0) * 2.0 - 1.0) * half_h;
                let yr1 = mid_y - ((self.latest_scope.wave_r[s1] as f32 / 255.0) * 2.0 - 1.0) * half_h;
                painter.line_segment([egui::pos2(x0, yr0), egui::pos2(x1, yr1)], stroke_r);
            }
        });
    }

    pub fn draw_shader_controls(&mut self, ui: &mut egui::Ui, hovering_wgsl: bool) {
        ui.label(egui::RichText::new("Custom WGSL Shader & Audio Scope").strong());
        if hovering_wgsl {
            ui.colored_label(
                egui::Color32::from_rgb(130, 235, 175),
                ">> Release to load .wgsl into Feedback Pipeline <<",
            );
        }

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.shader_path_input)
                    .desired_width(248.0)
                    .hint_text(DEFAULT_STARTER_PATH),
            );
            if ui.button("Load .wgsl").clicked() {
                let target = self.resolved_input_path();
                let _ = self.load_shader_path(&target);
            }
        });

        ui.horizontal_wrapped(|ui| {
            if ui.button("Export Starter .wgsl").clicked() {
                let target = self.resolved_input_path();
                match Self::export_starter_template(&target) {
                    Ok(()) => {
                        self.shader_error = None;
                        self.shader_status =
                            format!("Exported starter WGSL -> {}", target.display());
                    }
                    Err(err) => {
                        self.shader_error = Some(err);
                    }
                }
            }

            let can_reset = self.active_shader.is_some()
                || self.watched_path.is_some()
                || self.shader_error.is_some();
            if ui
                .add_enabled(can_reset, egui::Button::new("Reset Built-In Shader"))
                .clicked()
            {
                self.reset_builtin_shader();
            }

            ui.toggle_value(&mut self.show_audio_scope, "Audio Scope")
                .on_hover_text("Toggle live 256x2 @group(1) audio_tex waveform & 4-band scope");
        });

        ui.small(&self.shader_status);
        if self.show_audio_scope {
            self.draw_audio_scope_canvas(ui);
        }
        if let Some(err) = &self.shader_error {
            ui.colored_label(
                egui::Color32::from_rgb(255, 115, 115),
                format!("WGSL error (keeping last valid shader): {err}"),
            );
        }
    }

    pub fn render_frame(
        &mut self,
        ctx: &egui::Context,
        cv: &CvFrame,
        scope: &AudioScopeSnapshot,
    ) -> egui::TextureId {
        self.latest_scope = *scope;
        let frame = self.frame_index;
        self.frame_index = self.frame_index.wrapping_add(1);

        match &mut self.backend {
            Backend::Hardware(gpu) => {
                let uniforms = VisualUniforms::from_cv(cv, frame, gpu.width, gpu.height);
                gpu.step(&uniforms, scope)
            }
            Backend::Software {
                surface,
                preview_tex,
            } => {
                let surf_w = surface.width;
                let surf_h = surface.height;
                let uniforms = VisualUniforms::from_cv(cv, frame, surf_w as u32, surf_h as u32);
                let rgba = surface.step(&uniforms);
                let color_img = egui::ColorImage::from_rgba_unmultiplied([surf_w, surf_h], rgba);
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
    fn software_viewport_steps_headlessly_and_updates_audio_scope() {
        let mut vp = FeedbackViewport::new_software();
        assert!(vp.status_label().contains("CPU fallback"));
        assert!(vp.show_audio_scope);

        let ctx = egui::Context::default();
        let mut cv = CvFrame::default();
        cv.gate_v[0] = 10.0;
        cv.feedback_v = 5.0;

        let mut scope = AudioScopeSnapshot::default();
        scope.wave_l[0] = 240;
        scope.bands = [0.85, 0.4, 0.2, 0.1];

        let id0 = vp.render_frame(&ctx, &cv, &scope);
        let id1 = vp.render_frame(&ctx, &cv, &scope);
        assert_eq!(id0, id1);
        assert_eq!(vp.latest_scope, scope);

        // Verify both T10 @group(0)-only shaders and T13 @group(1) shaders load cleanly.
        let t10_only = r#"
            struct VisualUniforms { beat: f32, z_norm: f32, roll_rad: f32, feedback_norm: f32,
                gate_energy: vec4<f32>, pitch_norm: vec4<f32>, byte_mask: u32, frame_index: u32,
                width: f32, height: f32 };
            @group(0) @binding(0) var<uniform> u: VisualUniforms;
            @vertex fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
                return vec4<f32>(0.0, 0.0, 0.0, 1.0);
            }
            @fragment fn fs_main() -> @location(0) vec4<f32> {
                return vec4<f32>(u.beat, 0.0, 0.0, 1.0);
            }
        "#;
        vp.apply_custom_shader_source("t10_compat.wgsl".to_string(), t10_only.to_string(), None)
            .expect("T10 @group(0)-only shader must remain compatible");
        vp.apply_custom_shader_source(
            "t13_audio.wgsl".to_string(),
            STARTER_SHADER_TEMPLATE_WGSL.to_string(),
            None,
        )
        .expect("T13 @group(0)+@group(1) shader must load");
    }

    #[test]
    fn wgsl_drop_loads_custom_shader_and_ignores_wav_files() {
        assert!(is_wgsl_drop("feedback.wgsl", None));
        assert!(is_wgsl_drop("FEEDBACK.WGSL", None));
        assert!(is_wgsl_drop("", Some(Path::new("/tmp/custom.wgsl"))));
        assert!(!is_wgsl_drop("drum.wav", Some(Path::new("/tmp/drum.wav"))));

        let mut vp = FeedbackViewport::new_software();
        let ctx = egui::Context::default();

        let mut wav_raw = egui::RawInput::default();
        wav_raw.dropped_files.push(egui::DroppedFile {
            name: "snare.wav".to_string(),
            bytes: Some(Arc::from([0u8, 1, 2, 3].as_slice())),
            ..Default::default()
        });
        let _ = ctx.run(wav_raw, |ctx| {
            assert!(!vp.poll_shader_drop(ctx));
        });
        assert!(vp.custom_shader().is_none());
        assert!(vp.shader_error.is_none());

        let mut wgsl_raw = egui::RawInput::default();
        wgsl_raw.hovered_files.push(egui::HoveredFile {
            path: Some(PathBuf::from("dropped_fx.wgsl")),
            ..Default::default()
        });
        wgsl_raw.dropped_files.push(egui::DroppedFile {
            name: "dropped_fx.wgsl".to_string(),
            bytes: Some(Arc::from(STARTER_SHADER_TEMPLATE_WGSL.as_bytes())),
            ..Default::default()
        });
        let mut hov = false;
        let _ = ctx.run(wgsl_raw, |ctx| {
            hov = vp.poll_shader_drop(ctx);
        });
        assert!(hov);
        assert_eq!(
            vp.custom_shader().map(|s| s.name.as_str()),
            Some("dropped_fx.wgsl")
        );
        assert!(vp.shader_error.is_none());
    }

    #[test]
    fn invalid_shader_retains_previous_valid_shader_and_hot_reloads_on_mtime_change() {
        let path = std::env::temp_dir().join(format!(
            "leviathan-hot-reload-{}.wgsl",
            std::process::id()
        ));
        FeedbackViewport::export_starter_template(&path).expect("export starter wgsl");

        let mut vp = FeedbackViewport::new_software();
        vp.load_shader_path(&path).expect("load exported starter");
        assert_eq!(
            vp.custom_shader().map(|s| s.wgsl.as_str()),
            Some(STARTER_SHADER_TEMPLATE_WGSL)
        );
        assert!(vp.shader_error.is_none());

        let base_mtime = fs::metadata(&path).unwrap().modified().unwrap();

        fs::write(&path, "// broken live edit\nfn fs_main() {").unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(base_mtime + std::time::Duration::from_secs(2))
            .unwrap();
        vp.poll_hot_reload(1.0);
        assert!(vp.shader_error.is_some());
        assert_eq!(
            vp.custom_shader().map(|s| s.wgsl.as_str()),
            Some(STARTER_SHADER_TEMPLATE_WGSL),
            "previous valid shader must remain active when hot-reload fails"
        );

        let updated_wgsl = STARTER_SHADER_TEMPLATE_WGSL.replace("12.0 * length", "20.0 * length");
        fs::write(&path, &updated_wgsl).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(base_mtime + std::time::Duration::from_secs(4))
            .unwrap();
        vp.poll_hot_reload(2.0);
        assert!(vp.shader_error.is_none());
        assert_eq!(
            vp.custom_shader().map(|s| s.wgsl.as_str()),
            Some(updated_wgsl.as_str())
        );

        vp.reset_builtin_shader();
        assert!(vp.custom_shader().is_none());
        assert!(vp.watched_path.is_none());

        let _ = fs::remove_file(&path);
    }
                             }
