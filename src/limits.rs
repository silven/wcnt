use std::collections::HashMap;
use std::error::Error;
use std::fmt::Display;
use std::path::{Path, PathBuf};

use id_arena::Id;
use serde::Deserialize;
use toml;

use crate::settings::Kind;
use crate::utils;
use crate::utils::SearchableArena;

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub(crate) struct Category(Option<Id<String>>);

impl Category {
    pub fn new(id: Id<String>) -> Self {
        Category(Some(id))
    }

    pub fn none() -> Self {
        Category(None)
    }

    pub fn remap_id(&mut self, from: &SearchableArena, to: &SearchableArena) {
        if let Some(cat_id) = self.0 {
            let cat_str = from
                .lookup(cat_id)
                .expect("String not present in new arena. Did you forget to call add_all?");
            self.0 = to.get_id(cat_str);
        }
    }

    pub fn to_str<'arena>(&self, arena: &'arena SearchableArena) -> &'arena str {
        match self.0 {
            Some(cat_id) => arena.lookup(cat_id).unwrap(),
            None => "_",
        }
    }
}

pub(crate) struct LimitsFile {
    inner: HashMap<Kind, Limit>,
}

impl LimitsFile {
    pub fn iter(&self) -> impl Iterator<Item = (&Kind, &Limit)> {
        self.inner.iter()
    }

    pub fn get_limit(&self, kind: &Kind) -> Option<&Limit> {
        self.inner.get(kind)
    }

    pub fn display(&self, arena: &SearchableArena) -> impl Display {
        use std::fmt::Write;

        let mut buff = String::new();
        writeln!(buff, "LimitsFile {{");
        for (kind, limit) in &self.inner {
            let kind_str = kind.to_str(&arena);
            match limit {
                Limit::Number(x) => {
                    writeln!(buff, "{} = {}", kind_str, x);
                }
                Limit::PerCategory(dict) => {
                    writeln!(buff, "[{}]", kind_str);
                    for (cat, x) in dict {
                        writeln!(buff, "{} = {}", cat.to_str(&arena), x);
                    }
                }
            }
        }
        write!(buff, "}}");
        buff
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum Limit {
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
    pub fn new(limits_file: Option<&Path>, kind: Kind, category: Category) -> Self {
        LimitsEntry {
            limits_file: limits_file.map(PathBuf::from),
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

    pub(crate) fn display(&self, arena: &SearchableArena) -> impl Display {
        use std::fmt::Write;

        let mut buff = String::new();
        match self.limits_file {
            Some(ref pb) => {
                let tmp: PathBuf; // Sometimes I think things are a little silly
                let path = if pb.components().count() > 5 {
                    // Silly way to take the last 4 components of the path
                    tmp = PathBuf::from("...").join(
                        pb.components()
                            .rev()
                            .take(4)
                            .collect::<PathBuf>()
                            .components()
                            .rev()
                            .collect::<PathBuf>(),
                    );
                    &tmp
                } else {
                    pb
                };
                write!(buff, "{}", path.display());
            }
            None => {
                write!(buff, "_");
            }
        };
        write!(buff, ":[{}/{}]", self.kind.to_str(&arena), self.category.to_str(&arena));
        buff
    }
}

pub(crate) fn parse_limits_file(
    arena: &mut SearchableArena,
    file: &Path,
) -> Result<LimitsFile, Box<dyn Error>> {
    let file_contents = utils::read_file(file)?;
    parse_limits_file_from_str(arena, &file_contents)
        .map_err(|e| format!("Could not parse `{}`: Reason `{}`", file.display(), e).into())
}

fn parse_limits_file_from_str(
    arena: &mut SearchableArena,
    cfg: &str,
) -> Result<LimitsFile, Box<dyn Error>> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum RawLimitEntry<'input> {
        Number(u64),
        #[serde(borrow)]
        PerCategory(HashMap<&'input str, u64>),
    }

    let as_raw_dict: HashMap<&str, RawLimitEntry> = toml::from_str(&cfg)?;
    let mut result = HashMap::new();

    for (key, val) in as_raw_dict.into_iter() {
        // TODO; Turn this is a prettier error
        let kind_id = arena.get_id(&key).ok_or_else(|| {
            format!(
                "Referred to kind `{}` which has not been configured in the settings.",
                key
            )
        })?;
        let converted = match val {
            RawLimitEntry::Number(x) => Limit::Number(x),
            RawLimitEntry::PerCategory(dict) => Limit::PerCategory(
                dict.into_iter()
                    .map(|(cat_str, x)| {
                        let cat_id = arena.get_or_insert(cat_str);
                        (Category::new(cat_id), x)
                    })
                    .collect(),
            ),
        };
        result.insert(Kind::new(kind_id), converted);
    }

    Ok(LimitsFile { inner: result })
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn can_deserialize_empty() {
        let limits_str = r#"
        "#;

        let mut arena = SearchableArena::new();
        parse_limits_file_from_str(&mut arena, &limits_str).unwrap();
    }

    #[test]
    #[should_panic(expected = "kind `gcc`")]
    fn cannot_deserialize_with_unknown_kind() {
        let limits_str = r#"
        gcc = 1
        "#;

        let mut arena = SearchableArena::new();
        parse_limits_file_from_str(&mut arena, &limits_str).unwrap();
    }

    #[test]
    fn can_deserialize_with_known_kind() {
        let limits_str = r#"
        gcc = 1
        "#;

        let mut arena = SearchableArena::new();
        let gcc_kind = Kind(arena.insert("gcc".to_owned()));
        let limits = parse_limits_file_from_str(&mut arena, &limits_str).unwrap();

        assert_eq!(limits.get_limit(&gcc_kind), Some(&Limit::Number(1)));
    }

    #[test]
    fn can_deserialize_with_categories() {
        let limits_str = r#"
        [gcc]
        -wbad-code = 1
        -wpedantic = 2
        "#;

        let mut arena = SearchableArena::new();
        let gcc_kind = Kind(arena.insert("gcc".to_owned()));
        let limits = parse_limits_file_from_str(&mut arena, &limits_str).expect("parse");

        let cat_bad_code = Category::new(arena.get_id("-wbad-code").expect("bad code"));
        let cat_pedantic = Category::new(arena.get_id("-wpedantic").expect("pedantic"));
        let expected_mapping: HashMap<Category, u64> = vec![(cat_bad_code, 1), (cat_pedantic, 2)]
            .into_iter()
            .collect();
        assert_eq!(
            limits.get_limit(&gcc_kind),
            Some(&Limit::PerCategory(expected_mapping))
        );
    }
}
