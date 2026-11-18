//! The paper rule of pen resolution: a pen equal to the paper color plots in the contrasting
//! extreme unless the style table set the color.

#![allow(clippy::unwrap_used, clippy::float_cmp, reason = "test code")]

mod support;

use arc_base::Color;
use arc_doc::{ColorSource, ColorValue};
use arc_draft::plot_style::{Appearance, PlotStyleTable, resolve_pen_on_paper};
use serde_json::json;
use support::Fixture;

fn pen_color(
    table: Option<&PlotStyleTable>,
    rgba: Color,
    index: Option<u8>,
    paper: Color,
) -> Color {
    let value = ColorValue {
        rgba,
        source: index.map(ColorSource::Indexed),
    };
    let appearance = Appearance {
        color: &value,
        lineweight: None,
        linetype: None,
        transparency: 0.0,
        style_name: None,
    };
    resolve_pen_on_paper(table, &appearance, paper).color
}

#[test]
fn paper_colored_pens_take_the_contrasting_extreme() {
    let black = Color::new(0.0, 0.0, 0.0, 1.0);
    assert_eq!(pen_color(None, Color::WHITE, None, Color::WHITE), black);
    // Within the tolerance counts as equal; outside does not.
    let near = Color::new(0.997, 1.0, 1.0, 1.0);
    assert_eq!(pen_color(None, near, None, Color::WHITE), black);
    let far = Color::new(0.98, 1.0, 1.0, 1.0);
    assert_eq!(pen_color(None, far, None, Color::WHITE), far);
    // Black paper gets white ink; other colors are untouched.
    assert_eq!(pen_color(None, black, None, black), Color::WHITE);
    let red = Color::new(1.0, 0.0, 0.0, 1.0);
    assert_eq!(pen_color(None, red, None, Color::WHITE), red);
    // The alpha survives.
    let half = Color::new(1.0, 1.0, 1.0, 0.5);
    assert_eq!(pen_color(None, half, None, Color::WHITE).a, 0.5);
}

#[test]
fn an_explicit_style_color_is_kept_even_when_it_equals_the_paper() {
    let mut f = Fixture::new();
    let id = f.create(
        "core.draft.plot_style_table.create",
        json!({"name": "t", "kind": "color_mapped",
               "styles": [{"index": 7, "color": [1.0, 1.0, 1.0, 1.0]}]}),
    );
    let table = PlotStyleTable::from_values(&f.values(id));
    // Index 7 has the explicit white; index 8 has no style and falls under the rule.
    assert_eq!(
        pen_color(Some(&table), Color::WHITE, Some(7), Color::WHITE),
        Color::WHITE
    );
    assert_eq!(
        pen_color(Some(&table), Color::WHITE, Some(8), Color::WHITE),
        Color::new(0.0, 0.0, 0.0, 1.0)
    );
}
