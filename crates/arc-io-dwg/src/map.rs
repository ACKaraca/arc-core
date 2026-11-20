//! Mapping of a library document to the neutral drawing model: the header variables the model
//! carries, the symbol tables, the block definitions, the entities and the record limits.

use std::collections::HashSet;

use acadrust::CadDocument;
use acadrust::entities::EntityType;
use acadrust::types::{Vector2, Vector3};
use arc_base::{Cancel, Limits, Result};
use arc_io_dxf::loss::{Argument, LossHeader, LossReport};
use arc_io_dxf::model::{
    self as m, Block, Drawing, Entity, EntityKind, HeaderVariable, Table, TableEntry, Tables,
};
use arc_io_dxf::{Group, Progress, Version};

use crate::convert::{self, RecordBudget, array, finite2, finite3, p2, p3, text};
use crate::entity::{self, Outcome};
use crate::error::{limit, reasons};
use crate::loss::LossLog;

/// How many records pass between two progress reports.
const PROGRESS_EVERY: u64 = 1024;

/// The symbol tables the drawing model carries; each one is counted as a mapped record, as a table
/// record of the DXF reader is.
const CARRIED_TABLES: u64 = 6;

/// Maps one library document, collecting the loss items of the mapping.
pub(crate) struct Mapper<'a, 'q> {
    limits: &'a Limits,
    cancel: &'a Cancel,
    progress: Option<&'a mut (dyn FnMut(Progress) + 'q)>,
    total: u64,
    records: u64,
    loss: LossLog,
}

