//! Module responsible for searching through the file system looking for files of interest.
//!
//! Files of interest are either Limits.toml files, or files matching the glob patterns registered
//! for the different [Kind](../settings/struct.Kind.html)s or warnings.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crossbeam_channel::{bounded, Receiver, Sender};
use globset::GlobSet;
use jwalk::{DirEntry, WalkDir};
use rayon::prelude::*;

use crate::settings::Kind;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
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

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
/// A partial result of the file search. Signal having found either a
/// [LimitsFile](struct.LimitsFile.html) or a relevant [log file](struct.LogFile.html).
pub(crate) enum FileData {
    LimitsFile(PathBuf),
    LogFile(LogFile),
}

pub(crate) trait FileSearcher {
    fn normalize_path(path: &Path) -> PathBuf;
    fn traverse<F: Fn(&Path) + Sync>(start: &Path, callback: F);
}

pub(crate) struct JWalkWalker;

impl FileSearcher for JWalkWalker {
    fn normalize_path(path: &Path) -> PathBuf {
        path.canonicalize().expect("Could not make abs")
    }

    fn traverse<F: Fn(&Path) + Sync>(start: &Path, callback: F) {
        WalkDir::new(start).skip_hidden(false).into_iter().par_bridge().for_each(|result| {
            if let Some(entry) = is_file(result) {
                callback(&entry.path());
            };
        });
    }
}

/// Starts the threads which searches the `start_dir` for files. Uses `types` to know what
/// [Kind](struct.Kind.html)s of warnings we should look for in the files.
pub(crate) fn construct_file_searcher<F: FileSearcher>(
    start_dir: &Path,
    types: HashMap<Kind, GlobSet>,
) -> Receiver<FileData> {
    let (tx, rx) = bounded(100);
    let start_dir = start_dir.to_path_buf();
    std::thread::spawn(move || {
        F::traverse(&start_dir, |entry| {
            process_file::<F>(&tx, entry, &types);
        });
    });
    rx
}

/// Is the `entry` a file? (See [DirEntry](../../jwalk/struct.DirEntry.html))
fn is_file(entry: Result<DirEntry, std::io::Error>) -> Option<DirEntry> {
    if let Ok(dent) = entry {
        if dent.file_type.as_ref().map(|ft| ft.is_file()).unwrap_or(false) {
            return Some(dent);
        }
    }
    None
}

/// Process the `entry` and reply on the `tx` channel if this is an entry of interest.
fn process_file<F: FileSearcher>(tx: &Sender<FileData>, entry: &Path, types: &HashMap<Kind, GlobSet>) {
    if entry.ends_with("Limits.toml") {
        tx.send(FileData::LimitsFile(
            F::normalize_path(entry),
        ))
        .expect("Could not send FileData::LimitsFile.");
    } else {
        let file_ts: Vec<Kind> = types
            .iter()
            .filter(|(_ft, globs)| globs.is_match(&entry))
            .map(|(ft, _glob)| ft.clone())
            .collect();

        if !file_ts.is_empty() {
            let abs_path = F::normalize_path(&entry);
            tx.send(FileData::LogFile(LogFile(abs_path, file_ts)))
                .expect("Could not send FileData::LogFile");
        }
    }
}


#[cfg(test)]
mod test {
    use globset::{Glob, GlobSetBuilder};

    use crate::utils::SearchableArena;

    use super::*;

    macro_rules! assert_eq_sorted {
        ($v1:expr, $v2:expr) => {
            let v1_sorted = {
                let mut tmp = $v1;
                tmp.sort();
                tmp
            };

            let v2_sorted = {
                let mut tmp = $v2;
                tmp.sort();
                tmp
            };

            assert_eq!(v1_sorted, v2_sorted);
        }
    }

    #[test]
    fn file_searcher_turns_paths_into_file_data_according_to_globsets() {
        struct DummyFileSearcher;

        impl FileSearcher for DummyFileSearcher {
            fn normalize_path(path: &Path) -> PathBuf {
                path.to_path_buf()
            }

            fn traverse<F: Fn(&Path)>(_start: &Path, callback: F) {
                callback(Path::new("/src/Limits.toml"));
                callback(Path::new("/src/script.py"));
                callback(Path::new("/src/main.c"));
            }
        }

        let mut arena = SearchableArena::new();
        let mut interesting_types = HashMap::new();
        let gcc_kind = Kind::new(arena.insert("gcc".to_owned()));
        let c_globber = GlobSetBuilder::new().add(Glob::new("*.c").expect("Glob")).build().expect("GlobSet");
        interesting_types.insert(gcc_kind.clone(), c_globber);
        let rx = construct_file_searcher::<DummyFileSearcher>(Path::new("somewhere"), interesting_types);

        assert_eq_sorted!(
            vec![
                FileData::LimitsFile(PathBuf::from("/src/Limits.toml")),
                FileData::LogFile(LogFile(PathBuf::from("/src/main.c"), vec![gcc_kind])),
            ],
            rx.into_iter().collect::<Vec<_>>());
    }
}
