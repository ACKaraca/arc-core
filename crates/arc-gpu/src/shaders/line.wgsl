// Exact-width lines as instanced segment quads with signed-distance caps and joins
// (CORE-GPU-008), tapered polyline widths, the lineweight display switch and dash patterns
// evaluated by the position along the path (CORE-GPU-009).

struct View {
    view_proj: mat4x4<f32>,
    light: vec4<f32>,
    // width, height, paper factor (pixels per paper millimetre at clip w = 1), line smoothing.
    viewport: vec4<f32>,
    // w: pick tolerance in pixels, added to the half width in the identifier pass.
    shading: vec4<f32>,
    edge_color: vec4<f32>,
    // The tint color; alpha is the tint strength.
    highlight: vec4<f32>,
    // x: the highlight mask code.
    codes: vec4<u32>,
    background: vec4<f32>,
    paper: vec4<f32>,
    // x: 1 when lineweights are displayed; y: pixels per item unit at clip w = 1.
    draft: vec4<f32>,
};

@group(0) @binding(0) var<uniform> view: View;

// End kinds.
const ROUND_CAP: u32 = 0u;
const SQUARE_CAP: u32 = 1u;
const CUT: u32 = 2u;
const ROUND_JOIN: u32 = 3u;
const MITER_JOIN: u32 = 4u;
const BEVEL_JOIN: u32 = 5u;

struct SegmentIn {
    @builtin(vertex_index) corner: u32,
    @builtin(instance_index) instance: u32,
    @location(0) a: vec3<f32>,
    @location(1) b: vec3<f32>,
    @location(2) prev: vec3<f32>,
    @location(3) next: vec3<f32>,
    @location(4) color: vec4<f32>,
    // full width, miter limit.
    @location(5) params: vec2<f32>,
    // style bits, identifier slot.
    @location(6) bits: vec2<u32>,
    @location(7) translation: vec3<f32>,
    // width at b, pattern positions at a and b, pattern period.
    @location(8) dash: vec4<f32>,
    // Four (start, end) intervals of the pattern; unused ones are negative.
    @location(9) pattern0: vec4<f32>,
    @location(10) pattern1: vec4<f32>,
};

struct SegmentOut {
    @builtin(position) clip: vec4<f32>,
    // Screen positions in pixels (y down): a, b, previous, next.
    @location(0) @interpolate(flat) ends: vec4<f32>,
    @location(1) @interpolate(flat) neighbors: vec4<f32>,
    // half width at a, miter limit, half width at b.
    @location(2) @interpolate(flat) params: vec3<f32>,
    // end kinds at a and b.
    @location(3) @interpolate(flat) kinds: vec2<u32>,
    @location(4) @interpolate(flat) color: vec4<f32>,
    // The segment's index in the segment pool.
    @location(5) @interpolate(flat) instance: u32,
    // pattern positions at a and b, period (0: solid).
    @location(6) @interpolate(flat) dash: vec3<f32>,
    @location(7) @interpolate(flat) pattern0: vec4<f32>,
    @location(8) @interpolate(flat) pattern1: vec4<f32>,
};

// Style bit 6: the points are pixel positions (overlays), not offsets from the item origin.
fn place(p: vec3<f32>, t: vec3<f32>, screen: bool) -> vec4<f32> {
    if (screen) {
        return vec4<f32>(p.x / view.viewport.x * 2.0 - 1.0, 1.0 - p.y / view.viewport.y * 2.0, 0.5, 1.0);
    }
    return view.view_proj * vec4<f32>(p + t, 1.0);
}

fn to_screen(c: vec4<f32>) -> vec2<f32> {
    let n = c.xy / c.w;
    return vec2<f32>((n.x * 0.5 + 0.5) * view.viewport.x, (0.5 - n.y * 0.5) * view.viewport.y);
}

// Distance in front of the near plane in reversed-Z clip space (depth <= 1).
fn inside(c: vec4<f32>) -> f32 {
    return c.w - c.z;
}

// Moves `c`, which lies behind the near plane, along the segment to `toward` onto the plane.
fn clip_to(c: vec4<f32>, toward: vec4<f32>) -> vec4<f32> {
    let dc = inside(c);
    return mix(c, toward, dc / (dc - inside(toward)));
}

fn end_kind(style: u32, joined: bool) -> u32 {
    if (joined) {
        return ROUND_JOIN + ((style >> 1u) & 3u);
    }
    // Polyline widths (bit 9) end flat at free ends.
    if ((style & 512u) != 0u) {
        return CUT;
    }
    return style & 1u;
}

