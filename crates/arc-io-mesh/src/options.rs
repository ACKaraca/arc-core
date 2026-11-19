//! Options of the readers and writers, and the progress and cancellation control they share.

use arc_base::{Cancel, Limits, Result};

/// How far a read or write has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Bytes read, or items written.
    pub done: u64,
    /// Total bytes, or total items.
    pub total: u64,
}

/// Options of the readers.
pub struct ReadOptions<'a> {
    /// The limits profile; [`Limits::CONSTRAINED`] by default.
    pub limits: Limits,
    /// Cancelling it makes the read return `base.cancelled` before the next record.
    pub cancel: Cancel,
    /// Called on the first record, every 1024 records and at the end.
    pub progress: Option<&'a mut dyn FnMut(Progress)>,
}

impl Default for ReadOptions<'_> {
    fn default() -> Self {
        Self {
            limits: Limits::CONSTRAINED,
            cancel: Cancel::new(),
            progress: None,
        }
    }
}

impl ReadOptions<'_> {
    /// Options with the given limits profile.
    #[must_use]
    pub fn with_limits(limits: Limits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    /// The progress and cancellation control of one read.
    pub(crate) fn control(&mut self) -> Control<'_> {
        Control::new(
            &self.cancel,
            self.progress
                .as_deref_mut()
                .map(|p| p as &mut dyn FnMut(Progress)),
        )
    }
}

/// The encoding of a written file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    /// Text. Numbers are written in their shortest form that reads back to the same value.
    #[default]
    Ascii,
    /// Binary, little endian.
    BinaryLittleEndian,
    /// Binary, big endian.
    BinaryBigEndian,
}

/// Options of the writers.
pub struct WriteOptions<'a> {
    /// The encoding; OBJ has only [`Encoding::Ascii`], STL has ASCII and little-endian binary, and
    /// glTF has JSON (`Ascii`) and GLB (`BinaryLittleEndian`).
    pub encoding: Encoding,
    /// The name written in the `mtllib` statement of an OBJ file that has materials. No statement
    /// is written when `None`; the caller stores the MTL bytes under the name it chooses.
    pub mtl_name: Option<String>,
    /// glTF only: the URI of the separate binary buffer. With `None` the buffer is embedded as a
    /// data URI in the JSON (`Encoding::Ascii`) or stored in the GLB binary chunk
    /// (`Encoding::BinaryLittleEndian`). With a name, the JSON refers to the buffer by it and the
    /// bytes come back in `WriteOutput::buffer`; this needs `Encoding::Ascii`.
    pub buffer_name: Option<String>,
    /// Cancelling it makes the write return `base.cancelled` before the next part.
    pub cancel: Cancel,
    /// Called at the start, every 1024 triangles and at the end.
    pub progress: Option<&'a mut dyn FnMut(Progress)>,
}

impl Default for WriteOptions<'_> {
    fn default() -> Self {
        Self {
            encoding: Encoding::Ascii,
            mtl_name: None,
            buffer_name: None,
            cancel: Cancel::new(),
            progress: None,
        }
    }
}

impl WriteOptions<'_> {
    /// Options with the given encoding.
    #[must_use]
    pub fn with_encoding(encoding: Encoding) -> Self {
        Self {
            encoding,
            ..Self::default()
        }
    }

    /// The progress and cancellation control of one write.
    pub(crate) fn control(&mut self) -> Control<'_> {
        Control::new(
            &self.cancel,
            self.progress
                .as_deref_mut()
                .map(|p| p as &mut dyn FnMut(Progress)),
        )
    }
}

/// The result of a read.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadOutput {
    /// The mesh scene.
    pub scene: crate::model::MeshScene,
    /// The loss report.
    pub report: crate::loss::LossReport,
}

/// The result of a write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteOutput {
    /// The file bytes: OBJ geometry, or the STL or PLY file.
    pub bytes: Vec<u8>,
    /// The MTL bytes of an OBJ scene that has materials; `None` for the other formats.
    pub mtl: Option<Vec<u8>>,
    /// The separate binary buffer of a glTF file written with `WriteOptions::buffer_name`.
    pub buffer: Option<Vec<u8>>,
    /// The loss report.
    pub report: crate::loss::LossReport,
}

/// Progress reporting and cancellation shared by the readers and writers.
pub(crate) struct Control<'a> {
    cancel: Cancel,
    progress: Option<&'a mut dyn FnMut(Progress)>,
    steps: u64,
}

impl<'a> Control<'a> {
    pub(crate) fn new(cancel: &Cancel, progress: Option<&'a mut dyn FnMut(Progress)>) -> Self {
        Self {
            cancel: cancel.clone(),
            progress,
            steps: 0,
        }
    }

    /// Checks the token and reports progress every 1024 steps.
    pub(crate) fn step(&mut self, done: u64, total: u64) -> Result<()> {
        self.cancel.check()?;
        self.steps = self.steps.saturating_add(1);
        if self.steps % 1024 == 1 {
            self.report(Progress { done, total });
        }
        Ok(())
    }

    /// Reports the end of the transfer.
    pub(crate) fn finish(&mut self, total: u64) {
        self.report(Progress { done: total, total });
    }

    fn report(&mut self, progress: Progress) {
        if let Some(callback) = self.progress.as_mut() {
            callback(progress);
        }
    }
}
