// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
// mc2-render scene shader: terrain, water and TGL shapes.
//
// World space is the game's: x east, y north, z up.  Colours stay in the game's (gamma) space; the
// original blended in that space and the render target is not sRGB, so nothing is converted.
//
// Lighting is the game's model evaluated per pixel:
//   terrain tiles (roads, pavement): ramp(dot(vertexNormal, terrainSun))       -- quad.cpp
//   shapes: emissive + infiniteColour * max(dot(n, shapeSun), 0) + ambient      -- tgl.cpp
// where ramp(i) = i > 0 ? min(sun * i + ambient, 1) : ambient (Camera::getLightRed).  Plain terrain
// is not lit at all: its shading is burned into the colour map.

struct Globals {
    view_proj: mat4x4<f32>,
    light_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    terrain_dir: vec4<f32>,
    shape_dir: vec4<f32>,
    sun: vec4<f32>,            // rgb sun colour, a = night factor
    ambient: vec4<f32>,        // rgb ambient, a = 1 at night
    fog: vec4<f32>,            // rgb fog colour
    fog_height: vec4<f32>,     // x = start, y = full (world z)
    map: vec4<f32>,            // x = map side, y = top-left x, z = top-left y, w = terrain light scale (weather)
    tiling: vec4<f32>,         // x detail, y water, z water detail, w unused
    water: vec4<f32>,          // x = surface z, y = wave offset, z = alpha depth, w = cloud scroll
    water_alpha: vec4<f32>,    // shallow, middle, deep alpha (0..1)
    haze: vec4<f32>,           // x = haze start, y = full haze distance, z = shadow strength, w = shadow on
    shadow_info: vec4<f32>,    // x = 1 / shadow map size, y = depth bias, z = terrain lightning (weather, additive), w = terrain detail bitmap on
    water_anim: vec4<f32>,     // enhanced water: x = cloud angle, y = spray offset, z = frame blend, w = glint
};

// Visual upgrades (visual.rs).  Override constants, so the default pipelines contain none of
// the enhanced code and render exactly as before.
//
// Everything between the begin / end marker comment lines below is the enhanced code.  The
// baseline renderer (`SceneRenderer::new_baseline`, test support) compiles this file with those
// lines removed, so the default-off identity can be checked against a shader that has no
// enhanced code at all.  `ID_MODE` is test support too: fragments write a material class
// instead of a colour.
// @enhanced:begin
override ENHANCED_LIGHT: bool = false;
override ENHANCED_WATER: bool = false;
// @enhanced:end
override ID_MODE: bool = false;

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var shadow_map: texture_depth_2d;
@group(0) @binding(2) var shadow_cmp: sampler_comparison;
// Night light pools baked from the building lights (terrain_lights.rs); the terrain adds them and so do shapes.
@group(0) @binding(3) var night_lights: texture_2d<f32>;
@group(0) @binding(4) var night_smp: sampler;

// ------------------------------------------------------------------------------------------------
// shared helpers

fn visibility(world: vec3<f32>, edge: f32) -> f32 {
    let dist = distance(world, g.camera_pos.xyz);
    let haze = clamp((dist - g.haze.x) / max(g.haze.y - g.haze.x, 1.0), 0.0, 1.0);
    var vis = 1.0 - max(haze, edge);
    let span = g.fog_height.x - g.fog_height.y;
    if (abs(span) > 1.0e-3 && world.z < g.fog_height.x) {
        let f = clamp((g.fog_height.x - world.z) / span, 0.0, 1.0);
        vis = min(vis, 1.0 - f * f);
    }
    return vis;
}

fn apply_fog(color: vec3<f32>, world: vec3<f32>, edge: f32) -> vec3<f32> {
    return mix(g.fog.rgb, color, visibility(world, edge));
}

fn ramp(intensity: f32) -> vec3<f32> {
    if (intensity > 0.0) {
        return min(g.sun.rgb * intensity + g.ambient.rgb, vec3<f32>(1.0));
    }
    return g.ambient.rgb;
}

