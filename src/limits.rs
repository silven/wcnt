use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};

use config::ConfigError;
use id_arena::Id;
use serde::Deserialize;

use crate::settings::Kind;
use crate::utils::SearchableArena;

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub(crate) struct Category(pub(crate) Option<Id<String>>);

impl Category {
    pub fn new(id: Id<String>) -> Self {
        Category(Some(id))
    }

    pub fn none() -> Self {
        Category(None)
    }

    pub(crate) fn convert(&mut self, from: &SearchableArena, to: &SearchableArena) {
        if let Some(cat_id) = self.0 {
            let cat_str = from.lookup(cat_id).expect("No such string?");
            self.0 = to.get_id(cat_str);
        }
    }
}

pub(crate) struct LimitsFile {
    inner: HashMap<Kind, Threshold>,
}

impl LimitsFile {
    pub fn iter(&self) -> impl Iterator<Item = (&Kind, &Threshold)> {
        self.inner.iter()
    }

    pub fn display(&self, arena: &SearchableArena) -> impl Display {
        use std::fmt::Write;

        let mut buff = String::new();
        writeln!(buff, "LimitsFile {{");
        for (kind, threshold) in &self.inner {
            let kind_str = arena.lookup(kind.0).unwrap();
            match threshold {
                Threshold::Number(x) => { writeln!(buff, "{} = {}", kind_str, x); },
                Threshold::PerCategory(dict) => {
                    writeln!(buff, "[{}]", kind_str);
                    for (cat, x) in dict {
                        match cat.0 {
                            Some(cat_id) => {
                                let cat_str = arena.lookup(cat_id).unwrap();
                                writeln!(buff, "{} = {}", cat_str, x);
                            },
                            None => {
                                writeln!(buff, "_ = {}",  x);
                            }
                        }
                    }
                },
            }
        }
        write!(buff, "}}");
        buff
    }
}

#[derive(Debug)]
pub(crate) enum Threshold {
    Number(u64),
    PerCategory(HashMap<Category, u64>),
}


#[derive(PartialEq, Eq, Hash)]
pub(crate) struct LimitsEntry {
    pub(crate) limits_file: Option<PathBuf>,
    pub(crate) kind: Kind,
    pub(crate) category: Category,
}

impl LimitsEntry {
    pub fn new(
        limits_file: Option<&Path>,
        kind: Kind,
        category: Category,
    ) -> Self {
        LimitsEntry {
            limits_file: limits_file.map(|x| PathBuf::from(x)),
            kind: kind,
            category: category,
        }
    }

    pub fn without_category(&self) -> Self {
        LimitsEntry {
            limits_file: self.limits_file.clone(),
            kind: self.kind.clone(),
            category: Category::none(),
        }
    }
}

impl core::fmt::Debug for LimitsEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self.limits_file {
            Some(ref pb) => {
                // Silly way to take the last 3 components of the path
                let tail: PathBuf = pb
                    .components()
                    .rev()
                    .take(3)
                    .collect::<PathBuf>()
                    .components()
                    .rev()
                    .collect();
                write!(f, "..{}", tail.display());
            }
            None => {
                write!(f, "_");
            }
        };
        write!(f, ":[{:?}/{:?}]", self.kind, self.category)
    }
}


pub(crate) fn parse_limits_file(arena: &mut SearchableArena, file: &Path) -> Result<LimitsFile, ConfigError> {
    let mut limits = config::Config::default();
    limits.merge(config::File::from(file))?;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum RawLimitEntry {
        Number(u64),
        PerCategory(HashMap<String, u64>),
    }

    let as_dict = limits.try_into::<HashMap<String, RawLimitEntry>>()?;
    let mut result = HashMap::new();

    for (key, val) in as_dict.into_iter() {
        // TODO; Turn this is a prettier error
        let kind_id = arena.get_id(&key).unwrap_or_else(|| panic!("Have not seen this kind '{}' before!", key));
        let converted = match val {
            RawLimitEntry::Number(x) => Threshold::Number(x),
            RawLimitEntry::PerCategory(dict) => {
                Threshold::PerCategory(dict.into_iter().map(|(cat, x)| {
                    let cat_id = arena.get_id(&cat).unwrap_or_else(|| arena.insert(cat));
                    (Category::new(cat_id), x)
                }).collect())
            },
        };
        result.insert(Kind::new(kind_id), converted);
    }

    Ok(LimitsFile {
        inner: result,
    })
}
