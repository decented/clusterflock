// Billboard particles. Dust (kind 0) is alpha-blended and is the most fill-hungry thing in the
// frame (hundreds of overlapping soft quads), so everything that is constant per puff is done in
// the vertex shader (lighting, fog, gamma) and the fragment shader is one texture fetch, a
// smoothstep and a multiply. Puffs that would cover a big part of the screen near the lens are
// shrunk to a cap, and puffs that are all but invisible are not rasterised at all.
// Glows, flames and stars (kinds 1..3, additive) keep their analytic fragment code: they are small.

@group(0) @binding(1) var dust_tex: texture_2d<f32>;
@group(0) @binding(2) var dust_samp: sampler;

// Largest dust puff radius on screen, in NDC (the screen is 2.0 tall).
const DUST_MAX_R: f32 = 0.38;
// Largest radius of the small bits (feathers, stars, mud, glows), NDC. A feather beside the lens
// must stay a feather, not a wall of colour.
const FEATHER_MAX_R: f32 = 0.055;
const STAR_MAX_R: f32 = 0.075;
const MUD_MAX_R: f32 = 0.06;
const GLOW_MAX_R: f32 = 0.30;

struct PIn {
    @builtin(vertex_index) vi: u32,
    @location(0) a: vec4<f32>,
    @location(1) b: vec4<f32>,
    @location(2) c: vec4<f32>,
};

struct POut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) info: vec4<f32>,
    @location(3) fogn: vec2<f32>,
    // Dust: gamma-encoded colour at neutral shade (xyz), how much the shade moves it (w).
    @location(4) base: vec4<f32>,
};

@vertex
fn vs_particle(in: PIn) -> POut {
    var q = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0));
    let corner = q[in.vi];
    let cs = cos(in.b.x);
    let sn = sin(in.b.x);
    let rc = vec2<f32>(corner.x * cs - corner.y * sn, corner.x * sn + corner.y * cs);
    let dist = length(in.a.xyz - g.cam_pos.xyz);
    let dust = in.b.z < 0.5;
    var half_size = in.a.w * 0.5;
    var near = 1.0;
    var alpha_k = 1.0;
    var o: POut;
    o.base = vec4<f32>(0.0);
    let fogf = fog_factor(dist);
    if (dust) {
        // Cap the on-screen radius: a puff right at the lens becomes a smaller, denser one
        // instead of a veil over the whole view.
        let centre = g.view_proj * vec4<f32>(in.a.xyz, 1.0);
        let ndc_r = half_size * g.view_proj[1][1] / max(centre.w, 0.05);
        let k = min(1.0, DUST_MAX_R / max(ndc_r, 1e-4));
        half_size = half_size * k;
        alpha_k = 0.6 + 0.4 * k;
        near = smoothstep(0.5, 1.6, dist) * (0.2 + 0.8 * smoothstep(1.5, 6.0, dist));
        let light = clamp(g.amb_sky.rgb * 1.1 + g.sun_color.rgb * 0.75, vec3<f32>(0.10), vec3<f32>(1.15));
        let rgb = mix(in.c.rgb * light, g.fog.rgb, fogf * 0.8);
        o.base = vec4<f32>(encode(rgb), 0.4545 * (1.0 - 0.8 * fogf));
    } else {
        near = 0.12 + 0.88 * smoothstep(1.5, 6.0, dist);
        // Everything else is capped too (feathers and stars hardest), and fades out as it nears the lens.
        var cap = GLOW_MAX_R;
        if (in.b.z > 4.5 && in.b.z < 5.5) {
            cap = FEATHER_MAX_R;
            near = smoothstep(0.8, 3.0, dist);
        } else if (in.b.z > 5.5) {
            cap = MUD_MAX_R;
            near = smoothstep(0.8, 3.0, dist);
        } else if (in.b.z > 2.5 && in.b.z < 3.5) {
            cap = STAR_MAX_R;
            near = 0.1 + 0.9 * smoothstep(1.0, 4.0, dist);
        }
        if (in.b.z < 3.5 || in.b.z > 4.5) {
            let centre = g.view_proj * vec4<f32>(in.a.xyz, 1.0);
            let ndc_r = half_size * g.view_proj[1][1] / max(centre.w, 0.05);
            half_size = half_size * min(1.0, cap / max(ndc_r, 1e-4));
        }
    }
    let world = in.a.xyz + (g.cam_right.xyz * rc.x + g.cam_up.xyz * rc.y) * half_size;
    o.clip = g.view_proj * vec4<f32>(world, 1.0);
    o.uv = corner;
    o.color = vec4<f32>(in.c.rgb, in.c.a * alpha_k);
    o.info = vec4<f32>(in.b.y, in.b.z, in.b.w, 0.0);
    o.fogn = vec2<f32>(fogf, near);
    // Next to invisible: do not rasterise it.
    if (dust && o.color.a * near < 0.006) {
        o.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    }
    return o;
}

