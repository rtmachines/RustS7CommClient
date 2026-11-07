# Changelog

---

## [0.2.0] - Unreleased

### Changed
- **Breaking:** all fallible public methods return `rust7::Result<T>` (an alias `Result<T, E = AnyError>`, so a two-parameter `Result<T, E>` still works after `use rust7::*;`), where `AnyError` is `anyerror::AnyError`, re-exported as `rust7::AnyError`. `rust7::S7Error` and `rust7::client::S7Error` are removed. The 13 affected methods are `set_connection_type`, `set_timeout`, `set_connection_port`, `connect_s71200_1500`, `connect_s7300`, `connect_rack_slot`, `connect_tsap`, `read_area`, `write_area`, `read_db`, `write_db`, `read_bit` and `write_bit`
- Errors carry a stable category string from `rust7::kind` (13 constants, `kind::ALL`), read with `e.get_type()`
- Every public method adds one context with the method name and its parameters (never buffer contents), e.g. `rust7::NotConnected: Not connected while: read_db db=7 start=3 len=4`; `{e:#}` prints context and `source:` causes on separate lines
- New dependency: `anyerror` 0.1.13 (pulls in `serde`); `serde_json` is a dev-dependency only. `#![forbid(unsafe_code)]` still applies to the code of `rust7` itself
- Read timeout bounds a whole response frame; a peer closing the socket yields `kind::CONNECTION_CLOSED` instead of an I/O `UnexpectedEof`
- A read timeout always reports `no complete response frame within <N> ms` (`kind::IO`), whether the socket or the frame deadline expired
- `kind::S7_UNSPECIFIED` messages include the S7 error class and code or the item return code; a framing error during PDU negotiation is kept as the `source()` of `kind::PDU_NEGOTIATION_FAILED`
- The client disconnects itself after transport or framing errors (`kind::IO`, `kind::CONNECTION_CLOSED`, `kind::ISO_INVALID_HEADER`, `kind::ISO_INVALID_TELEGRAM`, `kind::ISO_FRAGMENTED_PACKET`); PLC errors keep the connection
- Responses are checked against the request (reference, function, item count, transport size, lengths). A mismatch is `kind::ISO_INVALID_TELEGRAM` and disconnects, so a device that 0.1.2 tolerated may now be rejected
- `disconnect()` resets `pdu_length` to 0

### Added
- Minimum supported Rust version declared: `rust-version = "1.99"`
- Optional feature `tokio` (off by default) with `rust7::AsyncS7Client` (also `rust7::client::AsyncS7Client`), the async counterpart of `S7Client`: the same setters, `connect_*`, `read_area`/`write_area`, `read_db`/`write_db`, `read_bit`/`write_bit` as `async fn(&mut self)` returning `Send` futures, a synchronous `disconnect()`, and the same public fields (`connected`, `pdu_length`, `last_time`, `chunks`, `confirmed`). Requests, response checks, PDU limits, telegram splitting and address validation are shared with `S7Client`, so error categories, contexts and session rules are identical. Uses tokio 1 (>= 1.48) with only the `net`, `time` and `io-util` features; without the feature the dependency tree is unchanged
- `AsyncS7Client` cancellation: dropping a started call disconnects the client and closes the socket (`connected == false`, next call `kind::NOT_CONNECTED`); `chunks` and `confirmed` keep the values reached, with the usual write semantics. A never-polled future has no effect
- Async example `sdk_async_client` (`examples/sdk/async_client.rs`, requires `--features tokio`) and an async client section in `doc/Documentation.md`
- `AsyncS7Client` timeouts: the read timeout bounds a whole response frame with the same `kind::IO` message as `S7Client`; the write timeout bounds sending a whole request (`request not sent within <N> ms`) and, as in `S7Client`, is fixed when connecting; the connect timeout bounds the TCP connect (`TCP connection not established within <N> ms`). It needs a tokio 1 runtime with I/O and time enabled
- `S7Client::confirmed` reports the buffer bytes confirmed in the last read or write; after a write error the chunk starting there may have been applied
- Errors implement `std::error::Error` (via `AnyError`)
- Public module `rust7::error` with `kind`, `Result` and `AnyError`, also re-exported at the crate root

