# Examples

When working with an PLC SDK, it may be challenging to work with the SDK if you don't have a physical Siemens PLC available. Thanks to many open source projects, are there countless ways of running a Virtualized PLC or a specialized embedded software on a raspberry pi.

- See [Docker](./docker/README.md) for running the Rust7 SDK against a S7-PLC in Docker. 
- See [SDK](./sdk/README.md) for learning more with practical examples, that shows how to use the Rust7 SDK in a practical way.

The examples report errors with `eprintln!("...: {e:#}")`. The plain format `{e}` prints category, message, context and causes on one line; the alternate format `{e:#}` prints them on separate lines.

The Docker example (`examples/docker`) is a separate package that depends on the published `rust7 = "0.2"` from the registry. Until 0.2 is published, building it does not test the current checkout of this library.
