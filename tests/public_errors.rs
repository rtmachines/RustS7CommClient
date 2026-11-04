use rust7::S7Client;

type ReadArea = fn(&mut S7Client, u8, u16, u16, u8, &mut [u8]) -> rust7::Result<()>;
type WriteArea = fn(&mut S7Client, u8, u16, u16, u8, &[u8]) -> rust7::Result<()>;

#[test]
fn public_methods_return_anyerror() {
    let _: fn(&mut S7Client, u16) -> Result<(), rust7::AnyError> = S7Client::set_connection_type;
    let _: fn(&mut S7Client, u64, u64, u64) -> rust7::Result<()> = S7Client::set_timeout;
    let _: fn(&mut S7Client, u16) -> rust7::Result<()> = S7Client::set_connection_port;
    let _: fn(&mut S7Client, &str) -> rust7::Result<()> = S7Client::connect_s71200_1500;
    let _: fn(&mut S7Client, &str) -> rust7::Result<()> = S7Client::connect_s7300;
    let _: fn(&mut S7Client, &str, u16, u16) -> rust7::Result<()> = S7Client::connect_rack_slot;
    let _: fn(&mut S7Client, &str, u16, u16) -> rust7::Result<()> = S7Client::connect_tsap;
    let _: ReadArea = S7Client::read_area;
    let _: WriteArea = S7Client::write_area;
    let _: fn(&mut S7Client, u16, u16, &mut [u8]) -> rust7::Result<()> = S7Client::read_db;
    let _: fn(&mut S7Client, u8, u16, u16, u8) -> rust7::Result<bool> = S7Client::read_bit;
    let _: fn(&mut S7Client, u16, u16, &[u8]) -> rust7::Result<()> = S7Client::write_db;
    let _: fn(&mut S7Client, u8, u16, u16, u8, bool) -> rust7::Result<()> = S7Client::write_bit;
}

#[test]
fn public_errors_are_send_sync() {
    fn bound<E: std::error::Error + Send + Sync + 'static>() {}
    bound::<rust7::AnyError>();
}

#[test]
fn read_db_without_connection_is_not_connected() {
    let err = S7Client::new().read_db(7, 3, &mut [0; 4]).unwrap_err();
    assert_eq!(err.get_type(), Some(rust7::kind::NOT_CONNECTED));
}
