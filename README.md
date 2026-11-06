# Rust7 - Native Rust S7Client

Pragmatic native Rust S7 client (Snap7‑style) for Siemens PLCs. 

---

## Features
- Pure Rust; `#![forbid(unsafe_code)]` applies to the crate's own code.
- Few dependencies: `anyerror` (which pulls in `serde`). To pass `rust7::AnyError` through your application without conversion, your `anyerror` must be semver-compatible with 0.1.x so Cargo unifies the versions.
- Errors are `rust7::AnyError` with a stable category from `rust7::kind` and per-call context.
- Low latency: ≈ 1ms/PDU.
- Small footprint.
- Strict control of incoming headers.
- Automatic telegram splitting for large reads/writes.
- Connection helpers methods for S71200/1500 and S7300.
---

## Quick start
```rust,no_run
use rust7::S7Client;

fn main() {
    let mut client = S7Client::new();
    let db_number: u16 = 100; // Must exist into the PLC

    // Connection
    match client.connect_s71200_1500("192.168.0.100") {
        Ok(_) => { println!("Connected to PLC") },
        Err(e) => {
            eprintln!("Connection failed: {e:#}");
            return;
        }
    }
    // Reads 64 byte from DB100
    println!("");
    println!("Attempt to read 64 byte from DB100");
    let mut read_buffer = vec![0u8; 64];
    match client.read_db(db_number, 0, &mut read_buffer) {
        Ok(_) => {          
            println!("Success!");
            println!("Job time (ms) : {:.3}", client.last_time);
        },
        Err(e) => eprintln!("Read failed: {e:#}"),
    }

    client.disconnect();
}
```

## Transfer behavior

Byte reads and writes process the entire buffer, including lengths above 65535 bytes,
using the negotiated PDU size. Address ranges are checked before the first request;
the last byte must be at most `0x1fffff`. Bit helpers accept the full `u16` byte index
and bit indices `0..=7`. Raw bit operations require a nonempty buffer and use its
first byte. An empty byte transfer sends no request after parameter and connection
checks.

The S7 PDU offer is capped at 480 bytes and reduced for small COTP TPDU limits so
each request and expected response fits in one TPDU. COTP fragmentation is unsupported.

The client reads complete bounded TCP frames and validates S7 responses against the
request, including the reference, operation and payload length. It returns an error
for an incomplete or inconsistent response. Writes across multiple chunks are not
atomic. After an error, bytes `[0, confirmed)` were acknowledged by the PLC; the chunk
starting at `confirmed` may or may not have been written, also for a single-chunk write
or `write_bit`; later bytes were not sent. The client never retries a write. After a read
error only `[0, confirmed)` of the buffer is a result. After a low-level
communication error the client disconnects itself; reconnect before the next operation.
A valid terminal S7 `ACK` carrying a PLC error returns `kind::S7_UNSPECIFIED` and keeps
the session usable. An empty or malformed `ACK` disconnects the client.

`pdu_length` is diagnostic; changing it does not override the internal negotiated
limits. `chunks` counts attempted read/write chunks, including a failing chunk; `confirmed` counts
buffer bytes confirmed in the last read/write (copied from verified responses or acknowledged
by the PLC), reset together with `chunks`.

## Errors

Fallible methods return `rust7::Result<T>`, with `rust7::AnyError` (from the `anyerror` crate) as the
error. Match on the category, not on the text:

```rust,no_run
use rust7::{kind, S7Client};

let mut client = S7Client::new();
let mut buf = [0u8; 4];
if let Err(e) = client.read_db(7, 3, &mut buf) {
    match e.get_type() {
        Some(kind::NOT_CONNECTED) => { /* reconnect */ }
        _ => eprintln!("read failed: {e:#}"), // category, context and causes
    }
}
```

| Constant | Value | Breaks the session |
| --- | --- | --- |
| `kind::IO` | `rust7::Io` | yes |
| `kind::NOT_CONNECTED` | `rust7::NotConnected` | no |
| `kind::TCP_CONNECTION_FAILED` | `rust7::TcpConnectionFailed` | no |
| `kind::CONNECTION_CLOSED` | `rust7::ConnectionClosed` | yes |
| `kind::ISO_CONNECTION_FAILED` | `rust7::IsoConnectionFailed` | no |
| `kind::ISO_FRAGMENTED_PACKET` | `rust7::IsoFragmentedPacket` | yes |
| `kind::ISO_INVALID_HEADER` | `rust7::IsoInvalidHeader` | yes |
| `kind::ISO_INVALID_TELEGRAM` | `rust7::IsoInvalidTelegram` | yes |
| `kind::PDU_NEGOTIATION_FAILED` | `rust7::PduNegotiationFailed` | no |
| `kind::INVALID_FUN_PARAMETER` | `rust7::InvalidFunParameter` | no |
| `kind::S7_NOT_FOUND` | `rust7::S7NotFound` | no |
| `kind::S7_INVALID_ADDRESS` | `rust7::S7InvalidAddress` | no |
| `kind::S7_UNSPECIFIED` | `rust7::S7Unspecified` | no |

"Breaks the session" means the client disconnects itself (`client.connected` becomes `false`);
use that flag as the signal to reconnect. A connection attempt that fails after parameter
validation leaves the client disconnected (`connected == false`), because `connect_*` drops a
previous session first; only `connect_rack_slot` with an out-of-range rack or slot is rejected
(`kind::INVALID_FUN_PARAMETER`) before connecting and keeps the existing session. Every public method adds one context with the method
name and its parameters (never buffer contents); categories and the presence of context fields are
stable, the full `Display` text is not. Errors serialize with serde in the format owned by
`anyerror`, which preserves category, context and causes but is not schema-stable across `anyerror`
versions. `io::ErrorKind`, `raw_os_error` and downcasting are not available after conversion.

## Documentation
The detailed documentation is <a href="doc/Documentation.md" target="_blank">here</a>.

## Examples
For practical examples see the documentation <a href="examples/README.md" target="_blank">here</a>.

---

## License
Copyright © 2025 Davide Nardella

Distribuited under <a href="LICENSE" target="_blank">MIT License</a>.
