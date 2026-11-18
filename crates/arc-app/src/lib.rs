//! The window-free parts of the Arc application shell: product registration, the application
//! model (document tabs, open and save flows, read-only banners and the recovery prompt), typed
//! settings with scope resolution, settings files and the settings editor model, locale
//! selection and message formatting, structured logging and the recent-files model.
//!
//! This crate has no network dependency and makes no network request (CORE-APP-019). It does not
//! query the operating system: the operating-system locale list, the configuration directory and
//! the clock reach it as arguments. Windows, renderers and dialogs belong to later parts of the
//! shell. No function in this crate panics.

pub mod application;
pub mod error;
pub mod locale;
pub mod logging;
pub mod product;
pub mod recent;
pub mod settings;
pub mod settings_editor;
pub mod settings_file;

pub use error::codes;

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fresh empty directory under the system temporary directory.
    pub(crate) fn dir(name: &str) -> PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("arc-app-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}
