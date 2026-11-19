# arc-winsec

The Windows security FFI crate ([ADR-0017](../adr/0017-windows-security-ffi-crate.md)). Layer 5,
class `ffi(5)`; the only `unsafe` code of the settings path. Empty on targets other than Windows.

## Owns

- `owner_is_administrative(path) -> io::Result<bool>`: whether the owner of a file or folder is the
  built-in Administrators group, Local System or the TrustedInstaller service.

## Stays in products

What to do with the answer. `arc-app` ignores an administrator settings file whose file or folder
is not administratively owned ([arc-app](arc-app.md), managed settings).

## Behavior

- The function reads the owner of the object the path resolves to. For a symbolic link or other
  reparse point that is the owner of the target, not of the link; a caller that must not accept a
  link checks that itself.
- A missing path, an unreadable descriptor and a path containing a NUL character are errors, never
  `false`. An object without an owner is not administrative.
- The access control list is not examined.

## Dependencies

`windows-sys` on Windows only, features `Win32_Foundation`, `Win32_Security` and
`Win32_Security_Authorization`.

## Test strategy

Tests are gated to Windows and run on the Windows runner: the system
folder is administrative, and a missing path is an error. The owner of a file the test creates is
not asserted, because it depends on elevation.
