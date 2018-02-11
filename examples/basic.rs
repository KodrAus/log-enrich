#[macro_use]
extern crate log;
extern crate log_enrich;

fn main() {
    log_enrich::init();

    log_enrich::logger()
        .enrich("service", "basic.rs")
        .scope_sync(|| {
            info!("starting up");

            log_enrich::logger()
                .enrich("correlation", "Some Id")
                .enrich("operation", "request")
                .scope_sync(|| {
                    info!("handling a request for {}", "Timmy");
                });

            info!("finishing up");
        });
}