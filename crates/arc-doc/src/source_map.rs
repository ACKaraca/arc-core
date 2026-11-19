//! The source map (CORE-DOC-052): the source identifiers of imported objects, and the lookup that
//! resolves a re-import to the objects it created before.
//!
//! An import gives each source entity a new [`ObjectId`] and records `(source format, source
//! document, source identifier)` for it. Importing the same source again looks each entity up
//! and updates the object it finds instead of creating a second one. The records are foreign
//! data rows (`arc_foreign`) of the record kind [`RECORD_KIND`]: `source_format` and `source_id`
//! hold the format and the identifier, `object` the object, and `data` the UTF-8 bytes of the
//! source document, so they are part of the document state, journaled, undone and saved like any
//! foreign data row, and a reader that does not know the kind preserves them.
//!
//! A [`SourceMap`] is built from one snapshot in time proportional to its foreign data rows and
//! answers lookups in logarithmic time; build it once per import. A record whose object is not
//! live in that snapshot is kept but does not resolve: the importer creates a new object and
//! [`SourceMap::bind`] points the record at it.

use std::collections::BTreeMap;

use arc_base::ObjectId;
use arc_base::Result;
use arc_base::id::Identifier;

use crate::load::StoredForeign;
use crate::snapshot::Snapshot;
use crate::txn::Txn;

/// The `record_kind` of the foreign data rows that hold source map records.
pub const RECORD_KIND: &str = "source_map";

type Key = (String, String, String);

#[derive(Debug, Clone, Copy)]
struct Entry {
    object: ObjectId,
    row: i64,
    live: bool,
}

/// The source map of one snapshot.
#[derive(Debug, Clone, Default)]
pub struct SourceMap {
    entries: BTreeMap<Key, Entry>,
}

impl SourceMap {
    /// The source map of `snapshot`. A row that is not a well-formed source map record is
    /// ignored; of several rows for one key, the one with the lowest row key counts.
    #[must_use]
    pub fn of(snapshot: &Snapshot) -> Self {
        let mut entries = BTreeMap::new();
        for row in snapshot
            .foreign_rows()
            .filter(|r| r.record_kind == RECORD_KIND)
        {
            let (Some(id), Some(bytes), Ok(document)) = (
                row.source_id.clone(),
                row.object
                    .as_deref()
                    .and_then(|b| <[u8; 16]>::try_from(b).ok()),
                String::from_utf8(row.data.clone()),
            ) else {
                continue;
            };
            let object = ObjectId::from_u128(u128::from_be_bytes(bytes));
            let live = snapshot.resolve(object).is_ok();
            entries
                .entry((row.source_format.clone(), document, id))
                .or_insert(Entry {
                    object,
                    row: row.id,
                    live,
                });
        }
        Self { entries }
    }

    /// The live object a source entity was imported as, if any.
    #[must_use]
    pub fn resolve(&self, format: &str, document: &str, source_id: &str) -> Option<ObjectId> {
        let key = (format.to_owned(), document.to_owned(), source_id.to_owned());
        self.entries.get(&key).filter(|e| e.live).map(|e| e.object)
    }

    /// The source identifiers recorded for `object`, as `(format, document, source id)`, in key
    /// order. Costs time proportional to the number of records.
    #[must_use]
    pub fn sources_of(&self, object: ObjectId) -> Vec<(&str, &str, &str)> {
        self.entries
            .iter()
            .filter(|(_, e)| e.object == object)
            .map(|((f, d, i), _)| (f.as_str(), d.as_str(), i.as_str()))
            .collect()
    }

    /// Records, in `txn`, that the source entity was imported as `object`, replacing an earlier
    /// record of the same entity, and updates this map. `object` should be live in `txn`.
    /// Fails like [`Txn::set_foreign`] (`doc.foreign.invalid` for an empty format, or a
    /// limit).
    pub fn bind(
        &mut self,
        txn: &mut Txn,
        format: &str,
        document: &str,
        source_id: &str,
        object: ObjectId,
    ) -> Result<()> {
        let key = (format.to_owned(), document.to_owned(), source_id.to_owned());
        let row = StoredForeign {
            id: 0,
            source_format: format.to_owned(),
            source_version: None,
            source_id: Some(source_id.to_owned()),
            object: Some(object.as_u128().to_be_bytes().to_vec()),
            record_kind: RECORD_KIND.to_owned(),
            data: document.as_bytes().to_vec(),
            asset: None,
        };
        let id = match self.entries.get(&key) {
            Some(entry) => {
                txn.set_foreign(StoredForeign {
                    id: entry.row,
                    ..row
                })?;
                entry.row
            }
            None => txn.add_foreign(row)?,
        };
        self.entries.insert(
            key,
            Entry {
                object,
                row: id,
                live: true,
            },
        );
        Ok(())
    }
}
