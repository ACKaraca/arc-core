//! Decodes arbitrary bytes as a journal record against a registry of nested types with two
//! limits profiles; a decoded record must encode to exactly the input, and every failure must
//! carry one of the two record codes.

#![no_main]

use std::sync::{Arc, OnceLock};

use arc_base::Limits;
use arc_doc::{
    Cardinality, FieldDecl, OnCopy, OnDelete, ReferenceSpec, RelationshipRule, Schema,
    SchemaVersion, Strength, TxnRecord, TypeDecl, TypeKey, TypeRegistry, ValueType,
};
use libfuzzer_sys::fuzz_target;

/// Small enough for the fuzzer to reach every limit.
const TIGHT: Limits = Limits {
    input_bytes: 256,
    string_bytes: 8,
    array_length: 3,
    record_count: 4,
    nesting_depth: 2,
    ..Limits::STRICT
};

fn registry() -> &'static Arc<TypeRegistry> {
    static REGISTRY: OnceLock<Arc<TypeRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let reference = ValueType::Reference(ReferenceSpec {
            targets: Vec::new(),
            rule: RelationshipRule {
                strength: Strength::Associative,
                on_delete: OnDelete::Clear,
                on_copy: OnCopy::ShareTarget,
                cardinality: Cardinality::Optional,
            },
        });
        let nested = ValueType::Record(vec![
            FieldDecl::new("x", ValueType::F64).optional(),
            FieldDecl::new("l", ValueType::List(Box::new(ValueType::I64))),
        ]);
        let mut registry = TypeRegistry::new();
        for (key, fields) in [
            (
                "fuzz.a",
                vec![
                    FieldDecl::new("n", ValueType::I64),
                    FieldDecl::new("tags", ValueType::Map(Box::new(ValueType::String)))
                        .optional(),
                    FieldDecl::new("r", nested),
                ],
            ),
            (
                "fuzz.b",
                vec![
                    FieldDecl::new("p", ValueType::Point3),
                    FieldDecl::new("to", reference).optional(),
                ],
            ),
        ] {
            let schema = Schema::new(SchemaVersion::new(1, 0), fields);
            if let Ok(key) = TypeKey::new(key) {
                let _ = registry.register(TypeDecl::new(key, schema));
            }
        }
        Arc::new(registry)
    })
}

fuzz_target!(|data: &[u8]| {
    let registry = registry();
    for limits in [&Limits::STRICT, &TIGHT] {
        match TxnRecord::decode(data, registry, limits) {
            Ok(record) => assert_eq!(record.encode(registry, limits).ok().as_deref(), Some(data)),
            Err(error) => assert!(
                matches!(
                    error.code(),
                    "doc.journal.record_invalid" | "doc.journal.record_limit_exceeded"
                ),
                "{error:?}"
            ),
        }
    }
});