// Fraction of light that reaches `world` (1 = fully lit).  Percentage-closer filtering over a
// 3x3 footprint; outside the shadow map everything is lit.
fn shadow_factor(world: vec3<f32>, normal: vec3<f32>) -> f32 {
    if (g.haze.w < 0.5) {
        return 1.0;
    }
    let offset_world = world + normal * 3.0;
    let lp = g.light_view_proj * vec4<f32>(offset_world, 1.0);
    let ndc = lp.xyz / lp.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || ndc.z > 1.0) {
        return 1.0;
    }
    let texel = g.shadow_info.x;
    var sum = 0.0;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            sum = sum + textureSampleCompareLevel(
                shadow_map, shadow_cmp,
                uv + vec2<f32>(f32(dx), f32(dy)) * texel,
                ndc.z - g.shadow_info.y);
        }
    }
    return sum / 9.0;
}

// ------------------------------------------------------------------------------------------------
// terrain

@group(1) @binding(0) var color_map: texture_2d<f32>;
@group(1) @binding(1) var detail_tex: texture_2d<f32>;
@group(1) @binding(2) var tile_tex: texture_2d_array<f32>;
@group(1) @binding(3) var smp_clamp: sampler;
@group(1) @binding(4) var smp_wrap: sampler;
@group(1) @binding(5) var light_map: texture_2d<f32>;
@group(1) @binding(6) var normal_map: texture_2d<f32>;   // enhanced lighting: smooth per-pixel normal

struct TerrainIn {
    @location(0) pos: vec3<f32>,
    @location(1) flags: u32,
    @location(2) normal: vec3<f32>,
    @location(3) tile: u32,
    @location(4) tuv: vec2<f32>,
}

struct TerrainOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tuv: vec2<f32>,
    @location(3) @interpolate(flat) tile: u32,
    @location(4) edge: f32,
}

@vertex
fn vs_terrain(v: TerrainIn) -> TerrainOut {
    var o: TerrainOut;
    o.clip = g.view_proj * vec4<f32>(v.pos, 1.0);
    o.world = v.pos;
    o.normal = v.normal;
    o.tuv = v.tuv;
    o.tile = v.tile;
    o.edge = select(0.0, 1.0, (v.flags & 256u) != 0u);
    return o;
}

fn map_uv(world: vec3<f32>) -> vec2<f32> {
    return vec2<f32>((world.x - g.map.y) / g.map.x, (g.map.z - world.y) / g.map.x);
}