@fragment
fn fs_particle(in: POut) -> @location(0) vec4<f32> {
    let kind = in.info.y;
    let seed = in.info.x;
    if (kind < 0.5) {
        let r2 = dot(in.uv, in.uv);
        if (r2 >= 1.0) {
            discard;
        }
        let r = sqrt(r2);
        let nz = textureSampleLevel(dust_tex, dust_samp, in.uv * 0.5 + vec2<f32>(0.5 + seed * 0.113, 0.5 + seed * 0.071), 0.0).r;
        let a = smoothstep(1.0, 0.1, r * (1.0 + 0.5 * nz)) * (0.55 + 0.45 * nz);
        let shade = 0.78 + 0.3 * clamp(0.35 - in.uv.y * 0.6 + nz * 0.25, 0.0, 1.0);
        let col = in.base.rgb * (1.0 + in.base.w * (shade - 1.0));
        return vec4<f32>(col, a * in.color.a * in.fogn.y);
    }
    let r = length(in.uv);
    var rgb = in.color.rgb;
    var alpha = 0.0;
    if (kind < 1.5) {
        let core = exp(-r * r * 7.0);
        let halo = exp(-r * r * 2.2) * 0.3;
        alpha = core + halo;
        rgb = mix(rgb, vec3<f32>(1.0), core * 0.75) * 1.4;
        alpha = alpha * (1.0 - in.fogn.x);
    } else if (kind < 2.5) {
        let body = pow(smoothstep(1.0, 0.0, r), 1.4);
        alpha = body;
        rgb = mix(vec3<f32>(1.0, 0.92, 0.65), rgb, clamp(r * 1.1 + in.info.z * 0.9, 0.0, 1.0)) * 1.5;
        alpha = alpha * (1.0 - in.fogn.x);
    } else if (kind < 3.5) {
        let cross = exp(-min(abs(in.uv.x), abs(in.uv.y)) * 11.0) * smoothstep(1.0, 0.1, r);
        let core = exp(-r * r * 10.0);
        alpha = cross + core;
        rgb = mix(rgb, vec3<f32>(1.0), core * 0.8) * 1.5;
        alpha = alpha * (1.0 - in.fogn.x);
    } else if (kind < 4.5) {
        // A loose orange glow, with a clear centre and no solid shell.
        let p = in.uv * vec2<f32>(1.10, 0.86);
        let angle = atan2(p.y, p.x);
        let radius = length(p) + 0.035 * sin(angle * 5.0 - g.cam_pos.w * 3.0);
        let halo = exp(-pow((radius - 0.60) * 4.0, 2.0));
        alpha = halo * smoothstep(1.0, 0.76, radius) * smoothstep(1.0, 0.78, r) * (1.0 - in.fogn.x);
        rgb = rgb * 1.4;
    } else if (kind < 5.5) {
        // A feather: a tapered vane along the local y axis with a pale quill.
        let t = in.uv.y * 0.5 + 0.5;
        let hw = 0.40 * pow(max(sin(3.14159 * t), 0.0), 0.65) * (1.0 - 0.25 * t) + 0.02;
        let ax = abs(in.uv.x);
        let vane = smoothstep(hw, hw - 0.07, ax);
        if (vane <= 0.01) {
            discard;
        }
        let quill = smoothstep(0.07, 0.0, ax);
        let barbs = 0.92 + 0.08 * sin(in.uv.y * 34.0 + in.uv.x * 22.0);
        let edge = 1.0 - 0.25 * smoothstep(hw - 0.16, hw, ax);
        let lit = 0.78 + 0.28 * t;
        rgb = mix(rgb * barbs * edge * lit, vec3<f32>(0.95, 0.92, 0.85), quill * 0.8) * 1.15;
        alpha = vane * (1.0 - in.fogn.x);
    } else {
        // A mud droplet: dark, wet, with a glossy highlight.
        let wob = 1.0 + 0.12 * sin(atan2(in.uv.y, in.uv.x) * 5.0 + seed);
        let rr = r * wob;
        if (rr > 1.0) {
            discard;
        }
        let gloss = smoothstep(0.42, 0.0, length(in.uv - vec2<f32>(-0.32, 0.34)));
        rgb = rgb * (0.85 + 0.25 * (1.0 - rr)) + vec3<f32>(0.9, 0.8, 0.65) * gloss * 0.55;
        alpha = smoothstep(1.0, 0.82, rr) * (1.0 - in.fogn.x);
    }
    alpha = alpha * in.color.a * in.fogn.y;
    return vec4<f32>(encode(rgb), clamp(alpha, 0.0, 1.0));
}
