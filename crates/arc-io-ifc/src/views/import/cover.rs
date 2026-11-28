//! The coverage of the file by the views: for every entity class, how many instances were read
//! completely, how many in part and how many not at all.
//!
//! A read instance is one the views visited; a partly read one had a value or a target the views
//! could not use (it is in the loss report of the views). An instance the views did not visit is
//! kept in the file model: [`crate::IfcFile::write_step`] writes it back unchanged. Geometry items
//! are read by [`crate::IfcFile::geometry`], not by the views, and so count as not read here.

use std::collections::BTreeMap;

use arc_base::Result;

use super::super::Reader;
use super::ClassCoverage;

impl Reader<'_> {
    /// The coverage of every entity class, by class name; a complex instance is listed under its
    /// part names joined by `+`.
    pub(super) fn coverage(&mut self) -> Result<Vec<ClassCoverage>> {
        let file = self.file;
        let mut classes: BTreeMap<String, ClassCoverage> = BTreeMap::new();
        for instance in file.instances() {
            self.cancel.check()?;
            let key = instance
                .records
                .iter()
                .map(|r| r.type_name.as_str())
                .collect::<Vec<_>>()
                .join("+");
            if !classes.contains_key(&key) {
                self.fixed(96)?;
            }
            let class = classes.entry(key.clone()).or_insert_with(|| ClassCoverage {
                type_name: key,
                total: 0,
                interpreted: 0,
                partial: 0,
                preserved: 0,
            });
            class.total = class.total.saturating_add(1);
            let slot = match (
                self.imp.touched.contains(&instance.name),
                self.imp.flagged.contains(&instance.name),
            ) {
                (true, false) => &mut class.interpreted,
                (true, true) => &mut class.partial,
                (false, _) => &mut class.preserved,
            };
            *slot = slot.saturating_add(1);
        }
        Ok(classes.into_values().collect())
    }
}
