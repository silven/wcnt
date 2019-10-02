use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::{bounded, Receiver, Sender};
use globset::GlobSet;
use ignore::{DirEntry, Error, WalkBuilder};

use crate::settings::Kind;

#[derive(Debug)]
pub(crate) enum FileData {
    LimitsFile(PathBuf),
    LogFile(PathBuf, Vec<Kind>),
}

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

fn is_file(entry: Result<DirEntry, Error>) -> Option<DirEntry> {
    if let Ok(dent) = entry {
        if dent.file_type()?.is_file() {
            return Some(dent);
        }
    }
    None
}

fn process_file(tx: &Sender<FileData>, entry: DirEntry, types: &HashMap<Kind, GlobSet>) {
    if entry.path().ends_with("Limits.toml") {
        tx.send(FileData::LimitsFile(entry.path().canonicalize().expect("Could not make abs")));
    } else {
        let mut file_ts = vec![];
        for (file_t, globs) in types {
            if globs.is_match(entry.path()) {
                file_ts.push(file_t.clone());
            }
        }
        if !file_ts.is_empty() {
            let abs_path = entry
                .path()
                .canonicalize()
                .expect("Could not make logfile into abs path");
            tx.send(FileData::LogFile(abs_path, file_ts));
        }
    }
}
