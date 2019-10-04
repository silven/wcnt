//! Module responsible for searching through the file system looking for files of interest.
//!
//! Files of interest are either Limits.toml files, or files matching the glob patterns registered
//! for the different [Kind](../settings/struct.Kind.html)s or warnings.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::{bounded, Receiver, Sender};
use globset::GlobSet;
use ignore::{DirEntry, Error, WalkBuilder};

use crate::settings::Kind;

#[derive(Debug)]
/// LogFile declares a file on the file system that has been identified as relevant to be searched.
/// A LogFile may be searched multiple times to identify warnings related to multiple
/// [Kind](struct.Kind.html)s, each with its own Regex.
pub struct LogFile(PathBuf, Vec<Kind>);

impl LogFile {
    pub(crate) fn path(&self) -> &Path {
        &self.0.as_path()
    }

    pub(crate) fn kinds(&self) -> &[Kind] {
        &self.1
    }
}

#[derive(Debug)]
/// A partial result of the file search. Signal having found either a
/// [LimitsFile](struct.LimitsFile.html) or a relevant [log file](struct.LogFile.html).
pub(crate) enum FileData {
    LimitsFile(PathBuf),
    LogFile(LogFile),
}

/// Starts the threads which searches the `start_dir` for files. Uses `types` to know what
/// [Kind](struct.Kind.html)s of warnings we should look for in the files.
pub(crate) fn construct_file_searcher(
    start_dir: &Path,
    types: HashMap<Kind, GlobSet>,
) -> Receiver<FileData> {
    let (tx, rx) = bounded(100);
    let types = Arc::new(types);
    WalkBuilder::new(start_dir).build_parallel().run(|| {
        let tx = tx.clone();
        let my_types_copy = types.clone();
        Box::new(move |result| {
            if let Some(entry) = is_file(result) {
                process_file(&tx, entry, &my_types_copy);
            };
            ignore::WalkState::Continue
        })
    });
    rx
}

/// Is the `entry` a file? (See [DirEntry](../../ignore/struct.DirEntry.html))
fn is_file(entry: Result<DirEntry, Error>) -> Option<DirEntry> {
    if let Ok(dent) = entry {
        if dent.file_type()?.is_file() {
            return Some(dent);
        }
    }
    None
}

/// Process the `entry` and reply on the `tx` channel if this is an entry of interest.
fn process_file(tx: &Sender<FileData>, entry: DirEntry, types: &HashMap<Kind, GlobSet>) {
    if entry.path().ends_with("Limits.toml") {
        tx.send(FileData::LimitsFile(
            entry.path().canonicalize().expect("Could not make abs"),
        ))
        .expect("Could not send FileData::LimitsFile.");
    } else {
        let file_ts: Vec<Kind> = types
            .iter()
            .filter(|(_ft, globs)| globs.is_match(entry.path()))
            .map(|(ft, _glob)| ft.clone())
            .collect();

        if !file_ts.is_empty() {
            let abs_path = entry
                .path()
                .canonicalize()
                .expect("Could not make logfile into abs path");
            tx.send(FileData::LogFile(LogFile(abs_path, file_ts)))
                .expect("Could not send FileData::LogFile");
        }
    }
}
