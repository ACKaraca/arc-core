# Journal record encoding

The byte form of one operation journal record (CORE-DOC-019), format version 1. The record
shape and the operations are defined in the [document model](document-model.md#operation-journal);
the decision for field-level, self-inverting records is
[ADR-0003](../adr/0003-journal-records-and-structural-sharing.md), whose amendment records the
layout decisions below. The API is `arc_doc::record::TxnRecord`
([arc-doc](../crates/arc-doc.md#journal-undo-and-redo-core-doc-019-to-core-doc-023)).

The package stores these bytes as the record of an `arc_journal` row, beside its `seq` and
`txn_id` ([document package](document-package.md#logical-schema)).

## Conventions

| Form | Bytes |
|---|---|
| `varint` | Unsigned LEB128, at most 10 bytes, in its shortest form |
| `id` | 16 bytes: the 128-bit identifier, most significant byte first (UUID byte order) |
| `flag` | One byte, 0 or 1 |
| `option<T>` | A `flag`, then `T` when it is 1 |
| `text` | `varint` byte length, then UTF-8 bytes |
| `blob` | `varint` byte length, then the bytes |
| `version` | Schema major and minor, each a `varint` |
| `flags` | One byte: bit 0 `hidden_from_ui`, bit 1 `system`, bit 2 `read_only`; other bits 0 |

Every value has exactly one encoding, so equal records give equal bytes and the decoder accepts
only the bytes the encoder writes.

## Record

| Field | Form | Content |
|---|---|---|
| format | `varint` | `1` |
| `txn_id` | `id` | |
| `parent` | `option<id>` | |
| `session` | `id` | |
| `author` | `text` | |
| `time` | `varint` | Milliseconds since the Unix epoch, UTC |
| `kind` | `varint` tag | 0 `normal`, 1 `undo_of` followed by an `id`, 2 `redo_of` followed by an `id`, 3 `merge`, 4 `migration`, 5 `link_reload` |
| `command` | `text` | Opaque canonical JSON |
| `label` | `text` | Opaque |
| `group` | `option<text>` | Opaque |
| `selection_after` | `option<text>` | Opaque |
| type keys | `varint` count, then one `text` each | The type keys of the objects the operations change, each once, in order of first use |
| `ops` | `varint` count, then the operations | In the order they were applied |

## Operations

Each operation is a `varint` tag, the `varint` index into the type-key table of the object it
changes (except tag 5, which changes no object), and the fields below.

| Tag | Operation | Fields |
|---|---|---|
| 0 | `create` | object, `copied_from` as `option<id>` |
| 1 | `modify` | `id`, `version`, path, before as `option<blob>`, after as `option<blob>` |
| 2 | `modify_properties` | `id`, properties before, properties after |
| 3 | `delete` | object |
| 4 | `relink` | `id`, owner before as `option<id>`, owner after as `option<id>` |
| 5 | `modify_foreign` | no type-key index; the key as a zigzag `varint`, the row before and the row after as `option<row>` |

- **Object** (create and delete): `id`, `version`, owner as `option<id>`, properties, and the
  field values as a `blob` holding the object's payload (`postcard`, the payload codec of
  `arc-doc`, uncompressed) at that schema version.
- **Properties**: name as `option<text>`, partition as `option<text>`, `flags`.
- **Row** (`modify_foreign`): `source_format` as `text`, `source_version` and `source_id` as `option<text>`, `object` as `option<blob>`, `record_kind` as `text`, `data` as `blob`, `asset` as `option<blob>`. The key is the operation's, not repeated in the row. A foreign data operation uses no entry of the type-key table.
- **Path**: a `varint` segment count of at least 1, then per segment a `varint` tag and its
  content: 0 a field name (`text`), 1 a list index (`varint`), 2 a map key (`text`). The first
  segment is a field.
- **Field-level values** of `modify` are the payload form of one value of the type the path
  addresses (`payload::encode_value`), at the schema version the operation names; an absent
  value (an absent field, or a map entry that does not exist) is an empty option.

The type key of every operation, not only of creates and deletes, is part of the record, so a
reader decodes field-level values with the type registry alone, without the document state the
record was made on.

## Decoding rules

Records are hostile input. A decoder:

- refuses input longer than `Limits::input_bytes`, texts longer than `Limits::string_bytes`,
  more type keys or operations than `Limits::record_count` and paths longer than one segment
  per nesting level of a registered type (17), with `doc.journal.record_limit_exceeded`;
- checks every count and length against the remaining input before it allocates, and reserves
  at most 1,024 elements ahead of decoding them;
- decodes payloads and values with the payload codec under the same `Limits`;
- refuses, with `doc.journal.record_invalid`, truncated input, bytes after the record, another
  format version, unknown tags, a flag other than 0 or 1, unknown flag bits, text that is not
  UTF-8, malformed type keys, a type-key index out of range, a type or schema version the
  registry does not hold at exactly that version, a path that does not resolve, values the
  codec refuses, and any non-canonical form (overlong varints, unused or repeated type keys);
  a failure of the payload codec is its source.

## Versioning

Tag 5 was added to format 1 without changing the layout of tags 0 to 4: a reader that does not
know a tag fails with `doc.journal.record_invalid` (unknown operation) for that record only,
and a record's header, which package retention and `check` read, decodes the same.

The format version changes when the layout changes. A newer format is refused with
`doc.journal.record_invalid`; a reader of a later format keeps reading format 1. Records of an
older minor schema version of a type are not decoded by this version; converting them belongs
to the reader that holds both versions.
