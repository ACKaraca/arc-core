# Messages of the errors owned by arc-io-common. Identifiers are registered in
# crates/arc-io-common/src/error.rs.

io-limit-exceeded = A limit of the profile was exceeded: { $limit }.
io-zip-truncated = The archive ends unexpectedly.
io-zip-end-missing = The archive has no valid end record.
io-zip-structure-invalid = The archive structure is inconsistent.
io-zip-multi-disk = The archive spans several disks, which is not supported.
io-zip-encrypted = The archive contains an encrypted entry, which is not supported.
io-zip-method-unsupported = The archive uses a compression method that is not supported.
io-zip-data-descriptor-unverified = The data descriptor of an entry does not match its directory record.
io-zip-header-mismatch = A local header of the archive does not match its directory record.
io-zip-overlap = Two parts of the archive occupy the same bytes.
io-zip-name-invalid = The archive contains an entry name that is empty or malformed.
io-zip-path-unsafe = The archive contains an entry name that would leave the extraction folder.
io-zip-name-duplicate = The archive contains two entries with the same name.
io-zip-name-conflict = The archive contains a file whose name another entry needs as a folder.
io-zip-size-mismatch = The compressed data does not match the size the archive declares.
io-zip-inflate-failed = The compressed data of an entry is malformed.
io-zip-crc-mismatch = The data of an entry does not match its checksum.
io-zip-entry-missing = The archive has no entry with this index.
io-xml-truncated = The XML document ends unexpectedly.
io-xml-syntax = The XML document is not well formed.
io-xml-structure-invalid = The XML elements are not arranged as the document requires.
io-xml-text-invalid = The XML document contains text that is not valid UTF-8 or a character XML does not allow.
io-xml-doctype-refused = The XML document has a document type declaration, which is not supported.
io-xml-processing-instruction-refused = The XML document has a processing instruction, which is not supported.
io-xml-entity-refused = The XML document refers to an entity that is not predefined.
io-xml-reference-invalid = The XML document has a character reference that names no valid character.
io-xml-encoding-unsupported = The XML document is not encoded in UTF-8.
