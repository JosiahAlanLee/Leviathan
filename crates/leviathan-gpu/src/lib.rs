use bytemuck::{Pod, Zeroable};
use leviathan_core::CvFrame;

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
}

/// WGSL fullscreen triangle + reactive Möbius/PDE feedback fragment shader compiled via `wgpu`/`naga`.
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
    let p = vec2<f32>(p0.x * c - p0.y * s, p0.x * s + p0.y * c) * (1.5 - 0.8 * u.z_norm);
    let r = length(p);
    let a = atan2(p.y, p.x);
    let ring = sin(10.0 * r - u.beat * 6.2831853 + 3.0 * sin(4.0 * a));
    let grid = cos(p.x * (6.0 + 8.0 * u.gate_energy.y)) * cos(p.y * (6.0 + 8.0 * u.gate_energy.z));
    let kick = u.gate_energy.x;
    let col = vec3<f32>(
        0.08 + 0.55 * max(ring, 0.0) + 0.35 * kick,
        0.10 + 0.45 * abs(grid) + 0.25 * u.feedback_norm,
        0.16 + 0.65 * max(-ring, 0.0) + 0.25 * u.gate_energy.w
    );
    return vec4<f32>(clamp(col, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
"#;

/// Ping-pong RGBA8 feedback surface for instant zero-boilerplate preview & headless verification.
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
                let a = py.atan2(px);

                let ring = (10.0 * r - u.beat * core::f32::consts::TAU + 3.0 * (4.0 * a).sin()).sin();
                let grid = (px * (6.0 + 8.0 * u.gate_energy[1])).cos()
                    * (py * (6.0 + 8.0 * u.gate_energy[2])).cos();

                let idx = (y * self.width + x) * 4;
                let pr = self.front[idx] as f32 / 255.0;
                let pg = self.front[idx + 1] as f32 / 255.0;
                let pb = self.front[idx + 2] as f32 / 255.0;

                let cr = (0.08 + 0.55 * ring.max(0.0) + 0.35 * u.gate_energy[0]).clamp(0.0, 1.0);
                let cg = (0.10 + 0.45 * grid.abs() + 0.25 * u.feedback_norm).clamp(0.0, 1.0);
                let cb = (0.16 + 0.65 * (-ring).max(0.0) + 0.25 * u.gate_energy[3]).clamp(0.0, 1.0);

                self.back[idx] = (((1.0 - fb) * cr + fb * pr).clamp(0.0, 1.0) * 255.0) as u8;
                self.back[idx + 1] = (((1.0 - fb) * cg + fb * pg).clamp(0.0, 1.0) * 255.0) as u8;
                self.back[idx + 2] = (((1.0 - fb) * cb + fb * pb).clamp(0.0, 1.0) * 255.0) as u8;
                self.back[idx + 3] = 255;
            }
        }
        core::mem::swap(&mut self.front, &mut self.back);
        &self.front
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniforms_map_bytebus_and_step_feedback_surface() {
        let mut cv = CvFrame::default();
        cv.beat = 1.25;
        cv.gate_v[0] = 10.0;
        cv.bytebus_v[0] = 10.0;
        cv.bytebus_v[3] = 10.0;

        let u = VisualUniforms::from_cv(&cv, 1, 32, 18);
        assert_eq!(u.byte_mask, 0b1001);

        let mut surf = FeedbackSurface::new(32, 18);
        let rgba = surf.step(&u);
        assert_eq!(rgba.len(), 32 * 18 * 4);
        assert!(rgba.iter().any(|&b| b > 20));
    }
}
