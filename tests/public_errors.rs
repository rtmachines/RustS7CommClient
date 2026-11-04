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

fn contexts(e: &rust7::AnyError) -> Vec<String> {
    let value = serde_json::to_value(e).unwrap();
    value["context"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn disconnected_db_read_has_category_and_operation_context() {
    let mut client = S7Client::new();
    let err = client.read_db(7, 3, &mut [0; 4]).unwrap_err();
    assert_eq!(err.get_type(), Some(rust7::kind::NOT_CONNECTED));
    assert_eq!(contexts(&err), ["read_db db=7 start=3 len=4"]);
    assert!(std::error::Error::source(&err).is_none());
}

#[test]
fn shortcuts_have_single_context() {
    let mut client = S7Client::new();

    let err = client.write_db(7, 3, &[0; 2]).unwrap_err();
    assert_eq!(contexts(&err), ["write_db db=7 start=3 len=2"]);

    let err = client.read_bit(rust7::S7_AREA_DB, 1, 0, 8).unwrap_err();
    assert_eq!(err.get_type(), Some(rust7::kind::INVALID_FUN_PARAMETER));
    assert_eq!(
        contexts(&err),
        ["read_bit area=0x84 db=1 byte_num=0 bit_idx=8"]
    );

    let err = client
        .write_bit(rust7::S7_AREA_DB, 1, 2, 3, true)
        .unwrap_err();
    assert_eq!(
        contexts(&err),
        ["write_bit area=0x84 db=1 byte_num=2 bit_idx=3"]
    );

    let err = client.connect_rack_slot("127.0.0.1", 8, 0).unwrap_err();
    assert_eq!(err.get_type(), Some(rust7::kind::INVALID_FUN_PARAMETER));
    assert_eq!(
        contexts(&err),
        ["connect_rack_slot ip=127.0.0.1 port=102 rack=8 slot=0"]
    );
}

#[test]
fn setters_report_rejected_value() {
    let mut client = S7Client::new();
    let err = client.set_connection_port(0).unwrap_err();
    assert_eq!(contexts(&err), ["set_connection_port port=0"]);
    let err = client.set_timeout(0, 1, 1).unwrap_err();
    assert_eq!(
        contexts(&err),
        ["set_timeout co_timeout_ms=0 rd_timeout_ms=1 wr_timeout_ms=1"]
    );
    let err = client.set_connection_type(9).unwrap_err();
    assert_eq!(contexts(&err), ["set_connection_type connection_type=9"]);
}

#[test]
fn read_area_context_lists_transfer() {
    let err = S7Client::new()
        .read_area(rust7::S7_AREA_DB, 5, 10, rust7::S7_WL_BYTE, &mut [0; 6])
        .unwrap_err();
    assert_eq!(
        contexts(&err),
        ["read_area area=0x84 db=5 start=10 wordlen=2 len=6"]
    );
}
