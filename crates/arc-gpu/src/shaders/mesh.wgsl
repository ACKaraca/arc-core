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
    background: vec4<f32>,
    paper: vec4<f32>,
    // x: 1 when lineweights are displayed; y: pixels per item unit at clip w = 1; z: 1 for a
    // perspective view.
    draft: vec4<f32>,
    // Section: x plane count, y 1 when the box clips, z 1 when caps are drawn, w 1 when the caps
    // are hatched.
    section: vec4<f32>,
    section_color: vec4<f32>,
    // Planes (unit normal, distance) relative to the translation origin: a fragment is removed
    // when dot(normal, relative) > distance.
    planes: array<vec4<f32>, 6>,
    box_min: vec4<f32>,
    box_max: vec4<f32>,
    // The hatch of caps: x, y the normal of the lines in pixels, z the spacing and w the width
    // of a line, both in pixels.
    section_hatch: vec4<f32>,
    // The hatch color; alpha is 1.
    section_hatch_color: vec4<f32>,
    // x: 1 when faces with an override opacity below 1 are drawn in a second pass; y: 1 in the
    // identifier pass; z: 1 when the draw's object style replaces the category styles.
    over: vec4<f32>,
    // The object style of the draw (see `styles`).
    object_color: vec4<f32>,
    object_params: vec4<f32>,
    // Category styles: per category a color (w = 1 replaces the instance color) and
    // (opacity, hidden, ghost, 0).
    styles: array<vec4<f32>, 32>,
};

@group(0) @binding(0) var<uniform> view: View;

// Edges are moved toward the eye by this relative depth offset so they are not hidden by the
// faces they bound.
const EDGE_DEPTH_SCALE: f32 = 1.000244140625;
// Half of the edge allowance, on the logarithmic scale (its square is about the edge scale).
const FILL_DEPTH_SCALE: f32 = 1.0001220703125;

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
    // The override opacity.
    @location(5) @interpolate(flat) fade: f32,
    // The normal of the second face of a silhouette edge.
    @location(6) @interpolate(flat) other: vec3<f32>,
    // The sign of the determinant of the instance's linear part: negative for a mirrored instance,
    // whose triangles wind the other way.
    @location(7) @interpolate(flat) det: f32,
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
    out.other = vec3<f32>(0.0);
    out.det = dot(cross(c0, c1), c2);
    // Display overrides: the object's style, or the style of the instance's category.
    let category = (in.misc.y >> 24u) & 15u;
    let category_params = view.styles[category * 2u + 1u];
    var style_color = view.styles[category * 2u];
    var style_params = category_params;
    if (view.over.z > 0.5) {
        style_color = view.object_color;
        style_params = view.object_params;
    }
    if (style_color.w > 0.5) {
        out.color = vec4<f32>(style_color.rgb, in.color.a);
    }
    out.fade = style_params.x;
    // An instance is hidden when its category or the object's style hides it; a ghost is hidden
    // in the identifier pass. The collapse lies outside the depth range.
    let hidden = category_params.y > 0.5 || style_params.y > 0.5;
    if (hidden || (style_params.z > 0.5 && view.over.y > 0.5)) {
        out.clip = vec4<f32>(0.0, 0.0, -1.0, 1.0);
    }
    return out;
}

// Whether a fragment at `relative` is removed by the section planes or box.
fn clipped(relative: vec3<f32>) -> bool {
    let count = u32(view.section.x);
    for (var i = 0u; i < 6u; i++) {
        if (i < count && dot(view.planes[i].xyz, relative) > view.planes[i].w) {
            return true;
        }
    }
    return view.section.y > 0.5
        && (any(relative < view.box_min.xyz) || any(relative > view.box_max.xyz));
}

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out = transform(in);
    // Read in the entry point: the DirectX 12 backend adds the first instance of the draw only
    // to an instance index that an entry point reads directly.
    out.instance = in.instance;
    // Painted masks and paper lie in the plane of the drawing. Their depth is moved back by the
    // edge allowance so that coplanar lines drawn over them win on every backend: mesh and line
    // positions are computed by different shaders and need not agree to the last bit. Fills of
    // drawings are moved back by half of it (lines win, masks stay behind), and glyphs of
    // outline text toward the eye by all of it, so text wins over the fills it lies on.
    if ((in.misc.y & (PAINT_BACKGROUND | PAINT_PAPER)) != 0u) {
        out.clip.z = out.clip.z / EDGE_DEPTH_SCALE;
    } else if ((in.misc.y & DRAFT_FILL) != 0u) {
        out.clip.z = out.clip.z / FILL_DEPTH_SCALE;
    } else if ((in.misc.y & DRAFT_TEXT) != 0u) {
        out.clip.z = min(out.clip.z * EDGE_DEPTH_SCALE, out.clip.w);
    }
    return out;
}

@vertex
fn vs_edge(in: VertexIn) -> VertexOut {
    var out = transform(in);
    out.instance = in.instance;
    // Reversed-Z: nearer is greater; never in front of the near plane.
    out.clip.z = min(out.clip.z * EDGE_DEPTH_SCALE, out.clip.w);
    return out;
}

struct SilhouetteIn {
    @builtin(instance_index) instance: u32,
    @location(0) position: vec3<f32>,
    // The unit normals of the two faces of the edge.
    @location(1) normal: vec3<f32>,
    @location(8) other: vec3<f32>,
    @location(2) row0: vec4<f32>,
    @location(3) row1: vec4<f32>,
    @location(4) row2: vec4<f32>,
    @location(5) color: vec4<f32>,
    @location(6) misc: vec4<u32>,
    @location(7) translation: vec3<f32>,
};