impl<'a, 'q> Mapper<'a, 'q> {
    pub(crate) fn new(
        limits: &'a Limits,
        cancel: &'a Cancel,
        progress: Option<&'a mut (dyn FnMut(Progress) + 'q)>,
        total: u64,
    ) -> Self {
        Self {
            limits,
            cancel,
            progress,
            total,
            records: 0,
            loss: LossLog::bounded(),
        }
    }

    /// Counts one record against the record limit and checks cancellation.
    fn record(&mut self) -> Result<()> {
        self.cancel.check()?;
        self.records = self.records.saturating_add(1);
        if self.records > self.limits.record_count {
            return Err(limit("record_count", self.records));
        }
        if let (true, Some(report)) = (
            self.records.is_multiple_of(PROGRESS_EVERY),
            self.progress.as_mut(),
        ) {
            report(Progress {
                done: self.records,
                total: self.total,
            });
        }
        Ok(())
    }

    /// Adds the loss item of a value the model does not accept, which is skipped.
    fn skipped(&mut self, kind: &str, what: &str) {
        self.loss.add_with(
            reasons::VALUE_SKIPPED,
            kind,
            None,
            1,
            vec![("name".to_owned(), Argument::Text(what.to_owned()))],
        );
    }

    /// The header variables the model carries, under their DXF names and group codes. The
    /// variables of the dimension system and of the view, camera, render and UCS settings are not
    /// mapped; they are reported together as one item of `io.dwg.header_dropped`. A number that is
    /// not finite is skipped and reported.
    fn header(&mut self, doc: &CadDocument) -> Result<Vec<HeaderVariable>> {
        let h = &doc.header;
        self.loss.add(reasons::HEADER_DROPPED, "", None, 1);
        let mut out = Vec::new();
        // Identity: the fingerprint and version identifiers, the handle seed and the code page.
        self.text_var(&mut out, "$FINGERPRINTGUID", 2, &h.fingerprint_guid)?;
        self.text_var(&mut out, "$VERSIONGUID", 2, &h.version_guid)?;
        self.var(
            &mut out,
            "$HANDSEED",
            vec![Group::text(5, arc_io_dxf::Handle(h.handle_seed).to_hex())],
        );
        self.text_var(&mut out, "$DWGCODEPAGE", 3, &h.code_page)?;
        self.text_var(&mut out, "$MENU", 1, &h.menu_name)?;
        self.text_var(&mut out, "$PROJECTNAME", 1, &h.project_name)?;
        self.text_var(&mut out, "$LASTSAVEDBY", 1, &h.last_saved_by)?;
        self.text_var(&mut out, "$HYPERLINKBASE", 1, &h.hyperlink_base)?;
        // Units and measurement.
        self.int_var(&mut out, "$INSUNITS", 70, i64::from(h.insertion_units));
        self.int_var(&mut out, "$MEASUREMENT", 70, i64::from(h.measurement));
        self.int_var(&mut out, "$LUNITS", 70, i64::from(h.linear_unit_format));
        self.int_var(&mut out, "$LUPREC", 70, i64::from(h.linear_unit_precision));
        self.int_var(&mut out, "$AUNITS", 70, i64::from(h.angular_unit_format));
        self.int_var(&mut out, "$AUPREC", 70, i64::from(h.angular_unit_precision));
        // The library stores angles in radians; the header model keeps degrees, as the DXF text does.
        self.real_var(&mut out, "$ANGBASE", 50, h.angle_base.to_degrees());
        self.int_var(&mut out, "$ANGDIR", 70, i64::from(h.angle_direction));
        // Limits, extents and the insertion base of model space.
        self.point2_var(&mut out, "$LIMMIN", h.model_space_limits_min);
        self.point2_var(&mut out, "$LIMMAX", h.model_space_limits_max);
        self.int_var(&mut out, "$LIMCHECK", 70, flag(h.limit_check));
        self.point3_var(&mut out, "$EXTMIN", h.model_space_extents_min);
        self.point3_var(&mut out, "$EXTMAX", h.model_space_extents_max);
        self.point3_var(&mut out, "$INSBASE", h.model_space_insertion_base);
        // The current layer, text style, linetype, dimension style and multiline style by name.
        self.text_var(&mut out, "$CLAYER", 8, &h.current_layer_name)?;
        self.text_var(&mut out, "$TEXTSTYLE", 7, &h.current_text_style_name)?;
        self.text_var(&mut out, "$CELTYPE", 6, &h.current_linetype_name)?;
        self.text_var(&mut out, "$DIMSTYLE", 2, &h.current_dimstyle_name)?;
        self.text_var(&mut out, "$CMLSTYLE", 2, &h.multiline_style)?;
        // Current drafting settings and the time record.
        self.real_var(&mut out, "$LTSCALE", 40, h.linetype_scale);
        self.real_var(&mut out, "$CELTSCALE", 40, h.current_entity_linetype_scale);
        self.real_var(&mut out, "$TEXTSIZE", 40, h.text_height);
        self.real_var(&mut out, "$TRACEWID", 40, h.trace_width);
        self.real_var(&mut out, "$ELEVATION", 40, h.elevation);
        self.real_var(&mut out, "$THICKNESS", 40, h.thickness);
        self.real_var(&mut out, "$PDSIZE", 40, h.point_display_size);
        self.int_var(&mut out, "$PDMODE", 70, i64::from(h.point_display_mode));
        self.int_var(&mut out, "$ORTHOMODE", 70, flag(h.ortho_mode));
        self.int_var(&mut out, "$FILLMODE", 70, flag(h.fill_mode));
        self.int_var(
            &mut out,
            "$PSLTSCALE",
            70,
            flag(h.paper_space_linetype_scaling),
        );
        self.int_var(
            &mut out,
            "$PLINEGEN",
            70,
            flag(h.polyline_linetype_generation),
        );
        // A flag of the Boolean group codes (290 to 299) carries a Boolean, not an integer.
        self.var(
            &mut out,
            "$LWDISPLAY",
            vec![Group {
                code: 290,
                value: arc_io_dxf::Value::Bool(h.lineweight_display),
            }],
        );
        self.int_var(
            &mut out,
            "$CELWEIGHT",
            370,
            i64::from(h.current_line_weight),
        );
        self.int_var(&mut out, "$USRTIMER", 70, flag(h.user_timer));
        self.real_var(&mut out, "$TDCREATE", 40, h.create_date_julian);
        self.real_var(&mut out, "$TDUPDATE", 40, h.update_date_julian);
        self.real_var(&mut out, "$TDINDWG", 40, h.total_editing_time);
        Ok(out)
    }

    /// Adds the header variable `name` with `values`.
    fn var(&self, out: &mut Vec<HeaderVariable>, name: &str, values: Vec<Group>) {
        out.push(HeaderVariable {
            name: name.to_owned(),
            values,
        });
    }

    /// Adds a text header variable, cut to the string bound of the profile.
    fn text_var(
        &mut self,
        out: &mut Vec<HeaderVariable>,
        name: &str,
        code: i16,
        value: &str,
    ) -> Result<()> {
        let value = text(value, self.limits)?;
        self.var(out, name, vec![Group::text(code, value)]);
        Ok(())
    }

    /// Adds an integer header variable.
    fn int_var(&self, out: &mut Vec<HeaderVariable>, name: &str, code: i16, value: i64) {
        self.var(out, name, vec![Group::int(code, value)]);
    }

    /// Adds a real header variable, or skips and reports it when the value is not finite.
    fn real_var(&mut self, out: &mut Vec<HeaderVariable>, name: &str, code: i16, value: f64) {
        if value.is_finite() {
            self.var(out, name, vec![Group::double(code, value)]);
        } else {
            self.skipped("HEADER", name);
        }
    }

    /// Adds a two-dimensional point header variable (codes 10 and 20).
    fn point2_var(&mut self, out: &mut Vec<HeaderVariable>, name: &str, value: Vector2) {
        if finite2(value) {
            let [x, y] = p2(value);
            self.var(out, name, vec![Group::double(10, x), Group::double(20, y)]);
        } else {
            self.skipped("HEADER", name);
        }
    }

    /// Adds a three-dimensional point header variable (codes 10, 20 and 30).
    fn point3_var(&mut self, out: &mut Vec<HeaderVariable>, name: &str, value: Vector3) {
        if finite3(value) {
            let [x, y, z] = p3(value);
            self.var(
                out,
                name,
                vec![
                    Group::double(10, x),
                    Group::double(20, y),
                    Group::double(30, z),
                ],
            );
        } else {
            self.skipped("HEADER", name);
        }
    }

    /// Adds one kept table entry, with the names of the data it does not carry.
    fn table_entry<T>(
        &mut self,
        kind: &str,
        name: &str,
        handle: acadrust::types::Handle,
        data: T,
        flags: i16,
        partial: &[&'static str],
    ) -> Result<TableEntry<T>> {
        self.record()?;
        self.loss.add_mapped(1);
        self.loss
            .add_partial(reasons::TABLE_ENTRY_PARTIAL, kind, partial);
        Ok(TableEntry {
            head: convert::head(handle, acadrust::types::Handle::new(0)),
            name: text(name, self.limits)?,
            flags,
            data,
            extras: Vec::new(),
            xdata: Vec::new(),
        })
    }

    fn tables(&mut self, doc: &CadDocument) -> Result<Tables> {
        let layers = self.layers(doc)?;
        let linetypes = self.linetypes(doc)?;
        let styles = self.text_styles(doc)?;
        let mut app_ids = Vec::new();
        for app_id in doc.app_ids.iter() {
            app_ids.push(self.table_entry(
                "APPID",
                &app_id.name,
                app_id.handle,
                m::AppId::default(),
                0,
                &[],
            )?);
        }
        let mut dim_styles = Vec::new();
        for dim_style in doc.dim_styles.iter() {
            dim_styles.push(self.table_entry(
                "DIMSTYLE",
                &dim_style.name,
                dim_style.handle,
                m::DimStyle::default(),
                0,
                &["dimension_variables"],
            )?);
        }
        let block_records = self.block_record_table(doc)?;
        for (kind, count) in [
            ("VPORT", doc.vports.len()),
            ("VIEW", doc.views.len()),
            ("UCS", doc.ucss.len()),
            ("VX", doc.vx_table.len()),
        ] {
            let count = u64::try_from(count).unwrap_or(u64::MAX);
            self.loss.add(reasons::TABLE_DROPPED, kind, None, count);
        }
        self.loss.add_mapped(CARRIED_TABLES);
        Ok(Tables {
            layers: Some(table(layers)),
            linetypes: Some(table(linetypes)),
            styles: Some(table(styles)),
            appids: Some(table(app_ids)),
            dimstyles: Some(table(dim_styles)),
            block_records: Some(table(block_records)),
            ..Tables::default()
        })
    }

    fn layers(&mut self, doc: &CadDocument) -> Result<Vec<TableEntry<m::Layer>>> {
        let mut layers = Vec::new();
        for layer in doc.layers.iter() {
            let mut partial = Vec::new();
            let color = convert::color(&layer.color);
            if color.color.is_none() && color.true_color.is_none() {
                partial.push("color");
            }
            if !layer.plot_style.is_empty() {
                partial.push("plot_style");
            }
            if layer.flags.xref_dependent {
                partial.push("xref");
            }
            let mut flags = 0;
            if layer.flags.frozen {
                flags |= 1;
            }
            if layer.flags.frozen_in_new_viewport {
                flags |= 2;
            }
            if layer.flags.locked {
                flags |= 4;
            }
            // The model keeps "off" as a negative color number, so a color number of 0 (by block)
            // cannot carry it: the default color 7 stands in for it and the layer is reported partial.
            let number = match color.color {
                Some(0) if layer.flags.off => {
                    partial.push("color");
                    7
                }
                Some(value) => value,
                None => 7,
            };
            let number = if layer.flags.off {
                number.saturating_neg()
            } else {
                number
            };
            let data = m::Layer {
                color: number,
                true_color: color.true_color,
                linetype: text(&layer.line_type, self.limits)?,
                plot: Some(layer.is_plottable),
                lineweight: Some(layer.line_weight.value()),
                ..m::Layer::default()
            };
            layers.push(self.table_entry(
                "LAYER",
                &layer.name,
                layer.handle,
                data,
                flags,
                &partial,
            )?);
        }
        Ok(layers)
    }

    fn linetypes(&mut self, doc: &CadDocument) -> Result<Vec<TableEntry<m::Linetype>>> {
        let mut linetypes = Vec::new();
        for linetype in doc.line_types.iter() {
            array(linetype.elements.len(), self.limits)?;
            if !linetype.pattern_length.is_finite()
                || !linetype.elements.iter().all(|e| e.length.is_finite())
            {
                self.skipped("LTYPE", &linetype.name);
                continue;
            }
            let mut partial = Vec::new();
            let mut elements = Vec::with_capacity(linetype.elements.len());
            for element in &linetype.elements {
                if element.complex.is_some() {
                    partial.push("complex_elements");
                }
                elements.push(m::LinetypeElement {
                    length: element.length,
                    kind: 0,
                    ..m::LinetypeElement::default()
                });
            }
            let alignment = match u8::try_from(linetype.alignment) {
                Ok(code) => i16::from(code),
                Err(_) => {
                    partial.push("alignment");
                    i16::from(b'A')
                }
            };
            let data = m::Linetype {
                description: text(&linetype.description, self.limits)?,
                alignment,
                pattern_length: linetype.pattern_length,
                elements,
            };
            linetypes.push(self.table_entry(
                "LTYPE",
                &linetype.name,
                linetype.handle,
                data,
                0,
                &partial,
            )?);
        }
        Ok(linetypes)
    }

    fn text_styles(&mut self, doc: &CadDocument) -> Result<Vec<TableEntry<m::TextStyle>>> {
        let mut styles = Vec::new();
        for style in doc.text_styles.iter() {
            let numbers = [
                style.height,
                style.width_factor,
                style.oblique_angle,
                style.last_height,
            ];
            if !numbers.iter().all(|v| v.is_finite()) {
                self.skipped("STYLE", &style.name);
                continue;
            }
            let mut partial = Vec::new();
            for (flag, name) in [
                (!style.true_type_font.is_empty(), "true_type_font"),
                (style.is_shape_file, "shape_file"),
                (style.is_vertical, "vertical"),
                (style.annotative, "annotative"),
                (style.xref_dependent, "xref"),
            ] {
                if flag {
                    partial.push(name);
                }
            }
            let generation =
                i16::from(style.flags.backward) * 2 + i16::from(style.flags.upside_down) * 4;
            let data = m::TextStyle {
                height: style.height,
                width_factor: style.width_factor,
                // The library stores the oblique angle in radians; the model keeps degrees.
                oblique: style.oblique_angle.to_degrees(),
                generation,
                last_height: style.last_height,
                font: text(&style.font_file, self.limits)?,
                big_font: (!style.big_font_file.is_empty())
                    .then(|| text(&style.big_font_file, self.limits))
                    .transpose()?,
            };
            styles.push(self.table_entry("STYLE", &style.name, style.handle, data, 0, &partial)?);
        }
        Ok(styles)
    }

    fn block_record_table(&mut self, doc: &CadDocument) -> Result<Vec<TableEntry<m::BlockRecord>>> {
        let mut block_records = Vec::new();
        for record in doc.block_records.iter() {
            let mut partial = Vec::new();
            for (flag, name) in [
                (!record.xref_path.is_empty(), "xref_path"),
                (!record.description.is_empty(), "description"),
                (!record.insert_handles.is_empty(), "insert_handles"),
                (record.flags.is_xref_unloaded, "xref_load_state"),
            ] {
                if flag {
                    partial.push(name);
                }
            }
            array(record.preview_data.len(), self.limits)?;
            let data = m::BlockRecord {
                layout: convert::handle(record.layout),
                explodable: Some(i16::from(record.explodable)),
                scalable: Some(i16::from(record.scale_uniformly)),
                preview: record.preview_data.clone(),
            };
            // The insertion units travel in the flags, as the DXF reader keeps group 70 there.
            block_records.push(self.table_entry(
                "BLOCK_RECORD",
                &record.name,
                record.handle,
                data,
                record.units,
                &partial,
            )?);
        }
        Ok(block_records)
    }

    /// Maps one entity of the drawing or of a block definition, appending it to `out`.
    fn entity(
        &mut self,
        entity: &EntityType,
        paper_space: bool,
        out: &mut Vec<Entity>,
    ) -> Result<()> {
        self.record()?;
        let kind = entity::type_name(entity);
        let mut budget = RecordBudget::new(&mut self.records, self.limits.record_count);
        match entity::map(entity, paper_space, self.limits, &mut budget)? {
            Outcome::Mapped { entity, partial } => {
                self.loss.add_mapped(1);
                self.loss
                    .add_partial(reasons::ENTITY_PARTIAL, kind, &partial);
                out.push(*entity);
            }
            Outcome::Unsupported => self.loss.add(reasons::ENTITY_DROPPED, kind, None, 1),
            Outcome::Invalid => self.loss.add(reasons::VALUE_SKIPPED, kind, None, 1),
        }
        Ok(())
    }

    /// The block definitions and the top-level entities of the drawing.
    fn blocks_and_entities(&mut self, doc: &CadDocument) -> Result<(Vec<Block>, Vec<Entity>)> {
        let mut owned = HashSet::new();
        for record in doc.block_records.iter() {
            owned.extend(record.entity_handles.iter().map(|h| h.value()));
        }
        let mut blocks = Vec::new();
        let mut top = Vec::new();
        let mut unresolved = 0_u64;
        for record in doc.block_records.iter() {
            let resolved = doc.entities_in_block(&record.name).count();
            unresolved = unresolved.saturating_add(
                u64::try_from(record.entity_handles.len().saturating_sub(resolved))
                    .unwrap_or(u64::MAX),
            );
            // A layout is a record with a layout handle, or one of the model and paper space
            // records by name; the paper-space flag follows the name.
            let layout = record.is_layout() || record.is_model_space() || record.is_paper_space();
            if layout {
                let paper_space = !record.is_model_space();
                for entity in doc.entities_in_block(&record.name) {
                    self.entity(entity, paper_space, &mut top)?;
                }
                continue;
            }
            let mut members = Vec::new();
            for entity in doc.entities_in_block(&record.name) {
                self.entity(entity, false, &mut members)?;
            }
            let base = if finite3(record.base_point) {
                p3(record.base_point)
            } else {
                self.skipped("BLOCK", &record.name);
                [0.0; 3]
            };
            let flags = {
                let block = record.flags;
                [
                    (block.anonymous, 1_i16),
                    (block.has_attributes, 2),
                    (block.is_xref, 4),
                    (block.is_xref_overlay, 8),
                    (block.is_external, 16),
                ]
                .into_iter()
                .filter(|(set, _)| *set)
                .fold(0_i16, |bits, (_, bit)| bits | bit)
            };
            let begin_kind = EntityKind::Block(m::BlockBegin {
                name: text(&record.name, self.limits)?,
                flags,
                base,
                alt_name: None,
                xref_path: (!record.xref_path.is_empty())
                    .then(|| text(&record.xref_path, self.limits))
                    .transpose()?,
                description: (!record.description.is_empty())
                    .then(|| text(&record.description, self.limits))
                    .transpose()?,
            });
            let mut begin = Entity::new(begin_kind);
            begin.head = convert::head(record.block_entity_handle, acadrust::types::Handle::new(0));
            let mut end = Entity::new(EntityKind::BlockEnd(m::SeqEnd::default()));
            end.head = convert::head(record.block_end_handle, acadrust::types::Handle::new(0));
            // The BLOCK and ENDBLK records are kept records, as they are in the DXF reader.
            self.loss.add_mapped(2);
            blocks.push(Block {
                begin,
                entities: members,
                end,
            });
        }
        if unresolved > 0 {
            self.loss
                .add(reasons::HANDLE_UNRESOLVED, "BLOCK_RECORD", None, unresolved);
        }
        for entity in doc.entities() {
            if !owned.contains(&entity.common().handle.value()) {
                self.loss.add_with(
                    reasons::ENTITY_DROPPED,
                    entity::type_name(entity),
                    None,
                    1,
                    vec![("reason".to_owned(), Argument::Text("unowned".to_owned()))],
                );
            }
        }
        Ok((blocks, top))
    }

    /// Maps the whole document.
    pub(crate) fn drawing(&mut self, doc: &CadDocument, version: Version) -> Result<Drawing> {
        let header = self.header(doc)?;
        let tables = self.tables(doc)?;
        let (blocks, entities) = self.blocks_and_entities(doc)?;
        let objects = u64::try_from(doc.objects.len()).unwrap_or(u64::MAX);
        self.loss.add(reasons::OBJECT_DROPPED, "", None, objects);
        let classes = u64::try_from(doc.classes.len()).unwrap_or(u64::MAX);
        self.loss
            .add(reasons::OBJECT_DROPPED, "CLASS", None, classes);
        let notifications = u64::try_from(doc.notifications.len()).unwrap_or(u64::MAX);
        self.loss
            .add(reasons::OBJECT_DROPPED, "NOTIFICATION", None, notifications);
        if let Some(report) = self.progress.as_mut() {
            report(Progress {
                done: self.records,
                total: self.total,
            });
        }
        Ok(Drawing {
            version,
            header,
            tables,
            blocks,
            entities,
            ..Drawing::default()
        })
    }

    /// The loss report of the mapping.
    pub(crate) fn finish(self, header: LossHeader) -> LossReport {
        self.loss.finish(header)
    }
}

