#![feature(proc_macro, conservative_impl_trait, generators)]

extern crate futures_await as futures;
extern crate futures_cpupool as cpupool;
#[macro_use]
extern crate log;
extern crate log_enrich;

use futures::prelude::*;
use cpupool::CpuPool;
use log_enrich::future::logger;

fn ok() -> Result<(), ()> {
    Ok(())
}

fn main() {
    log_enrich::init();

    let pool = CpuPool::new(1);

    let f = logger().enrich("service", "basic.rs").scope(async_block! {
        info!("starting up");

        await!({
            logger()
                .enrich("correlation", "Some Id")
                .enrich("operation", "request")
                .scope(async_block! {
                    info!("handling a request for {}", "Timmy");

                    await!({
                        logger().enrich("operation", "database").scope(async_block! {
                            info!("doing database stuff");

                            await!({
                                pool.spawn(logger().scope(async_block! {
                                    info!("working on a background thread");

                                    ok()
                                }))
                            });

                            ok()
                        })
                    });

                    ok()
                })
        });

        info!("finishing up");

        ok()
    });

    f.wait().unwrap();
}
