//! Module responsible for structures and functions related to what counts as a warning.
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
/// A Description is a nice-to-have textual component of a warning. This wrapper struct stores
/// an [Id](../../id-arena/struct.Id.html) instead of a String in order to save space.
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

#[derive(PartialEq, Eq, Hash, Clone, Debug)]
/// A warning is anything that counts towards a limit. We identity it with a path to a `culprit`,
/// the [Kind](../settings/struct.Kind.html) causing us to look for the warning in the first place,
/// and optionally line, column, [Category](../limits/struct.Category.html) and [Description](struct.Description.html)
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
        Some(self.cmp(&other))
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
            if let Some(cat_str) = self.category.to_str(&arena) {
                write!(f, " [{}]", cat_str)?;
            }
            Ok(())
        })
    }
}

/// A EntryCount is a pairing of a [Limit](../limits/struct.Limit.html) with an actual warning count.
pub(crate) struct EntryCount<'entry> {
    entry: &'entry LimitsEntry,
    limit: Option<u64>,
    actual: u64,
}

impl<'entry> EntryCount<'entry> {
    pub fn new(
        limits_entry: &'entry LimitsEntry,
        threshold: Option<u64>,
        num_warnings: u64,
    ) -> Self {
        EntryCount {
            entry: limits_entry,
            limit: threshold,
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
            if let Some(limit) = self.limit {
                write!(
                    f,
                    "{} ({} {} {})",
                    self.entry.display(&arena),
                    self.actual,
                    if self.actual > limit { ">" } else { "<=" },
                    limit
                )
            } else {
                write!(f, "{} ({} < inf)", self.entry.display(&arena), self.actual,)
            }
        })
    }
}

impl<'e> PartialOrd for EntryCount<'e> {
    fn partial_cmp(&self, other: &EntryCount) -> Option<Ordering> {
        Some(self.cmp(&other))
    }
}

impl<'e> PartialEq for EntryCount<'e> {
    fn eq(&self, other: &EntryCount) -> bool {
        self.entry.eq(&other.entry) && self.limit.eq(&other.limit) && self.actual.eq(&other.actual)
    }
}

impl<'e> Eq for EntryCount<'e> {}

impl<'e> Ord for EntryCount<'e> {
    fn cmp(&self, other: &EntryCount) -> Ordering {
        match self.entry.cmp(&other.entry) {
            Ordering::Equal => match self.limit.cmp(&other.limit) {
                Ordering::Equal => self.actual.cmp(&other.actual),
                threshold_cmp => threshold_cmp,
            },
            entry_cmp => entry_cmp,
        }
    }
}

/// A FinalTally is the combined counts, for every [LimitEntry](../limits/struct.LimitEntry.html), the limit
/// and the actual warning count.
pub(crate) struct FinalTally<'a> {
    violations: Vec<EntryCount<'a>>,
    others: Vec<EntryCount<'a>>,
}

impl<'a> FinalTally<'a> {
    pub(crate) fn new(capacity: usize) -> Self {
        FinalTally {
            violations: Vec::with_capacity(capacity),
            others: Vec::with_capacity(capacity),
        }
    }

    pub(crate) fn add(&mut self, entry: EntryCount<'a>) {
        if entry.limit.is_some() && entry.actual > entry.limit.unwrap() {
            self.violations.push(entry);
            self.violations.sort();
        } else {
            self.others.push(entry);
            self.others.sort();
        }
    }

    pub(crate) fn violations(&self) -> &[EntryCount<'_>] {
        &self.violations
    }

    pub(crate) fn non_violations(&self) -> &[EntryCount<'_>] {
        &self.others
    }
}
