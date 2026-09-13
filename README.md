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
            println!("Job time (ms) : {:.3}", client.last_time());
        },
        Err(e) => eprintln!("Read failed: {}", e),
    }

    client.disconnect();
}
```

## Error handling
Every transport or protocol error (`Timeout`, `ConnectionClosed`, bad framing, a reply that does not
match the request, ...) **drops the connection**: the stream may be out of sync with the CPU, so the
client refuses further jobs with `S7Error::NotConnected` until you reconnect. PLC-level rejections
(`S7NotFound`, `S7InvalidAddress`, ...) keep the connection.

```rust
use rust7::{S7Client, S7Error};

fn poll(client: &mut S7Client, ip: &str, buf: &mut [u8]) -> Result<(), S7Error> {
    if !client.is_connected() {
        client.connect_s71200_1500(ip)?;
    }
    match client.read_db(100, 0, buf) {
        Ok(()) => Ok(()),
        Err(e) if e.requires_reconnect() => Err(e), // reconnect on the next poll
        Err(e) => Err(e),                           // parameter or PLC error: fix the request
    }
}
```

A write that fails after some telegrams were acknowledged returns
`S7Error::WriteIncomplete { acknowledged, requested, source }`, so "how much was applied" is never
ambiguous.

`S7Client` is `Send`; every method takes `&mut self`, so share it between threads through a `Mutex`.

## Documentation
The detailed documentation is <a href="doc/Documentation.md" target="_blank">here</a>.

## Examples
For practical examples see the documentation <a href="examples/README.md" target="_blank">here</a>.

---

## License
Copyright © 2025 Davide Nardella

Distribuited under <a href="LICENSE" target="_blank">MIT License</a>.
