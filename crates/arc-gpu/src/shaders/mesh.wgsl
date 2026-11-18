// Instanced meshes with camera-relative translations and reversed-Z depth (CORE-GPU-007), their
// feature edges (CORE-GPU-014), highlight tint and mask (CORE-GPU-023) and identifiers
// (CORE-GPU-022).

struct View {
    // Reversed-Z projection times the view rotation; no translation.
    view_proj: mat4x4<f32>,
    // xyz: direction toward the headlight (world); w: ambient term.
    light: vec4<f32>,
    // width, height, paper factor, line smoothing.
    viewport: vec4<f32>,
    // x: 1 lit, 0 unlit; y: face opacity; z: 1 when edges use `edge_color`; w: pick tolerance.
    shading: vec4<f32>,
    edge_color: vec4<f32>,
    // The tint color; alpha is the tint strength.
    highlight: vec4<f32>,
    // x: the highlight mask code.
    codes: vec4<u32>,
};

@group(0) @binding(0) var<uniform> view: View;

// Edges are moved toward the eye by this relative depth offset so they are not hidden by the
// faces they bound.
const EDGE_DEPTH_SCALE: f32 = 1.000244140625;

struct VertexIn {
    @builtin(instance_index) instance: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) row0: vec4<f32>,
    @location(3) row1: vec4<f32>,
    @location(4) row2: vec4<f32>,
    @location(5) color: vec4<f32>,
    // material, flags, identifier slot, reserved.
    @location(6) misc: vec4<u32>,
    @location(7) translation: vec3<f32>,
};

struct VertexOut {
    // Invariant so the tint pass reproduces the depth of the faces exactly.
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) relative: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) @interpolate(flat) misc: vec4<u32>,
    // The instance's index in the instance pool.
    @location(4) @interpolate(flat) instance: u32,
};

fn transform(in: VertexIn) -> VertexOut {
    let local = vec4<f32>(in.position, 1.0);
    // Position relative to the eye: instance transform, then the item's translation.
    let relative = vec3<f32>(dot(in.row0, local), dot(in.row1, local), dot(in.row2, local)) + in.translation;
    // Normals transform by the cofactor matrix of the linear part.
    let c0 = vec3<f32>(in.row0.x, in.row1.x, in.row2.x);
    let c1 = vec3<f32>(in.row0.y, in.row1.y, in.row2.y);
    let c2 = vec3<f32>(in.row0.z, in.row1.z, in.row2.z);
    let normal = in.normal.x * cross(c1, c2) + in.normal.y * cross(c2, c0) + in.normal.z * cross(c0, c1);
    var out: VertexOut;
    out.clip = view.view_proj * vec4<f32>(relative, 1.0);
    out.relative = relative;
    out.normal = normal;
    out.color = in.color;
    out.misc = in.misc;
    out.instance = in.instance;
    return out;
}

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    return transform(in);
}

@vertex
fn vs_edge(in: VertexIn) -> VertexOut {
    var out = transform(in);
    // Reversed-Z: nearer is greater; never in front of the near plane.
    out.clip.z = min(out.clip.z * EDGE_DEPTH_SCALE, out.clip.w);
    return out;
}

// The headlight model with two-sided Lambert shading plus an ambient term; shading modes set
// whether it is lit and the face opacity.
fn shade(base: vec4<f32>, normal: vec3<f32>) -> vec4<f32> {
    let lambert = abs(dot(normal, view.light.xyz));
    let ambient = view.light.w;
    let lit = base.rgb * (ambient + (1.0 - ambient) * lambert);
    return vec4<f32>(mix(base.rgb, lit, view.shading.x), base.a * view.shading.y);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Meshes without normals use the face normal from screen-space derivatives.
    let face = cross(dpdx(in.relative), dpdy(in.relative));
    let smooth_length = length(in.normal);
    var normal = face / max(length(face), 1e-30);
    if (smooth_length > 1e-12) {
        normal = in.normal / smooth_length;
    }
    return shade(in.color, normal);
}

@fragment
fn fs_edge(in: VertexOut) -> @location(0) vec4<f32> {
    return select(in.color, view.edge_color, view.shading.z > 0.5);
}

@fragment
fn fs_tint(in: VertexOut) -> @location(0) vec4<f32> {
    return view.highlight;
}

@fragment
fn fs_mask(in: VertexOut) -> @location(0) u32 {
    return view.codes.x;
}

@fragment
fn fs_pick(in: VertexOut) -> @location(0) u32 {
    return in.instance + 1u;
}
