/*!
Enriched logging.

This crate allows you to enrich log records within a scope with a collection of properties.
It's compatible with `log`.

- Call `sync::logger()` to create a scope.
- Call `future::logger()` to create a scope that works with `futures`.
*/

#![feature(nll, catch_expr)]
#![cfg_attr(test, feature(test))]

extern crate futures;
extern crate log as stdlog;
extern crate serde;
#[macro_use]
extern crate serde_derive;
#[cfg_attr(test, macro_use)]
extern crate serde_json;
extern crate take_mut;

mod log;
mod ctxt;
mod properties;

use std::sync::Arc;

use self::ctxt::{Ctxt, LocalCtxt, Scope, SharedCtxt};
use self::properties::Properties;

pub use serde_json::Value;

pub struct Enriched<L> {
    inner: L,
}

impl<L> stdlog::Log for Enriched<L> where L: stdlog::Log {
    fn enabled(&self, metadata: &stdlog::Metadata) -> bool {
        self.inner.enabled(metadata)
    }

    fn log(&self, record: &stdlog::Record) {
        current_logger().scope(|scope| {
            if let Some(ctxt) = scope.current() {
                self.inner.log(&record.push(&ctxt.properties()));
            }
            else {
                self.inner.log(record);
            }
        })
    }

    fn flush(&self) {
        self.inner.flush()
    }
}

/**
Initialize a console logger that can read enriched properties.
*/
pub fn init<L>(inner: L, max_level: stdlog::LevelFilter) where L: stdlog::Log + 'static {
    stdlog::set_max_level(max_level);
    stdlog::set_boxed_logger(Box::new(Enriched { inner })).expect("failed to set logger");
}

fn current_logger() -> Logger {
    BuilderInner::default().get()
}

#[derive(Default)]
struct BuilderCtxt {
    properties: Properties,
}

#[derive(Default)]
struct BuilderInner {
    ctxt: Option<BuilderCtxt>,
}

struct Logger {
    ctxt: Option<LocalCtxt>,
}

impl BuilderInner {
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

    fn enrich<V>(mut self, k: &'static str, v: V) -> Self
    where
        V: Into<Value>,
    {
        let ctxt = self.ctxt.get_or_insert_with(|| BuilderCtxt {
            properties: Default::default(),
        });
        ctxt.properties_mut().insert(k, v.into());

        self
    }

    fn get(self) -> Logger {
        self.into_logger()
    }
}

pub mod future {
    /*!
    Property enrichment for futures.

    Scopes created by loggers in this module are _futures_.
    They maintain their context wherever the future is executed, including other threads.

    # Examples

    ```
    # #[macro_use] extern crate log;
    # extern crate futures;
    # extern crate log_enrich;
    # fn main() {
    # fn send_request() -> Box<futures::Future<Item = (), Error = ()>> { unimplemented!() }
    # fn read_response(r: ()) -> Box<futures::Future<Item = (), Error = ()>> { unimplemented!() }
    use futures::Future;
    use log_enrich::future;
    
    let scope = future::logger()
        .enrich("correlation_id", "abc")
        .enrich("operation", "docs example")
        .scope_fn(|| {
            info!("sending request");

            send_request().and_then(|sent| {
                info!("reading response");

                read_response(sent)
            })
        });
    # }
    ```

    Will output:

    ```text
    INFO: 2018-02-12T06:31:58Z: sending request: (correlation_id: "abc", operation: "docs example")
    INFO: 2018-02-12T06:31:58Z: reading response: (correlation_id: "abc", operation: "docs example")
    ```
    */

    use super::{BuilderCtxt, BuilderInner, Logger};

    use futures::{Future, IntoFuture, Poll};
    use futures::future::{lazy, Lazy};
    use serde_json::Value;

    /**
    A builder for a logging scope.

    Call `.enrich` to add properties and `.scope` to create a scope containing the enriched properties.
    */
    pub fn logger() -> Builder {
        // Ensure that user created scopes always have properties
        // This ensures they include properties when sent to other threads
        Builder {
            inner: BuilderInner {
                ctxt: Some(BuilderCtxt::default()),
            },
        }
    }

    /**
    A builder for a logging scope.

    Call `.enrich` to add properties and `.scope` to create a scope containing the enriched properties.
    */
    pub struct Builder {
        inner: BuilderInner,
    }

    /**
    A future returned by calling `scope` on a `Builder`.
    */
    pub struct ScopeFuture<TFuture> {
        logger: Logger,
        inner: TFuture,
    }

