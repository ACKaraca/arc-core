//! History queries (CORE-DOC-025): the journal records that match an object, a type, an author
//! and a time range.
//!
//! A [`HistoryQuery`] is a conjunction of the filters that are set. It runs over any sequence of
//! records in commit order: the journal of a session ([`crate::Journal::query`]) or, with the
//! records a package stored, [`crate::Document::history`].

use arc_base::ObjectId;

use crate::record::TxnRecord;
use crate::schema::TypeKey;

/// The filters of a history query; every filter that is set must match. The default matches
/// every record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryQuery {
    /// Only records with an operation on this object.
    pub object: Option<ObjectId>,
    /// Only records with an operation on an object of this type.
    pub type_key: Option<TypeKey>,
    /// Only records of this author.
    pub author: Option<String>,
    /// Only records committed at or after this time, in milliseconds since the Unix epoch.
    pub from_millis: Option<u64>,
    /// Only records committed before this time, in milliseconds since the Unix epoch.
    pub until_millis: Option<u64>,
}

impl HistoryQuery {
    /// The query for the records that touched `object`: [`HistoryQuery::object`] only.
    #[must_use]
    pub fn touching(object: ObjectId) -> Self {
        Self {
            object: Some(object),
            ..Self::default()
        }
    }

    /// True when `record` matches every filter that is set. An operation counts as on an object
    /// when it creates, modifies, relinks or deletes it, including the operations the deletion
    /// rules added; foreign data operations change no object and match neither filter.
    #[must_use]
    pub fn matches(&self, record: &TxnRecord) -> bool {
        if self.author.as_ref().is_some_and(|a| *a != record.author)
            || self
                .from_millis
                .is_some_and(|t| record.time_unix_millis < t)
            || self
                .until_millis
                .is_some_and(|t| record.time_unix_millis >= t)
        {
            return false;
        }
        if self.object.is_none() && self.type_key.is_none() {
            return true;
        }
        // One type key per operation, except foreign data operations, which have none; the
        // object filter reads the operation itself.
        let mut keys = record.type_keys().iter();
        record.ops.iter().any(|op| {
            let key = op.object_id().and_then(|_| keys.next());
            op.object_id().is_some()
                && self.object.is_none_or(|o| op.object_id() == Some(o))
                && self.type_key.as_ref().is_none_or(|t| key == Some(t))
        })
    }

    /// The records of `records` that match, in their order.
    pub fn filter<'a>(
        &'a self,
        records: impl IntoIterator<Item = &'a TxnRecord> + 'a,
    ) -> impl Iterator<Item = &'a TxnRecord> + 'a {
        records
            .into_iter()
            .filter(move |record| self.matches(record))
    }
}
