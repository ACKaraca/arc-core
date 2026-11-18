# Messages of the errors owned by arc-command. Identifiers are registered in
# crates/arc-command/src/error.rs.

cmd-unknown = No command is named { $name }.
cmd-spec-invalid = The command specification cannot be registered.
cmd-duplicate = The name { $name } is already registered.
cmd-alias-invalid = { $alias } is not a valid alias.
cmd-alias-unknown = There is no alias { $alias }.
cmd-alias-deprecated = { $alias } is a deprecated name of { $command }.
cmd-name-ambiguous = The name { $name } matches several commands.
cmd-params-malformed = The parameters are not valid JSON.
cmd-params-limit-exceeded = The parameters exceed a size or depth limit.
cmd-params-invalid = { $count } parameter values are not valid.
cmd-disabled = The command { $command } is not available now.
cmd-permission-denied = The command { $command } needs a permission this session does not have.
cmd-not-exposed = The command { $command } is not available here.
cmd-cancelled = The command was cancelled; nothing was changed.
cmd-fault = The command failed unexpectedly; the document was restored.
cmd-result-invalid = The command produced an invalid result; nothing was changed.
cmd-no-transaction = The command tried to change the document without a transaction.
cmd-prompt-kind = This prompt does not accept that input.
cmd-prompt-range = The value is outside the allowed range.
cmd-prompt-object = The object does not exist or cannot be used here.
cmd-prompt-unreadable = The input cannot be read here.
cmd-prompt-no-default = This prompt has no default.
cmd-prompt-invalid = The command asked for input it cannot accept.
cmd-prompt-limit-exceeded = The input exceeds a size or count limit.
cmd-prompt-ended = No command is waiting for input.
cmd-line-syntax = The command line cannot be read.
cmd-line-limit-exceeded = The command line is too long or too deeply nested.
cmd-line-unknown-param = The command has no parameter { $name }.
cmd-line-duplicate-param = The parameter { $name } is given twice.
cmd-line-too-many-values = The command line has more values than the command takes.
cmd-line-value = The value of { $name } is not valid.
cmd-repeat-none = There is no command to repeat.

# Prompt messages of machines derived from parameter schemas.
cmd-prompt-param = Enter { $name }.
cmd-keyword-yes = Yes
cmd-keyword-no = No
cmd-keyword-value = { $value }