@fragment
fn fs_terrain(i: TerrainOut) -> @location(0) vec4<f32> {
    let uv = map_uv(i.world);
    var color = textureSample(color_map, smp_clamp, uv).rgb;

    // detail bitmap, alpha blended over the colour map (drawn "white lit" in the original)
    let det = textureSample(detail_tex, smp_wrap, uv * g.tiling.x);
    let with_detail = mix(color, det.rgb, det.a * select(0.0, 1.0, g.shadow_info.w > 0.5));

    // road / pavement tile
    let layer_raw = i.tile & 0xffffu;
    let mode = (i.tile >> 16u) & 3u;
    let have_tile = layer_raw != 0xffffu;
    let layer = select(0i, i32(layer_raw), have_tile);
    let inset = 0.0078125;
    let tuv = i.tuv * (1.0 - 2.0 * inset) + vec2<f32>(inset);
    let tile = textureSample(tile_tex, smp_clamp, tuv, layer);

    if (ID_MODE) {
        // material class of this terrain pixel: 1 plain, 2 tile blended by a partial alpha, 3 tile
        // replacing the colour map (opaque alpha or solid mode), 9 fully fogged (nothing of the surface is visible)
        var kind_id = 1.0;
        if (visibility(i.world, i.edge) <= 0.0) {
            kind_id = 9.0;
        } else if (have_tile && mode == 1u && tile.a > 0.5 / 255.0 && tile.a < 254.5 / 255.0) {
            // the sampled alpha is strictly partial: the tile and the colour map both show
            kind_id = 2.0;
        } else if (have_tile && mode == 1u && tile.a >= 254.5 / 255.0) {
            kind_id = 3.0;
        } else if (have_tile && mode == 2u) {
            kind_id = 3.0;
        }
        return id_out(kind_id, 1.0);
    }

    let n = i.normal;
    var intensity = dot(n, g.terrain_dir.xyz);
// @enhanced:begin
    // Enhanced lighting: the same sun/ambient ramp, per pixel from the smooth normal.  The colour
    // map's burned-in shading was made from the vertex normals, so plain terrain gets the ratio.
    var shade = vec3<f32>(1.0);
    if (ENHANCED_LIGHT) {
        let np = normalize(textureSample(normal_map, smp_clamp, uv).xyz * 2.0 - vec3<f32>(1.0));
        let ip = dot(np, g.terrain_dir.xyz);
        shade = clamp(ramp(ip) / max(ramp(intensity), vec3<f32>(0.001)),
                      vec3<f32>(0.6), vec3<f32>(1.5));
        intensity = ip;
    }
// @enhanced:end
    let lit = ramp(intensity);
    let tile_rgb = tile.rgb * lit;

    var out_rgb = with_detail;
// @enhanced:begin
    if (ENHANCED_LIGHT) {
        out_rgb = with_detail * shade;
    }
// @enhanced:end
    if (have_tile && mode == 1u) {
        out_rgb = mix(out_rgb, tile_rgb, tile.a);
    } else if (have_tile && mode == 2u) {
        out_rgb = tile_rgb;
    }

    // Weather darkening of the terrain light (`TerrainQuad::rainLightLevel`, 1 when the sky is clear).
    out_rgb = out_rgb * g.map.w;

    // Object shadows.  The surface already carries the sun's light (burned in or ramped), so
    // being shadowed removes the sun term: ambient / ramp(i) per channel.
    let lit_frac = shadow_factor(i.world, normalize(n));
    let dark = min(g.ambient.rgb / max(ramp(intensity), vec3<f32>(0.001)), vec3<f32>(1.0));
    out_rgb = out_rgb * mix(vec3<f32>(1.0), dark, (1.0 - lit_frac) * g.haze.z);

    // Night light pools around buildings: an additive term baked from the building lights.  Local Shadows put a
    // shape's shadow (cast from the light) into the pool; the sun's shadow map never covers a night frame.
    out_rgb = min(out_rgb + textureSample(light_map, smp_clamp, uv).rgb * lit_frac, vec3<f32>(1.0));

    // Lightning on the terrain: the specular the C++ adds after the texture (quad.cpp:839-841).
    out_rgb = min(out_rgb + vec3<f32>(g.shadow_info.z), vec3<f32>(1.0));
    return to_target(vec4<f32>(apply_fog(out_rgb, i.world, i.edge), 1.0));
}

// ------------------------------------------------------------------------------------------------
// water

@group(1) @binding(0) var water_tex: texture_2d<f32>;
@group(1) @binding(1) var water_detail_tex: texture_2d<f32>;
@group(1) @binding(2) var water_smp: sampler;
@group(1) @binding(3) var water_detail_next: texture_2d<f32>;   // enhanced water: the next frame

struct WaterOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) alpha: f32,
    @location(3) edge: f32,
}

@vertex
fn vs_water(v: TerrainIn) -> WaterOut {
    var o: WaterOut;
    // wave phase bits: 128 = down, 64 = up, neither = up (TerrainQuad::setupTextures)
    var wave = g.water.y;
    if ((v.flags & 128u) != 0u) {
        wave = -wave;
    }
    let z = g.water.x + wave;
    let w = vec3<f32>(v.pos.x, v.pos.y, z);
    o.clip = g.view_proj * vec4<f32>(w, 1.0);
    o.world = w;
    o.normal = v.normal;
    // Per-vertex alpha class from the land height under the water (TerrainQuad::drawWater).
    var a = g.water_alpha.y;
    if (v.pos.z >= g.water.x - g.water.z) {
        a = g.water_alpha.x;
    }
    if (v.pos.z <= g.water.x - g.water.z * 3.0) {
        a = g.water_alpha.z;
    }
    o.alpha = a;
    o.edge = select(0.0, 1.0, (v.flags & 256u) != 0u);
    return o;
}

