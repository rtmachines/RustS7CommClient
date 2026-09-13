# Changelog

---

## [0.2.0] - 2026-09-13

Hardening release after a mission-critical review. **Breaking**: see *Changed*.

### Fixed
- A reply arriving after a read timeout could be delivered as the data of the *next* request
  (fixed PDU reference, no reply validation). Requests now carry an incrementing reference and
  every acknowledgement is checked for reference, message type, function and item size.
- A reply shorter (or longer) than the requested chunk returned `Ok(())` with a partially filled
  buffer; it is now `S7Error::S7InvalidResponse`.
- A reply split across TCP segments failed the job and left the connection permanently out of sync;
  telegrams are now read to completion.
- `read_bit` / `write_bit` with `byte_num >= 8192` panicked in debug and addressed the wrong bit in
  release; the bit address is now checked and rejected with `InvalidFunParameter`.
- Buffers longer than 65535 bytes were silently truncated; they are now rejected.
- `S7_WL_BIT` with an empty buffer panicked; it is now rejected before any I/O.
- A negotiated PDU length below 28 underflowed (panic / huge chunk), 28 left zero payload bytes per
  write telegram, and above 480 broke every full-size chunk; anything outside `29..=480` is now
  `PduNegotiationFailed`.
- `connect_rack_slot` silently aliased `slot >= 32` into the next rack; rack and slot are validated.
- Reconnecting while connected, or after a failed reconnect, leaked the previous socket.
- `Drop` could panic (abort) when the public `connected` flag was set by hand.

### Changed
- `pdu_length`, `connected`, `last_time` and `chunks` are private; use `pdu_length()`,
  `is_connected()`, `last_time()` and `chunks()`.
- Transport and protocol errors now **drop the connection**; the next operation returns
  `NotConnected` until a `connect_*` call succeeds. `S7Error::requires_reconnect()` and
  `S7Error::is_plc_error()` classify errors.
- New variants: `Timeout` (replaces `Io(TimedOut | WouldBlock)`), `S7InvalidResponse`, and
  `WriteIncomplete { acknowledged, requested, source }` for failures in the middle of a write.
  `ConnectionClosed` is now actually returned on EOF / reset. `Other` was removed.
- `S7Error` lives in `rust7::error` (re-exported as `rust7::S7Error`).
- `S7Client` implements `Default`.

### Added
- `tests/resilience.rs`: fault-injection suite (late, split, short, foreign and truncated replies,
  mid-write timeouts, argument limits) against the in-process fake PLC.

## [0.1.2] - 2025-08-15

### Added
- Added parameter check
- Added `InvalidFunParam`

### Modified
- Excluded /target folder from deploy

## [0.1.1] - 2025-08-14
- Initial release
