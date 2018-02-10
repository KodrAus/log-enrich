#![feature(nll)]
#![cfg_attr(test, feature(test))]

extern crate futures;
#[cfg_attr(test, macro_use)]
extern crate serde_json;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate log;

use std::sync::Arc;
use std::ops::Drop;
use std::mem;
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::{Iter, Entry};

use log::Record;
use futures::{Future, IntoFuture, Poll, Lazy};
use futures::future::lazy;
use serde::ser::{Serialize, Serializer, SerializeMap};

pub use serde_json::Value;

struct BuilderCtxt {
    properties: Properties,
}

#[derive(Clone, Debug)]
enum Properties {
    Empty,
    Single(&'static str, Value),
    Map(HashMap<&'static str, Value>),
}

enum PropertiesIter<'a> {
    Empty,
    Single(&'static str, &'a Value),
    Map(Iter<'a, &'static str, Value>),
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
            PropertiesIter::Map(ref mut map) => map.next().map(|(k, v)| (*k, v))
        }
    }
}

impl Default for Properties {
    fn default() -> Self {
        Properties::Empty
    }
}

impl Properties {
    fn insert(&mut self, k: &'static str, v: Value) {
        match *self {
            Properties::Empty => {
                *self = Properties::Single(k, v);
            },
            Properties::Single(_, _) => {
                if let Properties::Single(pk, pv) = mem::replace(self, Properties::Map(HashMap::new())) {
                    self.insert(pk, pv);
                    self.insert(k, v);
                }
                else {
                    unreachable!()
                }
            },
            Properties::Map(ref mut m) => {
                m.insert(k, v);
            }
        }
    }

    fn iter(&self) -> PropertiesIter {
        match *self {
            Properties::Empty => PropertiesIter::Empty,
            Properties::Single(ref k, ref v) => PropertiesIter::Single(k, v),
            Properties::Map(ref m) => PropertiesIter::Map(m.iter()),
        }
    }
}

pub struct Builder {
    ctxt: Option<BuilderCtxt>,
}

pub struct Logger {
    ctxt: Option<Arc<LocalCtxt>>,
}

pub struct LogFuture<TFuture> {
    logger: Logger,
    inner: TFuture,
}

pub struct Log<'a, 'b> {
    record: Record<'a>,
    _logger: &'b Logger,
}

#[derive(Clone, Default, Debug)]
struct LocalCtxt {
    parent: Option<SharedCtxt>,
    properties: Properties,
}

#[derive(Debug, Clone)]
enum SharedCtxt {
    Local(Arc<LocalCtxt>),
    Joined(Arc<LocalCtxt>, Arc<LocalCtxt>),
}

thread_local!(static LOCAL_CTXT: RefCell<Option<SharedCtxt>> = RefCell::new(Default::default()));

pub fn logger() -> Builder {
    Builder {
        ctxt: None,
    }
}

impl<'a, 'b> Serialize for Log<'a, 'b> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer
    {
        #[derive(Serialize)]
        struct SerializeLog<'a> {
            #[serde(serialize_with = "serialize_msg")]
            msg: &'a Record<'a>,
            #[serde(serialize_with = "serialize_ctxt", skip_serializing_if = "skip_serialize_ctxt")]
            ctxt: (),
        }

        fn skip_serialize_ctxt(_: &()) -> bool {
            LOCAL_CTXT.with(|shared| shared.borrow().is_none())
        }

        fn serialize_ctxt<S>(_: &(), serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            LOCAL_CTXT.with(|shared| {
                let mut seen = HashMap::new();
                let mut map = serializer.serialize_map(None)?;

                if let Some(ref ctxt) = *shared.borrow() {
                    ctxt.serialize(&mut map, &mut seen)?;
                }

                map.end()
            })
        }

        fn serialize_msg<'a, S>(record: &Record<'a>, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serializer.collect_str(record.args())
        }

        let log = SerializeLog {
            msg: &self.record,
            ctxt: (),
        };

        log.serialize(serializer)
    }
}

