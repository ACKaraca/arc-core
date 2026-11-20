//! The viewport widget through a headless egui context (CORE-UI-005, CORE-UI-029, CORE-UI-027
//! for the viewport): a click on an object rendered by the `arc-gpu` adapter gives its
//! selection intent, two viewports behave identically with independent or linked cameras, and
//! the accessibility tree names the viewport and describes its selection and prompt.

#![cfg(feature = "viewport")]
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    reason = "tests"
)]

use arc_base::error::Value;
use arc_base::id::Identifier;
use arc_base::math::{Point3, Vector3};
use arc_base::{Color, Limits, ObjectId, Tolerance};
use arc_command::prompt::{Accept, PointDims, Prompt};
use arc_geom::view::{Projection, ViewFrame};
use arc_gpu::{
    Gpu, GpuSettings, ItemKey, MeshInstance, MeshItem, PickHit, PickRegion, RenderItem,
    RenderSettings, Renderer, Scene, SceneLimits,
};
use arc_mesh::TriMesh;
use arc_ui::egui::{self, accesskit};
use arc_ui::{
    Intent, Localize, ScenePicker, ThemeId, UiScale, ViewLink, ViewPicker, ViewportCommands,
    ViewportInput, ViewportOutput, ViewportWidget, object_list,
};

struct English;
impl Localize for English {
    fn message(&self, id: &str, args: &[(&str, Value)]) -> Option<String> {
        let arg = |name: &str| {
            args.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| match v {
                    Value::Text(t) => t.clone(),
                    Value::Int(n) => n.to_string(),
                    other => format!("{other:?}"),
                })
                .unwrap_or_default()
        };
        Some(match id {
            "ui-viewport" => format!("{} viewport", arg("name")),
            "ui-viewport-selection" => format!("{} objects selected", arg("count")),
            "ui-viewport-status" => format!("{}. {}", arg("selection"), arg("prompt")),
            "demo-pick-point" => "Pick the first point".to_owned(),
            _ => return None,
        })
    }
}

fn top(height: f64) -> ViewFrame {
    ViewFrame::new(
        Point3::new(0.0, 0.0, 50.0),
        Point3::origin(),
        Vector3::y(),
        Projection::Orthographic { height },
        1.0,
        1000.0,
    )
    .unwrap()
}

fn pass<R>(
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    mut ui_fn: impl FnMut(&mut egui::Ui) -> R,
) -> (Vec<accesskit::Node>, R) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(400.0, 300.0),
        )),
        events,
        ..Default::default()
    };
    let mut result = None;
    let mut full = ctx.run_ui(input, |ui| result = Some(ui_fn(ui)));
    full.textures_delta.clear();
    let nodes = full
        .platform_output
        .accesskit_update
        .map(|u| u.nodes.into_iter().map(|(_, n)| n).collect())
        .unwrap_or_default();
    (nodes, result.unwrap())
}

fn button(at: egui::Pos2, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers,
    }
}

fn click(at: egui::Pos2) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(at),
        button(at, true, egui::Modifiers::NONE),
        button(at, false, egui::Modifiers::NONE),
    ]
}

fn select(ids: &[ObjectId], gesture: &str) -> Intent {
    Intent::new("core.select.set")
        .with("objects", object_list(ids))
        .with("mode", "replace")
        .with("gesture", gesture)
}

/// The square of side 2 in the xy plane, centered on the origin and facing +z.
fn square() -> TriMesh {
    let p = |x, y| Point3::new(x, y, 0.0);
    TriMesh::new(
        vec![p(-1.0, -1.0), p(1.0, -1.0), p(1.0, 1.0), p(-1.0, 1.0)],
        vec![[0, 1, 2], [0, 2, 3]],
        &Limits::DEFAULT,
    )
    .unwrap()
}

/// The position in egui points of model point `p` inside a viewport shown at `out`.
fn screen(out: &ViewportOutput, frame: &ViewFrame, p: Point3) -> egui::Pos2 {
    let rect = out.response.rect;
    let ndc = frame
        .project(&p, f64::from(out.size[0]) / f64::from(out.size[1]))
        .unwrap();
    egui::pos2(
        rect.min.x + ((ndc.x + 1.0) / 2.0) as f32 * rect.width(),
        rect.min.y + ((1.0 - ndc.y) / 2.0) as f32 * rect.height(),
    )
}

