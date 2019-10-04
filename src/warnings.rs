use std::cmp::Ordering;
use std::fmt::Display;
use std::num::NonZeroUsize;
use std::path::PathBuf;

use id_arena::Id;

use crate::limits::{Category, LimitsEntry};
use crate::settings::Kind;
use crate::utils;
use crate::utils::SearchableArena;

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub(crate) struct Description(Option<Id<String>>);

impl Description {
    pub fn new(id: Id<String>) -> Self {
        Description(Some(id))
    }

    pub fn none() -> Self {
        Description(None)
    }

    pub fn remap_id(&mut self, from: &SearchableArena, to: &SearchableArena) {
        if let Some(desc_id) = self.0 {
            let desc_str = from
                .lookup(desc_id)
                .expect("String not present in new arena. Did you forget to call add_all?");
            self.0 = to.get_id(desc_str);
        }
    }

    pub fn to_str<'arena>(&self, arena: &'arena SearchableArena) -> Option<&'arena str> {
        if let Some(desc_id) = self.0 {
            Some(
                arena
                    .lookup(desc_id)
                    .expect("Description not present in this arena."),
            )
        } else {
            None
        }
    }
}

#[derive(PartialEq, Eq, Hash)]
pub(crate) struct CountsTowardsLimit {
    culprit: PathBuf,
    line: Option<NonZeroUsize>,
    column: Option<NonZeroUsize>,
    kind: Kind,
    category: Category,
    description: Description,
}

impl PartialOrd for CountsTowardsLimit {
    fn partial_cmp(&self, other: &CountsTowardsLimit) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CountsTowardsLimit {
    fn cmp(&self, other: &CountsTowardsLimit) -> Ordering {
        match self.culprit.cmp(&other.culprit) {
            Ordering::Equal => match self.line.cmp(&other.line) {
                Ordering::Equal => self.column.cmp(&other.column),
                line_cmp => line_cmp,
            },
            path_cmp => path_cmp,
        }
    }
}

impl CountsTowardsLimit {
    pub fn new(
        culprit_file: PathBuf,
        line: Option<NonZeroUsize>,
        column: Option<NonZeroUsize>,
        kind: Kind,
        category: Category,
        desc: Description,
    ) -> Self {
        CountsTowardsLimit {
            culprit: culprit_file,
            line: line,
            column: column,
            kind: kind,
            category: category,
            description: desc,
        }
    }

    pub fn remap(mut self, from: &SearchableArena, to: &SearchableArena) -> Self {
        self.category.remap_id(&from, &to);
        self.description.remap_id(&from, &to);
        self
    }

    pub fn display<'me, 'arena: 'me>(
        &'me self,
        arena: &'arena SearchableArena,
    ) -> impl Display + 'me {
        utils::fmt_helper(move |f| {
            fn fmt_nonzero(val: Option<NonZeroUsize>) -> String {
                val.map(|x| x.to_string()).unwrap_or_else(|| "?".to_owned())
            }
            write!(
                f,
                "{}:{}:{}",
                self.culprit.display(),
                fmt_nonzero(self.line),
                fmt_nonzero(self.column),
            )?;

            if let Some(desc_str) = self.description.to_str(&arena) {
                write!(f, ": {}", desc_str)?;
            }
            Ok(())
        })
    }
}

pub(crate) struct Violation<'entry> {
    entry: &'entry LimitsEntry,
    threshold: u64,
    actual: u64,
}

impl<'entry> Violation<'entry> {
    pub fn new(limits_entry: &'entry LimitsEntry, threshold: u64, num_warnings: u64) -> Self {
        Violation {
            entry: limits_entry,
            threshold: threshold,
            actual: num_warnings,
        }
    }

    pub fn entry(&self) -> &LimitsEntry {
        self.entry
    }

    pub fn display<'me, 'arena: 'me>(
        &'me self,
        arena: &'arena SearchableArena,
    ) -> impl Display + 'me {
        utils::fmt_helper(move |f| {
            write!(
                f,
                "{} ({} > {})",
                self.entry.display(&arena),
                self.actual,
                self.threshold
            )
        })
    }
}

impl<'e> PartialOrd for Violation<'e> {
    fn partial_cmp(&self, other: &Violation) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<'e> PartialEq for Violation<'e> {
    fn eq(&self, other: &Violation) -> bool {
        self.entry.eq(&other.entry)
            && self.threshold.eq(&other.threshold)
            && self.actual.eq(&other.actual)
    }
}

impl<'e> Eq for Violation<'e> {}

impl<'e> Ord for Violation<'e> {
    fn cmp(&self, other: &Violation) -> Ordering {
        match self.entry.cmp(&other.entry) {
            Ordering::Equal => match self.threshold.cmp(&other.threshold) {
                Ordering::Equal => self.actual.cmp(&other.actual),
                threshold_cmp => threshold_cmp,
            },
            entry_cmp => entry_cmp,
        }
    }
}