impl SharedCtxt {
    fn current(&self) -> &Arc<LocalCtxt> {
        match *self {
            SharedCtxt::Local(ref local) => local,
            SharedCtxt::Joined(_, ref local) => local,
        }
    }

    fn serialize<S>(&self, serializer: &mut S, seen: &mut HashMap<&'static str, ()>) -> Result<(), S::Error>
    where
        S: SerializeMap,
    {
        match *self {
            SharedCtxt::Local(ref local) => local.serialize(serializer, seen)?,
            SharedCtxt::Joined(ref shared, ref local) => {
                shared.serialize(serializer, seen)?;
                local.serialize(serializer, seen)?;
            }
        }

        Ok(())
    }

    fn push(shared: &mut Option<SharedCtxt>, ctxt: Arc<LocalCtxt>) {
        if let Some(shared_ctxt) = mem::replace(shared, None) {
            // Move out of the current shared context, just to avoid a clone
            let shared_ctxt = match shared_ctxt {
                SharedCtxt::Local(local) => local,
                SharedCtxt::Joined(_, local) => local,
            };

            // Check if the parent of the context we're adding is the same as the current shared context
            // - If they're the same, it means we're in the same context we were when `ctxt` was build
            // - If they're not the same, it means `ctxt` has been moved somewhere (a different thread)
            //   We can't just replace the `shared` context without losing information, so we join them together
            match ctxt.parent.as_ref().map(|parent| parent.current()) {
                Some(ctxt_parent) if Arc::ptr_eq(&shared_ctxt, ctxt_parent) => *shared = Some(SharedCtxt::Local(ctxt)),
                _ => *shared = Some(SharedCtxt::Joined(shared_ctxt, ctxt))
            }
        }
        else {
            *shared = Some(SharedCtxt::Local(ctxt));
        }
    }

    fn pop(shared: &mut Option<SharedCtxt>) -> Option<Arc<LocalCtxt>> {
        match shared.take() {
            Some(SharedCtxt::Local(local)) => {
                *shared = local.parent.clone();
                Some(local)
            },
            Some(SharedCtxt::Joined(parent, local)) => {
                *shared = Some(SharedCtxt::Local(parent));
                Some(local)
            },
            None => None
        }
    }
}

impl LocalCtxt {
    fn serialize<S>(&self, serializer: &mut S, seen: &mut HashMap<&'static str, ()>) -> Result<(), S::Error>
    where
        S: SerializeMap,
    {
        for (k, v) in self.properties.iter() {
            if let Entry::Vacant(entry) = seen.entry(k) {
                serializer.serialize_entry(k, v)?;
                entry.insert(());
            }
        }

        if let Some(ref parent) = self.parent {
            parent.serialize(serializer, seen)?;
        }

        Ok(())
    }
}

impl Logger {
    fn scope<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        // Set the current shared log context
        // This makes the context available to other loggers on this thread
        // within the `scope` function
        // TODO: Handle re-entrancy
        LOCAL_CTXT.with(|shared| {
            struct SharedGuard<'a> {
                shared: Option<&'a RefCell<Option<SharedCtxt>>>,
                logger: &'a mut Logger,
            }

            impl<'a> Drop for SharedGuard<'a> {
                fn drop(&mut self) {
                    if let Some(shared) = self.shared.take() {
                        self.logger.ctxt = SharedCtxt::pop(&mut shared.borrow_mut());
                    }
                }
            }

            let guard = if let Some(ctxt) = self.ctxt.take() {
                SharedCtxt::push(&mut shared.borrow_mut(), ctxt);
                SharedGuard {
                    shared: Some(&shared),
                    logger: self,
                }
            }
            else {
                SharedGuard {
                    shared: None,
                    logger: self,
                }
            };

            let ret = f();

            drop(guard);

            ret
        })
    }

