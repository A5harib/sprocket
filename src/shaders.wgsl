// Sprocket GPU shaders:
// 1. Quad pipeline: solid color, tiled texture, or image texture with optional rounded-rect clip & opacity.
// 2. Post/composite pipeline: directional motion blur along vertical scroll velocity,
//    rounded viewport clipping, desaturation (matching CSS mix-blend-mode: saturation), and dim overlay.

struct QuadUniforms {
    // dst rect in pixels: (x, y, w, h)
    dst_rect: vec4<f32>,
    // uv rect: (u0, v0, u1, v1)
    uv_rect: vec4<f32>,
    // color / tint (rgba)
    color: vec4<f32>,
    // params: (canvas_w, canvas_h, mode, corner_radius)
    // mode: 0 = solid color, 1 = texture * color, 2 = tiled texture (tile_px in extra.xy)
    params: vec4<f32>,
    // extra: (tile_w, tile_h, 0.0, 0.0)
    extra: vec4<f32>,
};

@group(0) @binding(0) var<uniform> q_uni: QuadUniforms;
@group(0) @binding(1) var q_tex: texture_2d<f32>;
@group(0) @binding(2) var q_samp: sampler;

struct QuadOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) local_px: vec2<f32>,
};

@vertex
fn vs_quad(@builtin(vertex_index) vid: u32) -> QuadOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    let c = corners[vid];
    let px = q_uni.dst_rect.xy + c * q_uni.dst_rect.zw;
    let ndc = vec2<f32>(
        (px.x / q_uni.params.x) * 2.0 - 1.0,
        1.0 - (px.y / q_uni.params.y) * 2.0,
    );
    let uv = mix(q_uni.uv_rect.xy, q_uni.uv_rect.zw, c);
    var out: QuadOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = uv;
    out.local_px = c * q_uni.dst_rect.zw;
    return out;
}

fn rounded_rect_alpha(p: vec2<f32>, size: vec2<f32>, radius: f32) -> f32 {
    if (radius <= 0.0) {
        return 1.0;
    }
    let half_size = size * 0.5;
    let q = abs(p - half_size) - (half_size - vec2<f32>(radius));
    let d = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
    return 1.0 - smoothstep(-0.75, 0.75, d);
}

@fragment
fn fs_quad(in: QuadOut) -> @location(0) vec4<f32> {
    let mode = i32(q_uni.params.z + 0.5);
    let mask = rounded_rect_alpha(in.local_px, q_uni.dst_rect.zw, q_uni.params.w);
    if (mask <= 0.001) {
        discard;
    }
    var col = q_uni.color;
    if (mode == 1) {
        let tex = textureSample(q_tex, q_samp, in.uv);
        col = tex * q_uni.color;
    } else if (mode == 2) {
        let tile = max(q_uni.extra.xy, vec2<f32>(1.0));
        let tiled_uv = fract(in.local_px / tile);
        let tex = textureSample(q_tex, q_samp, tiled_uv);
        col = tex * q_uni.color;
    }
    col.a = col.a * mask;
    return col;
}

// ---------------------------------------------------------------------------
// Directional Motion Blur + Viewport Clip pipeline
// ---------------------------------------------------------------------------

struct BlurUniforms {
    // viewport rect in pixels: (x, y, w, h)
    vp_rect: vec4<f32>,
    // params: (canvas_w, canvas_h, corner_radius, blur_dy_uv)
    params: vec4<f32>,
    // extra: (samples, radial_blur, 0.0, 0.0)
    extra: vec4<f32>,
};

@group(0) @binding(0) var<uniform> b_uni: BlurUniforms;
@group(0) @binding(1) var b_tex: texture_2d<f32>;
@group(0) @binding(2) var b_samp: sampler;

struct BlurOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) local_px: vec2<f32>,
};

@vertex
fn vs_blur(@builtin(vertex_index) vid: u32) -> BlurOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    let c = corners[vid];
    let px = b_uni.vp_rect.xy + c * b_uni.vp_rect.zw;
    let ndc = vec2<f32>(
        (px.x / b_uni.params.x) * 2.0 - 1.0,
        1.0 - (px.y / b_uni.params.y) * 2.0,
    );
    var out: BlurOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = px / b_uni.params.xy;
    out.local_px = c * b_uni.vp_rect.zw;
    return out;
}

@fragment
fn fs_blur(in: BlurOut) -> @location(0) vec4<f32> {
    let mask = rounded_rect_alpha(in.local_px, b_uni.vp_rect.zw, b_uni.params.z);
    if (mask <= 0.001) {
        discard;
    }
    let blur_dy = b_uni.params.w;
    let radial = b_uni.extra.y;
    let base_n = max(1, i32(b_uni.extra.x + 0.5));
    if (base_n <= 1 || (abs(blur_dy) < 0.0004 && abs(radial) < 0.0004)) {
        var col = textureSample(b_tex, b_samp, in.uv);
        col.a = col.a * mask;
        return col;
    }
    // Scale tap count with blur distance in pixels so fast swipes stay silky-smooth with zero stepping
    let blur_px = abs(blur_dy) * b_uni.params.y;
    let n = clamp(max(base_n, i32(blur_px * 0.35)), base_n, 64);
    let center = vec2<f32>(0.5, 0.5);
    let dir = (in.uv - center) * radial + vec2<f32>(0.0, blur_dy);
    var acc = vec4<f32>(0.0);
    let nf = f32(n);
    for (var i = 0; i < n; i = i + 1) {
        let t = (f32(i) + 0.5) / nf - 0.5;
        let suv = clamp(in.uv + dir * t, vec2<f32>(0.001), vec2<f32>(0.999));
        acc = acc + textureSample(b_tex, b_samp, suv);
    }
    var col = acc / nf;
    col.a = col.a * mask;
    return col;
}

// ---------------------------------------------------------------------------
// Fullscreen Grade / PostFX pipeline (desaturate + dim)
// ---------------------------------------------------------------------------

struct GradeUniforms {
    // (desaturate 0..1, dim_alpha 0..1, 0, 0)
    params: vec4<f32>,
    // dim color rgb
    dim_color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g_uni: GradeUniforms;
@group(0) @binding(1) var g_tex: texture_2d<f32>;
@group(0) @binding(2) var g_samp: sampler;

struct FullOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) vid: u32) -> FullOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    let c = corners[vid];
    var out: FullOut;
    out.pos = vec4<f32>(c.x * 2.0 - 1.0, 1.0 - c.y * 2.0, 0.0, 1.0);
    out.uv = c;
    return out;
}

@fragment
fn fs_grade(in: FullOut) -> @location(0) vec4<f32> {
    var col = textureSample(g_tex, g_samp, in.uv);
    let desat = clamp(g_uni.params.x, 0.0, 1.0);
    if (desat > 0.0001) {
        // W3C CSS mix-blend-mode: saturation luminance weights
        let luma = dot(col.rgb, vec3<f32>(0.3, 0.59, 0.11));
        col = vec4<f32>(mix(col.rgb, vec3<f32>(luma), desat), col.a);
    }
    let dim_a = clamp(g_uni.params.y, 0.0, 1.0);
    if (dim_a > 0.0001) {
        col = vec4<f32>(mix(col.rgb, g_uni.dim_color.rgb, dim_a), 1.0);
    }
    return col;
}
