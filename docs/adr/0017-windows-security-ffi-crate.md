# 0017. Windows file owner check through an FFI crate

- Status: Accepted
- Issue: [arc-core#107](https://github.com/ACKaraca/arc-core/issues/107)

## Context

The administrator settings file (CORE-APP-013) lives in `C:\ProgramData\arc-shell\` on Windows.
Standard users can create folders and files under `C:\ProgramData`, so any user can plant the file
before an administrator does: to lock settings for others, or to block the start of the application
for every user with invalid content. On Unix the application refuses a file that the superuser does
not own. The owner of a file on Windows is not exposed by the standard library, and reading it
needs calls into the security API, which are `unsafe`. The suite rules allow `unsafe` only in FFI
crates named in an architecture decision.

## Decision

- One FFI crate, `arc-winsec`, at layer 5 so that `arc-app` (layer 6) can depend on it. It exposes
  one safe function, `owner_is_administrative(path) -> io::Result<bool>`: true when the owner of the
  file or folder is the built-in Administrators group, Local System or the TrustedInstaller service.
- The crate depends on `windows-sys` (MIT or Apache-2.0), only on Windows, with the features
  `Win32_Foundation`, `Win32_Security` and `Win32_Security_Authorization`. On other targets the
  crate is empty and is not part of the WebAssembly set.
- It does not inherit the workspace lint table, because `unsafe_code = "forbid"` cannot be lowered
  for one crate while inheriting. Its manifest repeats every other workspace lint and adds
  `clippy::undocumented_unsafe_blocks`, so every `unsafe` block carries a `SAFETY:` comment. The
  system memory it receives is held by a guard type that frees it once.
- `arc-app` requires the owner check for the settings file and its folder on Windows. A file that
  fails it, or whose check fails, is ignored with a logged warning; the application starts without
  locks, and the reason is returned to the caller, which reports it. A file that passes and is
  invalid still stops the start, as on the other platforms.
- The check reads the owner only. The access control list is not inspected.
- A folder created by a standard user before deployment makes the file ignored, so an administrator
  must create the folder. A file owned by its creating account instead of the Administrators group
  (default-owner policy "object creator") is ignored. An administrative file that is momentarily
  unreadable is ignored, not fatal.

## Consequences

- One more `unsafe` surface, small enough to review in one read, and one more third-party crate,
  which the allowlist and the technology baseline record.
- A user can no longer plant the administrator file or block other users through it. An
  administrator who grants write access to the folder defeats the check; a deployment guide that
  requires non-default permissions needs the access control list inspected too.
- The Windows tests of the crate run only on a Windows runner.

## Alternatives considered

- **A wrapping dependency** (a crate that offers file ownership or security descriptors). Rejected:
  it moves the same `unsafe` into code that the workspace does not review, and adds more than one
  function needs.
- **Leaving Windows unchecked.** Rejected: any user could lock settings for others or block their
  start.

## Reopen when

The standard library exposes the owner of a file on Windows, or the access control list must be
inspected.
