//! Module responsible for searching inside files, looking for warnings and matching them against
//! the identified limits.
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use crossbeam_channel::Receiver;
use log::{debug, error, trace};
use regex::Regex;

use crate::limits::{Category, LimitsEntry};
use crate::search_for_files::LogFile;
use crate::settings::{Kind, Settings};
use crate::utils::SearchableArena;
use crate::warnings::{CountsTowardsLimit, Description};

/// The LogSearchResult contains the information about what we found in a
/// [log file](struct.LogFile.html). Because the searches happen in parallel, each LogSearchResult
/// has its own [string arena](struct.SearchableArena.html) which must later be merged together
/// in order to get sensible results. The search results maps all matches warnings to the
/// corresponding [LimitsEntry](struct.LimitsEntry.html).
pub(crate) struct LogSearchResults {
    pub(crate) string_arena: SearchableArena,
    pub(crate) warnings: HashMap<LimitsEntry, HashSet<CountsTowardsLimit>>,
}

pub(crate) trait FileReader {
    fn read_file_to_string(path: &Path) -> std::io::Result<String>;
}

pub(crate) struct FileSystemReader;

impl FileReader for FileSystemReader {
    fn read_file_to_string(path: &Path) -> std::io::Result<String> {
        std::fs::read_to_string(path)
    }
}

/// Start the threads that searches through the `log_files`, using the regular expressions defined in
/// `settings`. The `limits` are then used to match any "culprit" file (responsible for the warning)
/// with a [LimitsFile](../limits/struct.Limits.html).
pub(crate) fn search_files<R: FileReader>(
    settings: &Settings,
    limit_files: &HashSet<PathBuf>,
    log_files: Vec<LogFile>,
) -> Receiver<Result<LogSearchResults, (LogFile, std::io::Error)>> {
    use rayon::iter::ParallelIterator;
    use rayon::iter::IntoParallelIterator;
    let (tx, rx) = crossbeam_channel::bounded(128);

    let regexes_to_use = settings.kinds_and_regex();
    let limit_files = limit_files.clone();

    std::thread::spawn(move || {
        // Parse all log files in parallel
        log_files.into_par_iter().for_each(|lf| {
            match R::read_file_to_string(lf.path()) {
                Ok(loaded_file) => {
                    // TODO: figure out a way to cleanly skip reading the file if we're skipping
                    // all of its kinds.
                    for kind in lf.kinds() {
                        if let Some(regex) = regexes_to_use.get(kind) {
                            let result = search_contents_with_regex(
                                &limit_files,
                                kind,
                                &loaded_file,
                                regex,
                            );
                            tx.send(Ok(result)).expect("Could not send() result");
                        }
                    }
                },
                Err(e) => {
                    error!("Could not read log file: {}, {}", lf.path().display(), e);
                    tx.send(Err((lf, e)))
                        .expect("Could not send() logfile io error");
                },
            }
        });
    });
    rx
}

// Most log files will only ever be parsed once,
// but some build system might do the equivalent of "make all" > big_log.txt,
// or it might be the console log from Jenkins

