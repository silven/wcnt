use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::Receiver;

use crate::{CountsTowardsLimit, LimitEntry, LimitsEntry, Settings};

pub(crate) fn search_files(
    settings_dict: &HashMap<String, Settings>,
    log_files: Vec<(PathBuf, Vec<String>)>,
    limits: Arc<HashMap<PathBuf, HashMap<String, LimitEntry>>>
) -> Receiver<(LimitsEntry, CountsTowardsLimit)> {
    let (tx, rx) = crossbeam_channel::bounded(100);
    // Parse all log files in parallel, once for each kind of warning
    crossbeam::scope(|scope| {
        for (log_file, kinds) in log_files {
            // I hope this doesn't exhaust your memory!
            if let Some(loaded_file) = read_file(&log_file) {
                // Most log files will only ever be parsed once,
                // but some build system might do "make all" > big_log.txt,
                // or it might be the console log from Jenkins
                for kind in kinds {
                    let my_limits = limits.clone();
                    let handle = loaded_file.clone();
                    let my_tx = tx.clone();
                    let re = settings_dict.get(&kind).unwrap().regex.0.clone();
                    scope.spawn(move |_| {
                        for matching in re.captures_iter(&handle.as_str()) {
                            // TODO: Send to central agent keeping count
                            // What file is the culprit?
                            let culprit_file = matching.name("file").unwrap().as_str().to_owned();
                            // Try to identify the warning using line, column and category
                            let line: Option<NonZeroUsize> =
                                matching.name("line").map(|m| m.as_str().parse().unwrap());
                            let column: Option<NonZeroUsize> =
                                matching.name("column").map(|m| m.as_str().parse().unwrap());
                            let category = matching.name("category").map(|m| m.as_str());

                            let limits_entry =
                                find_limits_for(&my_limits, Path::new(&culprit_file));
                            //let limits_hash = format_limit_path(limits_entry, &kind, category);
                            let warning = CountsTowardsLimit::new(
                                culprit_file,
                                line,
                                column,
                                &kind,
                                category,
                            );
                            my_tx.send((LimitsEntry::new(limits_entry, &kind, category), warning));
                        }
                    });
                }
            } else {
                eprintln!("Could not read log file: {}", log_file.display());
            }
        }
    });
    rx
}


fn read_file(filename: &Path) -> Option<Arc<String>> {
    use std::io::Read;

    let mut buff = String::with_capacity(4096);
    let mut f = std::fs::File::open(filename).unwrap();
    f.read_to_string(&mut buff).unwrap();
    Some(Arc::new(buff))
}


fn find_limits_for(
    my_limits: &HashMap<PathBuf, HashMap<String, LimitEntry>>,
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
