//! Mapping of the symbol tables and block definitions.

use std::collections::BTreeSet;

use arc_base::Color;
use arc_doc::ColorValue;
use arc_draft::types::{
    ElementKind, MAX_ATTRIBUTES, MAX_LINEWEIGHT_MM, MAX_PATTERN_ELEMENTS, NAME_BYTES,
};
use arc_io_dxf::loss::Argument;
use arc_io_dxf::model::{Layer, LinetypeElement, TableEntry};
use arc_io_dxf::{Entity, EntityKind};

use crate::color;
use crate::error::reasons;
use crate::geo::rad;
use crate::mapper::{Mapper, extra_f64, extra_int, hex};
use crate::plan::{
    AttributeSpec, BlockSpec, Container, DimStyleSpec, ElementSpec, LayerSpec, LinetypeSpec,
    NamedSource, TextStyleSpec,
};

/// The oblique limit of a drafting text style, in degrees.
const MAX_OBLIQUE_DEGREES: f64 = 85.0;

/// DXF dimension variables and the dimension style fields they map to; `true` for lengths.
const DIM_VARIABLES: [(i16, &str, bool); 7] = [
    (40, "overall_scale", false),
    (41, "arrow_size", true),
    (42, "extension_offset", true),
    (44, "extension_beyond", true),
    (140, "text_height", true),
    (144, "measurement_scale", false),
    (147, "text_gap", true),
];

pub(crate) fn is_builtin_linetype(name: &str) -> bool {
    ["continuous", "bylayer", "byblock", ""].contains(&name.to_lowercase().as_str())
}

