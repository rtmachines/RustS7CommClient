# Rust7 manual
## Background

This project is directly derived from <a href="https://github.com/davenardella/snap7" target="_blank">Snap7</a>. For more information on data formatting (endianess), PLC areas, etc., I recommend referring to the original project documentation, available into /doc folder.

---
## Briefly
Communication with Siemens PLCs via the S7 protocovl occurs according to the classic client-server model, in which the PC is the client and the PLC is the server.

The mechanism is quite simple: to read or write data to a PLC, you must first connect to it and then send a read request, after which you will receive the data, or a write request containing the data to be written.
In both cases, you will receive a response containing the outcome of the operation.

![cli srv](img/cli-srv.png)

### Connection
The S7 protocol is encapsulated in TCP/IP, so the connection must first follow TCP/IP rules, meaning it must contain the IP address and port.
In reality, there are other parameters, such as Rack and Slot or TSAP records, but there are support methods that hide these details.

![s7 encapsulation](img/s7_encapsulation.png)

There are four connection methods available:

* `connect_s71200_1500()` 
* `connect_s7300()` 
* `connect_rack_slot()`
* `connect_tsap()`

The first two are fairly self-explanatory; the third is for use with **S7400**, **WinAC**, and **Sinamics**; the rack and slot parameters will be copied from the project's hardware configuration.
The fourth is for use with **LOGO!** or **S7200** only.
We'll explore these in detail in the API. 

---

### Workflow

**First consideration:**

The connection process is quite complex, and the PLC is not a web server, so the connection must be kept active at all times.

**Second consideration:**

There is no such thing as problem-free communication, and this case is no exception. The library reports two groups of errors: low-level errors such as TCP, corrupt headers due to fragmented packets, etc., and high-level errors, such as a read request from a non-existent database.
In the first case (transport and framing errors, listed for each read/write method), the client disconnects itself (`connected` becomes `false`), and you reconnect before performing any other operations. This behavior is the same as that adopted by WinCC and all other SCADA programs.

**Third consideration:**

Your software will almost certainly have a thread-loop for reading and writing data ciclically, and you'll also have to handle communication errors, such as a physical disconnection of the network cable, without reporting errors, pop-ups, or clogging the system with exceptions to handle.  
But above all, the software must be **pass-through** (non-blocking)

What should we do?

Over the years, this is the question I've been asked most frequently.
So I'll show you a workflow that's the same one we use (not just with Snap7).

First, since low-level error handling involves disconnecting and connecting, these two methods will become part of our loop. When we initialize the program, we'll simply create the Client object and optionally preset the connection (set a port other than 102, set timeouts, etc.).


![workflow](img/workflow.png)

A very simplistic pseudo-code would be this:

```rust
while !exit_request{
    
    if !Client.connected {
        let _ = client.connect_s71200_1500(ip_address);
    }
    
    if Client.connected {
        if !do_read_and_write() { // <-- returns false when severe error occurs
            Client.disconnect();
        }
    }

    thread::sleep(100);
}
```

## Errors
---

