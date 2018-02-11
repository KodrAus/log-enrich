/*!
Enriched logging.

This crate allows you to enrich log records within a scope with a collection of properties.
It's compatible with `log`.
*/

#![feature(nll, catch_expr)]
#![cfg_attr(test, feature(test))]

extern crate futures;
#[cfg_attr(test, macro_use)]
extern crate serde_json;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate log;
extern crate env_logger;
extern crate take_mut;

use std::sync::Arc;
use std::ops::Drop;
use std::mem;
use std::iter::Extend;
use std::io::Write;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::collections::hash_map;

use log::Record;
use futures::{Future, IntoFuture, Poll, Lazy};
use futures::future::lazy;
use serde::ser::{Serialize, Serializer, SerializeMap};

pub use serde_json::Value;

thread_local!(static SHARED_CTXT: RefCell<Option<SharedCtxt>> = RefCell::new(Default::default()));

pub fn init() {
    env_logger::Builder::from_env(env_logger::Env::default())
        .format(|buf, record| {
            logger().get().scope(|scope| {
                do catch {
                    write!(buf, "{}: {}: {}", record.level(), buf.timestamp(), record.args())?;

                    if let Some(ctxt) = scope.ctxt {
                        let mut value_style = buf.style();
                            
                        value_style
                            .set_bold(true);

                        write!(buf, ": (")?;

                        let mut first = true;
                        for (k, v) in &ctxt.properties {
                            if first {
                                first = false;
                                write!(buf, "{}: {}", k, value_style.value(v))?;
                            }
                            else {
                                write!(buf, ", {}: {}", k, value_style.value(v))?;
                            }
                        }

                        write!(buf, ")")?;
                    }

                    writeln!(buf)?;

                    Ok(())
                }
            })
        })
        .init();
}

pub fn logger() -> Builder {
    Builder {
        ctxt: None,
    }
}

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
    Map(hash_map::Iter<'a, &'static str, Value>),
}

pub struct Builder {
    ctxt: Option<BuilderCtxt>,
}

pub struct Logger {
    ctxt: Option<LocalCtxt>,
}

pub struct LogFuture<TFuture> {
    logger: Logger,
    inner: TFuture,
}

pub struct Log<'a, 'b> {
    scope: Scope<'a>,
    record: Record<'b>,
}

#[derive(Clone, Default, Debug)]
struct Ctxt {
    parent: Option<Arc<Ctxt>>,
    properties: Properties,
}

#[derive(Debug)]
enum LocalCtxt {
     Local {
        local: Arc<Ctxt>,
    },
    Joined {
        original: Arc<Ctxt>,
        joined: Arc<Ctxt>,
    },
}

#[derive(Clone, Debug)]
struct SharedCtxt {
    inner: Arc<Ctxt>,
}

#[derive(Clone, Copy)]
struct Scope<'a> {
    ctxt: Option<&'a Ctxt>,
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

    fn contains_key(&self, key: &'static str) -> bool {
        match *self {
            Properties::Single(k, _) if k == key => true,
            Properties::Map(ref m) => m.contains_key(key),
            _ => false,
        }
    }

    fn iter(&self) -> PropertiesIter {
        self.into_iter()
    }

    fn len(&self) -> usize {
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
        T: IntoIterator<Item = (&'static str, &'a Value)>
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

impl<'a, 'b> Serialize for Log<'a, 'b> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct SerializeLog<'a, 'b> {
            #[serde(serialize_with = "serialize_msg")]
            msg: &'a Record<'a>,
            #[serde(serialize_with = "serialize_scope")]
            scope: Scope<'b>,
        }

        fn serialize_scope<S>(scope: &Scope, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            if let Some(ref ctxt) = scope.ctxt {
                let properties = &ctxt.properties;

                let mut map = serializer.serialize_map(Some(properties.len()))?;
                
                for (k, v) in properties.iter() {
                    map.serialize_entry(k, v)?;
                }
                
                map.end()
            }
            else {
                serializer.serialize_none()
            }
        }

        fn serialize_msg<'a, S>(record: &Record<'a>, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serializer.collect_str(record.args())
        }

        let log = SerializeLog {
            msg: &self.record,
            scope: self.scope,
        };

        log.serialize(serializer)
    }
}

impl LocalCtxt {
    fn clear_joined(&mut self) {
        take_mut::take(self, |ctxt| {
            if let LocalCtxt::Joined { original, .. } = ctxt {
                LocalCtxt::Local { local: original }
            }
            else {
                ctxt
            }
        })
    }

    fn set_joined(&mut self, joined: Arc<Ctxt>) {
        take_mut::take(self, |ctxt| {
            match ctxt {
                LocalCtxt::Local { local } => {
                    LocalCtxt::Joined { original: local, joined }
                }
                LocalCtxt::Joined { original, .. } => {
                    LocalCtxt::Joined { original, joined }
                }
            }
        });
    }