/// A boolean header value as the group integer of the file (0 or 1).
fn flag(value: bool) -> i64 {
    i64::from(u8::from(value))
}

/// A table of entries.
fn table<T>(entries: Vec<TableEntry<T>>) -> Table<T> {
    Table {
        head: m::Head::default(),
        extras: Vec::new(),
        xdata: Vec::new(),
        entries,
    }
}

#[cfg(test)]
mod tests {
    use acadrust::CadDocument;
    use acadrust::entities::EntityType;
    use acadrust::tables::{BlockRecord, Layer, LineType, LineTypeElement, TextStyle};
    use acadrust::types::{Color, DxfVersion, Handle, Vector2, Vector3};
    use arc_base::{Cancel, Limits};
    use arc_io_dxf::loss::Argument;

    use super::*;

    fn mapper_report(doc: &CadDocument) -> (Tables, LossReport) {
        let limits = Limits::CONSTRAINED;
        let cancel = Cancel::new();
        let mut mapper = Mapper::new(&limits, &cancel, None, 0);
        let tables = match mapper.tables(doc) {
            Ok(tables) => tables,
            Err(err) => panic!("the tables are mapped: {}", err.code()),
        };
        let header = LossHeader {
            format: "dwg".to_owned(),
            version: "R2018".to_owned(),
            direction: arc_io_dxf::loss::Direction::Import,
            file_name: None,
            file_hash: None,
            application: None,
            application_version: None,
            time: None,
            limits: None,
        };
        (tables, mapper.finish(header))
    }

