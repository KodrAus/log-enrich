/*!
Enriched logging.

This crate allows you to enrich log records within a scope with a collection of properties.
It's compatible with `log`.
*/

#![feature(nll, catch_expr, conservative_impl_trait)]
#![cfg_attr(test, feature(test))]

extern crate env_logger;
extern crate futures;
extern crate log as stdlog;
extern crate serde;
#[macro_use]
extern crate serde_derive;
#[cfg_attr(test, macro_use)]
extern crate serde_json;
extern crate take_mut;

mod ctxt;
mod log;
mod properties;

use std::sync::Arc;

use futures::{Future, IntoFuture, Lazy, Poll};
use futures::future::lazy;

use self::ctxt::{Ctxt, LocalCtxt, Scope, SharedCtxt};
use self::properties::Properties;

pub use serde_json::Value;
pub use self::log::Log;

pub fn init() {
    env_logger::Builder::from_env(env_logger::Env::default())
        .format(log::format())
        .init();
}

pub fn logger() -> Builder {
    Builder { ctxt: None }
}

struct BuilderCtxt {
    properties: Properties,
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

impl Builder {
    /// Create a `Logger` with the built context.
    ///
    /// Creating loggers with no context of their own is cheap.
    fn into_logger(self) -> Logger {
        // Capture the current context
        // Each logger keeps a copy of the context it was created in so it can be shared
        // This context is set by other loggers calling `.scope()`
        let ctxt = if let Some(ctxt) = self.ctxt {
            SharedCtxt::scope_current(|mut scope| {
                Some(Arc::new(Ctxt::from_scope(ctxt.properties, &mut scope)))
            })
        } else {
            None
        };

        Logger {
            ctxt: ctxt.map(|local| LocalCtxt::new(local)),
        }
    }

