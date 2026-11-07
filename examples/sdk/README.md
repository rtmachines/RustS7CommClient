# Previous documentation

Before this new structure the examples directory only had the client.rs file.

Build the SDK example with `cargo build --example sdk_client`. `async_client.rs` is the same program on `AsyncS7Client`, plus a read bounded with `tokio::time::timeout`; build it with `cargo build --example sdk_async_client --features tokio`. Run them only against a device you have explicitly chosen; they connect to a real PLC.
