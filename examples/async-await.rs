/*!
Try setting the `RUST_LOG` environment variable to `info` and run this example.
*/

#![feature(proc_macro, conservative_impl_trait, generators)]

extern crate futures_await as futures;
extern crate futures_cpupool as cpupool;
#[macro_use]
extern crate log;
extern crate log_enrich;
extern crate env_logger;
#[macro_use]
extern crate serde_json;

use futures::prelude::*;
use cpupool::CpuPool;
use log_enrich::future::logger;

fn ok() -> Result<(), ()> {
    Ok(())
}

fn main() {
    let stdlog = env_logger::Builder::from_env("MY_LOG").build();
    let max_level = stdlog.filter();
    log_enrich::init(stdlog, max_level);

    let pool = CpuPool::new(1);

    let f = logger().enrich("service", "basic.rs").scope(async_block! {
        info!("starting up");

        let result = await!({
            logger()
                .enrich("correlation", "Some Id")
                .enrich("operation", "request")
                .enrich("data", json!({ "id": 1, "username": "Timmy" }))
                .scope(async_block! {
                    info!("handling a request for {}", "Timmy");

                    await!({
                        logger().enrich("operation", "database").scope(async_block! {
                            info!("doing database stuff");

                            // Sending a scope to another thread keeps the enriched properties
                            await!({
                                pool.spawn(logger().scope(async_block! {
                                    info!("working on a background thread");

                                    ok()
                                }))
                            })
                        })
                    })
                })
        });

        info!("finishing up");

        result
    });

    f.wait().unwrap();
}