    /// Set a property on the logger.
    ///
    /// When there is a logger with enriched properties, any records logged within the
    /// logger's `scope` will include those properties.
    pub fn enrich<V>(mut self, k: &'static str, v: V) -> Self
    where
        V: Into<Value>,
    {
        let ctxt = self.ctxt.get_or_insert_with(|| BuilderCtxt {
            properties: Default::default(),
        });
        ctxt.properties_mut().insert(k, v.into());

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

        LogFuture { logger, inner: fut }
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

impl BuilderCtxt {
    pub(crate) fn properties_mut(&mut self) -> &mut Properties {
        &mut self.properties
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

impl Logger {
    fn scope<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(Scope) -> R,
    {
        // Set the current shared log context
        // This makes the context available to other loggers on this thread
        // within the `scope` function
        if let Some(ref mut ctxt) = self.ctxt {
            SharedCtxt::scope(ctxt, f)
        }
        else {
            SharedCtxt::scope_current(f)
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate test;

    use std::thread;

    use self::test::Bencher;
    use futures::future;
    use stdlog::{Level, Record, RecordBuilder};
    use super::*;

    impl<'a> Scope<'a> {
        fn log<'b, 'c>(&'b mut self, record: Record<'c>) -> Log<'b, 'c>
        where
            'a: 'b,
        {
            Log::new(self.current(), record)
        }

        fn log_value<'b>(&mut self, record: Record<'b>) -> Value {
            serde_json::to_value(&self.log(record)).unwrap()
        }

        fn log_string<'b>(&mut self, record: Record<'b>) -> String {
            serde_json::to_string(&self.log(record)).unwrap()
        }
    }

    macro_rules! record {
        () => {
            RecordBuilder::new()
                .args(format_args!("Hi {}!", "user"))
                .level(Level::Info)
                .build()
        };
    }

    fn log_value() -> Value {
        logger().get().scope(|mut ctxt| ctxt.log_value(record!()))
    }

    fn log_string() -> String {
        logger().get().scope(|mut ctxt| ctxt.log_string(record!()))
    }

    fn assert_log(expected: Value) {
        for _ in 0..5 {
            let log = log_value();
            assert_eq!(expected, log);
        }
    }

    #[test]
    fn basic() {
        let log = log_value();

        let expected = json!({
            "msg": "Hi user!",
            "ctxt": Value::Null
        });

        assert_eq!(expected, log);
    }

    #[test]
    fn enriched_empty() {
        let _: Result<_, ()> = logger()
            .scope_fn(|| {
                assert_log(json!({
                    "msg": "Hi user!",
                    "ctxt": Value::Null
                }));

                Ok(())
            })
            .wait();
    }

    #[test]
    fn enriched_basic() {
        let _: Result<_, ()> = logger()
            .enrich("correlation", "An Id")
            .enrich("service", "Banana")
            .scope_fn(|| {
                assert_log(json!({
                    "msg": "Hi user!",
                    "ctxt": {
                        "correlation": "An Id",
                        "service": "Banana"
                    }
                }));

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
                let log_1 = logger().enrich("service", "Mandarin").scope_fn(|| {
                    assert_log(json!({
                        "msg": "Hi user!",
                        "ctxt": {
                            "correlation": "An Id",
                            "service": "Mandarin"
                        }
                    }));

                    Ok(())
                });

                let log_2 = logger().enrich("service", "Onion").scope_fn(|| {
                    assert_log(json!({
                        "msg": "Hi user!",
                        "ctxt": {
                            "correlation": "An Id",
                            "service": "Onion"
                        }
                    }));

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
            .scope(logger().enrich("correlation", "Another Id").scope_fn(|| {
                assert_log(json!({
                    "msg": "Hi user!",
                    "ctxt": {
                        "correlation": "Another Id",
                        "context": "bg-thread",
                        "operation": "Logging",
                        "service": "Banana"
                    }
                }));

                Ok(())
            }));

        thread::spawn(move || {
            let _: Result<_, ()> = logger()
                .enrich("context", "bg-thread")
                .enrich("service", "Mandarin")
                .scope(f)
                .wait();
        }).join()
            .unwrap();
    }

    #[bench]
    fn create_scope_empty(b: &mut Bencher) {
        b.iter(|| logger().get())
    }

    #[bench]
    fn poll_empty(b: &mut Bencher) {
        let mut f = logger().scope(future::empty::<(), ()>());

        b.iter(|| f.poll())
    }

    #[bench]
    fn serialize_log_empty(b: &mut Bencher) {
        b.iter(|| log_string());
    }

    #[bench]
    fn serialize_log_1(b: &mut Bencher) {
        logger().enrich("correlation", "An Id").scope_sync(|| {
            b.iter(|| log_string());
        });
    }

    #[bench]
    fn create_scope_1(b: &mut Bencher) {
        b.iter(|| logger().enrich("correlation", "An Id").scope_sync(|| ()))
    }

    #[bench]
    fn poll_scope_1(b: &mut Bencher) {
        let mut f = logger()
            .enrich("correlation", "An Id")
            .scope(future::empty::<(), ()>());

        b.iter(|| f.poll())
    }

    #[bench]
    fn create_scope_1_nested(b: &mut Bencher) {
        logger().enrich("correlation", "An Id").scope_sync(|| {
            b.iter(|| logger().enrich("correlation", "An Id").scope_sync(|| ()));
        });
    }

    #[bench]
    fn poll_scope_1_nested(b: &mut Bencher) {
        let mut f = logger().enrich("correlation", "An Id").scope(
            logger()
                .enrich("correlation", "An Id")
                .scope(future::empty::<(), ()>()),
        );

        b.iter(|| f.poll())
    }

    #[bench]
    fn create_scope_1_nested_2(b: &mut Bencher) {
        logger().enrich("correlation", "An Id").scope_sync(|| {
            logger().enrich("correlation", "An Id").scope_sync(|| {
                b.iter(|| logger().enrich("correlation", "An Id").scope_sync(|| ()));
            });
        });
    }
}