fn cloud_offset() -> vec2<f32> {
    let s = g.water.w;
    return vec2<f32>(cos(6.2831853 * 32.0 * s) * 0.1, sin(6.2831853 * 32.0 * s) * 0.1);
}

// @enhanced:begin
// Enhanced water: the two neighbouring detail frames cross-faded.
fn detail_blended(uv: vec2<f32>) -> vec4<f32> {
    return mix(textureSample(water_detail_tex, water_smp, uv),
               textureSample(water_detail_next, water_smp, uv), g.water_anim.z);
}

fn detail_uv(world: vec3<f32>) -> vec2<f32> {
    return map_uv(world) * g.tiling.z + vec2<f32>(g.water_anim.y);
}

// Enhanced water: a sun glint on a ripple normal taken from the blended detail frames' brightness.
// Returns the glint strength (0 when the sun is behind the surface).
fn water_glint(world: vec3<f32>, edge: f32) -> f32 {
    if (g.water_anim.w <= 0.0) {
        return 0.0;
    }
    let uv = detail_uv(world);
    let e = 0.004;
    let lum = vec3<f32>(0.299, 0.587, 0.114);
    let hx = dot(detail_blended(uv + vec2<f32>(e, 0.0)).rgb - detail_blended(uv - vec2<f32>(e, 0.0)).rgb, lum);
    let hy = dot(detail_blended(uv + vec2<f32>(0.0, e)).rgb - detail_blended(uv - vec2<f32>(0.0, e)).rgb, lum);
    // u runs east, v runs south
    let n = normalize(vec3<f32>(-hx * 3.0, hy * 3.0, 1.0));
    let to_sun = normalize(g.shape_dir.xyz);
    let to_eye = normalize(g.camera_pos.xyz - world);
    let h = normalize(to_sun + to_eye);
    let s = pow(max(dot(n, h), 0.0), 120.0) * g.water_anim.w;
    return s * visibility(world, edge);
}
// @enhanced:end

@fragment
fn fs_water(i: WaterOut) -> @location(0) vec4<f32> {
    var offset = cloud_offset();
// @enhanced:begin
    if (ENHANCED_WATER) {
        offset = vec2<f32>(cos(g.water_anim.x), sin(g.water_anim.x)) * 0.1;
    }
// @enhanced:end
    let uv = map_uv(i.world) * g.tiling.y + offset;
    let tex = textureSample(water_tex, water_smp, uv);
    if (ID_MODE) {
        return id_out(4.0, i.alpha);
    }
    let lit = ramp(dot(i.normal, g.terrain_dir.xyz));
    let rgb = apply_fog(tex.rgb * lit, i.world, i.edge);
// @enhanced:begin
    if (ENHANCED_WATER) {
        // add the glint through the surface's alpha blend: rgb*a' + dst*(1-a') = rgb*a + glint + ...
        let s = water_glint(i.world, i.edge);
        let a = clamp(i.alpha + s, 0.0, 1.0);
        let c = (rgb * i.alpha + g.sun.rgb * s) / max(a, 1.0e-4);
        return to_target(vec4<f32>(min(c, vec3<f32>(1.0)), a));
    }
// @enhanced:end
    return to_target(vec4<f32>(rgb, i.alpha));
}

@fragment
fn fs_water_detail(i: WaterOut) -> @location(0) vec4<f32> {
    if (ID_MODE) {
        return id_out(5.0, i.alpha);
    }
// @enhanced:begin
    if (ENHANCED_WATER) {
        let tex = detail_blended(detail_uv(i.world));
        return to_target(vec4<f32>(apply_fog(tex.rgb, i.world, i.edge), tex.a * i.alpha));
    }
// @enhanced:end
    let spray = g.water.w * 10.0;
    let uv = map_uv(i.world) * g.tiling.z + vec2<f32>(spray);
    let tex = textureSample(water_detail_tex, water_smp, uv);
    let rgb = apply_fog(tex.rgb, i.world, i.edge);
    return to_target(vec4<f32>(rgb, tex.a * i.alpha));
}

