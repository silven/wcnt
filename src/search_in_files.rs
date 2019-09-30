use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use regex::{Captures, Regex};

use crate::CountsTowardsLimit;
use crate::settings::{Category, Kind, LimitEntry, LimitsEntry, Settings};

pub(crate) fn search_files(
    settings: &Settings,
    log_files: Vec<(PathBuf, Vec<Kind>)>,
    limits: Arc<HashMap<PathBuf, HashMap<Kind, LimitEntry>>>,
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
    limits: Arc<HashMap<PathBuf, HashMap<Kind, LimitEntry>>>,
    tx: Sender<(LimitsEntry, CountsTowardsLimit)>,
}

impl Processor {
    fn process_file(&self, file_contents: &String) {
        for matching in self.regex.captures_iter(&file_contents.as_str()) {
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

fn find_limits_for(
    my_limits: &HashMap<PathBuf, HashMap<Kind, LimitEntry>>,
    file: &Path,
) -> Option<PathBuf> {
    let mut maybe_parent = file.parent();
    while let Some(dir) = maybe_parent {
        if dir.parent().is_none() {
            break;
        }
        // This happens when parent of . turns into empty string
        for (key, data) in my_limits.iter() {
            //println!("Comparing {} to {}", dir.display(), key.display());
            if key.ends_with(dir) {
                println!(
                    "Culprit {} should count towards limits defined in {}",
                    file.display(),
                    key.display()
                );
                return Some(key.clone());
            }
        }
        maybe_parent = dir.parent();
    }
    None
}
