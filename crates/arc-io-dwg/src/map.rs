//! Mapping of a library document to the neutral drawing model: the header variables the model
//! carries, the symbol tables, the block definitions, the entities and the record limits.

use std::collections::HashSet;

use acadrust::CadDocument;
use acadrust::entities::EntityType;
use arc_base::{Cancel, Limits, Result};
use arc_io_dxf::loss::{Argument, LossHeader, LossReport};
use arc_io_dxf::model::{
    self as m, Block, Drawing, Entity, EntityKind, HeaderVariable, Table, TableEntry, Tables,
};
use arc_io_dxf::{Group, Progress, Version};

use crate::convert::{self, p3};
use crate::entity::{self, Outcome};
use crate::error::{limit, reasons};
use crate::loss::LossLog;

/// How many records pass between two progress reports.
const PROGRESS_EVERY: u64 = 1024;

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
            loss: LossLog::default(),
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

    /// The header variables the model carries; the rest of the header is reported as dropped.
    fn header(&mut self, doc: &CadDocument) -> Result<Vec<HeaderVariable>> {
        let h = &doc.header;
        let layer = entity::text(&h.current_layer_name, self.limits)?;
        self.loss.add(reasons::HEADER_DROPPED, "", 1, Vec::new());
        Ok(vec![
            HeaderVariable {
                name: "$INSUNITS".to_owned(),
                values: vec![Group::int(70, i64::from(h.insertion_units))],
            },
            HeaderVariable {
                name: "$LTSCALE".to_owned(),
                values: vec![Group::double(40, h.linetype_scale)],
            },
            HeaderVariable {
                name: "$MEASUREMENT".to_owned(),
                values: vec![Group::int(70, i64::from(h.measurement))],
            },
            HeaderVariable {
                name: "$CLAYER".to_owned(),
                values: vec![Group::text(8, layer)],
            },
        ])
    }

    /// Adds the partial-data item of a table entry, when it has any.
    fn partial_entry(&mut self, kind: &str, partial: &[&'static str]) {
        if !partial.is_empty() {
            let names = partial.join(", ");
            self.loss.add(
                reasons::TABLE_ENTRY_PARTIAL,
                kind,
                1,
                vec![("properties".to_owned(), Argument::Text(names))],
            );
        }
    }

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
        self.partial_entry(kind, partial);
        Ok(TableEntry {
            head: convert::head(handle, acadrust::types::Handle::new(0)),
            name: entity::text(name, self.limits)?,
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
            self.loss
                .add(reasons::TABLE_DROPPED, kind, count, Vec::new());
        }
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
            let number = color.color.unwrap_or(7);
            let number = if layer.flags.off {
                number.saturating_neg()
            } else {
                number
            };
            let data = m::Layer {
                color: number,
                true_color: color.true_color,
                linetype: entity::text(&layer.line_type, self.limits)?,
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
                description: entity::text(&linetype.description, self.limits)?,
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
                oblique: style.oblique_angle,
                generation,
                last_height: style.last_height,
                font: entity::text(&style.font_file, self.limits)?,
                big_font: (!style.big_font_file.is_empty())
                    .then(|| entity::text(&style.big_font_file, self.limits))
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
                (record.units != 0, "units"),
                (!record.xref_path.is_empty(), "xref_path"),
                (!record.description.is_empty(), "description"),
                (!record.insert_handles.is_empty(), "insert_handles"),
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
            block_records.push(self.table_entry(
                "BLOCK_RECORD",
                &record.name,
                record.handle,
                data,
                0,
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
        match entity::map(entity, paper_space, self.limits)? {
            Outcome::Mapped { entity, partial } => {
                self.loss.mapped += 1;
                if !partial.is_empty() {
                    let names = partial.join(", ");
                    self.loss.add(
                        reasons::ENTITY_PARTIAL,
                        kind,
                        1,
                        vec![("properties".to_owned(), Argument::Text(names))],
                    );
                }
                out.push(*entity);
            }
            Outcome::Unsupported => self.loss.add(reasons::ENTITY_DROPPED, kind, 1, Vec::new()),
            Outcome::Invalid => self.loss.add(reasons::VALUE_SKIPPED, kind, 1, Vec::new()),
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
        for record in doc.block_records.iter() {
            if record.is_layout() {
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
            let begin_kind = EntityKind::Block(m::BlockBegin {
                name: entity::text(&record.name, self.limits)?,
                flags: 0,
                base: p3(record.base_point),
                alt_name: None,
                xref_path: (!record.xref_path.is_empty())
                    .then(|| entity::text(&record.xref_path, self.limits))
                    .transpose()?,
                description: (!record.description.is_empty())
                    .then(|| entity::text(&record.description, self.limits))
                    .transpose()?,
            });
            let mut begin = Entity::new(begin_kind);
            begin.head = convert::head(record.block_entity_handle, acadrust::types::Handle::new(0));
            let mut end = Entity::new(EntityKind::BlockEnd(m::SeqEnd::default()));
            end.head = convert::head(record.block_end_handle, acadrust::types::Handle::new(0));
            blocks.push(Block {
                begin,
                entities: members,
                end,
            });
        }
        for entity in doc.entities() {
            if !owned.contains(&entity.common().handle.value()) {
                self.loss.add(
                    reasons::ENTITY_DROPPED,
                    entity::type_name(entity),
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
        self.loss
            .add(reasons::OBJECT_DROPPED, "", objects, Vec::new());
        let classes = u64::try_from(doc.classes.len()).unwrap_or(u64::MAX);
        self.loss
            .add(reasons::OBJECT_DROPPED, "CLASS", classes, Vec::new());
        let notifications = u64::try_from(doc.notifications.len()).unwrap_or(u64::MAX);
        self.loss.add(
            reasons::OBJECT_DROPPED,
            "NOTIFICATION",
            notifications,
            Vec::new(),
        );
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

/// A table of entries.
fn table<T>(entries: Vec<TableEntry<T>>) -> Table<T> {
    Table {
        head: m::Head::default(),
        extras: Vec::new(),
        xdata: Vec::new(),
        entries,
    }
}

fn array(len: usize, limits: &Limits) -> Result<()> {
    let len = u64::try_from(len).unwrap_or(u64::MAX);
    if len > limits.array_length {
        return Err(limit("array_length", len));
    }
    Ok(())
}
