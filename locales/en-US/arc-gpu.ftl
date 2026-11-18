# Messages of the errors owned by arc-gpu. Identifiers are registered in
# crates/arc-gpu/src/error.rs.

gpu-adapter-not-found = No graphics adapter matching the settings is available.
gpu-settings-invalid = A rendering setting is out of range.
gpu-device-request-failed = The graphics device could not be created.
gpu-device-lost = The graphics device was lost.
gpu-out-of-memory = The graphics device ran out of memory.
gpu-validation = The graphics device rejected an operation.
gpu-timeout = The graphics device did not respond in time.
gpu-readback-failed = The rendered image could not be read back.
gpu-limit-exceeded = The scene or image is larger than the limit allows.
gpu-value-not-finite = A value of the scene is not a finite number.
gpu-item-invalid = A render item is empty or has an invalid style.
gpu-geometry-invalid = The mesh has no triangles.
gpu-geometry-not-found = The geometry does not exist in the scene.
gpu-geometry-in-use = The geometry is still used by render items.
gpu-precision-insufficient = The view cannot be represented precisely enough.
gpu-pick-invalid = The pick region or tolerance is invalid.
gpu-navigation-invalid = A navigation value is out of range.
gpu-extractor-panicked = A render extractor failed unexpectedly.
gpu-thread-failed = A rendering thread could not run.
