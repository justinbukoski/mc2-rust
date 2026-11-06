// 2D GUI pass: textured or flat quads in target pixels, modulated by the vertex colour
// (gos_BlendModulateAlpha), no depth.
//
// Art is sampled "sharp bilinear": inside a texel the colour is flat, and only the one screen
// pixel that straddles a texel edge is blended. That keeps the original's pixel art crisp at any
// scale (2.7x at 4K) without the uneven duplicated rows of nearest sampling. At 1:1 the sample is
// snapped to the texel centre (nearest), which is what the original's FilterNone gave.

struct Globals {
    target_size: vec2<f32>,
    // 1.0 when the colour target is an sRGB view: colours are converted to linear so they come out
    // unchanged after the hardware encodes them; blending then happens in linear space, which
    // lightens translucent panels. The original blends gamma-encoded values: give the UI a
    // non-sRGB view of the target for exact results.
    srgb_target: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>) -> VsOut {
    var o: VsOut;
    let ndc = vec2<f32>(pos.x / g.target_size.x * 2.0 - 1.0, 1.0 - pos.y / g.target_size.y * 2.0);
    o.pos = vec4<f32>(ndc, 0.0, 1.0);
    o.uv = uv;
    o.color = color;
    return o;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs_main(i: VsOut) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(textureDimensions(tex));
    let texel = i.uv * dims;
    // screen pixels per texel
    let fw = max(fwidth(texel), vec2<f32>(1e-6));
    let scale = 1.0 / fw;
    let base = floor(texel);
    let frac_ = texel - base;
    let range = clamp(vec2<f32>(0.5) - vec2<f32>(0.5) / scale, vec2<f32>(0.0), vec2<f32>(0.5));
    let d = frac_ - vec2<f32>(0.5);
    var f = (d - clamp(d, -range, range)) * scale + vec2<f32>(0.5);
    // magnification close to 1:1: nearest
    let near_one = abs(scale - vec2<f32>(1.0)) < vec2<f32>(0.02);
    f = select(f, vec2<f32>(0.5), near_one);
    let uv = (base + f) / dims;
    let t = textureSample(tex, samp, uv);
    var c = t * i.color;
    if (c.a <= 0.0) {
        discard;
    }
    if (g.srgb_target > 0.5) {
        c = vec4<f32>(srgb_to_linear(c.rgb), c.a);
    }
    return c;
}
