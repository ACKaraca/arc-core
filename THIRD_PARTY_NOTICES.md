# Third-party notices

## Noto Sans (Regular)

- Used by: `arc-draft`, the bundled default outline font (feature `outline-text`), compiled in from
  `crates/arc-draft/fonts/NotoSans-Regular.ttf`.
- Origin: the Noto project. Copyright 2022 The Noto Project Authors
  (https://github.com/notofonts/latin-greek-cyrillic).
- License: SIL Open Font License, Version 1.1. The license text is in
  `crates/arc-draft/fonts/NotoSans-OFL.txt`.
- SHA-256 of `NotoSans-Regular.ttf`: `478c558ea716033cd60c03438f628dfa75694dcf6b5f6d505a2f05fd2b4f3823`.
- Modified: no.

## IFC 4.3 ADD2 schema tables

- Used by: `arc-io-ifc`, the IFC schema tables, stored as `crates/arc-io-ifc/src/schema/generated/ifc4x3.rs`.
- Origin: © buildingSMART International Ltd., Industry Foundation Classes 4.3 ADD2
  (ISO 16739-1:2024). Source repository: https://github.com/buildingSMART/Standards-builds, commit
  `e135492835eaf3bbd42a5bc5bb85ab6c92c1fb6a`, path `IFC/RELEASE/IFC4_3/HTML/IFC4X3_ADD2.exp`.
- License: Creative Commons Attribution-NoDerivatives 4.0 International (SPDX `CC-BY-ND-4.0`,
  https://creativecommons.org/licenses/by-nd/4.0/). The statement of the publisher, from the
  `README.md` of the buildingSMART `IFC4.3.x-output` repository: "Final IFC is licensed under the
  **Creative Commons Attribution-NoDerivatives 4.0 International License (CC BY-ND 4.0)**." The
  `LICENSE` file of the same repository reads: "(c) buildingSMART International Ltd. This work is
  licensed under the Creative Commons Attribution-NoDerivatives 4.0 International License. To view a
  copy of this license, visit http://creativecommons.org/licenses/by-nd/4.0/".
- BLAKE3 of `IFC4X3_ADD2.exp`: `57a975a98ce9402c06d3702de93d32175a79de7bb923a7e8488286bce93fa6b0`.
  SHA-256 of `IFC4X3_ADD2.exp`: `f67c8762b13a099c28082061e6f16b9ef1284ceec34069792afc702725675860`.
- SHA-256 of `ifc4x3.rs`: `92ce353546937bacf32cf5c63f6c9a13d7596fe5939ccdaf5349467d2df5a0ec`.
- Modified: no. The file is a generated lookup table that changes only the format of the schema
  (section 2(a)(4) of the license); the schema text is not modified.
