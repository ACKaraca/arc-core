//! The transport of platforms without a local socket: none can be bound, so every call answers
//! `cmd.socket.unsupported`. The stream types are never built.

#![allow(dead_code, reason = "no transport is built on this platform")]

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use arc_base::Error;

use super::{Channel, SocketConfig};
use crate::error::codes;

fn unsupported() -> Error {
    Error::new(codes::SOCKET_UNSUPPORTED)
}

pub(super) struct Stream(());

pub(super) struct Listener(());

impl Listener {
    pub(super) fn bind(_: &SocketConfig, _: &Path) -> Result<Self, Error> {
        Err(unsupported())
    }

    pub(super) fn accept(&self) -> io::Result<Stream> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

impl Channel for Stream {
    fn duplicate(&self) -> io::Result<Self> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }

    fn end(&self) {}

    fn set_read_deadline(&self, _: Option<Instant>) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }

    fn read_part(&self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }

    fn write_part(&self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

pub(super) fn endpoint(_: &SocketConfig) -> Result<PathBuf, Error> {
    Err(unsupported())
}

pub(super) fn prepare_directory(_: &Path) -> Result<(), Error> {
    Err(unsupported())
}

pub(super) type Held = ();

pub(super) fn wake(_: &Path) -> Held {}

pub(super) fn remove_endpoint(_: &Path) {}

pub(super) fn create_token_file(path: &Path) -> io::Result<File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

pub(super) fn owner_matches(_: &Path, _: &fs::Metadata) -> Result<(), Error> {
    Ok(())
}

pub(super) fn random_bytes(_: &mut [u8]) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}
