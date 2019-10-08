//! Module responsible for searching inside files, looking for warnings and matching them against
//! the identified limits.
use std::collections::{HashMap, HashSet};
use std::fs::read_to_string;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::Receiver;
use log::{error, trace};
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

/// Start the threads that searches through the `log_files`, using the regular expressions defined in
/// `settings`. The `limits` are then used to match any "culprit" file (responsible for the warning)
/// with a [LimitsFile](../limits/struct.Limits.html).
pub(crate) fn search_files<'logs>(
    settings: &Settings,
    log_files: &'logs [LogFile],
    limits: &HashSet<&Path>,
) -> Receiver<Result<LogSearchResults, (&'logs LogFile, std::io::Error)>> {
    let (tx, rx) = crossbeam_channel::bounded(100);
    // Parse all log files in parallel, once for each kind of warning
    crossbeam::scope(|scope| {
        for lf in log_files {
            let tx = tx.clone();
            scope.spawn(move |scope| {
                match read_to_string(lf.path()) {
                    Ok(loaded_file) => {
                        // Move the file into an Arc so we can share it across threads
                        let file_handle = Arc::new(loaded_file);
                        // Most log files will only ever be parsed once,
                        // but some build system might do the equivalent of "make all" > big_log.txt,
                        // or it might be the console log from Jenkins
                        for kind in lf.kinds() {
                            let file_contents_handle = file_handle.clone();
                            // TODO; Can be pre-construct these before we read the files?
                            // Since we need to clone the regex for every invocation, I think not.
                            let regex = settings.get(&kind).unwrap().regex.clone();
                            let tx = tx.clone();

                            scope.spawn(move |_| {
                                let result = build_regex_searcher(
                                    limits,
                                    kind,
                                    &file_contents_handle,
                                    regex,
                                );
                                tx.send(Ok(result))
                                    .expect("Could not send() logfile result");
                            });
                        }
                    }
                    Err(e) => {
                        error!("Could not read log file: {}, {}", lf.path().display(), e);
                        tx.send(Err((lf, e)))
                            .expect("Could not send() logfile io error");
                    }
                }
            });
        }
    })
    .expect("Could not create crossbeam scope.");
    rx
}

/// Search through the `file_contents` using the specified `regex`. Match any findings towards the
/// appropriate [LimitsEntry](../limits/struct.LimitsEntry.html) and return the
/// [search results](struct.LogSearchResults.html).
fn build_regex_searcher(
    limits: &HashSet<&Path>,
    kind: &Kind,
    file_contents: &str,
    regex: Regex,
) -> LogSearchResults {
    let mut result = LogSearchResults {
        string_arena: SearchableArena::new(),
        warnings: HashMap::new(),
    };
    // Let's cache the results we get from the calls to `find_limits_for`, in case we get multiple
    // warnings from the same file.
    let mut limits_cache: HashMap<PathBuf, Option<&Path>> = HashMap::new();

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

        // Hmm, it's either always two clones, or always two get-operations. I prefer the latter.rust sort
        let limits_file = if limits_cache.contains_key(&culprit_file) {
            *limits_cache.get(&culprit_file).unwrap()
        } else {
            *limits_cache
                .entry(culprit_file.clone())
                .or_insert_with(|| find_limits_for(&limits, culprit_file.as_path()))
        };

        let category = match cat_match {
            Some(cat_str) => Category::new(result.string_arena.get_or_insert(cat_str)),
            None => Category::none(),
        };
        let description = match desc_match {
            Some(desc_str) => Description::new(result.string_arena.get_or_insert(desc_str)),
            None => Description::none(),
        };
        let category_to_match = if limits_file.is_some() { category.clone() } else { Category::none() };
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
/// such file was found, and we should fallback to the kind default.
/// IMPORTANT NOTE: When run under Linux, `culprit_file` *must not* include \ -characters, because
/// of how Rust doesn't treat them as path separators. `build_regex_searcher` does a string replace
/// operation before calling this function, so it shouldn't be a problem in real world scenarios.
fn find_limits_for<'limits, 'culprit>(
    limits: &'limits HashSet<&Path>,
    culprit_file: &'culprit Path,
) -> Option<&'limits Path> {
    for parent_dir in culprit_file.ancestors() {
        // This happens when parent of . turns into empty string.
        // I want `while let Some(d) && d.parent().is_some() = culprit_file.parent()`
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
    None
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn find_limits_finds_files() {
        let limits_1 = Path::new("foo/bar/Limits.toml");
        let limits_2 = Path::new("foo/bar/baz/Limits.toml");
        let limits: HashSet<&Path> = vec![limits_1, limits_2].into_iter().collect();

        assert_eq!(find_limits_for(&limits, Path::new("data/file.c")), None);
        assert_eq!(
            find_limits_for(&limits, Path::new("foo/bar/file.c")),
            Some(limits_1)
        );
        assert_eq!(
            find_limits_for(&limits, Path::new("foo/bar/baz/badoo/main.c")),
            Some(limits_2)
        );
        assert_eq!(
            find_limits_for(&limits, Path::new("bar/baz/main.c")),
            Some(limits_2)
        );
    }
}
