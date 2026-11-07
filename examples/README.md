# Examples

When working with an PLC SDK, it may be challenging to work with the SDK if you don't have a physical Siemens PLC available. Thanks to many open source projects, are there countless ways of running a Virtualized PLC or a specialized embedded software on a raspberry pi.

- See [Docker](./docker/README.md) for running the Rust7 SDK against a S7-PLC in Docker. 
- See [SDK](./sdk/README.md) for learning more with practical examples, that shows how to use the Rust7 SDK in a practical way.

The examples report errors with `eprintln!("...: {e:#}")`. The plain format `{e}` prints category, message, context and causes on one line; the alternate format `{e:#}` prints them on separate lines.

The Docker example (`examples/docker`) is a separate package that depends on the published `rust7 = "0.1.2"` from the registry, so building it does not test the current checkout of this library, and its errors are the 0.1 `S7Error`, without the categories and contexts above. Its dependency moves to 0.2 once 0.2.0 is published.

Build the SDK example with `cargo build --example sdk_client`, and its async counterpart on tokio with `cargo build --example sdk_async_client --features tokio`. Run them only against a device you have explicitly chosen; they connect to a real PLC.