    pub fn log<'a, 'b>(&'b self, record: Record<'a>) -> Log<'a, 'b> {
        Log {
            record,
            _logger: self,
        }
    }

    #[cfg(test)]
    fn log_value<'a, 'b>(&'b self, record: Record<'a>) -> Value {
        serde_json::to_value(&self.log(record)).unwrap()
    }
}

impl Builder {
    fn into_logger(self) -> Logger {
        // Capture the current context
        // Each logger keeps a copy of the context it was created in so it can be shared
        // This context is set by other loggers calling `.scope()`
        // TODO: Should we just collect properties here from the context?
        //       Then we could avoid walking the context for every record logged
        //       Would we then need to worry about parents at all?
        let ctxt = if let Some(ctxt) = self.ctxt {
            LOCAL_CTXT.with(|shared| {
                Some(Arc::new(LocalCtxt {
                    parent: shared.borrow().to_owned(),
                    properties: ctxt.properties,
                }))
            })
        }
        else {
            None
        };

        Logger {
            ctxt,
        }
    }

    pub fn enrich<V>(mut self, k: &'static str, v: V) -> Self
    where
        V: Into<Value>
    {
        let ctxt = self.ctxt.get_or_insert_with(|| BuilderCtxt { properties: Default::default() });
        ctxt.properties.insert(k, v.into());

        self
    }

    pub fn scope<F>(self, f: F) -> LogFuture<F::Future>
    where
        F: IntoFuture,
    {
        let logger = self.into_logger();
        let fut = f.into_future();

        LogFuture {
            logger,
            inner: fut
        }
    }

    pub fn scope_fn<F, R>(self, f: F) -> LogFuture<Lazy<F, R>>
    where
        F: FnOnce() -> R,
        R: IntoFuture,
    {
        self.scope(lazy(f))
    }

    pub fn scope_sync<F, R>(self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        let mut logger = self.into_logger();

        logger.scope(f)
    }
    
    fn get(self) -> Logger {
        self.into_logger()
    }
}

impl<TFuture, TResult, TError> Future for LogFuture<TFuture>
where
    TFuture: Future<Item = TResult, Error = TError> + Send + 'static,
{
    type Item = TFuture::Item;
    type Error = TFuture::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let inner = &mut self.inner;
        let logger = &mut self.logger;

        logger.scope(|| inner.poll())
    }
}

#[cfg(test)]
mod tests {
    use std::thread;
    use log::{Level, RecordBuilder};
    use super::*;

    macro_rules! record {
        () => {
            RecordBuilder::new()
                .args(format_args!("Hi {}!", "user"))
                .level(Level::Info)
                .build()
        };
    }

    #[test]
    fn basic() {
        let logger = logger().get();

        let log = logger.log_value(record!());
        
        let expected = json!({
            "msg": "Hi user!"
        });

        assert_eq!(expected, log);
    }

    #[test]
    fn enriched() {
        let _: Result<_, ()> = logger()
            .enrich("correlation", "An Id")
            .enrich("service", "Banana")
            .scope_fn(|| {
                let log = logger().get().log_value(record!());

                let expected = json!({
                    "msg": "Hi user!",
                    "ctxt": {
                        "correlation": "An Id",
                        "service": "Banana"
                    }
                });

                assert_eq!(expected, log);

                Ok(())
            })
            .wait();
    }

    #[test]
    fn enriched_nested() {
        let _: Result<_, ()> = logger()
            .enrich("correlation", "An Id")
            .enrich("service", "Banana")
            .scope_fn(|| {
                let log_1 = logger()
                    .enrich("service", "Mandarin")
                    .scope_fn(|| {
                        let log = logger().get().log_value(record!());

                        let expected = json!({
                            "msg": "Hi user!",
                            "ctxt": {
                                "correlation": "An Id",
                                "service": "Mandarin"
                            }
                        });

                        assert_eq!(expected, log);

                        Ok(())
                    });

                let log_2 = logger()
                    .enrich("service", "Onion")
                    .scope_fn(|| {
                        let log = logger().get().log_value(record!());

                        let expected = json!({
                            "msg": "Hi user!",
                            "ctxt": {
                                "correlation": "An Id",
                                "service": "Onion"
                            }
                        });

                        assert_eq!(expected, log);

                        Ok(())
                    });

                log_1.and_then(|_| log_2)
            })
            .wait();
    }