@vertex
fn vs_main(in: SegmentIn) -> SegmentOut {
    var out: SegmentOut;
    let style = in.bits.x;
    let screen = (style & 64u) != 0u;
    out.instance = in.instance;
    var ca = place(in.a, in.translation, screen);
    var cb = place(in.b, in.translation, screen);
    var kind_a = end_kind(style, (style & 8u) != 0u);
    var kind_b = end_kind(style, (style & 16u) != 0u);
    if (inside(ca) < 0.0 && inside(cb) < 0.0) {
        // Entirely behind the near plane: a degenerate quad outside the depth range.
        out.clip = vec4<f32>(0.0, 0.0, -1.0, 1.0);
        return out;
    }
    if (inside(ca) < 0.0) {
        ca = clip_to(ca, cb);
        kind_a = CUT;
    }
    if (inside(cb) < 0.0) {
        cb = clip_to(cb, ca);
        kind_b = CUT;
    }
    let sa = to_screen(ca);
    let sb = to_screen(cb);
    var sp = sa;
    var sn = sb;
    if (kind_a >= ROUND_JOIN) {
        var cp = place(in.prev, in.translation, screen);
        if (inside(cp) < 0.0) {
            cp = clip_to(cp, ca);
        }
        sp = to_screen(cp);
    }
    if (kind_b >= ROUND_JOIN) {
        var cn = place(in.next, in.translation, screen);
        if (inside(cn) < 0.0) {
            cn = clip_to(cn, cb);
        }
        sn = to_screen(cn);
    }
    var width = vec2<f32>(in.params.x, in.dash.x);
    if ((style & 32u) != 0u) {
        // Paper width: millimetres to pixels at the depth of the start point.
        width = width * view.viewport.z / ca.w;
    }
    if ((style & 512u) != 0u) {
        // Polyline widths: item units to pixels at the depth of the start point.
        width = width * view.draft.y / ca.w;
    }
    if ((style & 128u) != 0u && view.draft.x < 0.5) {
        // Lineweight display is off: lineweights draw one pixel wide.
        width = vec2<f32>(1.0, 1.0);
    }
    let hws = max(width, vec2<f32>(1.0, 1.0)) * 0.5 + vec2<f32>(view.shading.w, view.shading.w);
    let hw = max(hws.x, hws.y);
    let miter = max(in.params.y, 1.0);

    let delta = sb - sa;
    let len = length(delta);
    var d = vec2<f32>(1.0, 0.0);
    if (len > 1e-4) {
        d = delta / len;
    }
    let n = vec2<f32>(-d.y, d.x);
    var reach = hw;
    if (kind_a == MITER_JOIN || kind_b == MITER_JOIN || kind_a == BEVEL_JOIN || kind_b == BEVEL_JOIN) {
        reach = hw * miter;
    }
    // Two triangles: corners (a,-) (b,-) (a,+) (a,+) (b,-) (b,+); one pixel of margin for
    // antialiasing.
    let at_b = in.corner == 1u || in.corner == 4u || in.corner == 5u;
    let side = select(-1.0, 1.0, in.corner == 2u || in.corner == 3u || in.corner == 5u);
    let base = select(sa, sb, at_b);
    let along = select(-(reach + 1.0), reach + 1.0, at_b);
    let p = base + d * along + n * side * (hw + 1.0);
    let depth = select(ca.z / ca.w, cb.z / cb.w, at_b);
    out.clip = vec4<f32>(p.x / view.viewport.x * 2.0 - 1.0, 1.0 - p.y / view.viewport.y * 2.0, depth, 1.0);
    out.ends = vec4<f32>(sa, sb);
    out.neighbors = vec4<f32>(sp, sn);
    out.params = vec3<f32>(hws.x, miter, hws.y);
    out.kinds = vec2<u32>(kind_a, kind_b);
    out.color = in.color;
    var period = 0.0;
    if ((style & 256u) != 0u) {
        period = in.dash.w;
    }
    out.dash = vec3<f32>(in.dash.y, in.dash.z, period);
    out.pattern0 = in.pattern0;
    out.pattern1 = in.pattern1;
    return out;
}

// The signed distance of the half-plane that bounds the segment at end `s`, whose outward
// direction is `o`; `w` points from `s` along the adjacent segment. x: distance; y: 1 when a
// disk of the half width around `s` belongs to the region.
fn end_distance(p: vec2<f32>, s: vec2<f32>, o: vec2<f32>, neighbor: vec2<f32>, kind: u32, hw: f32, miter: f32) -> vec2<f32> {
    let along = dot(p - s, o);
    if (kind == SQUARE_CAP) {
        return vec2<f32>(along - hw, 0.0);
    }
    if (kind == CUT) {
        return vec2<f32>(along, 0.0);
    }
    let toward = neighbor - s;
    if (kind == ROUND_CAP || kind == ROUND_JOIN || length(toward) < 1e-4) {
        return vec2<f32>(along, 1.0);
    }
    let w = toward / length(toward);
    let sum = o + w;
    let sum_length = length(sum);
    if (sum_length < 1e-3) {
        // The path reverses: a round join.
        return vec2<f32>(along, 1.0);
    }
    // Split at the bisector of the corner.
    var dist = dot(p - s, sum / sum_length);
    let half_cos = sum_length * 0.5;
    let difference = o - w;
    if ((kind == BEVEL_JOIN || 1.0 / half_cos > miter) && length(difference) > 1e-6) {
        dist = max(dist, dot(p - s, difference / length(difference)) - hw * half_cos);
    }
    return vec2<f32>(dist, 0.0);
}

