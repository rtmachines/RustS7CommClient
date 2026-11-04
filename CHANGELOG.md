# Changelog

---

## [Unreleased]

### Fixed
- `cargo test` no longer runs the README example against 192.168.0.100
- `connect_rack_slot` rejects rack > 7 or slot > 31 instead of panicking or silently connecting to another TSAP

## [0.1.2] - 2025-08-15

### Added
- Added parameter check
- Added `InvalidFunParam`

### Modified
- Excluded /target folder from deploy

## [0.1.1] - 2025-08-14
- Initial release
