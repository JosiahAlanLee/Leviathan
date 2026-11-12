use bytemuck::{Pod, Zeroable};
use leviathan_core::CvFrame;

/// Maximum accepted custom `.wgsl` shader source size (`256 KiB`).
pub const MAX_CUSTOM_WGSL_BYTES: usize = 256 * 1024;

/// 16-byte aligned uniform block shared directly with WGSL shaders (`64` bytes total).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct VisualUniforms {
    pub beat: f32,
    pub z_norm: f32,
    pub roll_rad: f32,
    pub feedback_norm: f32,
    pub gate_energy: [f32; 4],
    pub pitch_norm: [f32; 4],
    pub byte_mask: u32,
    pub frame_index: u32,
    pub width: f32,
    pub height: f32,
}

const _: () = assert!(core::mem::size_of::<VisualUniforms>() == 64);
const _: () = assert!(core::mem::size_of::<VisualUniforms>() % 16 == 0);

impl VisualUniforms {
    pub fn from_cv(cv: &CvFrame, frame_index: u32, width: u32, height: u32) -> Self {
        let v_scale = 1.0 / 10.0;
        let mut mask = 0u32;
        for b in 0..8 {
            if cv.bytebus_v[b] > 5.0 {
                mask |= 1 << b;
            }
        }
        Self {
            beat: cv.beat as f32,
            z_norm: (cv.z_v * v_scale).clamp(0.0, 1.0),
            roll_rad: (cv.roll_v / 5.0) * core::f32::consts::PI,
            feedback_norm: (cv.feedback_v * v_scale).clamp(0.0, 0.98),
            gate_energy: [
                (cv.gate_v[0] * v_scale).clamp(0.0, 1.0),
                (cv.gate_v[1] * v_scale).clamp(0.0, 1.0),
                (cv.gate_v[2] * v_scale).clamp(0.0, 1.0),
                (cv.gate_v[3] * v_scale).clamp(0.0, 1.0),
            ],
            pitch_norm: [
                cv.pitch_v[0],
                cv.pitch_v[1],
                cv.pitch_v[2],
                cv.pitch_v[3],
            ],
            byte_mask: mask,
            frame_index,
            width: width.max(1) as f32,
            height: height.max(1) as f32,
        }
    }

    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }
}

/// WGSL fullscreen triangle + ping-pong reactive Möbius/PDE feedback fragment shader compiled via `wgpu`/`naga`.
pub const FEEDBACK_SHADER_WGSL: &str = r#"
struct VisualUniforms {
    beat: f32,
    z_norm: f32,
    roll_rad: f32,
    feedback_norm: f32,
    gate_energy: vec4<f32>,
    pitch_norm: vec4<f32>,
    byte_mask: u32,
    frame_index: u32,
    width: f32,
    height: f32,
};

@group(0) @binding(0)
var<uniform> u: VisualUniforms;
@group(0) @binding(1)
var prev_tex: texture_2d<f32>;
@group(0) @binding(2)
var prev_sampler: sampler;

struct VertexOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOut {
    var out: VertexOut;
    let x = f32((idx << 1u) & 2u);
    let y = f32(idx & 2u);
    out.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let aspect = u.width / max(u.height, 1.0);
    let p0 = (in.uv * 2.0 - vec2<f32>(1.0, 1.0)) * vec2<f32>(aspect, 1.0);
    let c = cos(u.roll_rad);
    let s = sin(u.roll_rad);
    let zoom = 1.5 - 0.8 * u.z_norm;
    let p = vec2<f32>(p0.x * c - p0.y * s, p0.x * s + p0.y * c) * zoom;
    let r = length(p);
    let a = atan2(p.y, p.x);

    let ring = sin(10.0 * r - u.beat * 6.2831853 + 3.0 * sin(4.0 * a));
    let grid = cos(p.x * (6.0 + 8.0 * u.gate_energy.y)) * cos(p.y * (6.0 + 8.0 * u.gate_energy.z));
    let byte_pulse = f32(u.byte_mask & 255u) / 255.0;
    let kick = u.gate_energy.x;
    let excite = vec3<f32>(
        0.08 + 0.55 * max(ring, 0.0) + 0.35 * kick,
        0.10 + 0.45 * abs(grid) + 0.18 * byte_pulse,
        0.16 + 0.65 * max(-ring, 0.0) + 0.25 * u.gate_energy.w
    );

    let fb_rot = 0.035 * sin(u.roll_rad) + 0.015 * (u.pitch_norm.x * 0.2);
    let fc = cos(fb_rot);
    let fs = sin(fb_rot);
    let centered = in.uv - vec2<f32>(0.5, 0.5);
    let warped = vec2<f32>(centered.x * fc - centered.y * fs, centered.x * fs + centered.y * fc)
        * (0.985 - 0.04 * kick)
        + vec2<f32>(0.5, 0.5);
    let prev_col = textureSample(prev_tex, prev_sampler, warped).rgb;
    let shifted = vec3<f32>(prev_col.y, prev_col.z, prev_col.x);

