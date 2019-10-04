use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::Receiver;
use log::{error, trace};
use regex::Regex;

use crate::utils;
use crate::limits::{Category, LimitsEntry, LimitsFile};
use crate::settings::{Kind, Settings};
use crate::utils::SearchableArena;
use crate::warnings::{CountsTowardsLimit, Description};

pub(crate) struct LogSearchResult {
    pub(crate) string_arena: SearchableArena,
    pub(crate) warnings: HashMap<LimitsEntry, HashSet<CountsTowardsLimit>>,
}

pub(crate) fn search_files(
    settings: &Settings,
    log_files: Vec<(PathBuf, Vec<Kind>)>,
    limits: &HashMap<PathBuf, LimitsFile>,
) -> Receiver<Result<LogSearchResult, (PathBuf, std::io::Error)>> {
    let (tx, rx) = crossbeam_channel::bounded(100);
    // Parse all log files in parallel, once for each kind of warning
    crossbeam::scope(|scope| {
        for (log_file, kinds) in log_files {
            let tx = tx.clone();
            scope.spawn(move |scope| {
                match utils::read_file(&log_file) {
                    Ok(loaded_file) => {
                        let file_handle = Arc::new(loaded_file);
                        // Most log files will only ever be parsed once,
                        // but some build system might do the equivalent of "make all" > big_log.txt,
                        // or it might be the console log from Jenkins
                        for kind in kinds {
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
                                tx.send(Ok(result)).expect("Could not send()");
                            });
                        }
                    }
                    Err(e) => {
                        error!("Could not read log file: {}, {}", log_file.display(), e);
                        tx.send(Err((log_file, e))).expect("Could not send()");
                    }
                }
            });
        }
    })
    .expect("Could not create crossbeam scope.");
    rx
}

fn build_regex_searcher(
    limits: &HashMap<PathBuf, LimitsFile>,
    kind: Kind,
    file_contents_handle: &Arc<String>,
    regex: Regex,
) -> LogSearchResult {
    let mut result = LogSearchResult {
        string_arena: SearchableArena::new(),
        warnings: HashMap::new(),
    };

    let mut limits_cache: HashMap<PathBuf, Option<&Path>> = HashMap::new();

    for matching in regex.captures_iter(&file_contents_handle) {
        // What file is the culprit?
        let culprit_file = matching
            .name("file")
            .map(|m| PathBuf::from(m.as_str()))
            .unwrap();
        // Try to identify the warning using line, column, category and description
        let line: Option<NonZeroUsize> = matching.name("line").map(|m| m.as_str().parse().unwrap());
        let column: Option<NonZeroUsize> =
            matching.name("column").map(|m| m.as_str().parse().unwrap());
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
        let limits_entry = LimitsEntry::new(limits_file, kind.clone(), category.clone());
        let warning = CountsTowardsLimit::new(culprit_file, line, column, kind.clone(), category, description);

        result
            .warnings
            .entry(limits_entry)
            .or_insert_with(HashSet::new)
            .insert(warning);
    }
    result
}

fn find_limits_for<'a, 'b>(
    limits: &'a HashMap<PathBuf, LimitsFile>,
    culprit_file: &'b Path,
) -> Option<&'a Path> {
    let mut maybe_parent = culprit_file.parent();
    while let Some(parent_dir) = maybe_parent {
        // This happens when parent of . turns into empty string.
        // I want `while let Some(d) && d.parent.is_some() = culprit_file.parent()`
        if parent_dir.parent().is_none() {
            break;
        }

        // TODO: This should be able to be done more efficiently
        for found_limit_file in limits.keys() {
            let limit_file_folder = found_limit_file.parent().unwrap();
            //println!("Checking {} against {}", limit_file_folder.display(), parent_dir.display());
            if limit_file_folder.ends_with(parent_dir) {
                trace!(
                    "Culprit {} should count towards limits defined in {}",
                    culprit_file.display(),
                    found_limit_file.display()
                );
                return Some(found_limit_file);
            }
        }
        maybe_parent = parent_dir.parent();
    }
    None
}