    fn layer_color(tables: &Tables, name: &str) -> Option<i16> {
        tables
            .layers
            .as_ref()
            .and_then(|t| t.entries.iter().find(|e| e.name == name))
            .map(|e| e.data.color)
    }

    #[test]
    fn an_off_layer_with_a_by_block_color_stays_off() {
        let mut doc = CadDocument::with_version(DxfVersion::AC1032);
        let mut layer = Layer::new("OFF");
        layer.color = Color::ByBlock;
        layer.turn_off();
        doc.layers.add_or_replace(layer);
        let mut on = Layer::new("ON");
        on.color = Color::ByBlock;
        doc.layers.add_or_replace(on);
        let (tables, report) = mapper_report(&doc);
        assert_eq!(layer_color(&tables, "OFF"), Some(-7));
        assert_eq!(layer_color(&tables, "ON"), Some(0));
        let partial = report.items.iter().any(|i| {
            i.reason == "io.dwg.table_entry_partial" && i.record_kind.as_deref() == Some("LAYER")
        });
        assert!(partial, "the stand-in color of the off layer is reported");
    }

    #[test]
    fn a_non_finite_linetype_length_skips_the_linetype_and_reports_it() {
        let mut doc = CadDocument::with_version(DxfVersion::AC1032);
        let mut linetype = LineType::new("BROKEN");
        linetype.elements.push(LineTypeElement::dash(f64::NAN));
        doc.line_types.add_or_replace(linetype);
        let (tables, report) = mapper_report(&doc);
        let names: Vec<&str> = tables
            .linetypes
            .as_ref()
            .map(|t| t.entries.iter().map(|e| e.name.as_str()).collect())
            .unwrap_or_default();
        assert!(!names.contains(&"BROKEN"));
        assert!(
            report
                .items
                .iter()
                .any(|i| i.reason == "io.dwg.value_skipped"
                    && i.arguments.get("name") == Some(&Argument::Text("BROKEN".to_owned())))
        );
    }

