# Messages owned by arc-draft: its error codes and the labels and descriptions of its commands.
# Error identifiers are registered in crates/arc-draft/src/error.rs, command identifiers in
# crates/arc-draft/src/commands.rs.

draft-geometry-invalid = The stored geometry is not a valid curve of the kind this object holds.
draft-extrusion-zero = The extrusion direction has no length.
draft-space-layout-unitless = A layout must declare a length unit.
draft-linetype-pattern-empty = A linetype pattern needs lengths that are not negative and add up to more than zero.
draft-linetype-scale-invalid = A linetype scale must be a positive number.
draft-block-scale-zero = A block reference cannot be scaled by zero.
draft-block-cycle = The block definitions would contain each other.
draft-block-nesting-too-deep = Block references are nested more than { $limit } levels deep.
draft-block-attribute-unknown = The block definition has no attribute { $tag }.
draft-name-taken = The name { $name } is already in use.
draft-object-wrong-type = The object is not of a kind this operation accepts.
draft-field-invalid = The value does not fit the field { $field }.
draft-order-reference-moved = An object cannot be placed above or below itself.
draft-order-container-mismatch = The objects are not in the same space or block definition.

draft-command-space-create = New space
draft-command-space-create-description = Creates a model space or layout with a declared unit.
draft-command-space-modify = Change space
draft-command-space-modify-description = Sets a field of model spaces or layouts.
draft-command-entity-create = New drawing object
draft-command-entity-create-description = Creates a point or curve in a space or block definition.
draft-command-entity-modify = Change drawing objects
draft-command-entity-modify-description = Sets a field of drawing objects.
draft-command-entity-delete = Delete drawing objects
draft-command-entity-delete-description = Deletes drawing objects.
draft-command-layer-create = New layer
draft-command-layer-create-description = Creates a layer.
draft-command-layer-modify = Change layers
draft-command-layer-modify-description = Sets a field of layers.
draft-command-layer-delete = Delete layers
draft-command-layer-delete-description = Deletes layers that no object uses.
draft-command-linetype-load = Load linetype
draft-command-linetype-load-description = Adds a linetype definition from its pattern.
draft-command-linetype-modify = Change linetypes
draft-command-linetype-modify-description = Sets a field of linetype definitions.
draft-command-block-define = Define block
draft-command-block-define-description = Creates a block definition and moves the given objects into it.
draft-command-block-modify = Change block definitions
draft-command-block-modify-description = Sets a field of block definitions.
draft-command-block-insert = Insert block
draft-command-block-insert-description = Places a reference to a block definition.
draft-command-order-front = Bring to front
draft-command-order-front-description = Draws the objects after every other object of their space.
draft-command-order-back = Send to back
draft-command-order-back-description = Draws the objects before every other object of their space.
draft-command-order-above = Bring above object
draft-command-order-above-description = Draws the objects directly after another object.
draft-command-order-below = Send below object
draft-command-order-below-description = Draws the objects directly before another object.
