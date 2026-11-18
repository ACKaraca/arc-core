// The highlight outline (CORE-GPU-023): pixels outside the highlight mask within the outline
// radius of a highlighted pixel take the outline color, hover before selection.

struct Outline {
    selection: vec4<f32>,
    hover: vec4<f32>,
    // x: radius in pixels.
    radius: vec4<i32>,
};

@group(0) @binding(0) var mask: texture_2d<u32>;
@group(0) @binding(1) var<uniform> outline: Outline;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    // One triangle covering the target.
    let x = f32((index << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(index & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.5, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(mask));
    let at = vec2<i32>(floor(position.xy));
    if (textureLoad(mask, at, 0).x != 0u) {
        discard;
    }
    let r = outline.radius.x;
    var found = 0u;
    for (var dy = -r; dy <= r; dy++) {
        for (var dx = -r; dx <= r; dx++) {
            let q = at + vec2<i32>(dx, dy);
            if (dx * dx + dy * dy <= r * r && all(q >= vec2<i32>(0)) && all(q < size)) {
                found = max(found, textureLoad(mask, q, 0).x);
            }
        }
    }
    if (found == 0u) {
        discard;
    }
    if (found >= 2u) {
        return outline.hover;
    }
    return outline.selection;
}