    #[test]
    fn a_non_finite_text_style_number_skips_the_style() {
        let mut doc = CadDocument::with_version(DxfVersion::AC1032);
        let mut style = TextStyle::new("BROKEN");
        style.width_factor = f64::INFINITY;
        doc.text_styles.add_or_replace(style);
        let (tables, _) = mapper_report(&doc);
        let styles = tables.styles.as_ref().map(|t| t.entries.len()).unwrap_or(0);
        let broken = tables
            .styles
            .as_ref()
            .is_some_and(|t| t.entries.iter().any(|e| e.name == "BROKEN"));
        assert!(!broken, "{styles} styles kept");
    }

    #[test]
    fn a_text_style_oblique_angle_is_converted_to_degrees() {
        let mut doc = CadDocument::with_version(DxfVersion::AC1032);
        let mut style = TextStyle::new("SLANT");
        style.oblique_angle = std::f64::consts::FRAC_PI_6;
        doc.text_styles.add_or_replace(style);
        let (tables, _) = mapper_report(&doc);
        let oblique = tables
            .styles
            .as_ref()
            .and_then(|t| t.entries.iter().find(|e| e.name == "SLANT"))
            .map(|e| e.data.oblique);
        assert!(
            oblique.is_some_and(|v| (v - 30.0).abs() < 1e-9),
            "{oblique:?}"
        );
    }