Every fallible method returns `rust7::Result<T>`. The alias is `rust7::Result<T, E = rust7::AnyError>`, so
`Result<T>` means `Result<T, AnyError>` and `use rust7::*;` does not break other two-parameter `Result<T, E>` uses.
`AnyError` is the type of the [`anyerror`](https://crates.io/crates/anyerror) crate, re-exported by `rust7`
(also as `rust7::error::AnyError`). The category of an error is a string constant from `rust7::kind`,
read with `get_type()`:

```rust,no_run
use rust7::{kind, S7Client};

let mut client = S7Client::new();
let mut buf = [0u8; 4];
if let Err(e) = client.read_db(7, 3, &mut buf) {
    if e.get_type() == Some(kind::NOT_CONNECTED) {
        // reconnect
    }
    eprintln!("read failed: {e:#}"); // category, message, context and causes
}
```

|Constant|Value|Breaks the session|
|---|---|---|
|`kind::IO`|`rust7::Io`|yes|
|`kind::NOT_CONNECTED`|`rust7::NotConnected`|no|
|`kind::TCP_CONNECTION_FAILED`|`rust7::TcpConnectionFailed`|no|
|`kind::CONNECTION_CLOSED`|`rust7::ConnectionClosed`|yes|
|`kind::ISO_CONNECTION_FAILED`|`rust7::IsoConnectionFailed`|no|
|`kind::ISO_FRAGMENTED_PACKET`|`rust7::IsoFragmentedPacket`|yes|
|`kind::ISO_INVALID_HEADER`|`rust7::IsoInvalidHeader`|yes|
|`kind::ISO_INVALID_TELEGRAM`|`rust7::IsoInvalidTelegram`|yes|
|`kind::PDU_NEGOTIATION_FAILED`|`rust7::PduNegotiationFailed`|no|
|`kind::INVALID_FUN_PARAMETER`|`rust7::InvalidFunParameter`|no|
|`kind::S7_NOT_FOUND`|`rust7::S7NotFound`|no|
|`kind::S7_INVALID_ADDRESS`|`rust7::S7InvalidAddress`|no|
|`kind::S7_UNSPECIFIED`|`rust7::S7Unspecified`|no|

The constant values are a stable contract; `kind::ALL` lists all 13. "Breaks the session" means the client
disconnects itself (`connected` becomes `false`) and you must reconnect before the next operation. An error with a
missing or unknown category should be treated as session-breaking. A connection attempt that fails after parameter
validation leaves the client disconnected, because `connect_*` drops a previous session first; only `connect_rack_slot`
with an out-of-range rack or slot is rejected (`kind::INVALID_FUN_PARAMETER`) before connecting and keeps the existing
session.

Each public method adds exactly one context: the method name and its parameters (for example `db=`, `start=`, `len=`,
`ip=`, `port=`), never buffer contents or PLC data. A `Display` looks like
`rust7::NotConnected: Not connected while: read_db db=7 start=3 len=4`. The categories and the presence of context
fields are stable, the full text is not. Plain `{e}` prints category, message, context and causes on one line; the alternate format `{e:#}` prints the context and the `source:` chain on
separate lines; the chain is also available through `std::error::Error::source()`.

`AnyError` implements serde `Serialize`/`Deserialize` in the format defined by `anyerror`
(`typ`, `msg`, `source`, `context`, `backtrace`). A JSON round trip preserves the category, the context and the cause
chain; the schema is not guaranteed across `anyerror` versions. After conversion to `AnyError`, `io::ErrorKind`,
`raw_os_error` and downcasting to `io::Error` are not available.

Errors never imply a retry: multi-chunk writes are not atomic and writes are not retried automatically.

# API
---

#### Connection Setup


|Prototype|Behaviour|      
|---|---|
|`set_connection_type`|Changes the S7 connection type to the PLC       |
|`set_timeout`        |Sets operations timeout                         |
|`set_connection_port`|Sets the TCP Connection Port                    |

#### Connection
|Prototype|Behaviour|      
|---|---|
|`connect_s71200_1500`|Connects to the S71200 or S71500 families            |
|`connect_s7300`      |Connects to S7300 family                             |
|`connect_rack_slot`  |Connects to a Siemens PLC/Drive using Rack and Slot  |
|`connect_tsap`       |Connects to a Siemens ISO-Hardware using TSAP records|
|`disconnect`         |Closes the connection                                |

#### Raw Read/Write methods
|Prototype|Behaviour|      
|---|---|
|`read_area`     |Reads a block of data from a specific S7 memory area  |
|`write_area`    |Writes a block of data to a specific S7 memory area   |

#### Simplified Read/Write methods
|Prototype|Behaviour|      
|---|---|
|`read_db`       |Reads a block of byte from a specific Data Block (DB) |
|`read_bit`      |Reads a bit from a specific S7 memory area            |
|`write_db`      |Writes a block of byte to a specific Data Block (DB)  |
|`write_bit`     |Writes a bit to a specific S7 memory area             |

## Connection setup methods
---

```rust
pub fn set_connection_type(&mut self, connection_type: u16) -> rust7::Result<()>
```
### Changes the S7 connection type to the PLC

The three possible connection types are:
- `CT_PG`: (as a programming device)
- `CT_OP`: (as an HMI)
- `CT_S7`: (as a generic device)

In practice, there aren't many differences; the CT_PG connection should ensure
better system responsiveness, but in reality, I've never noticed any noticeable differences.

`CT_PG` is used by default.

With very old PLCs (early S7300 series) that have limited communication resources,
the connection may be rejected if we have S7Manager with many online windows open at the same time.  
In this case, use `CT_OP` or `CT_S7`. 
 
#### Parameters
- `connection_type`: Connection type.
 
#### Returns
`Ok(())` on success, or an `AnyError` on failure.

#### Errors
- `rust7::kind::INVALID_FUN_PARAMETER`: Invalid parameter supplied to the function.

#### Notes
1. The client must not be connected (that is, call this method before connecting).
2. This method is not useful if you use `connect_tsap()` because the connection_type is already contained in the REMOTE_TSAP record.

---

```rust
pub fn set_timeout(&mut self, co_timeout_ms: u64, rd_timeout_ms: u64, wr_timeout_ms: u64) -> rust7::Result<()>
```
### Sets operations timeout

#### Parameters
- `co_timeout_ms` : TCP Connection timeout (ms) (Default = 3000 ms)
- `rd_timeout_ms` : Maximum time to receive one complete response frame (ms) (Default = 1000 ms)
- `wr_timeout_ms` : Write Connection timeout (ms) (Default = 500 ms)

#### Returns
`Ok(())` on success, or an `AnyError` on failure.

#### Errors
- `rust7::kind::INVALID_FUN_PARAMETER`: Invalid parameter supplied to the function.

#### Notes
1. Values must be > 0, otherwise `rust7::kind::INVALID_FUN_PARAMETER` is returned
2. The client must not be connected (that is, call this method before connecting).

---
```rust
pub fn set_connection_port(&mut self, port: u16) -> rust7::Result<()>
```
### Sets the TCP Connection Port
The default S7 Port is 102, but if you need NAT the addresses you can use this method to change the default value.

#### Parameters
- `connection_type`: Connection type.
 
#### Returns
`Ok(())` on success, or an `AnyError` on failure.

#### Errors
- `rust7::kind::INVALID_FUN_PARAMETER`: Invalid parameter supplied to the function.

### Notes
1. Value must be > 0, otherwise `rust7::kind::INVALID_FUN_PARAMETER` is returned
2. The client must not be connected (that is, call this method before connecting).

---
## Connection methods
---

```rust
pub fn connect_s71200_1500(&mut self, ip: &str) -> rust7::Result<()>
```
### Connects to the S71200 or S71500 families   

This helper method is same as `connect_rack_slot()` with rack=0 and slot=0
### Parameters
- `ip`  : PLC IPV4 address.
 
For Notes, Return and Errors look at `connect_tsap()`

---
```rust
pub fn connect_s7300(&mut self, ip: &str) -> rust7::Result<()>
```
### Connects to S7300 family

This helper method is same as `connect_rack_slot()` with rack=0 and slot=2
### Parameters
- `ip`  : PLC IPV4 address.
 
For Notes, Return and Errors look at `connect_tsap()`

---
```rust
pub fn connect_rack_slot(&mut self, ip: &str, rack: u16, slot: u16) -> rust7::Result<()>
```
### Connects to a Siemens PLC/Drive using Rack and Slot
Rack and Slot are Hardware configuration parameters.

For S7300 and S71200/1500 they are fixed, (see `connect_s7300()` and `connect_s71200_1500()` ).
 
Ultimately, you will need of this method only to connect to S7400, WinAC or other Siemens hardware, like Drives, in which Rack and Slot can vary.
 
### Parameters
- `ip` : PLC IPV4 address.
- `rack` : CPU/CU Rack (0..=7).
- `slot` : CPU/CU Slot (0..=31).

#### Errors
- `rust7::kind::INVALID_FUN_PARAMETER`: rack or slot out of range. Returned before connecting; an existing connection is kept.

For Notes, Return and other Errors look at `connect_tsap()`

---
```rust
pub fn connect_tsap(&mut self, ip: &str, local_tsap: u16, remote_tsap: u16) -> rust7::Result<()>
```
### Connects to a Siemens ISO-Hardware using TSAP records

This is the deepest connection method, you will need it only to connect to LOGO! or S7200.

It's internally called by all other connection methods.
#### Parameters
- `ip` : PLC IPV4 address.
- `local_tsap` : Client TSAP.
- `remote_tsap` : Server TSAP (PLC).
 
#### Notes
The connection port used is 102 (S7Protocol Port) unless you
changed it via `set_connection_port()`

#### Returns
`Ok(())` on success, or an `AnyError` on failure.

#### Errors
- `rust7::kind::TCP_CONNECTION_FAILED`: the address resolves to no socket address.
- `rust7::kind::IO`: name resolution, TCP connect (refused, timeout) or socket I/O error.
- `rust7::kind::CONNECTION_CLOSED`: the peer closed the connection.
- `rust7::kind::ISO_INVALID_HEADER` / `rust7::kind::ISO_INVALID_TELEGRAM`: malformed or oversized TPKT reply.
- `rust7::kind::ISO_CONNECTION_FAILED`: ISO connection failed
- `rust7::kind::PDU_NEGOTIATION_FAILED`: PDU negotiation failed.

Example of using TSAPs (from Snap7 manual)
![tsap](img/tsap.png)

---
## Raw Read/Write methods
---

```rust
pub fn read_area(&mut self, area: u8, db_number: u16, start: u16, wordlen: u8, buffer: &mut [u8]) -> rust7::Result<()>
```
### Reads a block of data from a specific S7 memory area.

#### Parameters
- `area`: S7 memory area constant (e.g., `S7_AREA_PE`, `S7_AREA_PA`, `S7_AREA_DB`, `S7_AREA_MK`).
- `db_number`: DB number (ignored for non-DB areas).
- `start`: Starting element index (byte index for bytes, bit index for bits).
- `wordlen`: Word length constant (e.g., `S7_WL_BYTE`, `S7_WL_BIT`).
- `buffer`: Destination buffer to store the read data.

#### Values
##### area
- `S7_AREA_PE` (0x81): Process Inputs
- `S7_AREA_PA` (0x82): Process Outputs
- `S7_AREA_MK` (0x83): Merkers
- `S7_AREA_DB` (0x84): Data Block
##### wordlen 
- `S7_WL_BIT` (0x01) : Bit access
- `S7_WL_BYTE` (0x02): Byte access
#### Bit access notes
1. The start must be expressed in bits.
For example, if you want to access bit `DBX 45.3`, the start value would be 45 * 8 + 3 = 363.
2. Whatever buffer is passed, only the first byte will be used, which is considered true if !=0 or false if ==0

#### Returns
`Ok(())` Operation succeeded.

#### Errors
##### Low level
- `rust7::kind::NOT_CONNECTED`: An attempt was made to read while the client was not connected.
- `rust7::kind::ISO_INVALID_HEADER`: Invalid ISO Header
- `rust7::kind::ISO_INVALID_TELEGRAM`: Inconsistent expected telegram length.
- `rust7::kind::ISO_FRAGMENTED_PACKET`: ISO Packet fragmented.
- `rust7::kind::IO`: network I/O error, including a response not received within the read timeout.
- `rust7::kind::CONNECTION_CLOSED`: the PLC closed the connection.

##### Connection after errors
After `kind::IO`, `kind::CONNECTION_CLOSED`, `kind::ISO_INVALID_HEADER`, `kind::ISO_INVALID_TELEGRAM` or `kind::ISO_FRAGMENTED_PACKET` the client disconnects itself (`connected` becomes `false`); reconnect before the next operation. PLC errors (`kind::S7_NOT_FOUND`, `kind::S7_INVALID_ADDRESS`, `kind::S7_UNSPECIFIED`) keep the connection.
  
##### High level
- `rust7::kind::S7_NOT_FOUND`: The resource was not found (e.g. Inexistent DB).
- `rust7::kind::S7_UNSPECIFIED`: Other error reported by the PLC.
- `rust7::kind::S7_INVALID_ADDRESS`:
1. Attempt to read beyond the limits.
2. The DB is optimized.

#### Notes
- The number of bytes to read will be equal to the size of the buffer passed.
- Large blocks are automatically split into chunks based on the negotiated PDU size.
- In case of error the buffer contents will be inconsistent and should not be considered.
 
---
```rust
pub fn write_area(&mut self, area: u8, db_number: u16, start: u16, wordlen: u8, data: &[u8]) -> rust7::Result<()>
```
 
### Writes a block of data to a specific S7 memory area.

#### Parameters
 - `area`: S7 memory area constant (e.g., `S7_AREA_PE`, `S7_AREA_PA`, `S7_AREA_DB`, `S7_AREA_MK`).
 - `db_number`: DB number (ignored for non-DB areas).
 - `start`: Starting element index (byte index for bytes, bit index for bits).
 - `wordlen`: Word length constant (e.g., `S7_WL_BYTE`, `S7_WL_BIT`).
 - `buffer`: Source buffer to write.

#### Values
##### area
- `S7_AREA_PE` (0x81): Process Inputs
- `S7_AREA_PA` (0x82): Process Outputs
- `S7_AREA_MK` (0x83): Merkers
- `S7_AREA_DB` (0x84): Data Block
##### wordlen 
- `S7_WL_BIT` (0x01) : Bit access
- `S7_WL_BYTE` (0x02): Byte access
##### Bit access notes
1. The start must be expressed in bits.
For example, if you want to access bit `DBX 45.3`, the start value would be 45 * 8 + 3 = 363.
2. Whatever buffer is passed, only the first byte will be used, which is considered true if !=0 or false if ==0
3. Writing a bit affects **only that bit**, leaving adjacent bits in the byte unchanged. 

#### Returns
`Ok(())` Operation succeeded.

#### Errors
##### Low level
- `rust7::kind::NOT_CONNECTED`: An attempt was made to write while the client was not connected.
- `rust7::kind::ISO_INVALID_HEADER`: Invalid ISO Header
- `rust7::kind::ISO_INVALID_TELEGRAM`: Inconsistent expected telegram length.
- `rust7::kind::ISO_FRAGMENTED_PACKET`: ISO Packet fragmented.
- `rust7::kind::IO`: network I/O error, including a response not received within the read timeout.
- `rust7::kind::CONNECTION_CLOSED`: the PLC closed the connection.

##### Connection after errors
After `kind::IO`, `kind::CONNECTION_CLOSED`, `kind::ISO_INVALID_HEADER`, `kind::ISO_INVALID_TELEGRAM` or `kind::ISO_FRAGMENTED_PACKET` the client disconnects itself (`connected` becomes `false`); reconnect before the next operation. PLC errors (`kind::S7_NOT_FOUND`, `kind::S7_INVALID_ADDRESS`, `kind::S7_UNSPECIFIED`) keep the connection.

##### High level
- `rust7::kind::S7_NOT_FOUND`: The resource was not found (e.g. Inexistent DB).
- `rust7::kind::S7_UNSPECIFIED`: Other error reported by the PLC.
- `rust7::kind::S7_INVALID_ADDRESS`:
1. Attempt to write beyond the limits.
2. The DB is optimized.

#### Notes
- The number of bytes to write will be equal to the size of the buffer passed.
- Large blocks are automatically split into chunks based on the negotiated PDU size.
- Writing the output buffer (`S7_AREA_PA`) usually does not produce useful results, in fact the output process image will be rewritten by OB1 in the next round

---
## Simplified Read/Write methods
---

```rust
pub fn read_db(&mut self, db_number: u16, start: u16, buffer: &mut [u8]) -> rust7::Result<()>
```
#### Reads a block of byte from a specific Data Block (DB)

This helper method is same as `read_area()` with:
- area = `S7_AREA_DB`
- wordlen = `S7_WL_BYTE`

#### Parameters
- `db_number`: DB number 
- `start`: Starting byte index 
- `buffer`: Destination buffer to store the read data.

#### Notes
- The number of bytes to read will be equal to the size of the buffer passed.
 
For further info, please refer to `read_area()`

---
```rust
pub fn write_db(&mut self, db_number: u16, start: u16, buffer: &[u8]) -> rust7::Result<()>
```
#### Writes a block of byte to a specific Data Block (DB)

This helper method is same as `write_area()` with:
- area = `S7_AREA_DB`
- wordlen = `S7_WL_BYTE`

#### Parameters
- `db_number`: DB number 
- `start`: Starting byte index 
- `buffer`: Source buffer to write.

#### Notes
- The number of bytes to write will be equal to the size of the buffer passed.
 
For further info, please refer to `write_area()`

---
```rust
pub fn read_bit(&mut self, area: u8, db_number: u16, byte_num: u16, bit_idx: u8) -> rust7::Result<bool>
```
 #### Reads a bit from a specific S7 memory area

 This helper method is same as `read_area()` with:
 - wordlen = `S7_WL_BIT`
 - start = `byte_num * 8 + bit_idx`
 
 #### Parameters
 - `area`: S7 memory area constant (e.g., `S7_AREA_PE`, `S7_AREA_PA`, `S7_AREA_DB`, `S7_AREA_MK`).
 - `db_number`: DB number (ignored for non-DB areas).
 - `byte_num`: Byte Number. 
 - `bit_idx`: Bit index inside the byte (0..7).
 
 #### Example
 To read DB10.DBX71.4 use:
 
 ```my_bit = read_bit(S7_AREA_DB, 10, 71, 4);```
 
 #### Returns
 `Ok(<bool>)` or `Err(<AnyError>)`

#### Errors
- `rust7::kind::INVALID_FUN_PARAMETER`: Invalid parameter supplied to the function.
- Other reported by read_area()

 #### Suggestion
 - Even reading a single bit requires an entire telegram.
 Since reading is non-invasive, if you need to read multiple bits 
 (more or less adjacent in the same area), I recommend reading blocks 
 of bytes and then unpacking them.
 
 For further info, please refer to `read_area()`

---
```rust
pub fn write_bit(&mut self, area: u8, db_number: u16, byte_num: u16, bit_idx: u8, value: bool) -> rust7::Result<()>
```
#### Writes a bit to a specific S7 memory area

This helper method is same as `write_area()` with:
- wordlen = `S7_WL_BIT`
- start = `byte_num * 8 + bit_idx`

#### Parameters
- `area`: S7 memory area constant (e.g., `S7_AREA_PE`, `S7_AREA_PA`, `S7_AREA_DB`, `S7_AREA_MK`).
- `db_number`: DB number (ignored for non-DB areas).
- `byte_num`: Byte Number. 
- `bit_idx`: Bit index inside the byte (0..7).
- `value`: Value to write (true | false).

#### Example
To write **1** into DB10.DBX71.4 use:

```write_bit(S7_AREA_DB, 10, 71, 4, true);```

#### Returns
`Ok(())` Operation succeeded.

#### Errors
- `rust7::kind::INVALID_FUN_PARAMETER`: Invalid parameter supplied to the function.
- Other reported by write_area()

#### Notes
- Writing a bit affects only that bit, leaving adjacent bits in the byte unchanged. 

For further info, please refer to `write_area()`
 
# Fields
---

```rust
pub connected: bool
```
#### The Client is connected to the PLC
#### Note
- This is just a memory bit and doesn't reflect the actual state of the connection. For example, if you unplug the network cable, the underlying socket won't notice anything until the next read or write operation.
- The client clears it after a low-level communication error (see `read_area()`).

---
```rust
pub last_time: f64
```
#### Last operation time (ms).
#### Note
- If an error occurred the value will be 0.0

---
```rust
pub chunks:  usize
```
#### Indicates how many pieces the data to be read or written in the last operation was divided into
Maybe you need to know it only for extreme tuning
#### Note
- Reset at the start of each read/write; on error it includes chunks already attempted, including the failing chunk.


## Transfer validation and limits

Byte reads/writes process the entire buffer and split it by negotiated PDU size,
including buffers larger than 65535 bytes. The last addressed byte must not exceed
`0x1fffff`; a range exceeding the 24-bit bit address is rejected before I/O.
Bit helpers support byte indices through 65535. A raw bit operation requires a
nonempty buffer and uses only its first byte; an empty byte operation sends no
telegram after parameter and connection validation.

TCP segmentation is handled by reading complete TPKT frames. S7 response headers,
request references, functions, lengths and item statuses are validated before data
is copied or a write is acknowledged. COTP fragmentation is not supported.

The client offers a 1024-byte COTP TPDU. A Connection Confirm may select a smaller
TPDU through parameter `0xC0`, which must occur at most once, have length 1, and
contain an exponent from 7 through 10. Invalid or oversized selections fail with
`kind::ISO_CONNECTION_FAILED`. If the option is absent, the client retains a local 1024-byte
send cap for compatibility; absence is not treated as an explicit 128-byte choice.

The S7 Setup Communication offer fits in one COTP TPDU, including its 3-byte header:

| COTP TPDU limit (bytes) | Maximum S7 PDU offer (bytes) |
| --- | --- |
| 128 | 125 |
| 256 | 253 |
| 512 | 480 |
| 1024 (also the local cap when `0xC0` is absent) | 480 |

The CPU must select an S7 PDU size from 29 through the offered size; an out-of-range
reply fails with `kind::PDU_NEGOTIATION_FAILED`. No connection is exposed until both
handshakes succeed. Read payloads are limited to `pdu_length - 18` bytes and write
payloads to `pdu_length - 28`, keeping requests and expected responses within one
TPDU. The public `pdu_length` field reports the negotiated S7 size, excluding COTP;
it is diagnostic and cannot override internal limits.

A terminal S7 `ACK` (`ROSCTR=0x02`) is accepted only as a PLC error: it must have a
12-byte header, a matching request reference, zero parameter/data lengths, and a
nonzero error class/code pair. It returns `kind::S7_UNSPECIFIED` and keeps the session
usable. An `ACK` without an error or with extra parameters/data is a
`kind::ISO_INVALID_TELEGRAM` error and disconnects the client. It never confirms a successful
write. Successful reads and writes still require a validated `ACK_DATA` response.

A multichunk write is not atomic: earlier chunks may have reached the PLC when a
later chunk fails. No write is automatically retried. After a read error discard
the destination buffer, since earlier chunks may have been copied. After a low-level
communication error the client disconnects itself; reconnect before the next operation. `chunks` records attempted chunks, including the
failing attempt; parameter validation failures leave it zero.
