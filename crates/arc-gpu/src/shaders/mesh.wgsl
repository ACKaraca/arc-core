// Instanced meshes with camera-relative translations and reversed-Z depth (CORE-GPU-007).

struct View {
    // Reversed-Z projection times the view rotation; no translation.
    view_proj: mat4x4<f32>,
    // xyz: direction toward the headlight (world); w: ambient term.
    light: vec4<f32>,
    // width, height, paper factor, line smoothing.
    viewport: vec4<f32>,
};

@group(0) @binding(0) var<uniform> view: View;

struct VertexIn {
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
    @builtin(position) clip: vec4<f32>,
    @location(0) relative: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) @interpolate(flat) misc: vec4<u32>,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
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
    return out;
}

// The phase-0 lighting model: a headlight with two-sided Lambert shading plus an ambient term.
// Shading modes replace this function without re-extraction.
fn shade(base: vec4<f32>, normal: vec3<f32>) -> vec4<f32> {
    let lambert = abs(dot(normal, view.light.xyz));
    let ambient = view.light.w;
    return vec4<f32>(base.rgb * (ambient + (1.0 - ambient) * lambert), base.a);
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
