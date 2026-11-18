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