@vertex
fn vs_silhouette(in: SilhouetteIn) -> VertexOut {
    var v: VertexIn;
    v.instance = in.instance;
    v.position = in.position;
    v.normal = in.normal;
    v.row0 = in.row0;
    v.row1 = in.row1;
    v.row2 = in.row2;
    v.color = in.color;
    v.misc = in.misc;
    v.translation = in.translation;
    var out = transform(v);
    out.instance = in.instance;
    let c0 = vec3<f32>(in.row0.x, in.row1.x, in.row2.x);
    let c1 = vec3<f32>(in.row0.y, in.row1.y, in.row2.y);
    let c2 = vec3<f32>(in.row0.z, in.row1.z, in.row2.z);
    out.other = in.other.x * cross(c1, c2) + in.other.y * cross(c2, c0) + in.other.z * cross(c0, c1);
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

// Instance flags: bits 24 to 27 are the category of display overrides (see `transform`).
// Bit 4 marks a closed solid whose cut a section caps.
const SOLID: u32 = 16u;
// Instance flags: bit 0 paints the background color, bit 1 the paper color, both unlit
// (wipeout masks and sheet backgrounds).
const PAINT_BACKGROUND: u32 = 1u;
const PAINT_PAPER: u32 = 2u;
// Bit 2 marks a fill of a drawing and bit 3 a glyph of outline text; they only move the depth.
const DRAFT_FILL: u32 = 4u;
const DRAFT_TEXT: u32 = 8u;

// The face color: the section cap color for a back face of a closed solid that the section
// cut open, otherwise the shaded color. `faded` selects the pass of faces with an override
// opacity below 1.
fn face_color(in: VertexOut, front: bool, faded: bool) -> vec4<f32> {
    // Derivatives are taken before any discard, in uniform control flow.
    let face = cross(dpdx(in.relative), dpdy(in.relative));
    if (clipped(in.relative)) {
        discard;
    }
    // Two-pass transparency: faded faces are left to the blended pass, and only they draw there.
    if (view.over.x > 0.5 && (in.fade < 1.0) != faded) {
        discard;
    }
    if ((in.misc.y & PAINT_BACKGROUND) != 0u) {
        return vec4<f32>(view.background.rgb, 1.0);
    }
    if ((in.misc.y & PAINT_PAPER) != 0u) {
        return vec4<f32>(view.paper.rgb, 1.0);
    }
    var result: vec4<f32>;
    if (view.section.z > 0.5 && !front && (in.misc.y & SOLID) != 0u) {
        result = vec4<f32>(view.section_color.rgb, 1.0);
        // Lines where the screen position, measured across the lines, is within the width of a
        // multiple of the spacing. Screen positions are the same on every backend.
        if (view.section.w > 0.5) {
            let across = dot(in.clip.xy, view.section_hatch.xy) / view.section_hatch.z;
            if (fract(across) < view.section_hatch.w / view.section_hatch.z) {
                result = view.section_hatch_color;
            }
        }
    } else {
        // Meshes without normals use the face normal from screen-space derivatives.
        let smooth_length = length(in.normal);
        var normal = face / max(length(face), 1e-30);
        if (smooth_length > 1e-12) {
            normal = in.normal / smooth_length;
        }
        result = shade(in.color, normal);
    }
    return vec4<f32>(result.rgb, result.a * in.fade);
}

// A mirrored instance (negative determinant) winds its triangles the other way, so a face is
// front when its winding is front for a positive determinant and back otherwise.
@fragment
fn fs_main(in: VertexOut, @builtin(front_facing) facing: bool) -> @location(0) vec4<f32> {
    return face_color(in, facing == (in.det > 0.0), false);
}

// The blended pass of faces with an override opacity below 1.
@fragment
fn fs_fade(in: VertexOut, @builtin(front_facing) facing: bool) -> @location(0) vec4<f32> {
    return face_color(in, facing == (in.det > 0.0), true);
}

@fragment
fn fs_edge(in: VertexOut) -> @location(0) vec4<f32> {
    if (clipped(in.relative)) {
        discard;
    }
    let color = select(in.color, view.edge_color, view.shading.z > 0.5);
    return vec4<f32>(color.rgb, color.a * in.fade);
}

// A silhouette edge: the two faces of the edge face opposite ways as seen from the eye, which
// is decided per fragment so perspective views give the exact contour.
@fragment
fn fs_silhouette(in: VertexOut) -> @location(0) vec4<f32> {
    if (clipped(in.relative)) {
        discard;
    }
    let to_eye = select(view.light.xyz, normalize(-in.relative), view.draft.z > 0.5);
    if ((dot(in.normal, to_eye) > 0.0) == (dot(in.other, to_eye) > 0.0)) {
        discard;
    }
    let color = select(in.color, view.edge_color, view.shading.z > 0.5);
    return vec4<f32>(color.rgb, color.a * in.fade);
}

@fragment
fn fs_tint(in: VertexOut) -> @location(0) vec4<f32> {
    if (clipped(in.relative)) {
        discard;
    }
    return view.highlight;
}

@fragment
fn fs_mask(in: VertexOut) -> @location(0) u32 {
    if (clipped(in.relative)) {
        discard;
    }
    return view.codes.x;
}

@fragment
fn fs_pick(in: VertexOut) -> @location(0) u32 {
    if (clipped(in.relative)) {
        discard;
    }
    return in.instance + 1u;
}