    fn current(&self) -> &Arc<Ctxt> {
        match *self {
            LocalCtxt::Local { ref local } => local,
            LocalCtxt::Joined { ref joined, .. } => joined,
        }
    }
}

impl SharedCtxt {
    fn current(&self) -> &Arc<Ctxt> {
        &self.inner
    }

    fn push(shared: &mut Option<SharedCtxt>, logger: Option<&mut LocalCtxt>) {
        if let Some(incoming_ctxt) = logger {
            // Check whether there's already an active context
            if let Some(ref mut shared_ctxt) = *shared {
                // If we have a joined context, check it first
                // If the shared context is invalid, then we might recreate it
                if let LocalCtxt::Joined { ref joined, .. } = *incoming_ctxt {
                    if let Some(ref parent) = joined.parent {
                        if Arc::ptr_eq(&shared_ctxt.current(), parent) {
                            shared_ctxt.inner = joined.clone();
                            return;
                        }
                    }

                    incoming_ctxt.clear_joined();
                }

                // Check the parent of the original context
                if let LocalCtxt::Local { ref local, .. } = *incoming_ctxt {
                    if let Some(ref parent) = local.parent {
                        if Arc::ptr_eq(&shared_ctxt.current(), parent) {
                            shared_ctxt.inner = local.clone();
                            return;
                        }
                    }

                    // If the original context isn't a child of the current one, create
                    // a new joined context that combines them.
                    let joined = Arc::new(Ctxt::from_shared(local.properties.clone(), Some(&shared_ctxt)));
                    incoming_ctxt.set_joined(joined);

                    shared_ctxt.inner = incoming_ctxt.current().clone();
                    return;
                }

                unreachable!();
            }
            else {
                // Make sure the joined context is `None`
                // If this context is the root of this thread then there's no need for it
                incoming_ctxt.clear_joined();

                *shared = Some(SharedCtxt {
                    inner: incoming_ctxt.current().clone()
                });
            }
        }
    }

    fn pop(shared: &mut Option<SharedCtxt>, logger: Option<&LocalCtxt>) {
        if logger.is_some() {
            *shared = shared.take()
                .and_then(|shared| shared.current().parent.clone())
                .map(|local| SharedCtxt { inner: local });
        }
    }
}

impl Ctxt {
    /// Create a local context from a set of properties and a shared context.
    /// 
    /// If the shared context is `Some`, then the local context will contain the union
    /// of `properties` and the properties on the shared context.
    fn from_shared(mut properties: Properties, shared: Option<&SharedCtxt>) -> Self {
        properties
            .extend(shared
                .as_ref()
                .map(|shared| &shared.current().properties)
                .unwrap_or(&Properties::Empty));

        Ctxt {
            parent: shared.as_ref().map(|shared| shared.current().clone()),
            properties,
        }
    }
}

impl Logger {
    fn scope<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(Scope) -> R,
    {
        // Set the current shared log context
        // This makes the context available to other loggers on this thread
        // within the `scope` function
        // TODO: Handle re-entrancy
        SHARED_CTXT.with(|shared| {
            struct SharedGuard<'a> {
                shared: Option<&'a RefCell<Option<SharedCtxt>>>,
                logger: Option<&'a LocalCtxt>,
            }

            impl<'a> Drop for SharedGuard<'a> {
                fn drop(&mut self) {
                    if let Some(shared) = self.shared.take() {
                        SharedCtxt::pop(&mut shared.borrow_mut(), self.logger);
                    }
                }
            }

            let guard = {
                SharedCtxt::push(&mut shared.borrow_mut(), self.ctxt.as_mut());
                SharedGuard {
                    shared: Some(&shared),
                    logger: self.ctxt.as_ref(),
                }
            };

            let current = {
                shared.borrow().as_ref().map(|shared| shared.current().clone())
            };

            let ret = f(Scope {
                ctxt: current.as_ref().map(|current| current.as_ref())
            });

            drop(guard);

            ret
        })
    }
}

impl<'a> Scope<'a> {
    fn log<'b>(self, record: Record<'b>) -> Log<'a, 'b> {
        Log {
            record,
            scope: self,
        }
    }

    fn log_value<'b>(&self, record: Record<'b>) -> Value {
        serde_json::to_value(&self.log(record)).unwrap()
    }

    fn log_string<'b>(&self, record: Record<'b>) -> String {
        serde_json::to_string(&self.log(record)).unwrap()
    }
}

impl Builder {
    /// Create a `Logger` with the built context.
    /// 
    /// Creating loggers with no context of their own is cheap.
    fn into_logger(self) -> Logger {
        // Capture the current context
        // Each logger keeps a copy of the context it was created in so it can be shared
        // This context is set by other loggers calling `.scope()`
        let ctxt = if let Some(ctxt) = self.ctxt {
            SHARED_CTXT.with(|shared| {
                let shared = shared.borrow();

                Some(Arc::new(Ctxt::from_shared(ctxt.properties, shared.as_ref())))
            })
        }
        else {
            None
        };

        Logger {
            ctxt: ctxt.map(|local| LocalCtxt::Local { local })
        }
    }

