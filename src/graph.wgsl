// Graph scene shaders. Everything is premultiplied alpha.
//
// 1. Instanced quads: any 2D affine transform, rounded corners, a rounded clip in another
//    node's space, anti-aliased edges, stretch / tiled sampling with footprint supersampling,
//    and an optional gaussian blur (CSS filter: blur).
// 2. Layer composite with motion blur along a velocity field v(q) = L (q - t0) + v.
// 3. Grade: desaturate (CSS mix-blend-mode: saturation with grey) and dim.

struct Globals {
    canvas: vec4<f32>,
};

struct Inst {
    m: vec4<f32>,      // a b c d
    t: vec4<f32>,      // e f pad_x pad_y (local units)
    rect: vec4<f32>,   // local x y w h
    uv: vec4<f32>,     // stretch: u0 v0 u1 v1 | tile: tile_w tile_h off_x off_y
    color: vec4<f32>,  // premultiplied multiplier
    params: vec4<f32>, // mode, corner radius, blur sigma u, blur sigma v
    cm: vec4<f32>,     // clip inverse a b c d
    ct: vec4<f32>,     // clip inverse e f, clip radius, has clip
    cr: vec4<f32>,     // clip rect x y w h
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var<storage, read> insts: array<Inst>;
@group(0) @binding(2) var samp_clamp: sampler;
@group(0) @binding(3) var samp_repeat: sampler;
@group(1) @binding(0) var tex: texture_2d<f32>;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) iid: u32,
};

fn corner(vid: u32) -> vec2<f32> {
    var c = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    return c[vid];
}

@vertex
fn vs_inst(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> VOut {
    let it = insts[iid];
    let c = corner(vid);
    let pad = it.t.zw;
    let lo = it.rect.xy - pad;
    let size = it.rect.zw + pad * 2.0;
    let local = lo + c * size;
    let screen = vec2<f32>(
        it.m.x * local.x + it.m.z * local.y + it.t.x,
        it.m.y * local.x + it.m.w * local.y + it.t.y,
    );
    let rel = (local - it.rect.xy) / max(it.rect.zw, vec2<f32>(1e-6));
    var uv: vec2<f32>;
    let mode = i32(it.params.x + 0.5);
    if (mode == 2) {
        uv = (local - it.rect.xy + it.uv.zw) / max(it.uv.xy, vec2<f32>(1e-3));
    } else {
        uv = mix(it.uv.xy, it.uv.zw, rel);
    }
    var o: VOut;
    o.pos = vec4<f32>(screen.x / g.canvas.x * 2.0 - 1.0, 1.0 - screen.y / g.canvas.y * 2.0, 0.0, 1.0);
    o.local = local - it.rect.xy;
    o.uv = uv;
    o.iid = iid;
    return o;
}

// Coverage of a (rounded) rect [0,size] at p, anti-aliased using local-units-per-pixel `fw`.
fn rect_cov(p: vec2<f32>, size: vec2<f32>, radius: f32, fw: vec2<f32>) -> f32 {
    let hs = size * 0.5;
    let fwc = max(fw, vec2<f32>(1e-6));
    if (radius <= 0.0) {
        let q = abs(p - hs) - hs;
        let d = q / fwc;
        return clamp(0.5 - d.x, 0.0, 1.0) * clamp(0.5 - d.y, 0.0, 1.0);
    }
    let r = min(radius, min(hs.x, hs.y));
    let q = abs(p - hs) - (hs - vec2<f32>(r));
    let dist = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
    let lpp = 0.5 * (fwc.x + fwc.y);
    return clamp(0.5 - dist / lpp, 0.0, 1.0);
}

fn sample_footprint(uv: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>, tile: bool) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(tex, 0));
    let fp = max(length(dx * dims), length(dy * dims));
    let n = clamp(i32(ceil(fp)), 1, 4);
    let nf = f32(n);
    var acc = vec4<f32>(0.0);
    for (var i = 0; i < n; i = i + 1) {
        for (var j = 0; j < n; j = j + 1) {
            let off = ((f32(i) + 0.5) / nf - 0.5) * dx + ((f32(j) + 0.5) / nf - 0.5) * dy;
            if (tile) {
                acc = acc + textureSampleLevel(tex, samp_repeat, uv + off, 0.0);
            } else {
                acc = acc + textureSampleLevel(tex, samp_clamp, uv + off, 0.0);
            }
        }
    }
    return acc / (nf * nf);
}

fn sample_blur(uv: vec2<f32>, sigma: vec2<f32>) -> vec4<f32> {
    // 48 taps on a golden-angle spiral out to 2.5 sigma, gaussian weighted, plus the centre.
    var acc = textureSampleLevel(tex, samp_clamp, uv, 0.0);
    var wsum = 1.0;
    let taps = 48;
    for (var k = 0; k < taps; k = k + 1) {
        let fk = f32(k) + 0.5;
        let r = sqrt(fk / f32(taps)) * 2.5;
        let a = fk * 2.39996323;
        let off = vec2<f32>(cos(a), sin(a)) * r * sigma;
        let w = exp(-0.5 * r * r);
        acc = acc + textureSampleLevel(tex, samp_clamp, uv + off, 0.0) * w;
        wsum = wsum + w;
    }
    return acc / wsum;
}

