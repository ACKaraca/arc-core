// Filled overlay triangles in pixel coordinates, drawn above the scene (CORE-GPU-025).

struct View {
    view_proj: mat4x4<f32>,
    light: vec4<f32>,
    // width, height, paper factor, line smoothing.
    viewport: vec4<f32>,
    shading: vec4<f32>,
    edge_color: vec4<f32>,
    highlight: vec4<f32>,
    codes: vec4<u32>,
};

@group(0) @binding(0) var<uniform> view: View;

struct FillIn {
    // Pixels, y down.
    @location(0) position: vec2<f32>,
    @location(1) color: vec4<f32>,
};

struct FillOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(in: FillIn) -> FillOut {
    var out: FillOut;
    let p = in.position;
    out.clip = vec4<f32>(p.x / view.viewport.x * 2.0 - 1.0, 1.0 - p.y / view.viewport.y * 2.0, 0.5, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: FillOut) -> @location(0) vec4<f32> {
    return in.color;
}