#[test]
fn clicking_an_object_rendered_by_the_adapter_gives_its_selection_intent() {
    let gpu = Gpu::new(&GpuSettings::default()).unwrap_or_else(|e| {
        panic!(
            "no usable graphics adapter ({}); on Linux install the software Vulkan driver",
            e.code()
        )
    });
    let mut scene = Scene::new(&gpu, SceneLimits::DEFAULT).unwrap();
    let mut renderer = Renderer::new(&gpu);
    let settings = RenderSettings::default();
    let geometry = scene.add_geometry(&square(), &Point3::origin()).unwrap();
    let (a, b) = (ObjectId::from_u128(1), ObjectId::from_u128(2));
    let at = |x: f32, y: f32| MeshInstance {
        transform: [[8.0, 0.0, 0.0, x], [0.0, 8.0, 0.0, y], [0.0, 0.0, 8.0, 0.0]],
        ..MeshInstance::new(Color::WHITE)
    };
    let item = |x, y| {
        vec![RenderItem::Mesh(MeshItem {
            geometry,
            origin: Point3::origin(),
            instances: vec![at(x, y)],
        })]
    };
    scene
        .update(vec![(a, item(20.0, 10.0)), (b, item(-30.0, -15.0))])
        .unwrap();

    let frame = top(100.0);
    let colors = ThemeId::Light.theme().colors;
    let commands = ViewportCommands::default();
    let tolerance = Tolerance::DEFAULT;
    let ctx = egui::Context::default();
    let mut widget = ViewportWidget::new();
    let mut show = |events: Vec<egui::Event>,
                    renderer: &mut Renderer,
                    scene: &mut Scene,
                    texture: Option<egui::TextureId>| {
        pass(&ctx, events, |ui| {
            let mut picker = ScenePicker {
                renderer: &mut *renderer,
                scene: &mut *scene,
                settings: &settings,
            };
            widget.show(
                ui,
                ViewportInput {
                    texture,
                    frame: &frame,
                    prompt: None,
                    selected: 0,
                    picker: &mut picker,
                    commands: &commands,
                    localize: &English,
                    colors: &colors,
                    scale: UiScale::NORMAL,
                    tolerance: &tolerance,
                    name: "Top",
                    time: 0.0,
                    product: None,
                },
            )
        })
        .1
    };
    // The first frame reports the size; the caller renders at it and registers the texture.
    let first = show(vec![], &mut renderer, &mut scene, None);
    let [w, h] = first.size;
    let image = renderer
        .render_to_image(&mut scene, &frame, w, h, &settings)
        .unwrap();
    let handle = ctx.load_texture(
        "viewport",
        egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &image.rgba8),
        egui::TextureOptions::LINEAR,
    );
    let id = handle.id();
    // The pixel under object a is drawn (white) in the frame the widget shows.
    let pa = screen(&first, &frame, Point3::new(20.0, 10.0, 0.0));
    let px = image
        .pixel(
            ((pa.x - first.response.rect.min.x) as u32).min(w - 1),
            ((pa.y - first.response.rect.min.y) as u32).min(h - 1),
        )
        .unwrap();
    assert!(px[0] > 128, "object drawn under the click point: {px:?}");

    let out = show(click(pa), &mut renderer, &mut scene, Some(id));
    assert_eq!(out.intent, Some(select(&[a], "click")));
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    // Hovering b pre-selects it without an intent.
    let pb = screen(&first, &frame, Point3::new(-30.0, -15.0, 0.0));
    let out = show(
        vec![egui::Event::PointerMoved(pb)],
        &mut renderer,
        &mut scene,
        Some(id),
    );
    assert_eq!((out.hover, out.intent), (Some(b), None));
    // Right to left over both is a crossing selection of both; the band is an overlay.
    let (from, to) = (pa + egui::vec2(40.0, -40.0), pb + egui::vec2(0.0, 10.0));
    let mid = from + (to - from) / 2.0;
    let out = show(
        vec![
            egui::Event::PointerMoved(from),
            button(from, true, egui::Modifiers::NONE),
            egui::Event::PointerMoved(mid),
        ],
        &mut renderer,
        &mut scene,
        Some(id),
    );
    assert!(!out.overlay.primitives.is_empty());
    let out = show(
        vec![
            egui::Event::PointerMoved(to),
            button(to, false, egui::Modifiers::NONE),
        ],
        &mut renderer,
        &mut scene,
        Some(id),
    );
    assert_eq!(out.intent, Some(select(&[a, b], "crossing")));
    assert!(out.overlay.primitives.is_empty());
    drop(handle);
}

/// Object 1 covers pixels 10..30 in both directions of every viewport.
struct Fake;
impl ViewPicker for Fake {
    fn pick(
        &mut self,
        _: &ViewFrame,
        _: u32,
        _: u32,
        r: &PickRegion,
    ) -> arc_base::Result<Vec<PickHit>> {
        let overlap = r.x < 30 && r.x + r.width > 10 && r.y < 30 && r.y + r.height > 10;
        Ok(if overlap {
            vec![PickHit {
                key: ItemKey {
                    object: ObjectId::from_u128(1),
                    sub: 0,
                },
                element: 0,
            }]
        } else {
            Vec::new()
        })
    }
    fn bounds(&mut self) -> Option<(Point3, f64)> {
        None
    }
}

struct Two {
    widgets: [ViewportWidget; 3],
    frames: [ViewFrame; 3],
}