// ------------------------------------------------------------------------------------------------
// shapes

@group(1) @binding(0) var<storage, read> node_matrices: array<mat4x4<f32>>;
@group(2) @binding(0) var shape_tex: texture_2d<f32>;
@group(2) @binding(1) var shape_smp: sampler;

struct ModelParams {
    pink: vec4<f32>,      // HotPinkRGB: lit windows
    yellow: vec4<f32>,    // HotYellowRGB: outside building lights
    green: vec4<f32>,     // HotGreenRGB: building base lights
}
@group(2) @binding(2) var<uniform> model: ModelParams;

override CUTOUT: bool = false;
override ALPHA_CUT: f32 = 0.5;

struct ShapeIn {
    @location(0) pos: vec3<f32>,
    @location(1) node: u32,
    @location(2) normal: vec3<f32>,
    @location(3) argb: u32,
    @location(4) uv: vec2<f32>,
    @location(5) node_base: u32,
    @location(6) iflags: u32,
    @location(7) infinite: vec4<f32>,
    @location(8) ambient_override: vec4<f32>,
    @location(9) highlight: u32,
}

struct ShapeOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) emissive: vec3<f32>,
    @location(4) infinite: vec3<f32>,
    @location(5) ambient: vec3<f32>,
    @location(6) highlight: vec3<f32>,
}

fn byte_of(x: u32, shift: u32) -> f32 {
    return f32((x >> shift) & 255u) / 255.0;
}

// Per-vertex emissive: some exact colours are markers the artists used for lit windows and
// building lights (tgl.cpp: hot pink / yellow / green), anything else non-black is added as is.
fn vertex_emissive(argb: u32) -> vec3<f32> {
    let night_factor = g.sun.w;
    let is_night = g.ambient.w > 0.5;
    let rgb = argb & 0x00ffffffu;
    let pink = model.pink.rgb;
    let yellow = model.yellow.rgb;
    let green = model.green.rgb;
    if (argb == 0xffff00ffu) {                // lit windows
        if (is_night) { return pink; }
        if (night_factor > 1.0e-4) { return pink * night_factor; }
        return vec3<f32>(47.0 / 255.0);
    }
    if (argb == 0xffffff00u) {                // outside building lights
        if (night_factor >= 0.75) {
            return select(yellow * night_factor, yellow, is_night);
        }
        return vec3<f32>(0.0);
    }
    if (argb == 0xff00ff00u) {                // building base lights
        if (is_night) { return green; }
        if (night_factor > 1.0e-4) { return green * night_factor; }
        return vec3<f32>(0.0);
    }
    if (argb == 0xffff0000u || argb == 0xff0000ffu) {
        return vec3<f32>(0.0);                // blinking lights: not animated
    }
    if (rgb != 0u) {
        return vec3<f32>(byte_of(argb, 16u), byte_of(argb, 8u), byte_of(argb, 0u));
    }
    return vec3<f32>(0.0);
}

fn shape_vertex(v: ShapeIn) -> ShapeOut {
    var o: ShapeOut;
    let m = node_matrices[v.node_base + v.node];
    let world = (m * vec4<f32>(v.pos, 1.0)).xyz;
    o.clip = g.view_proj * vec4<f32>(world, 1.0);
    o.world = world;
    o.normal = (m * vec4<f32>(v.normal, 0.0)).xyz;
    o.uv = v.uv;
    o.emissive = vertex_emissive(v.argb);
    o.infinite = v.infinite.rgb;
    o.ambient = select(g.ambient.rgb, v.ambient_override.rgb, v.ambient_override.a > 0.5);
    // SetARGBHighLight: added to the lit vertex colour (tgl.cpp:2207-2227)
    o.highlight = vec3<f32>(byte_of(v.highlight, 16u), byte_of(v.highlight, 8u), byte_of(v.highlight, 0u));
    return o;
}

@vertex
fn vs_shape(v: ShapeIn) -> ShapeOut {
    return shape_vertex(v);
}

