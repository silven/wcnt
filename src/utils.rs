use std::collections::HashMap;

use id_arena::{Arena, Id};

#[derive(Debug)]
pub(crate) struct SearchableArena {
    arena: Arena<String>,
    mapping: HashMap<String, Id<String>>,
}

impl SearchableArena {
    pub fn new() -> Self {
        SearchableArena {
            arena: Arena::new(),
            mapping: HashMap::new(),
        }
    }

    fn iter(&self) -> impl Iterator<Item=(Id<String>, &String)> {
        self.arena.iter()
    }

    pub fn insert(&mut self, val: String) -> Id<String>{
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
