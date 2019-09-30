use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::LimitEntry;
use config::ConfigError;
use crossbeam_channel::{bounded, Receiver, Sender};
use globset::GlobSet;
use ignore::types::Types;
use ignore::{DirEntry, Error, WalkBuilder};

#[derive(Debug)]
pub(crate) enum FileData {
    Limits(PathBuf, HashMap<String, LimitEntry>),
    LogFile(PathBuf, Vec<String>),
    ParseLimitsError(PathBuf, ConfigError),
}

pub(crate) fn construct_file_searcher(
    start_dir: &Path,
    types: HashMap<String, GlobSet>,
) -> Receiver<FileData> {
    let (tx, rx) = bounded(100);
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

fn process_file(tx: &Sender<FileData>, entry: DirEntry, types: &HashMap<String, GlobSet>) {
    if entry.path().ends_with("Limits.toml") {
        match parse_limits_file(&entry) {
            Ok(dict) => {
                let directory = entry
                    .path()
                    .canonicalize()
                    .expect("Could not construct absolute path!")
                    .parent()
                    .expect("File has no parent!")
                    .to_path_buf();
                tx.send(FileData::Limits(directory, dict));
            }
            Err(err) => {
                tx.send(FileData::ParseLimitsError(entry.into_path(), err));
            }
        }
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

fn parse_limits_file(file: &DirEntry) -> Result<HashMap<String, LimitEntry>, ConfigError> {
    let mut limits = config::Config::default();
    limits.merge(config::File::from(file.path()))?;
    let dict = limits.try_into::<HashMap<String, LimitEntry>>()?;
    Ok(dict)
}
