//! Module responsible for structures and functionality related to Limits and Limit files.
use std::collections::HashMap;
use std::error::Error;
use std::fmt::Display;
use std::fs::read_to_string;
use std::path::{Path, PathBuf};

use id_arena::Id;
use serde::Deserialize;
use toml;

use crate::settings::Kind;
use crate::utils;
use crate::utils::SearchableArena;

#[derive(Debug, PartialEq, Eq, Ord, PartialOrd, Hash, Clone)]
/// A Category represents a specific type of warning, hierarchically below a [Kind](../settings/struct.Kind.html).
/// Examples of Categories could be [-Wunsued-value](https://gcc.gnu.org/onlinedocs/gcc/Warning-Options.html),
/// or [F401](https://flake8.pycqa.org/en/latest/user/error-codes.html).
/// The "_" category is the "wildcard" category. It matches all previously undeclared categories.
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

    fn from_str(as_str: &str, arena: &mut SearchableArena) -> Self {
        if as_str == "_" {
            Category::none()
        } else {
            Category::new(arena.get_or_insert(as_str))
        }
    }
}

/// A LimitsFile declares a [Limit](struct.Limit.html) for a [Kind](../settings/struct.Kind.html) as a threshold
/// of number of warnings allowed.
pub(crate) struct LimitsFile {
    inner: HashMap<Kind, Limit>,
}

impl LimitsFile {
    pub fn iter(&self) -> impl Iterator<Item = (&Kind, &Limit)> {
        self.inner.iter()
    }

    #[cfg(test)]
    pub fn get_limit(&self, kind: &Kind) -> Option<&Limit> {
        self.inner.get(kind)
    }

    pub fn display<'me, 'arena: 'me>(
        &'me self,
        arena: &'arena SearchableArena,
    ) -> impl Display + 'me {
        utils::fmt_helper(move |f| {
            writeln!(f, "LimitsFile {{")?;
            for (kind, limit) in &self.inner {
                write!(f, "{}", limit.display(kind, arena))?;
            }
            write!(f, "}}")
        })
    }
}

#[derive(Debug, PartialEq)]
/// A Limit can either be a single number, which should hold for any [Category](struct.Category.html)
/// of warnings for that [Kind](../settings/struct.Kind.html), or be declared per category.
/// A limit may also be "infinity", represented by None.
pub(crate) enum Limit {
    Number(Option<u64>),
    PerCategory(HashMap<Category, Option<u64>>),
}

impl Limit {
    pub fn display<'me, 'arena: 'me, 'kind: 'me>(
        &'me self,
        kind: &'kind Kind,
        arena: &'arena SearchableArena,
    ) -> impl Display + 'me {
        utils::fmt_helper(move |f| {
            fn write_pair(f: &mut std::fmt::Formatter<'_>, key: &str, value: &Option<u64>) -> std::fmt::Result {
                match value {
                    Some(number) => writeln!(f, "{} = {}", key, number),
                    None => writeln!(f, "{} = inf", key),
                }
            }

            let kind_str = kind.to_str(&arena);
            match self {
                Limit::Number(x) => {
                    write_pair(f, kind_str, x)?;
                }
                Limit::PerCategory(dict) => {
                    writeln!(f, "[{}]", kind_str)?;
                    for (cat, x) in dict {
                        write_pair(f, cat.to_str(&arena), x)?;
                    }
                }
            }
            Ok(())
        })
    }
}

#[derive(PartialEq, Eq, Ord, PartialOrd, Debug, Hash)]
/// A LimitsEntry is a shorthand representation for a single numerical threshold within the system.
/// To uniquely identify a [Limit](enum.Limit.html), you need a Path, a
/// [Kind](../settings/struct.Kind.html) and a [Category](struct.Category.html).
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

    pub fn display<'me, 'arena: 'me>(
        &'me self,
        arena: &'arena SearchableArena,
    ) -> impl Display + 'me {
        utils::fmt_helper(move |f| {
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
                    write!(f, "{}", path.display())?;
                }
                None => {
                    write!(f, "_")?;
                }
            };
            write!(
                f,
                ":[{}/{}]",
                self.kind.to_str(&arena),
                self.category.to_str(&arena)
            )
        })
    }
}

/// Parse a `Limits.toml` file into a [LimitsFile](struct.LimitsFile.html) structure.
pub(crate) fn parse_limits_file(
    arena: &mut SearchableArena,
    file: &Path,
) -> Result<LimitsFile, Box<dyn Error>> {
    let file_contents = read_to_string(file)?;
    parse_limits_file_from_str(arena, &file_contents)
        .map_err(|e| format!("Could not parse `{}`. Reason `{}`", file.display(), e).into())
}