    #[test]
    fn block_flags_and_unresolved_members_are_reported() {
        let mut doc = CadDocument::with_version(DxfVersion::AC1032);
        let mut record = BlockRecord::new("ANON");
        record.flags.anonymous = true;
        record.flags.is_xref = true;
        record.entity_handles.push(Handle::new(0xDEAD));
        doc.block_records.add_or_replace(record);
        let limits = Limits::CONSTRAINED;
        let cancel = Cancel::new();
        let mut mapper = Mapper::new(&limits, &cancel, None, 0);
        let drawing = match mapper.drawing(&doc, Version::R2018) {
            Ok(drawing) => drawing,
            Err(err) => panic!("the drawing is mapped: {}", err.code()),
        };
        let block = drawing
            .blocks
            .iter()
            .find(|b| matches!(&b.begin.kind, EntityKind::Block(begin) if begin.name == "ANON"));
        let flags = block.and_then(|b| match &b.begin.kind {
            EntityKind::Block(begin) => Some(begin.flags),
            _ => None,
        });
        assert_eq!(flags, Some(1 | 4));
        let report = mapper.finish(LossHeader {
            format: "dwg".to_owned(),
            version: "R2018".to_owned(),
            direction: arc_io_dxf::loss::Direction::Import,
            file_name: None,
            file_hash: None,
            application: None,
            application_version: None,
            time: None,
            limits: None,
        });
        let unresolved = report
            .items
            .iter()
            .find(|i| i.reason == "io.dwg.handle_unresolved")
            .and_then(|i| i.arguments.get("count").cloned());
        assert_eq!(unresolved, Some(Argument::Int(1)));
    }

