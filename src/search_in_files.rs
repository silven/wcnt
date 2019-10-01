use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use regex::{Captures, Regex};

use crate::CountsTowardsLimit;
use crate::limits::{Category, LimitEntry, LimitsEntry, LimitsFile};
use crate::settings::{Kind, Settings};

pub(crate) fn search_files(
    settings: &Settings,
    log_files: Vec<(PathBuf, Vec<Kind>)>,
    limits: Arc<HashMap<PathBuf, LimitsFile>>,
) -> Receiver<(LimitsEntry, CountsTowardsLimit)> {
    let (tx, rx) = crossbeam_channel::bounded(100);
    // Parse all log files in parallel, once for each kind of warning
    crossbeam::scope(|scope| {
        for (log_file, kinds) in log_files {
            // I hope this doesn't exhaust your memory!
            let limits = limits.clone();
            let tx = tx.clone();
            scope.spawn(move |scope| {
                if let Some(loaded_file) = read_file(&log_file) {
                    // Most log files will only ever be parsed once,
                    // but some build system might do the equivalent of "make all" > big_log.txt,
                    // or it might be the console log from Jenkins
                    for kind in kinds {
                        let file_contents_handle = loaded_file.clone();
                        // TODO; Can be pre-construct these before we read the files?
                        let processor = Processor {
                            regex: settings.get(&kind).unwrap().regex.clone(),
                            kind: kind,
                            limits: limits.clone(),
                            tx: tx.clone(),
                        };
                        scope.spawn(move |_| {
                            processor.process_file(&file_contents_handle);
                        });
                    }
                } else {
                    eprintln!("Could not read log file: {}", log_file.display());
                }
            });
        }
    });
    rx
}

struct Processor {
    regex: Regex,
    kind: Kind,
    limits: Arc<HashMap<PathBuf, LimitsFile>>,
    tx: Sender<(LimitsEntry, CountsTowardsLimit)>,
}

impl Processor {
    fn process_file(&self, file_contents: &str) {
        for matching in self.regex.captures_iter(&file_contents) {
            self.process_captures(matching);
        }
    }
    fn process_captures(&self, matching: Captures) {
        // What file is the culprit?
        let culprit_file = matching.name("file").unwrap().as_str().to_owned();
        // Try to identify the warning using line, column and category
        let line: Option<NonZeroUsize> = matching.name("line").map(|m| m.as_str().parse().unwrap());
        let column: Option<NonZeroUsize> =
            matching.name("column").map(|m| m.as_str().parse().unwrap());
        let category = matching
            .name("category")
            .map(|m| Category::from_str(m.as_str()));

        let limits_entry = find_limits_for(&self.limits, Path::new(&culprit_file));

        let warning =
            CountsTowardsLimit::new(culprit_file, line, column, &self.kind, category.as_ref());
        self.tx.send((
            LimitsEntry::new(limits_entry, &self.kind, category),
            warning,
        ));
    }
}

fn read_file(filename: &Path) -> Option<Arc<String>> {
    use std::io::Read;

    let mut buff = String::with_capacity(4096);
    let mut f = std::fs::File::open(filename).unwrap();
    f.read_to_string(&mut buff).unwrap();
    Some(Arc::new(buff))
}

fn find_limits_for<'a, 'b>(
    my_limits: &'a HashMap<PathBuf, LimitsFile>,
    file: &'b Path,
) -> Option<&'a Path> {
    let mut maybe_parent = file.parent();
    while let Some(parent_dir) = maybe_parent {
        // This happens when parent of . turns into empty string
        if parent_dir.parent().is_none() {
            break;
        }

        for found_limit_file in my_limits.keys() {
            if found_limit_file.ends_with(parent_dir) {
                println!(
                    "Culprit {} should count towards limits defined in {}",
                    file.display(),
                    found_limit_file.display()
                );
                return Some(found_limit_file.as_path());
            }
        }
        maybe_parent = parent_dir.parent();
    }
    None
}
