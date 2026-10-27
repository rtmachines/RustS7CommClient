# Rust7 - Native Rust S7Client

Pragmatic native Rust S7 client (Snap7‑style) for Siemens PLCs. 

---

## Features
- Pure Rust, no unsafe code.
- Low latency: ≈ 1ms/PDU.
- Small footprint.
- Strict control of incoming headers.
- Automatic telegram splitting for large reads/writes.
- Connection helpers methods for S71200/1500 and S7300.
---

## Quick start
```rust
use rust7::client::{S7Client};

fn main() {
    let mut client = S7Client::new();
    let db_number: u16 = 100; // Must exist into the PLC

    // Connection
    match client.connect_s71200_1500("192.168.0.100") {
        Ok(_) => { println!("Connected to PLC") },
        Err(e) => {
            eprintln!("Connection failed: {}", e);
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
        Err(e) => eprintln!("Read failed: {}", e),
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

The client reads complete bounded TCP frames and validates S7 responses against the
request, including the reference, operation and payload length. It returns an error
for an incomplete or inconsistent response. Writes across multiple chunks are not
atomic: an error can occur after earlier chunks have reached the PLC. The client does
not automatically retry writes. Discard a read buffer after an error and reconnect
after a low-level communication error.

`pdu_length` is diagnostic; changing it does not override the internal negotiated
limits. `chunks` counts attempted read/write chunks, including a failing chunk.

## Documentation
The detailed documentation is <a href="doc/Documentation.md" target="_blank">here</a>.

## Examples
For practical examples see the documentation <a href="examples/README.md" target="_blank">here</a>.

---

## License
Copyright © 2025 Davide Nardella

Distribuited under <a href="LICENSE" target="_blank">MIT License</a>.