    #[test]
    fn a_block_base_point_that_is_not_finite_becomes_the_origin() {
        let mut doc = CadDocument::with_version(DxfVersion::AC1032);
        let mut record = BlockRecord::new("DOOR");
        record.base_point = acadrust::types::Vector3::new(f64::NAN, 1.0, 1.0);
        doc.block_records.add_or_replace(record);
        let limits = Limits::CONSTRAINED;
        let cancel = Cancel::new();
        let mut mapper = Mapper::new(&limits, &cancel, None, 0);
        let drawing = match mapper.drawing(&doc, Version::R2018) {
            Ok(drawing) => drawing,
            Err(err) => panic!("the drawing is mapped: {}", err.code()),
        };
        let base = drawing.blocks.iter().find_map(|b| match &b.begin.kind {
            EntityKind::Block(begin) if begin.name == "DOOR" => Some(begin.base),
            _ => None,
        });
        assert_eq!(base, Some([0.0, 0.0, 0.0]));
        let report = mapper.finish(LossHeader {
            format: "dwg".to_owned(),
            version: "R2018".to_owned(),
            direction: arc_io_dxf::loss::Direction::Import,
            file_name: None,
            file_hash: None,
            application: None,
            application_version: None,
            time: None,
            limits: None,
        });
        assert!(
            report
                .items
                .iter()
                .any(|i| i.reason == "io.dwg.value_skipped"
                    && i.arguments.get("name") == Some(&Argument::Text("DOOR".to_owned())))
        );
    }

    #[test]
    fn a_model_space_without_a_layout_handle_holds_the_top_level_entities() {
        // The library rebuilds layout links when it reads a file, so this case is made from a
        // constructed document: a model space record with no layout handle, as the oldest versions
        // can have, and one line in it.
        let mut doc = CadDocument::with_version(DxfVersion::AC1012);
        let line = acadrust::entities::Line::from_points(
            acadrust::types::Vector3::new(0.0, 0.0, 0.0),
            acadrust::types::Vector3::new(1.0, 0.0, 0.0),
        );
        // `add_entity` routes the line into the model space record.
        if let Err(err) = doc.add_entity(EntityType::Line(line)) {
            panic!("the line is added: {err:?}");
        }
        if let Some(record) = doc.block_records.get_mut("*Model_Space") {
            record.layout = Handle::NULL;
        }
        let limits = Limits::CONSTRAINED;
        let cancel = Cancel::new();
        let mut mapper = Mapper::new(&limits, &cancel, None, 0);
        let drawing = match mapper.drawing(&doc, Version::R13) {
            Ok(drawing) => drawing,
            Err(err) => panic!("the drawing is mapped: {}", err.code()),
        };
        let top: Vec<&str> = drawing
            .entities
            .iter()
            .map(|e| e.kind.type_name())
            .collect();
        assert_eq!(top, vec!["LINE"]);
        assert!(drawing.blocks.is_empty());
    }

    /// The header variable `name` of a drawing mapped from `doc`, with its value groups.
    fn header_value(doc: &CadDocument, name: &str) -> Option<Vec<Group>> {
        let limits = Limits::CONSTRAINED;
        let cancel = Cancel::new();
        let mut mapper = Mapper::new(&limits, &cancel, None, 0);
        let drawing = match mapper.drawing(doc, Version::R2018) {
            Ok(drawing) => drawing,
            Err(err) => panic!("the drawing is mapped: {}", err.code()),
        };
        drawing.header_variable(name).map(<[Group]>::to_vec)
    }

    #[test]
    fn the_lineweight_display_flag_is_a_boolean_value() {
        let mut doc = CadDocument::with_version(DxfVersion::AC1032);
        doc.header.lineweight_display = true;
        let value = header_value(&doc, "$LWDISPLAY").unwrap_or_default();
        assert_eq!(value.len(), 1);
        assert_eq!(
            value.first().map(|g| (g.code, g.value.clone())),
            Some((290, arc_io_dxf::Value::Bool(true)))
        );
    }

