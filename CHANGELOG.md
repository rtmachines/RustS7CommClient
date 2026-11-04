# Changelog

---

## [Unreleased]

### Changed
- Read timeout bounds a whole response frame; a peer closing the socket yields `S7Error::ConnectionClosed` instead of `Io(UnexpectedEof)`

### Fixed
- `cargo test` no longer runs the README example against 192.168.0.100
- `connect_rack_slot` rejects rack > 7 or slot > 31 instead of panicking or silently connecting to another TSAP
- Raw bit writes send 0/1, matching the documented 'true if != 0' rule

## [0.1.2] - 2025-08-15

### Added
- Added parameter check
- Added `InvalidFunParam`

### Modified
- Excluded /target folder from deploy

## [0.1.1] - 2025-08-14
- Initial release