    impl Builder {
        /**
        Set a property on this logger.

        If this logger is inside another scope, and that scope has a property with the same name, then the previous value will be overriden.
        */
        pub fn enrich<V>(mut self, k: &'static str, v: V) -> Self
        where
            V: Into<Value>,
        {
            self.inner = self.inner.enrich(k, v);
            self
        }

        /**
        Create a scope where the enriched properties will be logged.

        Scopes are stacked, so if this logger is inside another scope, the scope created here will contain all of its properties too.
        The returned `ScopeFuture` will retain all of these properties, even if it's sent across threads.
        
        **NOTE:** Enriched properties aren't visible on threads spawned within a scope unless a child scope is sent to them.
        */
        pub fn scope<F>(self, f: F) -> ScopeFuture<F::Future>
        where
            F: IntoFuture,
        {
            let logger = self.inner.into_logger();
            let fut = f.into_future();

            ScopeFuture { logger, inner: fut }
        }

        /**
        Create a scope wghere the enriched properties will be logged.

        This is the same as calling `.scope(lazy(f))`.
        */
        pub fn scope_fn<F, R>(self, f: F) -> ScopeFuture<Lazy<F, R>>
        where
            F: FnOnce() -> R,
            R: IntoFuture,
        {
            self.scope(lazy(f))
        }
    }

    impl<TFuture, TResult, TError> Future for ScopeFuture<TFuture>
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
}

pub mod sync {
    /*!
    Property enrichment for futures.

    Scopes created by loggers in this module are _futures_.
    They maintain their context wherever the future is executed, including other threads.

    # Examples

    ```
    # #![feature(catch_expr)]
    # #[macro_use] extern crate log;
    # extern crate log_enrich;
    # fn main() {
    # fn send_request() -> Result<(), ()> { Ok(()) }
    # fn read_response(r: ()) -> Result<(), ()> { Ok(()) }
    use log_enrich::sync;
    
    sync::logger()
        .enrich("correlation_id", "abc")
        .enrich("operation", "docs example")
        .scope(|| do catch {
            info!("sending request");
            let sent = send_request()?;

            info!("reading response");
            read_response(sent)
        });
    # }
    ```

    Will output:

    ```text
    INFO: 2018-02-12T06:31:58Z: sending request: (correlation_id: "abc", operation: "docs example")
    INFO: 2018-02-12T06:31:58Z: reading response: (correlation_id: "abc", operation: "docs example")
    ```
    */

    use super::BuilderInner;

    use serde_json::Value;

    /**
    A builder for a logging scope.

    Call `.enrich` to add properties and `.scope` to create a scope containing the enriched properties.
    */
    pub fn logger() -> Builder {
        Builder {
            inner: Default::default(),
        }
    }

    /**
    A builder for a logging scope.

    Call `.enrich` to add properties and `.scope` to create a scope containing the enriched properties.
    */
    pub struct Builder {
        inner: BuilderInner,
    }

    impl Builder {
        /**
        Set a property on this logger.

        If this logger is inside another scope, and that scope has a property with the same name, then the previous value will be overriden.
        */
        pub fn enrich<V>(mut self, k: &'static str, v: V) -> Self
        where
            V: Into<Value>,
        {
            self.inner = self.inner.enrich(k, v);
            self
        }

        /**
        Create a scope where the enriched properties will be logged.

        Scopes are stacked, so if this logger is inside another scope, the scope created here will contain all of its properties too.
        
        **NOTE:** Enriched properties aren't visible on threads spawned within a scope unless a child `ScopeFuture` is sent to them.
        */
        pub fn scope<F, R>(self, f: F) -> R
        where
            F: FnOnce() -> R,
        {
            let mut logger = self.inner.into_logger();

            logger.scope(|_| f())
        }
    }
}

impl Logger {
    fn scope<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut Scope) -> R,
    {
        // Set the current shared log context
        // This makes the context available to other loggers on this thread
        // within the `scope` function
        if let Some(ref mut ctxt) = self.ctxt {
            SharedCtxt::scope(ctxt, f)
        } else {
            SharedCtxt::scope_current(f)
        }
    }
}

impl BuilderCtxt {
    fn properties_mut(&mut self) -> &mut Properties {
        &mut self.properties
    }
}

#[cfg(test)]
mod tests {
    extern crate test;

    use std::thread;
    use std::panic;

    use self::test::Bencher;
    use futures::Future;
    use futures::future::empty;
    use stdlog::{Level, Record, RecordBuilder};
    use super::*;
    use log::Log;

    impl Scope {
        fn log<'b, 'c>(&'b mut self, record: Record<'c>) -> Log<'b, 'c> {
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
        current_logger().scope(|ctxt| ctxt.log_value(record!()))
    }

