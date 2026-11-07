//! Async counterpart of `client.rs`. Build with:
//! `cargo build --example sdk_async_client --features tokio`
use rust7::client;
use rust7::AsyncS7Client;
use std::time::Duration;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut client = AsyncS7Client::new();
    let db_number: u16 = 100; // Must exist into the PLC

    match client.connect_s71200_1500("192.168.0.100").await {
        Ok(_) => {
            println!("Connected to PLC");
            println!("PDU negotiated: {} byte", client.pdu_length);
            println!("Job time (ms) : {:.3}", client.last_time);
        }
        Err(e) => {
            eprintln!("Connection failed: {e:#}");
            return;
        }
    }
    // Reads 462 byte from DB100
    println!();
    println!("Attempt to read 462 byte from DB100");
    let mut read_buffer = vec![0u8; 462];
    match client.read_db(db_number, 0, &mut read_buffer).await {
        Ok(_) => {
            println!("Success!");
            println!("Job time (ms) : {:.3}", client.last_time);
            println!("Chunks        : {}", client.chunks);
            println!("Data read:");
            for (i, chunk) in read_buffer.chunks(32).enumerate() {
                print!("{:04X}: ", i * 32); // Hex Offset
                for byte in chunk {
                    print!("{:02X} ", byte);
                }
                println!();
            }
        }
        Err(e) => eprintln!("Read failed: {e:#}"),
    }

    // Writes 1024 byte to DB100
    println!();
    println!("Attempt to write 1024 byte to DB100");
    let mut write_data = [0u8; 1024];

    for (i, val) in write_data.iter_mut().enumerate() {
        *val = (i % 256) as u8;
    }

    match client.write_db(db_number, 0, &write_data).await {
        Ok(_) => {
            println!("Success!");
            println!("Job time (ms) : {:.3}", client.last_time);
            println!("Chunks        : {}", client.chunks);
        }
        Err(e) => eprintln!("Write failed: {e:#}"),
    }

    // Read a bit
    println!();
    println!("Attempt to read DB100.DBX47.5");
    match client.read_bit(client::S7_AREA_DB, db_number, 47, 5).await {
        Ok(value) => {
            println!("Success!");
            println!("Job time (ms) : {:.3}", client.last_time);
            println!("Chunks        : {}", client.chunks);
            println!("Value read    : {}", value)
        }
        Err(e) => eprintln!("Read failed: {e:#}"),
    }

    // Write a bit
    println!();
    println!("Attempt to write 'false' into DB100.DBX16.0");
    match client
        .write_bit(client::S7_AREA_DB, db_number, 16, 0, false)
        .await
    {
        Ok(_) => {
            println!("Success!");
            println!("Job time (ms) : {:.3}", client.last_time);
            println!("Chunks        : {}", client.chunks);
        }
        Err(e) => eprintln!("Write failed: {e:#}"),
    }

    // A read bounded by the caller: if the deadline expires first, the call is
    // abandoned, the client disconnects itself and must reconnect.
    println!();
    println!("Attempt to read 16 byte from DB100 within 250 ms");
    let mut small = [0u8; 16];
    let read = client.read_db(db_number, 0, &mut small);
    match tokio::time::timeout(Duration::from_millis(250), read).await {
        Ok(Ok(_)) => println!("Success! Job time (ms) : {:.3}", client.last_time),
        Ok(Err(e)) => eprintln!("Read failed: {e:#}"),
        Err(_) => {
            println!("Abandoned after 250 ms");
            println!("Connected     : {}", client.connected); // false
            println!("Confirmed     : {} byte", client.confirmed);
        }
    }

    client.disconnect();
    println!();
    println!("Disconnected");
}
