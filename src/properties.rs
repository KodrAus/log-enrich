use std::mem;
use std::collections::hash_map::{self, HashMap};

use serde_json::Value;

/**
A map of enriched properties.

This map is optimised for contexts that are empty or contain a single property.
*/
#[derive(Clone, Debug)]
pub(crate) enum Properties {
    Empty,
    Single(&'static str, Value),
    Map(HashMap<&'static str, Value>),
}

pub(crate) enum PropertiesIter<'a> {
    Empty,
    Single(&'static str, &'a Value),
    Map(hash_map::Iter<'a, &'static str, Value>),
}

impl<'a> Iterator for PropertiesIter<'a> {
    type Item = (&'static str, &'a Value);

    fn next(&mut self) -> Option<Self::Item> {
        match *self {
            PropertiesIter::Empty => None,
            PropertiesIter::Single(k, v) => {
                *self = PropertiesIter::Empty;

                Some((k, v))
            }
            PropertiesIter::Map(ref mut map) => map.next().map(|(k, v)| (*k, v)),
        }
    }
}

impl Default for Properties {
    fn default() -> Self {
        Properties::Empty
    }
}

impl Properties {
    pub fn insert(&mut self, k: &'static str, v: Value) {
        match *self {
            Properties::Empty => {
                *self = Properties::Single(k, v);
            }
            Properties::Single(_, _) => {
                if let Properties::Single(pk, pv) =
                    mem::replace(self, Properties::Map(HashMap::new()))
                {
                    self.insert(pk, pv);
                    self.insert(k, v);
                } else {
                    unreachable!()
                }
            }
            Properties::Map(ref mut m) => {
                m.insert(k, v);
            }
        }
    }

    pub fn contains_key(&self, key: &'static str) -> bool {
        match *self {
            Properties::Single(k, _) if k == key => true,
            Properties::Map(ref m) => m.contains_key(key),
            _ => false,
        }
    }

    pub fn iter(&self) -> PropertiesIter {
        self.into_iter()
    }

    pub fn len(&self) -> usize {
        match *self {
            Properties::Empty => 0,
            Properties::Single(_, _) => 1,
            Properties::Map(ref m) => m.len(),
        }
    }
}

impl<'a> Extend<(&'static str, &'a Value)> for Properties {
    fn extend<T>(&mut self, iter: T)
    where
        T: IntoIterator<Item = (&'static str, &'a Value)>,
    {
        for (k, v) in iter {
            if !self.contains_key(k) {
                self.insert(k, v.to_owned());
            }
        }
    }
}

impl<'a> IntoIterator for &'a Properties {
    type IntoIter = PropertiesIter<'a>;
    type Item = (&'static str, &'a Value);

    fn into_iter(self) -> Self::IntoIter {
        match *self {
            Properties::Empty => PropertiesIter::Empty,
            Properties::Single(ref k, ref v) => PropertiesIter::Single(k, v),
            Properties::Map(ref m) => PropertiesIter::Map(m.iter()),
        }
    }
}
