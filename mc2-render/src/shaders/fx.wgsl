// Effect (gosFX) triangles: texture times vertex colour, blended by the pipeline state.
// Positions are in game world coordinates, like every other scene pass.

struct Globals {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(1) @binding(0) var fx_tex: texture_2d<f32>;
@group(1) @binding(1) var fx_samp: sampler;

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) ground_clearance: f32,
    @location(3) fade_rgb: f32,
};

@vertex
fn vs_fx(
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) ground_clearance: f32,
    @location(4) fade_rgb: f32,
) -> VOut {
    var o: VOut;
    o.clip = g.view_proj * vec4<f32>(pos, 1.0);
    o.uv = uv;
    o.color = color;
    o.ground_clearance = ground_clearance;
    o.fade_rgb = fade_rgb;
    return o;
}

@fragment
fn fs_fx(v: VOut) -> @location(0) vec4<f32> {
    let t = textureSample(fx_tex, fx_samp, v.uv);
    let ground_fade = smoothstep(0.0, 32.0, v.ground_clearance);
    let rgb_fade = select(1.0, ground_fade, v.fade_rgb > 0.5);
    return to_target(vec4(t.rgb * v.color.rgb * rgb_fade, t.a * v.color.a * ground_fade));
}

// Colour targets: the scene computes display (gamma-space) values like the original. An sRGB
// target re-encodes what it stores, so its pipelines decode first and the stored bytes match an
// 8-bit UNORM target (blending then happens in linear light).
override SRGB_TARGET: bool = false;

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((max(c, vec3<f32>(0.0)) + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn to_target(c: vec4<f32>) -> vec4<f32> {
    if (SRGB_TARGET) {
        return vec4<f32>(srgb_to_linear(c.rgb), c.a);
    }
    return c;
}