impl Two {
    /// Shows three viewports side by side; returns their outputs and applies their cameras.
    fn show(&mut self, ctx: &egui::Context, events: Vec<egui::Event>) -> Vec<ViewportOutput> {
        let colors = ThemeId::Dark.theme().colors;
        let commands = ViewportCommands::default();
        let tolerance = Tolerance::DEFAULT;
        let frames = self.frames;
        let outs = pass(ctx, events, |ui| {
            let mut outs = Vec::new();
            ui.columns(3, |cols| {
                for ((col, widget), frame) in cols.iter_mut().zip(&mut self.widgets).zip(&frames) {
                    outs.push(widget.show(
                        col,
                        ViewportInput {
                            texture: None,
                            frame,
                            prompt: None,
                            selected: 0,
                            picker: &mut Fake,
                            commands: &commands,
                            localize: &English,
                            colors: &colors,
                            scale: UiScale::NORMAL,
                            tolerance: &tolerance,
                            name: "Side",
                            time: 0.0,
                            product: None,
                        },
                    ));
                }
            });
            outs
        })
        .1;
        for (frame, out) in self.frames.iter_mut().zip(&outs) {
            if let Some(c) = out.camera {
                *frame = c;
            }
        }
        outs
    }
}

#[test]
fn several_viewports_select_identically_with_independent_or_linked_cameras() {
    let link = ViewLink::new();
    let mut views = Two {
        widgets: [
            ViewportWidget::linked(link.clone()),
            ViewportWidget::linked(link),
            ViewportWidget::new(),
        ],
        frames: [top(100.0), top(100.0), top(100.0)],
    };
    let ctx = egui::Context::default();
    let outs = views.show(&ctx, vec![]);
    let origins: Vec<egui::Pos2> = outs.iter().map(|o| o.response.rect.min).collect();
    // The same click in each viewport gives the same intent.
    for (i, origin) in origins.iter().enumerate() {
        views.show(&ctx, vec![]);
        let outs = views.show(&ctx, click(*origin + egui::vec2(20.0, 20.0)));
        for (j, out) in outs.iter().enumerate() {
            let want = (i == j).then(|| select(&[ObjectId::from_u128(1)], "click"));
            assert_eq!(out.intent, want, "viewport {j} after a click in {i}");
        }
        // Cycling state is per viewport: a click on empty space selects nothing.
        let outs = views.show(&ctx, click(*origin + egui::vec2(60.0, 60.0)));
        assert!(outs.iter().all(|o| o.intent.is_none()));
    }
    // The wheel over the unlinked viewport moves only its camera.
    let wheel = |at: egui::Pos2| {
        vec![
            egui::Event::PointerMoved(at),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, 100.0),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    };
    let outs = views.show(&ctx, wheel(origins[2] + egui::vec2(40.0, 40.0)));
    assert!(outs[2].camera.is_some());
    assert!(outs[0].camera.is_none() && outs[1].camera.is_none());
    assert_eq!(views.frames[0], top(100.0));
    // The wheel over a linked viewport moves it; the other linked one, drawn after it,
    // follows in the same frame, and the unlinked one keeps its camera.
    let unlinked = views.frames[2];
    let outs = views.show(&ctx, wheel(origins[0] + egui::vec2(40.0, 40.0)));
    assert!(outs[0].camera.is_some());
    assert_eq!(outs[1].camera, outs[0].camera);
    let outs = views.show(&ctx, vec![]);
    assert!(outs.iter().all(|o| o.camera.is_none()));
    // Moving the second one: the first, drawn before it, follows on its next frame.
    let outs = views.show(&ctx, wheel(origins[1] + egui::vec2(40.0, 40.0)));
    assert!(outs[0].camera.is_none() && outs[1].camera.is_some());
    let outs = views.show(&ctx, vec![]);
    assert_eq!(outs[0].camera, Some(views.frames[1]));
    assert_eq!(views.frames[0], views.frames[1]);
    assert_eq!(views.frames[2], unlinked);
}

#[test]
fn the_viewport_exposes_its_name_role_selection_and_prompt() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let colors = ThemeId::HighContrastLight.theme().colors;
    let commands = ViewportCommands::default();
    let tolerance = Tolerance::DEFAULT;
    let frame = top(100.0);
    let prompt = Prompt::new(
        "start",
        "demo-pick-point",
        Accept::Point {
            dims: PointDims::Two,
        },
    );
    let mut widget = ViewportWidget::new();
    let mut node = |prompt: Option<&Prompt>, selected: usize| {
        let (nodes, _) = pass(&ctx, vec![], |ui| {
            widget.show(
                ui,
                ViewportInput {
                    texture: None,
                    frame: &frame,
                    prompt,
                    selected,
                    picker: &mut Fake,
                    commands: &commands,
                    localize: &English,
                    colors: &colors,
                    scale: UiScale::NORMAL,
                    tolerance: &tolerance,
                    name: "Top",
                    time: 0.0,
                    product: None,
                },
            )
        });
        nodes
            .into_iter()
            .find(|n| n.role() == accesskit::Role::Canvas)
            .expect("viewport node")
    };
    let idle = node(None, 2);
    assert_eq!(idle.label(), Some("Top viewport"));
    assert_eq!(idle.description(), Some("2 objects selected"));
    let prompting = node(Some(&prompt), 0);
    assert_eq!(
        prompting.description(),
        Some("0 objects selected. Pick the first point")
    );
}