### Fixed
- Documentation and comments match the code: connection type constants, `set_connection_port` and `write_area` parameters, error lists, timeout and `CONNECTION_CLOSED` descriptions, the reconnect loop, the `confirmed` and `write_bit` wording and Connection Request byte labels
- The SDK example builds as `sdk_client` and is checked by `cargo check --all-targets`; `examples/docker` is no longer compiled as an example of the `rust7` package
- Respect the COTP TPDU size selected in Connection Confirm when offering an S7 PDU and splitting transfers; reject malformed or duplicate size options and S7 sizes exceeding the offer
- Treat a matching, header-only S7 error `ACK` as `kind::S7_UNSPECIFIED` without disconnecting; reject empty or malformed `ACK` responses without acknowledging or retrying writes
- `cargo test` no longer runs the README example against 192.168.0.100
- `connect_rack_slot` rejects rack > 7 or slot > 31 instead of panicking or silently connecting to another TSAP
- Raw bit writes send 0/1, matching the documented 'true if != 0' rule
- Error documentation matches actual categories
- `read_bit`/`write_bit` with `byte_num` >= 8192 no longer overflow the bit address. 0.1.2 computed it in `u16`: a release build read or wrote byte `byte_num % 8192` (e.g. DBX8192.0 became DBX0.0), a debug build panicked
- Transfers larger than 65535 bytes are no longer truncated. 0.1.2 transferred only the first 65535 bytes and returned `Ok`
- A short or truncated read response is no longer reported as success. 0.1.2 could return `Ok` with a partly filled buffer
- A response split across several TCP reads is assembled instead of being rejected
- A negotiated PDU below 29 bytes is rejected with `kind::PDU_NEGOTIATION_FAILED`. In 0.1.2 a PDU below 28 underflowed the chunk sizes (a debug build panicked), and a PDU of 28 gave writes a chunk size of 0, so a write repeated requests without progress
- Bit access with an empty buffer returns `kind::INVALID_FUN_PARAMETER` before any I/O

### Migration 0.1 to 0.2
- Passing `rust7::AnyError` through an application without conversion requires the application's `anyerror` to be semver-compatible with 0.1.x (it is the same type only when Cargo unifies the versions).
- Signatures: replace `Result<T, S7Error>` with `rust7::Result<T>`; `use rust7::S7Error` no longer compiles.
- Matching: `matches!(e, S7Error::X)` becomes `e.get_type() == Some(rust7::kind::X)`. Variant names map to upper-snake constants (`S7Error::NotConnected` to `kind::NOT_CONNECTED`, `Io(_)` to `kind::IO`). `S7Error::Other` was never produced and has no replacement. Treat an unknown or missing category as session-breaking (reconnect).
- Reconnecting: use `client.connected` as the signal to reconnect. After a session-breaking category the client sets it to `false`; PLC errors (`kind::S7_NOT_FOUND`, `kind::S7_INVALID_ADDRESS`, `kind::S7_UNSPECIFIED`) and parameter errors keep the session.
- Causes: walk the chain with `std::error::Error::source()`; `{e}` prints it inline and `{e:#}` on separate lines.
- Lost information: `io::ErrorKind`, `raw_os_error` and downcasting to `io::Error` are not available after conversion; use `kind::IO` and the message text instead.
- Serialization: `AnyError` serializes with serde in the format owned by `anyerror` (`typ`, `msg`, `source`, `context`, `backtrace`). A round trip preserves category, context and the cause chain. There is no schema stability promise across `anyerror` versions, and full `Display` text is not stable; only categories and the presence of context fields are.
- Bit writes with `byte_num` >= 8192 made by a 0.1.2 release build reached byte `byte_num % 8192`: check the affected PLC data. Writes of more than 65535 bytes wrote only the first 65535; the same call in 0.2.0 writes the whole buffer.
- Unchanged: writes are not atomic and are never retried automatically.
- After an error, use `S7Client::confirmed`: for writes bytes `[0, confirmed)` were acknowledged and the next chunk may or may not have been applied; for reads only `[0, confirmed)` of the buffer is a result.

## [0.1.2] - 2025-08-15

### Added
- Added parameter check
- Added `InvalidFunParam`

### Modified
- Excluded /target folder from deploy

## [0.1.1] - 2025-08-14
- Initial release
