//! A headless harness over the shell composition: the reference product, a headless egui
//! context with the AccessKit tree, fake dialogs and helpers to find and click nodes.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    dead_code,
    reason = "tests"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_app::application::{AppConfig, Application, CloseAnswer};
use arc_app::locale::Localizer;
use arc_app::product::FileType;
use arc_app::recent::RecentFiles;
use arc_app::shell::platform::{self, Done, FileDialogs};
use arc_app::shell::{FrameInput, Request, Shell};
use arc_base::{IdGenerator, Limits};
use arc_command::FixedClock;
use arc_doc::package::LockOwner;
use arc_gpu::Gpu;
use arc_ui::Intent;
use arc_ui::egui::{self, accesskit};

/// A directory removed when dropped.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("arc-shell-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Dialogs that answer from fields.
#[derive(Default)]
pub struct FakeDialogs {
    pub open: Option<PathBuf>,
    pub save: Option<PathBuf>,
    pub close: Option<CloseAnswer>,
    pub asked: Vec<&'static str>,
}

impl FileDialogs for FakeDialogs {
    fn pick_open(&mut self, _: &[FileType]) -> Option<PathBuf> {
        self.asked.push("open");
        self.open.clone()
    }

    fn pick_save(&mut self, _: &[FileType], _: Option<&str>) -> Option<PathBuf> {
        self.asked.push("save");
        self.save.clone()
    }

    fn ask_close(&mut self, _: &str) -> CloseAnswer {
        self.asked.push("close");
        self.close.unwrap_or(CloseAnswer::Cancel)
    }
}

pub fn app_in(data: &Path) -> Application {
    let owner = LockOwner {
        user: "tester".into(),
        host: "test-host".into(),
        application: "arc-shell".into(),
        time_unix_ms: 0,
    };
    let product = arc_shell::product(Arc::new(FixedClock(0)), owner).unwrap();
    let next = AtomicU64::new(1 << 32);
    Application::new(
        product,
        AppConfig {
            data_dir: data.to_path_buf(),
            limits: Limits::DEFAULT,
            id_generators: Box::new(move || {
                IdGenerator::seeded(next.fetch_add(1, Ordering::Relaxed))
            }),
            recent: RecentFiles::new(20),
        },
    )
}

/// One frame's AccessKit nodes.
pub struct Pass {
    pub nodes: Vec<(accesskit::NodeId, accesskit::Node)>,
    pub requests: Vec<Request>,
}

impl Pass {
    fn label_of(&self, id: accesskit::NodeId) -> Option<&str> {
        self.nodes
            .iter()
            .find(|(n, _)| *n == id)
            .and_then(|(_, n)| n.label().or(n.value()))
    }

    /// The first node of `role` whose label, or the label of a node it is labelled by,
    /// contains `name`.
    pub fn find(&self, role: accesskit::Role, name: &str) -> Option<&accesskit::Node> {
        self.all(role, name).into_iter().next()
    }

    /// Every node of `role` named as for [`Pass::find`], in tree order.
    pub fn all(&self, role: accesskit::Role, name: &str) -> Vec<&accesskit::Node> {
        self.nodes
            .iter()
            .map(|(_, n)| n)
            .filter(|n| n.role() == role && n.bounds().is_some())
            .filter(|n| {
                n.label().is_some_and(|l| l.contains(name))
                    || n.labelled_by()
                        .iter()
                        .any(|id| self.label_of(*id).is_some_and(|l| l.contains(name)))
            })
            .collect()
    }
}

pub fn center(node: &accesskit::Node) -> egui::Pos2 {
    let b = node.bounds().unwrap();
    egui::pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
}

pub fn click(at: egui::Pos2) -> Vec<egui::Event> {
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    vec![egui::Event::PointerMoved(at), button(true), button(false)]
}

pub fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

/// The application, the shell and a headless egui context.
pub struct Harness {
    pub app: Application,
    pub shell: Shell,
    pub ctx: egui::Context,
    pub localizer: Localizer,
    pub dialogs: FakeDialogs,
    pub time: f64,
    pub done: Vec<Done>,
}

impl Harness {
    pub fn new(data: &Path, gpu: Option<Gpu>) -> Self {
        let app = app_in(data);
        let shell = Shell::new(&app, arc_shell::config(42).unwrap(), gpu);
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Self {
            app,
            shell,
            ctx,
            localizer: arc_shell::localizer(&[]).unwrap(),
            dialogs: FakeDialogs::default(),
            time: 0.0,
            done: Vec::new(),
        }
    }

    /// One frame with `events` and native menu `intents`; requests are carried out with the
    /// fake dialogs.
    pub fn frame_with(&mut self, events: Vec<egui::Event>, intents: Vec<Intent>) -> Pass {
        self.time += 0.5;
        let textures = BTreeMap::new();
        let input = FrameInput {
            raw: egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 900.0),
                )),
                events,
                ..Default::default()
            },
            time: self.time,
            localize: &self.localizer,
            textures: &textures,
            menu_intents: intents,
            native_menu: false,
        };
        let mut out = self.shell.frame(&mut self.app, &self.ctx, input);
        out.full.textures_delta.clear();
        for request in &out.requests {
            let done = platform::carry_out(&mut self.app, &mut self.dialogs, request).unwrap();
            self.done.push(done);
        }
        let update = out.full.platform_output.accesskit_update;
        let nodes = update.map(|u| u.nodes).unwrap_or_default();
        Pass {
            nodes,
            requests: out.requests,
        }
    }

    pub fn frame(&mut self, events: Vec<egui::Event>) -> Pass {
        self.frame_with(events, Vec::new())
    }

    pub fn menu(&mut self, command: &str) -> Pass {
        self.frame_with(Vec::new(), vec![Intent::new(command)])
    }

    /// The message text of `id`, as the shell shows it.
    pub fn text(&self, id: &str) -> String {
        self.localizer.format(id, &[]).text
    }

    /// Types `line` into the command line and presses Enter.
    pub fn command(&mut self, line: &str) {
        let label = self.text("ui-command-line");
        let pass = self.frame(Vec::new());
        let field = pass
            .find(accesskit::Role::TextInput, &label)
            .unwrap_or_else(|| panic!("no command line field labelled {label}"));
        let at = center(field);
        self.frame(click(at));
        self.frame(vec![egui::Event::Text(line.into())]);
        self.frame(vec![key(egui::Key::Enter, egui::Modifiers::NONE)]);
    }
}