    #[test]
    fn the_angle_base_is_mapped_in_degrees() {
        let mut doc = CadDocument::with_version(DxfVersion::AC1032);
        doc.header.angle_base = std::f64::consts::FRAC_PI_2;
        let value = header_value(&doc, "$ANGBASE").unwrap_or_default();
        assert_eq!(value.len(), 1);
        assert_eq!(
            value.first().map(|g| g.value.clone()),
            Some(arc_io_dxf::Value::Double(90.0))
        );
    }

    #[test]
    fn identity_and_settings_of_the_header_are_mapped_under_their_dxf_names() {
        let mut doc = CadDocument::with_version(DxfVersion::AC1032);
        doc.header.fingerprint_guid = "{FP-1}".to_owned();
        doc.header.version_guid = "{VER-2}".to_owned();
        doc.header.handle_seed = 0x2A;
        doc.header.insertion_units = 4;
        doc.header.current_layer_name = "WALLS".to_owned();
        doc.header.current_text_style_name = "Standard".to_owned();
        doc.header.current_linetype_name = "DASHED".to_owned();
        doc.header.current_dimstyle_name = "Metric".to_owned();
        doc.header.code_page = "ANSI_1252".to_owned();
        doc.header.model_space_extents_max = Vector3::new(10.0, 20.0, 0.0);
        doc.header.model_space_limits_min = Vector2::new(-1.0, -2.0);
        doc.header.text_height = 2.5;
        doc.header.limit_check = true;

        let text = |v: &[Group]| v.first().and_then(Group::as_text).map(str::to_owned);
        assert_eq!(
            header_value(&doc, "$FINGERPRINTGUID")
                .as_deref()
                .and_then(text),
            Some("{FP-1}".to_owned())
        );
        assert_eq!(
            header_value(&doc, "$VERSIONGUID").as_deref().and_then(text),
            Some("{VER-2}".to_owned())
        );
        // The handle seed is hexadecimal text, as the DXF reader and writer keep it.
        assert_eq!(
            header_value(&doc, "$HANDSEED").as_deref().and_then(text),
            Some("2A".to_owned())
        );
        assert_eq!(
            header_value(&doc, "$DWGCODEPAGE").as_deref().and_then(text),
            Some("ANSI_1252".to_owned())
        );
        assert_eq!(
            header_value(&doc, "$CLAYER").as_deref().and_then(text),
            Some("WALLS".to_owned())
        );
        assert_eq!(
            header_value(&doc, "$CELTYPE").as_deref().and_then(text),
            Some("DASHED".to_owned())
        );
        assert_eq!(
            header_value(&doc, "$INSUNITS")
                .as_deref()
                .map(|v| v[0].value.clone()),
            Some(arc_io_dxf::Value::Int(4))
        );
        assert_eq!(
            header_value(&doc, "$TEXTSIZE")
                .as_deref()
                .map(|v| v[0].value.clone()),
            Some(arc_io_dxf::Value::Double(2.5))
        );
        assert_eq!(
            header_value(&doc, "$LIMCHECK")
                .as_deref()
                .map(|v| v[0].value.clone()),
            Some(arc_io_dxf::Value::Int(1))
        );
        let extmax = header_value(&doc, "$EXTMAX").unwrap_or_default();
        assert_eq!(
            extmax.iter().map(|g| g.code).collect::<Vec<_>>(),
            vec![10, 20, 30]
        );
        let limmin = header_value(&doc, "$LIMMIN").unwrap_or_default();
        assert_eq!(
            limmin.iter().map(|g| g.code).collect::<Vec<_>>(),
            vec![10, 20]
        );
    }

    #[test]
    fn a_header_number_that_is_not_finite_is_skipped_and_reported() {
        let mut doc = CadDocument::with_version(DxfVersion::AC1032);
        doc.header.linetype_scale = f64::NAN;
        assert!(header_value(&doc, "$LTSCALE").is_none());
        let limits = Limits::CONSTRAINED;
        let cancel = Cancel::new();
        let mut mapper = Mapper::new(&limits, &cancel, None, 0);
        if let Err(err) = mapper.drawing(&doc, Version::R2018) {
            panic!("the drawing is mapped: {}", err.code());
        }
        let report = mapper.finish(LossHeader {
            format: "dwg".to_owned(),
            version: "R2018".to_owned(),
            direction: arc_io_dxf::loss::Direction::Import,
            file_name: None,
            file_hash: None,
            application: None,
            application_version: None,
            time: None,
            limits: None,
        });
        assert!(
            report
                .items
                .iter()
                .any(|i| i.reason == "io.dwg.value_skipped"
                    && i.record_kind.as_deref() == Some("HEADER")),
            "the skipped header number is reported"
        );
    }
}