@fragment
fn fs_shape(i: ShapeOut) -> @location(0) vec4<f32> {
    let tex = textureSample(shape_tex, shape_smp, i.uv);
    if (CUTOUT && tex.a < ALPHA_CUT) {
        discard;
    }
    if (ID_MODE) {
        return id_out(6.0, 1.0);
    }
    let n = normalize(i.normal);
    var ndl = max(dot(n, g.shape_dir.xyz), 0.0);
// @enhanced:begin
    if (ENHANCED_LIGHT) {
        // soft terminator: unchanged above 0.12, fading out to zero below it
        ndl = ndl * smoothstep(-0.08, 0.12, dot(n, g.shape_dir.xyz));
    }
// @enhanced:end
    let shade = shadow_factor(i.world, n);
    // In shadow the direct term disappears; ambient stays.
    let lit = min(min(i.emissive + i.infinite * ndl * shade + i.ambient, vec3<f32>(1.0)) + i.highlight, vec3<f32>(1.0));
    // `TG_LIGHT_TERRAIN` lights reach the shape's vertices as the additive specular term (tgl.cpp:2023-2051; only with
    // shadows on, `useShadows`); the pool is sampled at the pixel's ground position. The map is black without baked lights
    // (daytime), and the sun casts no shadows at night, so this is deliberately independent of `Frame::shadows`.
    let pool = textureSampleLevel(night_lights, night_smp, map_uv(i.world), 0.0).rgb;
    let rgb = apply_fog(min(tex.rgb * lit + pool, vec3<f32>(1.0)), i.world, 0.0);
    return to_target(vec4<f32>(rgb, select(1.0, tex.a, CUTOUT)));
}

// Night spotlight cones: untextured, additive, coloured by their vertices only.
@fragment
fn fs_spot(i: ShapeOut) -> @location(0) vec4<f32> {
    if (ID_MODE) {
        return id_out(8.0, 1.0);
    }
    // the spotlight vertices take SetARGBHighLight too (msl.h:467, tgl.cpp:2207)
    return to_target(vec4<f32>(min(min(i.emissive, vec3<f32>(1.0)) + i.highlight, vec3<f32>(1.0)), 1.0));
}

// Window quads: dark glass by day, lit at night; no sun.
@fragment
fn fs_window(i: ShapeOut) -> @location(0) vec4<f32> {
    let tex = textureSample(shape_tex, shape_smp, i.uv);
    if (ID_MODE) {
        return id_out(7.0, 1.0);
    }
    let rgb = apply_fog(tex.rgb * min(min(i.emissive, vec3<f32>(1.0)) + i.highlight, vec3<f32>(1.0)), i.world, 0.0);
    return to_target(vec4<f32>(rgb, tex.a));
}

// Depth-only pass from the sun for the shadow map.
struct ShadowOut {
    @builtin(position) clip: vec4<f32>,
}

struct ShadowCutoutOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_shadow(v: ShapeIn) -> ShadowOut {
    var o: ShadowOut;
    let m = node_matrices[v.node_base + v.node];
    let world = (m * vec4<f32>(v.pos, 1.0)).xyz;
    o.clip = g.light_view_proj * vec4<f32>(world, 1.0);
    return o;
}

// Cut-out casters (tree canopies) keep only their opaque texels.
@vertex
fn vs_shadow_cutout(v: ShapeIn) -> ShadowCutoutOut {
    var o: ShadowCutoutOut;
    let m = node_matrices[v.node_base + v.node];
    let world = (m * vec4<f32>(v.pos, 1.0)).xyz;
    o.clip = g.light_view_proj * vec4<f32>(world, 1.0);
    o.uv = v.uv;
    return o;
}

@fragment
fn fs_shadow_cutout(i: ShadowCutoutOut) {
    let tex = textureSample(shape_tex, shape_smp, i.uv);
    if (tex.a < 0.5) {
        discard;
    }
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

// Test support (`ID_MODE`): a material class (x16 in the red byte) and a coverage value.
fn id_out(kind_id: f32, alpha: f32) -> vec4<f32> {
    return to_target(vec4<f32>(kind_id * 16.0 / 255.0, alpha, 0.0, 1.0));
}