/// Search through the `file_contents` using the specified `regex`. Match any findings towards the
/// appropriate [LimitsEntry](../limits/struct.LimitsEntry.html) and return the
/// [search results](struct.LogSearchResults.html).
fn search_contents_with_regex(
    limits: &HashSet<PathBuf>,
    kind: &Kind,
    file_contents: &str,
    regex: &Regex,
) -> LogSearchResults {
    let mut result = LogSearchResults {
        string_arena: SearchableArena::new(),
        warnings: HashMap::new(),
    };
    // Let's cache the results we get from the calls to `find_limits_for`, in case we get multiple
    // warnings from the same file.
    let mut limits_cache: HashMap<PathBuf, Option<&PathBuf>> = HashMap::new();

    for matching in regex.captures_iter(file_contents) {
        // What file is the culprit? TODO: We don't have any decent normalize() function yet..
        let culprit_file = matching
            .name("file")
            .map(|m| PathBuf::from(m.as_str().replace("\\", "/")))
            .unwrap();

        // Try to identify the warning using line, column, category and description
        let line: Option<NonZeroUsize> = matching.name("line").map(|m| {
            m.as_str()
                .parse()
                .unwrap_or_else(|e| panic!("Capture for `line` was not a non zero number: `{}`", e))
        });
        let column: Option<NonZeroUsize> = matching.name("column").map(|m| {
            m.as_str().parse().unwrap_or_else(|e| {
                panic!("Capture for `column` was not a non zero number: `{}`", e)
            })
        });
        let cat_match = matching.name("category").map(|m| m.as_str());
        let desc_match = matching.name("description").map(|m| m.as_str());

        let limits_file = limits_cache.entry(culprit_file.clone())
            .or_insert_with(|| find_limits_for(limits, culprit_file.as_path()))
            .as_deref();

        let category = match cat_match {
            Some(cat_str) => Category::new(result.string_arena.get_or_insert(cat_str)),
            None => Category::none(),
        };
        let description = match desc_match {
            Some(desc_str) => Description::new(result.string_arena.get_or_insert(desc_str)),
            None => Description::none(),
        };
        let category_to_match = if limits_file.is_some() {
            category.clone()
        } else {
            Category::none()
        };
        let limits_entry = LimitsEntry::new(limits_file, kind.clone(), category_to_match);
        let warning = CountsTowardsLimit::new(
            culprit_file,
            line,
            column,
            kind.clone(),
            category,
            description,
        );

        result
            .warnings
            .entry(limits_entry)
            .or_insert_with(HashSet::new)
            .insert(warning);
    }
    result
}

/// Every warning originates at a "culprit" file. These files are located under a Limits.toml file
/// in the file system tree. `find_limits_for` finds the Limits.toml file "responsible" for the
/// culprit, so we know which [limits](../limits/enum.Limit.html) to use. Returns `None` if no
/// such file was found, and we default to zero.
/// IMPORTANT NOTE: When run under Linux, `culprit_file` *must not* include \ -characters, because
/// of how Rust doesn't treat them as path separators. `build_regex_searcher` does a string replace
/// operation before calling this function, so it shouldn't be a problem in real world scenarios.
fn find_limits_for<'limits, 'culprit>(
    limits: &'limits HashSet<PathBuf>,
    culprit_file: &'culprit Path,
) -> Option<&'limits PathBuf> {
    for parent_dir in culprit_file.ancestors() {
        // This happens when parent_dir turns into empty string,
        // and everything ends with an empty string...
        if parent_dir.parent().is_none() {
            break;
        }
        // TODO: This should be possible to do more efficiently
        for limit_file in limits {
            let limit_file_folder = limit_file
                .parent()
                .unwrap_or_else(|| panic!("Limits file `{}` has no parent!", limit_file.display()));
            if limit_file_folder.ends_with(parent_dir) {
                trace!(
                    "Culprit `{}` should count towards limits defined in `{}`",
                    culprit_file.display(),
                    limit_file.display()
                );
                return Some(limit_file);
            }
        }
    }
    debug!("Did not find a Limits.toml for culprit `{}`", culprit_file.display());
    None
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn find_limits_finds_files() {
        let limits_1 = PathBuf::from("foo/bar/Limits.toml");
        let limits_2 = PathBuf::from("foo/bar/baz/Limits.toml");
        let limits: HashSet<PathBuf> = vec![limits_1.clone(), limits_2.clone()].into_iter().collect();

        assert_eq!(find_limits_for(&limits, Path::new("data/file.c")), None);
        assert_eq!(
            find_limits_for(&limits, Path::new("foo/bar/file.c")),
            Some(&limits_1)
        );
        assert_eq!(
            find_limits_for(&limits, Path::new("foo/bar/baz/badoo/main.c")),
            Some(&limits_2)
        );
        assert_eq!(
            find_limits_for(&limits, Path::new("bar/baz/main.c")),
            Some(&limits_2)
        );
    }
}