impl Mapper<'_> {
    /// `name` cut to the length bound of drafting names, noted when it changed.
    pub(crate) fn clip_name(
        &mut self,
        name: &str,
        kind: &str,
        source: Option<arc_io_dxf::Handle>,
    ) -> String {
        let bound = usize::try_from(NAME_BYTES).unwrap_or(255);
        if name.len() <= bound {
            return name.to_owned();
        }
        let mut end = bound;
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        self.rep.note(
            reasons::NAME_ADJUSTED,
            kind,
            hex(source),
            &[("what", Argument::Text("length".to_owned()))],
        );
        name.get(..end).unwrap_or_default().to_owned()
    }

    pub(crate) fn map_tables(&mut self) {
        let d = self.d;
        if let Some(table) = &d.tables.styles {
            for entry in &table.entries {
                self.map_text_style(entry);
            }
        }
        if let Some(table) = &d.tables.linetypes {
            for entry in &table.entries {
                self.map_linetype(entry);
            }
        }
        if let Some(table) = &d.tables.layers {
            for entry in &table.entries {
                self.map_layer(entry);
            }
        }
        if let Some(table) = &d.tables.dimstyles {
            for entry in &table.entries {
                self.map_dim_style(entry);
            }
        }
    }

    pub(crate) fn layer_name(&mut self, name: &str, source: Option<arc_io_dxf::Handle>) -> String {
        if let Some(found) = self.layers.find(name) {
            return found.name.clone();
        }
        let name = self.clip_name(name, "LAYER", source);
        self.layers.add(&name, default_layer(&name));
        self.rep
            .note(reasons::DEFINITION_CREATED, "LAYER", hex(source), &[]);
        name
    }

    /// The linetype name of a reference, `None` for continuous or an unknown name.
    pub(crate) fn linetype_name(
        &mut self,
        name: &str,
        source: Option<arc_io_dxf::Handle>,
    ) -> Option<String> {
        if is_builtin_linetype(name) {
            return None;
        }
        if let Some(found) = self.linetypes.find(name) {
            return Some(found.name.clone());
        }
        self.rep.note(
            reasons::VALUE_ADJUSTED,
            "LTYPE",
            hex(source),
            &[("what", Argument::Text("linetype_unresolved".to_owned()))],
        );
        None
    }

    pub(crate) fn text_style_name(
        &mut self,
        name: &str,
        source: Option<arc_io_dxf::Handle>,
    ) -> String {
        if let Some(found) = self.text_styles.find(name) {
            return found.name.clone();
        }
        let name = self.clip_name(name, "STYLE", source);
        self.text_styles.add(&name, default_text_style(&name));
        self.rep
            .note(reasons::DEFINITION_CREATED, "STYLE", hex(source), &[]);
        name
    }

    pub(crate) fn dim_style_name(
        &mut self,
        name: &str,
        source: Option<arc_io_dxf::Handle>,
    ) -> String {
        if let Some(found) = self.dim_styles.find(name) {
            return found.name.clone();
        }
        let name = self.clip_name(name, "DIMSTYLE", source);
        self.dim_styles.add(
            &name,
            DimStyleSpec {
                name: name.clone(),
                values: Vec::new(),
                source: None,
            },
        );
        self.rep
            .note(reasons::DEFINITION_CREATED, "DIMSTYLE", hex(source), &[]);
        name
    }

    fn map_text_style(&mut self, entry: &TableEntry<arc_io_dxf::model::TextStyle>) {
        let handle = entry.head.handle;
        if entry.flags & 1 != 0 {
            self.note(reasons::TABLE_SKIPPED, "STYLE", handle);
            return;
        }
        let name = self.clip_name(&entry.name, "STYLE", handle);
        let data = &entry.data;
        let font = data
            .font
            .rsplit(['/', '\\'])
            .next()
            .and_then(|f| f.rsplit_once('.').map(|(stem, _)| stem).or(Some(f)))
            .filter(|f| !f.is_empty())
            .map(str::to_owned);
        let oblique = data
            .oblique
            .clamp(-MAX_OBLIQUE_DEGREES, MAX_OBLIQUE_DEGREES);
        let width_factor = if data.width_factor.is_finite() && data.width_factor > 0.0 {
            data.width_factor
        } else {
            1.0
        };
        let spec = TextStyleSpec {
            name: name.clone(),
            font,
            height: (data.height.max(0.0)) * self.factor,
            width_factor,
            oblique: rad(oblique),
            backwards: data.generation & 2 != 0,
            upside_down: data.generation & 4 != 0,
            vertical: entry.flags & 4 != 0,
            source: handle,
        };
        if self.text_styles.add(&name, spec)
            && let Some(h) = handle
        {
            self.style_by_handle.insert(h.0, name);
        }
    }

    fn map_linetype(&mut self, entry: &TableEntry<arc_io_dxf::model::Linetype>) {
        let handle = entry.head.handle;
        if is_builtin_linetype(&entry.name) {
            self.standard_linetypes.push(NamedSource {
                name: entry.name.clone(),
                source: handle,
            });
            return;
        }
        let bound = usize::try_from(MAX_PATTERN_ELEMENTS).unwrap_or(256);
        if entry.data.elements.len() > bound {
            self.rep.note(
                reasons::VALUE_ADJUSTED,
                "LTYPE",
                hex(handle),
                &[("what", Argument::Text("pattern_elements".to_owned()))],
            );
        }
        let elements: Vec<ElementSpec> = entry
            .data
            .elements
            .iter()
            .take(bound)
            .map(|e| self.element(e))
            .collect();
        let total: f64 = elements.iter().map(|e| e.length).sum();
        if !(total.is_finite() && total > 0.0) {
            self.note(reasons::LINETYPE_INVALID, "LTYPE", handle);
            return;
        }
        let name = self.clip_name(&entry.name, "LTYPE", handle);
        let spec = LinetypeSpec {
            name: name.clone(),
            description: truncate(&entry.data.description, 4096),
            elements,
            source: handle,
        };
        self.linetypes.add(&name, spec);
    }

    fn element(&self, e: &LinetypeElement) -> ElementSpec {
        let kind = if e.kind & 2 != 0 {
            ElementKind::Text
        } else if e.kind & 4 != 0 {
            ElementKind::Shape
        } else if e.length > 0.0 {
            ElementKind::Dash
        } else if e.length < 0.0 {
            ElementKind::Gap
        } else {
            ElementKind::Dot
        };
        let scale = e.scale.filter(|s| s.is_finite() && *s > 0.0).unwrap_or(1.0);
        // Lengths and offsets are drawing lengths, so they convert with the file's unit like the
        // geometry; the scale and the rotation are ratios and an angle.
        let offset = (e.offset_x.is_some() || e.offset_y.is_some()).then(|| {
            [
                e.offset_x.unwrap_or(0.0) * self.factor,
                e.offset_y.unwrap_or(0.0) * self.factor,
            ]
        });
        ElementSpec {
            kind,
            length: e.length.abs() * self.factor,
            content: e
                .text
                .clone()
                .filter(|_| kind == ElementKind::Text)
                .map(|t| truncate(&t, 4096)),
            style: e
                .style
                .and_then(|h| self.style_by_handle.get(&h.0).cloned())
                .filter(|_| matches!(kind, ElementKind::Text | ElementKind::Shape)),
            scale,
            rotation: rad(e.rotation.unwrap_or(0.0)),
            offset,
        }
    }

    fn map_layer(&mut self, entry: &TableEntry<Layer>) {
        let handle = entry.head.handle;
        let name = self.clip_name(&entry.name, "LAYER", handle);
        let data = &entry.data;
        let index = data.color.unsigned_abs();
        let color = match (data.true_color, u8::try_from(index)) {
            (Some(rgb), _) => color::from_true(rgb),
            (None, Ok(i)) if i >= 1 => color::from_index(i),
            _ => ColorValue {
                rgba: Color::WHITE,
                source: None,
            },
        };
        let linetype = self.linetype_name(&data.linetype, handle);
        let lineweight = data.lineweight.filter(|w| *w >= 0).map(|w| {
            let mm = f64::from(w) / 100.0;
            if mm > MAX_LINEWEIGHT_MM {
                self.rep.note(
                    reasons::VALUE_ADJUSTED,
                    "LAYER",
                    hex(handle),
                    &[("what", Argument::Text("lineweight".to_owned()))],
                );
            }
            mm.min(MAX_LINEWEIGHT_MM)
        });
        let transparency =
            extra_int(&entry.extras, 440).map_or(0.0, |v| transparency_of(v).unwrap_or(0.0));
        let spec = LayerSpec {
            name: name.clone(),
            color,
            linetype,
            lineweight,
            transparency,
            on: data.color >= 0,
            frozen: entry.flags & 1 != 0,
            locked: entry.flags & 4 != 0,
            plottable: data.plot.unwrap_or(true),
            source: handle,
        };
        self.layers.add(&name, spec);
    }

    fn map_dim_style(&mut self, entry: &TableEntry<arc_io_dxf::model::DimStyle>) {
        let handle = entry.head.handle;
        let name = self.clip_name(&entry.name, "DIMSTYLE", handle);
        let mut values = Vec::new();
        for (code, field, is_length) in DIM_VARIABLES {
            let Some(v) = extra_f64(&entry.extras, code) else {
                continue;
            };
            let valid = v.is_finite() && if is_length { v >= 0.0 } else { v > 0.0 };
            if valid {
                values.push((field, if is_length { v * self.factor } else { v }));
            } else {
                self.rep.note(
                    reasons::VALUE_ADJUSTED,
                    "DIMSTYLE",
                    hex(handle),
                    &[("what", Argument::Text(field.to_owned()))],
                );
            }
        }
        if let Some(p) = extra_int(&entry.extras, 271).filter(|p| (0..=8).contains(p)) {
            #[expect(clippy::cast_precision_loss, reason = "a value of 0 to 8")]
            values.push(("precision", p as f64));
        }
        self.dim_styles.add(
            &name.clone(),
            DimStyleSpec {
                name,
                values,
                source: handle,
            },
        );
    }

    pub(crate) fn map_blocks(&mut self) {
        let d = self.d;
        let inserted = self.inserted_names();
        let dim_blocks = self.dimension_blocks();
        let mut included = Vec::new();
        for block in &d.blocks {
            let EntityKind::Block(begin) = &block.begin.kind else {
                continue;
            };
            let lower = begin.name.to_lowercase();
            let skip = lower == "*model_space" || lower.starts_with("*paper_space");
            let xref =
                begin.flags & 4 != 0 || begin.xref_path.as_deref().is_some_and(|p| !p.is_empty());
            if xref {
                self.note(reasons::ENTITY_UNMAPPED, "XREF", block.begin.source_id());
            } else if !skip && (!dim_blocks.contains(&lower) || inserted.contains(&lower)) {
                included.push((block, begin));
            }
        }
        let mut created = Vec::new();
        for (block, begin) in &included {
            let spec = self.block_spec(block, begin);
            if self.blocks.add(&spec.name.clone(), spec) {
                created.push((block, begin));
            } else {
                self.note(reasons::NAME_ADJUSTED, "BLOCK", block.begin.source_id());
            }
        }
        for (block, begin) in &created {
            let name = self
                .blocks
                .find(&begin.name)
                .map(|b| b.name.clone())
                .unwrap_or_default();
            let container = Container::Block(name);
            for e in &block.entities {
                if !matches!(e.kind, EntityKind::AttDef(_)) {
                    self.add_entity(e, &container, 1.0);
                }
            }
        }
    }

    fn block_spec(
        &mut self,
        block: &arc_io_dxf::model::Block,
        begin: &arc_io_dxf::model::BlockBegin,
    ) -> BlockSpec {
        let handle = block.begin.source_id();
        let name = self.clip_name(&begin.name, "BLOCK", handle);
        let record = self.d.tables.block_records.as_ref().and_then(|t| {
            t.entries
                .iter()
                .find(|e| e.name.eq_ignore_ascii_case(&begin.name))
        });
        let record_units = record.and_then(|e| crate::units::unit_of_code(i64::from(e.flags)));
        let source = record.and_then(|e| e.head.handle);
        let unit = record_units.unwrap_or_else(|| self.file_unit());
        let mut seen = BTreeSet::new();
        let mut attributes = Vec::new();
        let bound = usize::try_from(MAX_ATTRIBUTES).unwrap_or(1024);
        for e in &block.entities {
            if let EntityKind::AttDef(a) = &e.kind {
                if attributes.len() < bound && seen.insert(a.tag.to_lowercase()) {
                    attributes.push(AttributeSpec {
                        tag: truncate(&a.tag, 255),
                        prompt: Some(truncate(&a.prompt, 4096)).filter(|p| !p.is_empty()),
                        default: Some(clean_line(&a.value)).filter(|p| !p.is_empty()),
                        constant: a.flags & 2 != 0,
                        invisible: a.flags & 1 != 0,
                        preset: a.flags & 8 != 0,
                        position: arc_base::math::Point2::new(a.insert[0], a.insert[1]),
                    });
                } else {
                    self.note(reasons::ATTRIBUTE_DROPPED, "ATTDEF", e.source_id());
                }
            }
        }
        BlockSpec {
            name,
            base: arc_base::math::Point2::new(begin.base[0], begin.base[1]),
            unit,
            attributes,
            source,
        }
    }

    fn inserted_names(&self) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        let all = self
            .d
            .entities
            .iter()
            .chain(self.d.blocks.iter().flat_map(|b| b.entities.iter()));
        for e in all {
            if let EntityKind::Insert(i) = &e.kind {
                names.insert(i.block.to_lowercase());
            }
        }
        names
    }

    fn dimension_blocks(&self) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        let all: Vec<&Entity> = self
            .d
            .entities
            .iter()
            .chain(self.d.blocks.iter().flat_map(|b| b.entities.iter()))
            .collect();
        for e in all {
            if let EntityKind::Dimension(dim) = &e.kind
                && let Some(b) = &dim.block
            {
                names.insert(b.to_lowercase());
            }
        }
        names
    }
}

fn default_layer(name: &str) -> LayerSpec {
    LayerSpec {
        name: name.to_owned(),
        color: color::from_index(7),
        linetype: None,
        lineweight: None,
        transparency: 0.0,
        on: true,
        frozen: false,
        locked: false,
        plottable: true,
        source: None,
    }
}

fn default_text_style(name: &str) -> TextStyleSpec {
    TextStyleSpec {
        name: name.to_owned(),
        font: None,
        height: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        backwards: false,
        upside_down: false,
        vertical: false,
        source: None,
    }
}

/// `text` cut to `max` bytes at a character boundary.
pub(crate) fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or_default().to_owned()
}

/// `text` without control characters other than tab.
pub(crate) fn clean_line(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\t')
        .collect()
}

/// The transparency (0 opaque to 1) of a DXF transparency value (code 440), `None` for by-layer
/// and by-block.
pub(crate) fn transparency_of(value: i64) -> Option<f64> {
    if value & 0x0300_0000 == 0x0200_0000 {
        #[expect(clippy::cast_precision_loss, reason = "a value of 0 to 255")]
        let alpha = (value & 0xFF) as f64;
        Some(1.0 - alpha / 255.0)
    } else {
        None
    }
}
