# Messages of the errors owned by arc-doc. Identifiers are registered in
# crates/arc-doc/src/error.rs.

doc-type-duplicate-key = The object type { $key } is already registered.
doc-type-malformed-key = { $key } is not a valid key: use two or more lowercase names separated by dots.
doc-type-version-invalid = The object type { $type } has an invalid schema version.
doc-type-duplicate-field = The field { $field } is declared twice.
doc-type-field-invalid = The declaration of the field { $field } is not valid.
doc-type-relationship-inconsistent = The relationship rule of the field { $field } does not fit the field.
doc-type-default-invalid = The default value of the field { $field } does not fit the field.
doc-type-migration-chain-invalid = The object type { $type } needs exactly one migration from each earlier major version.
doc-value-missing = The field { $field } needs a value.
doc-value-type-mismatch = The value of the field { $field } has the wrong type.
doc-value-out-of-range = The value of the field { $field } is outside its valid range.
doc-value-not-finite = The value of the field { $field } is not a finite number.
doc-payload-invalid = The stored object data is damaged.
doc-payload-limit-exceeded = The stored object data exceeds a size limit.
doc-payload-encoding-unsupported = The stored object data uses an encoding this version cannot read.
doc-object-not-found = The object does not exist.
doc-reference-dangling = A reference points to an object that does not exist or has the wrong type.
