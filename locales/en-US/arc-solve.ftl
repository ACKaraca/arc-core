# Messages of the errors owned by arc-solve. Identifiers are registered in
# crates/arc-solve/src/error.rs.

solve-expr-syntax = The expression is not written correctly.
solve-expr-unknown-unit = The unit { $name } is not known.
solve-expr-offset-unit = The unit { $name } cannot be combined with other units or raised to a power.
solve-expr-unknown-function = The function { $name } is not known.
solve-expr-unknown-name = The name { $name } is not known.
solve-expr-dimension-mismatch = The values measure different things and cannot be combined here.
solve-expr-type-mismatch = A value of another kind is needed here.
solve-expr-argument-count = The function was given the wrong number of arguments.
solve-expr-domain = The value cannot be computed: an argument is out of range or the result is not a finite number.
solve-expr-limit-exceeded = The expression is too long, too deeply nested or takes too many steps.
solve-expr-separator-invalid = The decimal separator cannot be used in expressions.
solve-function-invalid = The function { $name } cannot be registered.
