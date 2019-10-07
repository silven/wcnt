//! Misc utilities.
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;

use id_arena::{Arena, Id};

/// A small helper struct, wrapping a closure, implementing Display by calling that closure.
struct FmtHelper<'obj, F>
where
    F: Fn(&mut std::fmt::Formatter<'_>) -> std::fmt::Result + 'obj,
{
    inner: F,
    _phantom: PhantomData<&'obj usize>,
}

impl<'obj, F> Display for FmtHelper<'obj, F>
where
    F: Fn(&mut std::fmt::Formatter<'_>) -> std::fmt::Result + 'obj,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        (self.inner)(f)
    }
}

/// "Constructor" function for our FmtHelper
///
///  # Examples
/// ```rust
/// # use std::fmt::Display;
/// # use wcnt::utils::fmt_helper;
/// struct Foo { value: usize };
/// struct AsBinary(bool);
///
/// impl Foo {
///   fn new(value: usize) -> Self { Foo { value } }
///   fn display(&self, as_binary: AsBinary) -> impl Display + '_ {
///     fmt_helper(move |f| {
///       if as_binary.0 {
///         write!(f, "{:#b}", self.value)
///       } else {
///         write!(f, "{}", self.value)
///       }
///     })
///   }
/// }
///
/// let foo = Foo::new(5);
/// assert_eq!(format!("{}", foo.display(AsBinary(true))), "0b101");
/// ```
pub fn fmt_helper<'obj>(
    fmt_fn: impl Fn(&mut std::fmt::Formatter<'_>) -> std::fmt::Result + 'obj,
) -> impl Display + 'obj {
    FmtHelper {
        inner: fmt_fn,
        _phantom: PhantomData,
    }
}

#[derive(Debug)]
/// A String [Arena](../../id_arena/struct.Arena.html) that is also searchable, ie. you can get an
/// ID back from a String, if the String already exists in the Arena.
pub(crate) struct SearchableArena {
    arena: Arena<String>,
    // I'd like to store a &str here, but that would create a self-referential type
    mapping: HashMap<String, Id<String>>,
}

impl SearchableArena {
    pub fn new() -> Self {
        SearchableArena {
            arena: Arena::new(),
            mapping: HashMap::new(),
        }
    }

    fn iter(&self) -> impl Iterator<Item = (Id<String>, &String)> {
        self.arena.iter()
    }

    pub fn insert(&mut self, val: String) -> Id<String> {
        let id = self.arena.alloc(val);
        let reference = self.arena.get(id).unwrap();
        self.mapping.insert(reference.clone(), id);
        id
    }

    pub fn get_id(&self, val: &str) -> Option<Id<String>> {
        self.mapping.get(val).cloned()
    }

    pub fn lookup(&self, id: Id<String>) -> Option<&String> {
        self.arena.get(id)
    }

    pub fn get_or_insert(&mut self, val: &str) -> Id<String> {
        self.get_id(val)
            .unwrap_or_else(|| self.insert(val.to_owned()))
    }

    pub fn add_all(&mut self, other: &SearchableArena) {
        for (_key, val) in other.iter() {
            self.get_or_insert(val);
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn can_insert_and_get() {
        let mut arena = SearchableArena::new();
        let a_string = "a string";
        let inserted_as = arena.insert(a_string.to_owned());
        let found_as = arena.get_id(a_string).unwrap();
        assert_eq!(inserted_as, found_as);

        let inside_arena = arena.lookup(found_as).unwrap();
        assert_eq!(a_string, inside_arena);
    }
}