    /// Set a property on the logger.
    /// 
    /// When there is a logger with enriched properties, any records logged within the
    /// logger's `scope` will include those properties.
    pub fn enrich<V>(mut self, k: &'static str, v: V) -> Self
    where
        V: Into<Value>
    {
        let ctxt = self.ctxt.get_or_insert_with(|| BuilderCtxt { properties: Default::default() });
        ctxt.properties.insert(k, v.into());

        self
    }

    /// Enrich records logged within the scope using the properties configured.
    /// 
    /// This method returns a future.
    /// Any record logged within that future will include the properties added using `enrich`.
    /// 
    /// **NOTE:** Properties are stored in thread-local storage.
    /// If this future is sent to another thread it will carry its properties with it,
    /// but if a thread is spawned without sending a scope then it won't carry properties.
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

        logger.scope(|_| f())
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

        logger.scope(|_| inner.poll())
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
        let log = logger().get().scope(|ctxt| ctxt.log_value(record!()));

        let expected = json!({
            "msg": "Hi user!",
            "scope": Value::Null
        });

        assert_eq!(expected, log);
    }

    #[test]
    fn enriched_empty() {
        let _: Result<_, ()> = logger()
            .scope_fn(|| {
                let log = logger().get().scope(|ctxt| ctxt.log_value(record!()));

                let expected = json!({
                    "msg": "Hi user!",
                    "scope": Value::Null
                });

                assert_eq!(expected, log);

                Ok(())
            })
            .wait();
    }

    #[test]
    fn enriched() {
        let _: Result<_, ()> = logger()
            .enrich("correlation", "An Id")
            .enrich("service", "Banana")
            .scope_fn(|| {
                let log = logger().get().scope(|ctxt| ctxt.log_value(record!()));

                let expected = json!({
                    "msg": "Hi user!",
                    "scope": {
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
                        let log = logger().get().scope(|ctxt| ctxt.log_value(record!()));

                        let expected = json!({
                            "msg": "Hi user!",
                            "scope": {
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
                        let log = logger().get().scope(|ctxt| ctxt.log_value(record!()));

                        let expected = json!({
                            "msg": "Hi user!",
                            "scope": {
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
            .enrich("operation", "Logging")
            .enrich("service", "Banana")
            .scope(
                logger()
                    .enrich("correlation", "Another Id")
                    .scope_fn(|| {
                        let log = logger().get().scope(|ctxt| ctxt.log_value(record!()));

                        let expected = json!({
                            "msg": "Hi user!",
                            "scope": {
                                "correlation": "Another Id",
                                "context": "bg-thread",
                                "operation": "Logging",
                                "service": "Banana"
                            }
                        });

                        assert_eq!(expected, log);

                        Ok(())
                    }));

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

    use std::rc::Rc;
    use futures::future;
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
    fn clone_rc_ctxt(b: &mut Bencher) {
        let ctxt = Rc::new(Ctxt { parent: None, properties: Properties::Empty });

        b.iter(|| {
            ctxt.clone();
        })
    }

    #[bench]
    fn clone_arc_ctxt(b: &mut Bencher) {
        let ctxt = Arc::new(Ctxt { parent: None, properties: Properties::Empty });

        b.iter(|| {
            ctxt.clone();
        })
    }

    #[bench]
    fn create_scope_empty(b: &mut Bencher) {
        b.iter(|| {
            logger().get()
        })
    }

    #[bench]
    fn poll_empty(b: &mut Bencher) {
        let mut f = logger().scope(future::empty::<(), ()>());

        b.iter(|| {
            f.poll()
        })
    }

    #[bench]
    fn serialize_log_empty(b: &mut Bencher) {
        b.iter(|| {
            logger().get().scope(|ctxt| ctxt.log_string(record!()));
        });
    }

    #[bench]
    fn serialize_log_1(b: &mut Bencher) {
        logger()
            .enrich("correlation", "An Id")
            .scope_sync(|| {
                b.iter(|| {
                    logger().get().scope(|ctxt| ctxt.log_string(record!()));
                });
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
    fn poll_scope_1(b: &mut Bencher) {
        let mut f = logger()
            .enrich("correlation", "An Id")
            .scope(future::empty::<(), ()>());

        b.iter(|| {
            f.poll()
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
    fn poll_scope_1_nested(b: &mut Bencher) {
        let mut f = logger()
            .enrich("correlation", "An Id")
            .scope(logger()
                .enrich("correlation", "An Id")
                .scope(future::empty::<(), ()>()));

        b.iter(|| {
            f.poll()
        })
    }

    #[bench]
    fn create_scope_1_nested_2(b: &mut Bencher) {
        logger()
            .enrich("correlation", "An Id")
            .scope_sync(|| {
                logger()
                    .enrich("correlation", "An Id")
                    .scope_sync(|| {
                        b.iter(|| {
                            logger()
                                .enrich("correlation", "An Id")
                                .scope_sync(|| ())
                        });
                    });
            });
    }
}