use std::mem;
use std::collections::btree_map::{self, BTreeMap};

use serde_json::Value;
use stdlog::key_values::source::{self, Source};

/**
A map of enriched properties.

This map is optimised for contexts that are empty or contain a single property.
*/
#[derive(Clone, Debug)]
pub(crate) enum Properties {
    Empty,
    Single(&'static str, Value),
    Map(BTreeMap<&'static str, Value>),
}

pub(crate) enum Iter<'a> {
    Empty,
    Single(&'static str, &'a Value),
    Map(btree_map::Iter<'a, &'static str, Value>),
}

impl<'a> Iterator for Iter<'a> {
    type Item = (&'static str, &'a Value);

    fn next(&mut self) -> Option<Self::Item> {
        match *self {
            Iter::Empty => None,
            Iter::Single(k, v) => {
                *self = Iter::Empty;

                Some((k, v))
            }
            Iter::Map(ref mut map) => map.next().map(|(k, v)| (*k, v)),
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
                    mem::replace(self, Properties::Map(BTreeMap::new()))
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

    pub fn iter(&self) -> Iter {
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
    type IntoIter = Iter<'a>;
    type Item = (&'static str, &'a Value);

    fn into_iter(self) -> Self::IntoIter {
        match *self {
            Properties::Empty => Iter::Empty,
            Properties::Single(ref k, ref v) => Iter::Single(k, v),
            Properties::Map(ref m) => Iter::Map(m.iter()),
        }
    }
}

impl Source for Properties {
    fn visit<'kvs>(&'kvs self, visitor: &mut dyn source::Visitor<'kvs>) -> Result<(), source::Error> {
        for (k, v) in self {
            visitor.visit_pair(source::Key::from_str(k, None), source::Value::from_serde(v))?;
        }

        Ok(())
    }
}
