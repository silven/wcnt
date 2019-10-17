//! Module responsible for searching through the file system looking for files of interest.
//!
//! Files of interest are either Limits.toml files, or files matching the glob patterns registered
//! for the different [Kind](../settings/struct.Kind.html)s or warnings.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crossbeam_channel::{bounded, Receiver, Sender};
use globset::GlobSet;
use ignore::{DirEntry, Error, WalkBuilder};

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
    fn canonicalize(path: &Path) -> PathBuf;
    fn traverse(start: &Path) -> Receiver<PathBuf>;
}

pub(crate) struct IgnoreWalker;

impl FileSearcher for IgnoreWalker {
    fn canonicalize(path: &Path) -> PathBuf {
        path.canonicalize().expect("Could not make abs")
    }

    fn traverse(start: &Path) -> Receiver<PathBuf> {
        let (tx, rx) = bounded(100);
        WalkBuilder::new(start).build_parallel().run(|| {
            let tx = tx.clone();
            Box::new(move |result| {
                if let Some(entry) = is_file(result) {
                    tx.send(entry.path().to_path_buf())
                        .expect("Could not send traverse result");
                };
                ignore::WalkState::Continue
            })
        });
        rx
    }
}

/// Starts the threads which searches the `start_dir` for files. Uses `types` to know what
/// [Kind](struct.Kind.html)s of warnings we should look for in the files.
pub(crate) fn construct_file_searcher<F: FileSearcher>(
    start_dir: &Path,
    types: HashMap<Kind, GlobSet>,
) -> Receiver<FileData> {
    let (tx, rx) = bounded(100);
    crossbeam::scope(|scope| {
        for entry in F::traverse(&start_dir) {
            scope.spawn(|_| process_file::<F>(&tx, entry, &types));
        }
    }).expect("Could not create crossbeam scope for file searching");
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
fn process_file<F: FileSearcher>(tx: &Sender<FileData>, entry: PathBuf, types: &HashMap<Kind, GlobSet>) {
    if entry.ends_with("Limits.toml") {
        tx.send(FileData::LimitsFile(
            F::canonicalize(entry.as_path()),
        ))
        .expect("Could not send FileData::LimitsFile.");
    } else {
        let file_ts: Vec<Kind> = types
            .iter()
            .filter(|(_ft, globs)| globs.is_match(&entry))
            .map(|(ft, _glob)| ft.clone())
            .collect();

        if !file_ts.is_empty() {
            let abs_path = F::canonicalize(&entry);
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
            fn canonicalize(path: &Path) -> PathBuf {
                path.to_path_buf()
            }

            fn traverse(_start: &Path) -> Receiver<PathBuf> {
                let (tx, rx) = bounded(100);
                tx.send(PathBuf::from("/src/Limits.toml")).expect("Send1");
                tx.send(PathBuf::from("/src/script.py")).expect("Send2");
                tx.send(PathBuf::from("/src/main.c")).expect("Send3");
                rx
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