    fn log_string() -> String {
        current_logger().scope(|ctxt| ctxt.log_string(record!()))
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
        sync::logger().scope(|| {
            assert_log(json!({
                    "msg": "Hi user!",
                    "ctxt": Value::Null
                }));
        });
    }

    #[test]
    fn enriched_basic() {
        sync::logger()
            .enrich("correlation", "An Id")
            .enrich("service", "Banana")
            .scope(|| {
                assert_log(json!({
                    "msg": "Hi user!",
                    "ctxt": {
                        "correlation": "An Id",
                        "service": "Banana"
                    }
                }));
            });
    }

    #[test]
    fn enriched_nested() {
        sync::logger()
            .enrich("correlation", "An Id")
            .enrich("service", "Banana")
            .scope(|| {
                sync::logger().enrich("service", "Mandarin").scope(|| {
                    assert_log(json!({
                        "msg": "Hi user!",
                        "ctxt": {
                            "correlation": "An Id",
                            "service": "Mandarin"
                        }
                    }));
                });

                sync::logger().enrich("service", "Onion").scope(|| {
                    assert_log(json!({
                        "msg": "Hi user!",
                        "ctxt": {
                            "correlation": "An Id",
                            "service": "Onion"
                        }
                    }));
                });
            });
    }

    #[test]
    fn enriched_panic() {
        sync::logger()
            .enrich("correlation", "An Id")
            .enrich("service", "Banana")
            .scope(|| {
                let _ = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                    sync::logger().enrich("service", "Mandarin").scope(|| {
                        sync::logger().enrich("service", "Onion").scope(|| {
                            assert_log(json!({
                                "msg": "Hi user!",
                                "ctxt": {
                                    "correlation": "An Id",
                                    "service": "Onion"
                                }
                            }));

                            panic!("panic to catch_unwind");
                        });
                    });
                }));

                assert_log(json!({
                    "msg": "Hi user!",
                    "ctxt": {
                        "correlation": "An Id",
                        "service": "Banana"
                    }
                }));
            });
    }

    #[test]
    fn enriched_multiple_threads() {
        let f = future::logger()
            .enrich("correlation", "An Id")
            .enrich("operation", "Logging")
            .enrich("service", "Banana")
            .scope(
                future::logger()
                    .enrich("correlation", "Another Id")
                    .scope_fn(|| {
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
                    }),
            );

        thread::spawn(move || {
            let _: Result<_, ()> = future::logger()
                .enrich("context", "bg-thread")
                .enrich("service", "Mandarin")
                .scope(f)
                .wait();
        }).join()
            .unwrap();
    }

    #[bench]
    fn create_scope_empty(b: &mut Bencher) {
        b.iter(|| current_logger())
    }

    #[bench]
    fn poll_empty(b: &mut Bencher) {
        let mut f = future::logger().scope(empty::<(), ()>());

        b.iter(|| f.poll())
    }

    #[bench]
    fn serialize_log_empty(b: &mut Bencher) {
        b.iter(|| log_string());
    }

    #[bench]
    fn serialize_log_1(b: &mut Bencher) {
        sync::logger().enrich("correlation", "An Id").scope(|| {
            b.iter(|| log_string());
        });
    }

    #[bench]
    fn create_scope_1(b: &mut Bencher) {
        b.iter(|| sync::logger().enrich("correlation", "An Id").scope(|| ()))
    }

    #[bench]
    fn poll_scope_1(b: &mut Bencher) {
        let mut f = future::logger()
            .enrich("correlation", "An Id")
            .scope(empty::<(), ()>());

        b.iter(|| f.poll())
    }

    #[bench]
    fn create_scope_1_nested(b: &mut Bencher) {
        sync::logger().enrich("correlation", "An Id").scope(|| {
            b.iter(|| sync::logger().enrich("correlation", "An Id").scope(|| ()));
        });
    }

    #[bench]
    fn poll_scope_1_nested(b: &mut Bencher) {
        let mut f = future::logger().enrich("correlation", "An Id").scope(
            future::logger()
                .enrich("correlation", "An Id")
                .scope(empty::<(), ()>()),
        );

        b.iter(|| f.poll())
    }

    #[bench]
    fn create_scope_1_nested_2(b: &mut Bencher) {
        sync::logger().enrich("correlation", "An Id").scope(|| {
            sync::logger().enrich("correlation", "An Id").scope(|| {
                b.iter(|| sync::logger().enrich("correlation", "An Id").scope(|| ()));
            });
        });
    }
}