// The half width at the fragment: interpolated between the ends along the segment.
fn half_width(in: SegmentOut, p: vec2<f32>) -> f32 {
    let sa = in.ends.xy;
    let delta = in.ends.zw - sa;
    let len2 = dot(delta, delta);
    var t = 0.0;
    if (len2 > 1e-8) {
        t = clamp(dot(p - sa, delta) / len2, 0.0, 1.0);
    }
    return mix(in.params.x, in.params.z, t);
}

// The signed distance in pixels from the fragment to the segment's region.
fn signed_distance(in: SegmentOut) -> f32 {
    let p = in.clip.xy;
    let sa = in.ends.xy;
    let sb = in.ends.zw;
    let delta = sb - sa;
    let len = length(delta);
    var d = vec2<f32>(1.0, 0.0);
    if (len > 1e-4) {
        d = delta / len;
    }
    let n = vec2<f32>(-d.y, d.x);
    let hw_a = in.params.x;
    let hw_b = in.params.z;
    let end_a = end_distance(p, sa, -d, in.neighbors.xy, in.kinds.x, hw_a, in.params.y);
    let end_b = end_distance(p, sb, d, in.neighbors.zw, in.kinds.y, hw_b, in.params.y);
    var dist = max(abs(dot(p - sa, n)) - half_width(in, p), max(end_a.x, end_b.x));
    if (end_a.y > 0.5) {
        dist = min(dist, length(p - sa) - hw_a);
    }
    if (end_b.y > 0.5) {
        dist = min(dist, length(p - sb) - hw_b);
    }
    return dist;
}

// The interval `k` of the pattern: (start, end), negative when unused.
fn interval(in: SegmentOut, k: u32) -> vec2<f32> {
    switch k {
        case 0u: { return in.pattern0.xy; }
        case 1u: { return in.pattern0.zw; }
        case 2u: { return in.pattern1.xy; }
        default: { return in.pattern1.zw; }
    }
}

// The signed distance of the dashed region: the segment's region where the pattern position
// is inside a dash (ends cut flat), joined with the disks of dots. The pattern position
// interpolates linearly in the image between the positions of the segment's ends, so it is
// fixed in the document.
fn dashed_distance(in: SegmentOut, shape: f32) -> f32 {
    let period = in.dash.z;
    let span = in.dash.y - in.dash.x;
    let sa = in.ends.xy;
    let delta = in.ends.zw - sa;
    let len = length(delta);
    if (period <= 0.0 || span <= 0.0 || len < 1e-4) {
        return shape;
    }
    let p = in.clip.xy;
    let d = delta / len;
    let along = clamp(dot(p - sa, d), 0.0, len);
    let perp = abs(dot(p - sa, vec2<f32>(-d.y, d.x)));
    let px = len / span;
    let s = in.dash.x + along / px;
    let u = s - floor(s / period) * period;
    let hw = half_width(in, p);
    var dash = 1e9;
    var dots = 1e9;
    for (var k = 0u; k < 4u; k = k + 1u) {
        let iv = interval(in, k);
        if (iv.x < 0.0) {
            continue;
        }
        for (var r = -1; r <= 1; r = r + 1) {
            let o = f32(r) * period;
            if (iv.y > iv.x) {
                dash = min(dash, max(iv.x + o - u, u - iv.y - o) * px);
            } else {
                dots = min(dots, length(vec2<f32>((u - iv.x - o) * px, perp)) - hw);
            }
        }
    }
    return min(max(shape, dash), dots);
}

fn color_distance(in: SegmentOut) -> f32 {
    return dashed_distance(in, signed_distance(in));
}

fn coverage(dist: f32) -> f32 {
    if (view.viewport.w > 0.5) {
        return clamp(0.5 - dist, 0.0, 1.0);
    }
    return select(0.0, 1.0, dist <= 0.0);
}

@fragment
fn fs_main(in: SegmentOut) -> @location(0) vec4<f32> {
    let c = coverage(color_distance(in));
    if (c <= 0.0) {
        discard;
    }
    return vec4<f32>(in.color.rgb, in.color.a * c);
}

@fragment
fn fs_tint(in: SegmentOut) -> @location(0) vec4<f32> {
    let c = coverage(color_distance(in));
    if (c <= 0.0) {
        discard;
    }
    return vec4<f32>(view.highlight.rgb, view.highlight.a * c);
}

// The mask and identifier passes cover a pixel when its center is inside.
@fragment
fn fs_mask(in: SegmentOut) -> @location(0) u32 {
    if (signed_distance(in) > 0.0) {
        discard;
    }
    return view.codes.x;
}

@fragment
fn fs_pick(in: SegmentOut) -> @location(0) u32 {
    if (signed_distance(in) > 0.0) {
        discard;
    }
    return (in.instance + 1u) | 0x80000000u;
}