/// Parse the string `cfg` in toml format into a [LimitsFile](struct.LimitsFile.html) structure.
fn parse_limits_file_from_str(
    arena: &mut SearchableArena,
    cfg: &str,
) -> Result<LimitsFile, Box<dyn Error>> {

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum IntOrFloat {
        I(u64),
        F(f32),
    }

    impl IntOrFloat {
        fn to_limit(&self) -> Result<Option<u64>, Box<dyn Error>> {
            match *self {
                IntOrFloat::I(i) => Ok(Some(i)),
                IntOrFloat::F(f) => if f.is_sign_positive() && f.is_infinite() {
                    Ok(None)
                } else {
                    Err("Limit values can only be a positive integer or `inf`.".into())
                }
            }
        }
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum RawLimitEntry<'input> {
        Number(IntOrFloat),
        #[serde(borrow)]
        PerCategory(HashMap<&'input str, IntOrFloat>),
    }

    let as_raw_dict: HashMap<&str, RawLimitEntry> = toml::from_str(&cfg)?;
    let mut result = HashMap::new();

    for (key, val) in as_raw_dict.into_iter() {
        let kind_id = arena.get_id(&key).ok_or_else(|| {
            format!(
                "Referred to kind `{}` which has not been configured in the settings.",
                key
            )
        })?;
        let converted = match val {
            RawLimitEntry::Number(x) => Limit::Number(x.to_limit()?),
            RawLimitEntry::PerCategory(dict) => {
                let mut per_category= HashMap::new();
                for (cat_str, x) in dict {
                    let limit = x.to_limit()?;
                    let category = Category::from_str(cat_str, arena);
                    per_category.insert(category, limit);
                }

                Limit::PerCategory(per_category)
            },
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
        let gcc_kind = Kind::new(arena.insert("gcc".to_owned()));
        let limits = parse_limits_file_from_str(&mut arena, &limits_str).unwrap();

        assert_eq!(limits.get_limit(&gcc_kind), Some(&Limit::Number(Some(1))));
    }

    #[test]
    fn can_deserialize_with_categories() {
        let limits_str = r#"
        [gcc]
        -Wbad-code = 1
        -Wpedantic = 2
        "#;

        let mut arena = SearchableArena::new();
        let gcc_kind = Kind::new(arena.insert("gcc".to_owned()));
        let limits = parse_limits_file_from_str(&mut arena, &limits_str).expect("parse");

        let cat_bad_code = Category::new(arena.get_id("-Wbad-code").expect("bad code"));
        let cat_pedantic = Category::new(arena.get_id("-Wpedantic").expect("pedantic"));
        let expected_mapping: HashMap<Category, Option<u64>> = vec![(cat_bad_code, Some(1)), (cat_pedantic, Some(2))]
            .into_iter()
            .collect();
        assert_eq!(
            limits.get_limit(&gcc_kind),
            Some(&Limit::PerCategory(expected_mapping))
        );
    }


    #[test]
    fn can_deserialize_with_wildcard() {
        let limits_str = r#"
        [gcc]
        -Wbad-code = 2
        _ = 1
        "#;

        let mut arena = SearchableArena::new();
        let gcc_kind = Kind::new(arena.insert("gcc".to_owned()));
        let limits = parse_limits_file_from_str(&mut arena, &limits_str).expect("parse");

        let cat_bad_code = Category::new(arena.get_id("-Wbad-code").expect("bad code"));
        let expected_mapping: HashMap<Category, Option<u64>> = vec![(cat_bad_code, Some(2)), (Category::none(), Some(1))]
            .into_iter()
            .collect();
        assert_eq!(
            limits.get_limit(&gcc_kind),
            Some(&Limit::PerCategory(expected_mapping))
        );
    }

    #[test]
    fn can_deserialize_inf() {
        let limits_str = r#"
        [gcc]
        -Wbad-code = inf
        _ = 1
        "#;

        let mut arena = SearchableArena::new();
        let gcc_kind = Kind::new(arena.insert("gcc".to_owned()));
        let limits = parse_limits_file_from_str(&mut arena, &limits_str).expect("parse");

        let cat_bad_code = Category::new(arena.get_id("-Wbad-code").expect("bad code"));
        let expected_mapping: HashMap<Category, Option<u64>> = vec![(cat_bad_code, None), (Category::none(), Some(1))]
            .into_iter()
            .collect();
        assert_eq!(
            limits.get_limit(&gcc_kind),
            Some(&Limit::PerCategory(expected_mapping))
        );
    }

    #[test]
    #[should_panic(expected="only be a positive integer or `inf`")]
    fn wont_deserialize_floats() {
        let limits_str = r#"
        [gcc]
        -Wbad-code = 2.0
        _ = 1
        "#;

        let mut arena = SearchableArena::new();
        let gcc_kind = Kind::new(arena.insert("gcc".to_owned()));
        let limits = parse_limits_file_from_str(&mut arena, &limits_str).expect("parse");

        let cat_bad_code = Category::new(arena.get_id("-Wbad-code").expect("bad code"));
        let expected_mapping: HashMap<Category, Option<u64>> = vec![(cat_bad_code, Some(2)), (Category::none(), Some(1))]
            .into_iter()
            .collect();
        assert_eq!(
            limits.get_limit(&gcc_kind),
            Some(&Limit::PerCategory(expected_mapping))
        );
    }
}