    #[test]
    fn enriched_multiple_threads() {
        let f = logger()
            .enrich("correlation", "An Id")
            .enrich("service", "Banana")
            .scope_fn(|| {
                let log = logger().get().log_value(record!());

                let expected = json!({
                    "msg": "Hi user!",
                    "ctxt": {
                        "correlation": "An Id",
                        "context": "bg-thread",
                        "service": "Mandarin"
                    }
                });

                assert_eq!(expected, log);

                Ok(())
            });

        thread::spawn(move || {
            let _: Result<_, ()> = logger()
                .enrich("context", "bg-thread")
                .enrich("service", "Mandarin")
                .scope(f)
                .wait();
        })
        .join()
        .unwrap();
    }
}

#[cfg(test)]
mod benches {
    extern crate test;

    use log::{Level, RecordBuilder};

    use super::*;
    use self::test::{black_box, Bencher};

    macro_rules! record {
        () => {
            RecordBuilder::new()
                .args(format_args!("Hi {}!", "user"))
                .level(Level::Info)
                .build()
        };
    }

    #[bench]
    fn create_scope_empty(b: &mut Bencher) {
        b.iter(|| {
            logger().get()
        })
    }

    #[bench]
    fn serialize_log_empty(b: &mut Bencher) {
        b.iter(|| {
            logger().get().log_value(record!())
        });
    }

    #[bench]
    fn create_scope_1(b: &mut Bencher) {
        b.iter(|| {
            logger()
                .enrich("correlation", "An Id")
                .scope_sync(|| ())
        })
    }

    #[bench]
    fn create_scope_2(b: &mut Bencher) {
        b.iter(|| {
            logger()
                .enrich("correlation", "An Id")
                .enrich("environment", "Test")
                .scope_sync(|| ())
        })
    }

    #[bench]
    fn create_scope_3(b: &mut Bencher) {
        b.iter(|| {
            logger()
                .enrich("correlation", "An Id")
                .enrich("environment", "Test")
                .enrich("context", "Stuff")
                .scope_sync(|| ())
        })
    }

    #[bench]
    fn create_scope_3_numbers(b: &mut Bencher) {
        b.iter(|| {
            logger()
                .enrich("correlation", 1)
                .enrich("environment", 2)
                .enrich("context", 3)
                .scope_sync(|| ())
        })
    }

    #[bench]
    fn create_scope_4(b: &mut Bencher) {
        b.iter(|| {
            logger()
                .enrich("correlation", "An Id")
                .enrich("environment", "Test")
                .enrich("context", "Stuff")
                .enrich("more", "Even more")
                .scope_sync(|| ())
        })
    }

    #[bench]
    fn create_scope_4_numbers(b: &mut Bencher) {
        b.iter(|| {
            logger()
                .enrich("correlation", 1)
                .enrich("environment", 2)
                .enrich("context", 3)
                .enrich("more", 4)
                .scope_sync(|| ())
        })
    }

    #[bench]
    fn create_scope_1_nested(b: &mut Bencher) {
        logger()
            .enrich("correlation", "An Id")
            .scope_sync(|| {
                b.iter(|| {
                    logger()
                        .enrich("correlation", "An Id")
                        .scope_sync(|| ())
                });
            });
    }

    #[bench]
    fn serialize_log_1(b: &mut Bencher) {
        logger()
            .enrich("correlation", "An Id")
            .scope_sync(|| {
                b.iter(|| {
                    logger().get().log_value(record!())
                });
            });
    }
}