@fragment
fn fs_inst(in: VOut) -> @location(0) vec4<f32> {
    let it = insts[in.iid];
    // Derivatives first, in uniform control flow.
    let fw = fwidth(in.local);
    let dx = dpdx(in.uv);
    let dy = dpdy(in.uv);
    let sp = in.pos.xy;
    let cq = vec2<f32>(it.cm.x * sp.x + it.cm.z * sp.y + it.ct.x, it.cm.y * sp.x + it.cm.w * sp.y + it.ct.y);
    let cfw = fwidth(cq);

    let mode = i32(it.params.x + 0.5);
    var col: vec4<f32>;
    if (mode == 0) {
        col = it.color;
    } else if (mode == 2) {
        col = sample_footprint(in.uv, dx, dy, true) * it.color;
    } else if (mode == 3) {
        col = sample_blur(in.uv, it.params.zw) * it.color;
    } else {
        col = sample_footprint(in.uv, dx, dy, false) * it.color;
    }

    var cov = 1.0;
    if (mode != 3) {
        cov = rect_cov(in.local, it.rect.zw, it.params.y, fw);
    }
    if (it.ct.w > 0.5) {
        cov = cov * rect_cov(cq - it.cr.xy, it.cr.zw, it.ct.z, cfw);
    }
    if (cov <= 0.0) {
        discard;
    }
    return col * cov;
}

// ---------------------------------------------------------------------------------------------
// Fullscreen passes
// ---------------------------------------------------------------------------------------------

struct FullOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) vid: u32) -> FullOut {
    let c = corner(vid);
    var o: FullOut;
    o.pos = vec4<f32>(c.x * 2.0 - 1.0, 1.0 - c.y * 2.0, 0.0, 1.0);
    o.uv = c;
    return o;
}

struct CompU {
    l: vec4<f32>,      // velocity matrix a b c d (px/s per px)
    tv: vec4<f32>,     // t0.x t0.y v.x v.y
    params: vec4<f32>, // canvas w, canvas h, shutter seconds, base taps
    extra: vec4<f32>,  // weight, 0, 0, 0
};

@group(0) @binding(0) var<uniform> cu: CompU;
@group(0) @binding(1) var ctex: texture_2d<f32>;
@group(0) @binding(2) var csamp: sampler;

@fragment
fn fs_composite(in: FullOut) -> @location(0) vec4<f32> {
    let size = cu.params.xy;
    let q = in.uv * size;
    let rel = q - cu.tv.xy;
    let vel = vec2<f32>(cu.l.x * rel.x + cu.l.z * rel.y, cu.l.y * rel.x + cu.l.w * rel.y) + cu.tv.zw;
    let blur_px = vel * cu.params.z;
    let len = length(blur_px);
    let base = max(1, i32(cu.params.w + 0.5));
    if (len < 0.75 || base <= 1) {
        return textureSampleLevel(ctex, csamp, in.uv, 0.0) * cu.extra.x;
    }
    let n = clamp(max(base, i32(len * 0.5)), base, 64);
    let nf = f32(n);
    let d = blur_px / size;
    var acc = vec4<f32>(0.0);
    for (var i = 0; i < n; i = i + 1) {
        let t = (f32(i) + 0.5) / nf - 0.5;
        acc = acc + textureSampleLevel(ctex, csamp, in.uv + d * t, 0.0);
    }
    return acc / nf * cu.extra.x;
}

struct GradeU {
    params: vec4<f32>, // desaturate, dim alpha, 0, 0
    dim: vec4<f32>,
};

@group(0) @binding(0) var<uniform> gu: GradeU;
@group(0) @binding(1) var gtex: texture_2d<f32>;
@group(0) @binding(2) var gsamp: sampler;

@fragment
fn fs_grade(in: FullOut) -> @location(0) vec4<f32> {
    var col = textureSampleLevel(gtex, gsamp, in.uv, 0.0);
    let desat = clamp(gu.params.x, 0.0, 1.0);
    if (desat > 0.0001) {
        let luma = dot(col.rgb, vec3<f32>(0.3, 0.59, 0.11));
        col = vec4<f32>(mix(col.rgb, vec3<f32>(luma), desat), col.a);
    }
    let dim_a = clamp(gu.params.y, 0.0, 1.0);
    if (dim_a > 0.0001) {
        col = vec4<f32>(mix(col.rgb, gu.dim.rgb * col.a, dim_a), col.a);
    }
    return col;
}
