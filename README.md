# `log_enrich`

This is a rough-and-ready experimental API for enriching logs with contextual properties without having to pass explicit loggers through your application. It works with the `log` crate, currently using a custom `env_logger` format. Property enrichment is a feature that works naturally with structured logging.

See the examples for more details.

## Loggers as arguments

[`slog`](https://github.com/slog-rs/slog) supports enriching logs with properties drawn from a context, but requires you pass a logger as an explicit argument. Passing loggers is the most flexible and only universal way to ensure the context you expect to be available actually is. But because logging is a cross-cutting concern, it can be noisy to see logger arguments peppered throughout your API, and tedious to try and introduce logging deep into a logical call context where it isn't already available. The [seanmonstar method](https://github.com/rust-lang-nursery/futures-rfcs/pull/2#issuecomment-363923477) of stuffing extra context in arbitrary self types is a potentially less intrusive way to thread loggers and other bits of cross-cutting context around. `slog` also doesn't prevent a user from building their own mechanism for ambient loggers if they want (there already is a `slog-scope` library).

This library is a compromise between the flexibility of explicit loggers for property enrichment, and the convenience of just calling `info!` anywhere and having ambient context available. The compromise is in additional background plumbing and a potential loss of context if threads are spawned without passing a context to them. It works on the assumption that _anything is better than nothing_ and less friction (with a few tactical changes to capture context before passing to threads) makes it more likely that logs will exist and carry useful information.
