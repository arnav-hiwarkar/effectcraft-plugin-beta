//! Output files: the file system, or an in-memory [`Sink`] chosen per job (the web app, which
//! offers every written file as a download; tests).

use std::io::{BufWriter, Cursor, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use crate::{ExportError, Result, io};

/// Receives each finished output file (path as resolved, complete contents) instead of the file
/// system. Called once per file; image sequences call it from the render threads.
pub type Sink = dyn Fn(&str, Vec<u8>) + Send + Sync;

pub(crate) enum Out<'a> {
    File { w: Box<BufWriter<craft_atomic::Stage<'static>>>, path: String, fresh: bool, outputs: &'a Outputs },
    Mem { path: String, cur: Cursor<Vec<u8>>, sink: &'a Sink },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnconfirmedOutput {
    pub path: String,
    pub reason: craft_atomic::Reason,
}

#[derive(Default)]
pub(crate) struct Outputs {
    owned: Mutex<Vec<(PathBuf, craft_atomic::Identity)>>,
    unconfirmed: Mutex<Vec<UnconfirmedOutput>>,
}

impl Outputs {
    pub(crate) fn unconfirmed(&self) -> Vec<UnconfirmedOutput> {
        self.unconfirmed.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub(crate) fn remove_owned(&self) -> Result<()> {
        let mut failure = None;
        for (path, identity) in self.owned.lock().unwrap_or_else(PoisonError::into_inner).iter() {
            if craft_atomic::Identity::at(path).ok() != Some(*identity) {
                continue;
            }
            if let Err(e) = std::fs::remove_file(path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                failure.get_or_insert_with(|| ExportError::Io(format!("cannot remove cancelled output {}: {e}", path.display())));
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

pub(crate) fn create<'a>(sink: Option<&'a Sink>, outputs: &'a Outputs, path: &str) -> Result<Out<'a>> {
    match sink {
        Some(sink) => Ok(Out::Mem { path: path.to_string(), cur: Cursor::new(Vec::new()), sink }),
        None => {
            let file = Path::new(path);
            if let Some(dir) = file.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir).map_err(io)?;
            }
            let fresh = std::fs::symlink_metadata(file).is_err();
            let stage = craft_atomic::WriteOptions::default().stage(file).map_err(io)?;
            Ok(Out::File { w: Box::new(BufWriter::new(stage)), path: path.to_string(), fresh, outputs })
        }
    }
}

pub(crate) fn exists(sink: Option<&Sink>, path: &str) -> bool {
    sink.is_none() && Path::new(path).exists()
}

impl Out<'_> {
    pub(crate) fn finish(self) -> Result<u64> {
        match self {
            Out::File { w, path, fresh, outputs } => {
                let mut stage = (*w).into_inner().map_err(|e| ExportError::Io(e.to_string()))?;
                let bytes = stage.file().metadata().map(|m| m.len()).map_err(io)?;
                match stage.publish() {
                    craft_atomic::Receipt::Durable => {}
                    craft_atomic::Receipt::VisibleUnconfirmed(u) => {
                        outputs.unconfirmed.lock().unwrap_or_else(PoisonError::into_inner).push(UnconfirmedOutput { path: path.clone(), reason: u.reason });
                    }
                    craft_atomic::Receipt::Unchanged(error) | craft_atomic::Receipt::Displaced { error, .. } => return Err(io(error)),
                }
                if fresh && let Ok(identity) = craft_atomic::Identity::at(Path::new(&path)) {
                    outputs.owned.lock().unwrap_or_else(PoisonError::into_inner).push((PathBuf::from(path), identity));
                }
                Ok(bytes)
            }
            Out::Mem { path, cur, sink } => {
                let data = cur.into_inner();
                let n = data.len() as u64;
                sink(&path, data);
                Ok(n)
            }
        }
    }
}

impl Write for Out<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Out::File { w, .. } => w.write(buf),
            Out::Mem { cur, .. } => cur.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Out::File { w, .. } => w.flush(),
            Out::Mem { .. } => Ok(()),
        }
    }
}

impl Seek for Out<'_> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        match self {
            Out::File { w, .. } => w.seek(pos),
            Out::Mem { cur, .. } => cur.seek(pos),
        }
    }
}
