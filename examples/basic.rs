/*!
Try setting the `RUST_LOG` environment variable to `info` and run this example.
*/

#[macro_use]
extern crate log;
extern crate log_enrich;
extern crate env_logger;

use log_enrich::sync::logger;

fn main() {
    let stdlog = env_logger::Builder::from_env("MY_LOG").json_format().build();
    let max_level = stdlog.filter();
    log_enrich::init(stdlog, max_level);

    logger().enrich("service", "basic.rs").scope(|| {
        info!("starting up");

        logger()
            .enrich("correlation", "Some Id")
            .enrich("operation", "request")
            .scope(|| {
                info!("handling a request for {}", "Timmy");

                logger().enrich("operation", "database").scope(|| {
                    info!("doing database stuff");
                });
            });

        info!("finishing up");
    });
}
