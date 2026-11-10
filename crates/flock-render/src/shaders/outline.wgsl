// Inverted-hull outline: the mesh pushed out along its normals, drawn with front faces culled,
// in a flat colour (instance tint 0, w = width multiplier). Used for the local player.
struct OIn {
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec4<f32>,
    @location(2) col: vec4<f32>,
    @location(3) m0: vec4<f32>,
    @location(4) m1: vec4<f32>,
    @location(5) m2: vec4<f32>,
    @location(6) m3: vec4<f32>,
    @location(7) t0: vec4<f32>,
    @location(8) t1: vec4<f32>,
};

struct OOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
    // Translucent ghost passes: the instance's t1.w (1 for the opaque local outline).
    @location(1) alpha: f32,
};

@vertex
fn vs_outline(in: OIn) -> OOut {
    var o: OOut;
    let model = mat4x4<f32>(in.m0, in.m1, in.m2, in.m3);
    let wp = model * vec4<f32>(in.pos, 1.0);
    let sx = max(dot(in.m0.xyz, in.m0.xyz), 1e-8);
    let sy = max(dot(in.m1.xyz, in.m1.xyz), 1e-8);
    let sz = max(dot(in.m2.xyz, in.m2.xyz), 1e-8);
    let n = in.nrm.xyz;
    let wn = normalize(in.m0.xyz * (n.x / sx) + in.m1.xyz * (n.y / sy) + in.m2.xyz * (n.z / sz));
    let dist = length(g.cam_pos.xyz - wp.xyz);
    // Roughly constant on screen: a little wider the further away.
    let width = (0.010 + 0.0032 * dist) * in.t0.w;
    o.clip = g.view_proj * vec4<f32>(wp.xyz + wn * width, 1.0);
    o.color = in.t0.rgb;
    o.alpha = in.t1.w;
    return o;
}

@fragment
fn fs_outline(in: OOut) -> @location(0) vec4<f32> {
    return vec4<f32>(encode(in.color), 1.0);
}

// Ghost birds seen as a clean outline: the same hull (width multiplier t0.w, 0 for the flat
// fill) drawn translucent in the player's colour with alpha t1.w. The stencil lets only the
// first fragment of each pixel through, so overlapping parts never stack.
@fragment
fn fs_flat(in: OOut) -> @location(0) vec4<f32> {
    return vec4<f32>(encode(in.color), clamp(in.alpha, 0.0, 1.0));
}