    let fb = clamp(u.feedback_norm, 0.0, 0.94);
    let mixed = excite * (1.0 - fb) + shifted * (fb * 0.22) + prev_col * (fb * 0.78);
    return vec4<f32>(clamp(mixed, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
"#;

/// Documented starter `.wgsl` shader template exported by `"Export Starter .wgsl"` for live-coding.
pub const STARTER_SHADER_TEMPLATE_WGSL: &str = r#"// Leviathan Custom Ping-Pong Feedback Shader Starter (.wgsl)
// BSD-3-Clause — Edit `fs_main` while Leviathan watches this file to live-reload!
//
// `@group(0)` ABI contract (64-byte VisualUniforms + ping-pong previous frame):
//   - u.beat          : continuous musical beat phase
//   - u.z_norm        : normalized Z automation [0.0 .. 1.0]
//   - u.roll_rad      : roll automation angle in radians [-pi .. +pi]
//   - u.feedback_norm : feedback mix amount [0.0 .. 0.98]
//   - u.gate_energy   : per-bank gate envelopes (x=B0, y=B1, z=B2, w=B3) [0.0 .. 1.0]
//   - u.pitch_norm    : per-bank 1V/oct pitch offsets (x=B0, y=B1, z=B2, w=B3)
//   - u.byte_mask     : 8-bit ByteBus bitmask (0..255)
//   - u.frame_index   : monotonic render frame counter
//   - u.width, height : render target dimensions in pixels

struct VisualUniforms {
    beat: f32,
    z_norm: f32,
    roll_rad: f32,
    feedback_norm: f32,
    gate_energy: vec4<f32>,
    pitch_norm: vec4<f32>,
    byte_mask: u32,
    frame_index: u32,
    width: f32,
    height: f32,
};

@group(0) @binding(0)
var<uniform> u: VisualUniforms;
@group(0) @binding(1)
var prev_tex: texture_2d<f32>;
@group(0) @binding(2)
var prev_sampler: sampler;

struct VertexOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOut {
    var out: VertexOut;
    let x = f32((idx << 1u) & 2u);
    let y = f32(idx & 2u);
    out.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let aspect = u.width / max(u.height, 1.0);
    let uv_c = (in.uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(aspect, 1.0);
    let c = cos(u.roll_rad);
    let s = sin(u.roll_rad);
    let p = vec2<f32>(uv_c.x * c - uv_c.y * s, uv_c.x * s + uv_c.y * c) * (1.35 - 0.6 * u.z_norm);

    let wave = 0.5 + 0.5 * sin(12.0 * length(p) - u.beat * 6.2831853);
    let byte_glow = f32(u.byte_mask & 255u) / 255.0;
    let fg = vec3<f32>(
        0.12 + 0.65 * wave * (0.35 + 0.65 * u.gate_energy.x),
        0.14 + 0.55 * abs(sin(p.x * 8.0)) * (0.3 + 0.7 * u.gate_energy.y) + 0.2 * byte_glow,
        0.20 + 0.60 * abs(cos(p.y * 8.0)) * (0.3 + 0.7 * u.gate_energy.z)
    );

    let drift = (in.uv - vec2<f32>(0.5, 0.5)) * (0.988 - 0.03 * u.gate_energy.x) + vec2<f32>(0.5, 0.5);
    let prev_rgb = textureSample(prev_tex, prev_sampler, drift).rgb;
    let fb = clamp(u.feedback_norm, 0.0, 0.94);
    let color = mix(fg, prev_rgb.gbr * 0.25 + prev_rgb * 0.75, fb);
    return vec4<f32>(clamp(color, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
"#;

/// Returns `true` when `name` ends with a case-insensitive `.wgsl` file extension.
pub fn is_wgsl_filename(name: &str) -> bool {
    let trimmed = name.trim();
    match trimmed.rsplit_once('.') {
        Some((_, ext)) => ext.eq_ignore_ascii_case("wgsl"),
        None => false,
    }
}

fn strip_wgsl_comments(src: &str) -> Result<String, String> {
    let mut out = String::with_capacity(src.len());
    let bytes = src.as_bytes();
    let mut i = 0;
    let mut block_depth = 0usize;

    while i < bytes.len() {
        if block_depth > 0 {
            if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
                block_depth += 1;
                i += 2;
            } else if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                block_depth -= 1;
                out.push(' ');
                i += 2;
            } else {
                if bytes[i] == b'\n' {
                    out.push('\n');
                }
                i += 1;
            }
        } else if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            block_depth = 1;
            i += 2;
        } else {
            let ch = src[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }

    if block_depth != 0 {
        return Err("Unterminated /* ... */ block comment in WGSL source".to_string());
    }
    Ok(out)
}

/// Fast pre-flight contract check for custom Leviathan WGSL shaders before GPU compilation.
pub fn validate_wgsl_contract(source: &str) -> Result<(), String> {
    if source.trim().is_empty() {
        return Err("WGSL shader source is empty".to_string());
    }
    if source.len() > MAX_CUSTOM_WGSL_BYTES {
        return Err(format!(
            "WGSL shader exceeds max size ({} > {} bytes)",
            source.len(),
            MAX_CUSTOM_WGSL_BYTES
        ));
    }

    let stripped = strip_wgsl_comments(source)?;

    let mut brace_depth = 0i32;
    let mut paren_depth = 0i32;
    for ch in stripped.chars() {
        match ch {
            '{' => brace_depth += 1,
            '}' => {
                brace_depth -= 1;
                if brace_depth < 0 {
                    return Err("Unbalanced '}' in WGSL shader source".to_string());
                }
            }
            '(' => paren_depth += 1,
            ')' => {
                paren_depth -= 1;
                if paren_depth < 0 {
                    return Err("Unbalanced ')' in WGSL shader source".to_string());
                }
            }
            _ => {}
        }
    }
    if brace_depth != 0 {
        return Err("Unclosed '{' in WGSL shader source".to_string());
    }
    if paren_depth != 0 {
        return Err("Unclosed '(' in WGSL shader source".to_string());
    }

    for marker in [
        "@group(0)",
        "@binding(0)",
        "@vertex",
        "fn vs_main",
        "@fragment",
        "fn fs_main",
    ] {
        if !stripped.contains(marker) {
            return Err(format!(
                "WGSL shader missing required Leviathan ABI marker `{marker}`"
            ));
        }
    }

    Ok(())
}

/// Ping-pong RGBA8 software feedback surface for headless fallback & deterministic verification.
pub struct FeedbackSurface {
    pub width: usize,
    pub height: usize,
    front: Vec<u8>,
    back: Vec<u8>,
}

impl FeedbackSurface {
    pub fn new(width: usize, height: usize) -> Self {
        let len = width * height * 4;
        Self {
            width,
            height,
            front: vec![0; len],
            back: vec![0; len],
        }
    }

    pub fn step(&mut self, u: &VisualUniforms) -> &[u8] {
        let w = self.width.max(1) as f32;
        let h = self.height.max(1) as f32;
        let aspect = w / h;
        let (sin_r, cos_r) = u.roll_rad.sin_cos();
        let zoom = 1.5 - 0.8 * u.z_norm;
        let fb = u.feedback_norm.clamp(0.0, 0.94);

        for y in 0..self.height {
            let ny = (y as f32 / h) * 2.0 - 1.0;
            for x in 0..self.width {
                let nx = ((x as f32 / w) * 2.0 - 1.0) * aspect;
                let px = (nx * cos_r - ny * sin_r) * zoom;
                let py = (nx * sin_r + ny * cos_r) * zoom;
                let r = (px * px + py * py).sqrt();
                let a = ny.atan2(nx);

                let ring = (10.0 * r - u.beat * 6.2831853 + 3.0 * (4.0 * a).sin()).sin();
                let grid = (px * (6.0 + 8.0 * u.gate_energy[1])).cos()
                    * (py * (6.0 + 8.0 * u.gate_energy[2])).cos();
                let kick = u.gate_energy[0];

                let r_val = 0.08 + 0.55 * ring.max(0.0) + 0.35 * kick;
                let g_val = 0.10 + 0.45 * grid.abs() + 0.25 * fb;
                let b_val = 0.16 + 0.65 * (-ring).max(0.0) + 0.25 * u.gate_energy[3];

                let idx = (y * self.width + x) * 4;
                let prev_r = self.back[idx] as f32 / 255.0;
                let prev_g = self.back[idx + 1] as f32 / 255.0;
                let prev_b = self.back[idx + 2] as f32 / 255.0;

                let final_r = (r_val * (1.0 - fb) + prev_r * fb).clamp(0.0, 1.0);
                let final_g = (g_val * (1.0 - fb) + prev_g * fb).clamp(0.0, 1.0);
                let final_b = (b_val * (1.0 - fb) + prev_b * fb).clamp(0.0, 1.0);

                self.front[idx] = (final_r * 255.0) as u8;
                self.front[idx + 1] = (final_g * 255.0) as u8;
                self.front[idx + 2] = (final_b * 255.0) as u8;
                self.front[idx + 3] = 255;
            }
        }

        std::mem::swap(&mut self.front, &mut self.back);
        &self.back
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_uniforms_byte_layout_and_cv_normalization() {
        let mut cv = CvFrame::default();
        cv.beat = 4.25;
        cv.z_v = 5.0;
        cv.feedback_v = 7.5;
        cv.gate_v = [10.0, 2.5, 0.0, 5.0, 0.0, 0.0];
        cv.pitch_v = [0.25, -0.5, 1.0, 0.0, 0.0, 0.0];
        cv.bytebus_v[0] = 6.0;
        cv.bytebus_v[3] = 8.0;

        let u = VisualUniforms::from_cv(&cv, 42, 640, 360);
        assert_eq!(u.beat, 4.25);
        assert!((u.z_norm - 0.5).abs() < 1e-5);
        assert!((u.feedback_norm - 0.75).abs() < 1e-5);
        assert!((u.gate_energy[0] - 1.0).abs() < 1e-5);
        assert_eq!(u.gate_energy, [1.0, 0.25, 0.0, 0.5]);
        assert_eq!(u.pitch_norm, [0.25, -0.5, 1.0, 0.0]);
        assert_eq!(u.byte_mask, 0b0000_1001);
        assert_eq!(u.frame_index, 42);
        assert_eq!(u.width, 640.0);
        assert_eq!(u.height, 360.0);
        assert_eq!(u.as_bytes().len(), 64);
    }

    #[test]
    fn feedback_shader_declares_ping_pong_bindings_and_entry_points() {
        assert!(FEEDBACK_SHADER_WGSL.contains("@group(0) @binding(0)"));
        assert!(FEEDBACK_SHADER_WGSL.contains("@group(0) @binding(1)"));
        assert!(FEEDBACK_SHADER_WGSL.contains("@group(0) @binding(2)"));
        assert!(FEEDBACK_SHADER_WGSL.contains("fn vs_main"));
        assert!(FEEDBACK_SHADER_WGSL.contains("fn fs_main"));
        assert!(FEEDBACK_SHADER_WGSL.contains("textureSample(prev_tex, prev_sampler"));
    }

    #[test]
    fn software_feedback_surface_ping_pongs_deterministically() {
        let mut surf = FeedbackSurface::new(32, 18);
        let mut cv = CvFrame::default();
        cv.gate_v[0] = 10.0;
        cv.feedback_v = 6.0;
        let u0 = VisualUniforms::from_cv(&cv, 0, 32, 18);
        let frame0 = surf.step(&u0).to_vec();
        assert_eq!(frame0.len(), 32 * 18 * 4);
        assert!(frame0.chunks_exact(4).all(|px| px[3] == 255));

        cv.gate_v[0] = 0.0;
        let u1 = VisualUniforms::from_cv(&cv, 1, 32, 18);
        let frame1 = surf.step(&u1);
        assert_ne!(frame0.as_slice(), frame1);
    }

    #[test]
    fn starter_wgsl_template_matches_leviathan_abi_contract() {
        validate_wgsl_contract(FEEDBACK_SHADER_WGSL)
            .expect("built-in feedback shader must satisfy contract");
        validate_wgsl_contract(STARTER_SHADER_TEMPLATE_WGSL)
            .expect("starter template shader must satisfy contract");

        for marker in [
            "struct VisualUniforms",
            "@group(0) @binding(0)",
            "@group(0) @binding(1)",
            "@group(0) @binding(2)",
            "prev_tex",
            "prev_sampler",
            "fn vs_main",
            "fn fs_main",
        ] {
            assert!(
                STARTER_SHADER_TEMPLATE_WGSL.contains(marker),
                "starter template missing marker {marker}"
            );
        }
    }

    #[test]
    fn wgsl_contract_validator_rejects_malformed_or_incomplete_shaders() {
        assert!(validate_wgsl_contract("   ").is_err());
        assert!(validate_wgsl_contract("// @vertex fn vs_main @fragment fn fs_main").is_err());

        let unclosed_brace = format!("{STARTER_SHADER_TEMPLATE_WGSL}\nfn broken() {{");
        assert!(validate_wgsl_contract(&unclosed_brace).is_err());

        let missing_fs = STARTER_SHADER_TEMPLATE_WGSL.replace("fn fs_main", "fn wrong_entry");
        assert!(validate_wgsl_contract(&missing_fs).is_err());

        assert!(is_wgsl_filename("custom_feedback.wgsl"));
        assert!(is_wgsl_filename("PATCH.WGSL"));
        assert!(!is_wgsl_filename("kick_01.wav"));
        assert!(!is_wgsl_filename("project.leviathan.json"));
    }
}
