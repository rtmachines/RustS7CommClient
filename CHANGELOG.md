# Changelog

---

## [Unreleased]

### Added
- `S7Error` implements `std::error::Error`

### Changed
- Read timeout bounds a whole response frame; a peer closing the socket yields `S7Error::ConnectionClosed` instead of `Io(UnexpectedEof)`
- The client disconnects itself after transport or framing errors; PLC errors keep the connection

### Fixed
- Respect the COTP TPDU size selected in Connection Confirm when offering an S7 PDU and splitting transfers; reject malformed or duplicate size options and S7 sizes exceeding the offer
- Treat a matching, header-only S7 error `ACK` as `S7Unspecified` without disconnecting; reject empty or malformed `ACK` responses without acknowledging or retrying writes
- `cargo test` no longer runs the README example against 192.168.0.100
- `connect_rack_slot` rejects rack > 7 or slot > 31 instead of panicking or silently connecting to another TSAP
- Raw bit writes send 0/1, matching the documented 'true if != 0' rule
- Error documentation matches actual variants

## [0.1.2] - 2025-08-15

### Added
- Added parameter check
- Added `InvalidFunParam`

### Modified
- Excluded /target folder from deploy

## [0.1.1] - 2025-08-14
- Initial release
