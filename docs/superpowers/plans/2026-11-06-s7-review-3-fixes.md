# Rust7 — naprawy po przeglądzie 3 (R3) — plan implementacji

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Przypiąć testami kontrole długości odpowiedzi, raportować postęp transferu polem `confirmed`, poprawić diagnostykę timeoutu, błędów S7 i negocjacji, uporządkować targety przykładów i dokumentację.

**Architecture:** Logika protokołu zostaje w `src/client/protocol.rs`; konstruktory błędów w `src/error.rs`; stan klienta i rustdoc w `src/client.rs`. Liczniki transferu przechodzą przez nową prywatną strukturę `protocol::Progress`, kopiowaną do publicznych pól `chunks` i `confirmed`. Testy bez sieci używają transportu `Scripted`; transport TCP sprawdza `tests/tcp_smoke.rs`.

**Tech Stack:** Rust edition 2021, std, `anyerror` 0.1.13; dev: `serde_json`, `proptest` 1 (`default-features = false, features = ["std"]`, sprawdzone offline). Cargo, rustfmt, Clippy, reviewdog.

**Spec:** [docs/superpowers/specs/2026-11-06-s7-review-3-fixes-design.md](../specs/2026-11-06-s7-review-3-fixes-design.md) (zaakceptowany 2026-11-06). Wykonawca czyta spec i plan.

## Global Constraints

- Edition 2021, `#![forbid(unsafe_code)]`, kod synchroniczny, bez nowych zależności runtime; jedyna nowa dev-dependency: `proptest = { version = "1", default-features = false, features = ["std"] }`.
- Kategorie `kind`, `kind::ALL` (13) i reguły rozłączania bez zmian; zmienia się tylko treść komunikatów.
- Każda publiczna metoda dodaje dokładnie jeden kontekst; przyczyna (`source()`) nie jest kontekstem.
- Bez automatycznych ponowień zapisu; zapis wieloczęściowy pozostaje nieatomowy.
- Testy jednostkowe bez sieci; TCP tylko w `tests/tcp_smoke.rs` (`#[ignore]`). Bez kontaktu z PLC; przykłady tylko kompilowane.
- Wszystkie komendy Cargo z `--offline`, uruchamiane z katalogu `RustS7CommClient`.
- Gałąź `fix/s7-review-3` (spec: commit `1717415`). Commit na zadanie, konwencja `test:`/`fix:`/`feat:`/`docs:`, autor jak w repo:
  `git -c user.name="Bad badgers" -c user.email="207913709+rtmachines@users.noreply.github.com" commit`, wiadomość zakończona trailerem `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Każde zadanie zmieniające zachowanie lub dokumentację dopisuje jedną linię do `CHANGELOG.md`, sekcja `## [0.2.0] - Unreleased` (`### Added` / `### Changed` / `### Fixed`).
- Nie dodawać do commitów nieśledzonego `docs/superpowers/plans/2026-11-04-s7-review-2-fixes.md`.
- Numery linii w planie odnoszą się do `1ac916e`; po wcześniejszych zadaniach szukaj miejsca po cytowanej treści.

## Review Focus

- Bardzo duży `rd_timeout` (`u64::MAX` ms, termin `None`) z `WouldBlock`: ujednolicony komunikat, bez paniki (test w zadaniu 2).
- `read()` zwracające `Interrupted`: nadal ponawiane, nie zamieniane na timeout (test w zadaniu 2).
- Nagłówek błędu S7 z jedną zerową częścią (`[0x81, 0]`, `[0, 4]`): komunikat pokazuje obie wartości dokładnie (test w zadaniu 3).
- Zapis bitowy z buforem dłuższym niż 1 B (`[1, 2, 3]`): `confirmed == 1`, nie 3 (test w zadaniu 4).
- Pusty bufor bajtowy oraz błąd `NOT_CONNECTED`: `confirmed == 0` (test w zadaniu 4).

## Pliki

| Plik | Zadania |
|---|---|
| `Cargo.toml`, `Cargo.lock` | 1 (dev-dep), 5 (targety) |
| `src/client/protocol.rs` | 2 (`read_full`), 3 (`ack_data`, `item_status`, `negotiate`), 4 (`Progress`, transfery) |
| `src/error.rs`, `src/error/tests.rs` | 3 |
| `src/client.rs`, `src/client/tests.rs` | 3, 4, 6 |
| `src/client/protocol/tests.rs` | 1, 2, 3, 4 |
| `tests/tcp_smoke.rs` | 2, 4 |
| `tests/public_errors.rs` | 4 |
| `examples/sdk/client.rs`, `examples/README.md`, `examples/sdk/README.md` | 5 |
| `README.md`, `doc/Documentation.md`, `AGENTS.md`, `CHANGELOG.md` | 2–6 |

---

### Task 1: Testy przypinające kontrole długości i property testy (R3-01…03)

**Files:**
- Modify: `Cargo.toml` (`[dev-dependencies]`), `Cargo.lock`
- Test: `src/client/protocol/tests.rs`

**Interfaces:**
- Consumes: istniejące helpery testowe `Scripted`, `data_frame`, `ack`, `read_ack`, `transfer`, `assert_kind`.
- Produces: helper testowy `fn coherent_ack() -> impl Strategy<Value = Vec<u8>>` (lokalny dla modułu `properties`).

- [ ] **Step 1: Dodaj dev-dependency** w `Cargo.toml`: `proptest = { version = "1", default-features = false, features = ["std"] }`. Run: `cargo test --offline --lib 2>&1 | tail -3` → kompiluje się, 42 passed.

- [ ] **Step 2: Napisz testy przykładowe**

```rust
#[test]
fn read_payload_length_must_match_bits_field() {
    let limits = PduLimits::new(480).unwrap();
    let mut data = vec![0xff, 4, 0, 32];
    data.extend_from_slice(&[9; 5]);
    let mut io = Scripted::pieces([data_frame(&ack(1, 4, &data))]);
    let (mut reference, mut chunks) = (0, 0);
    let mut buffer = [0xaa; 4];
    let result = read_transfer(&mut io, &limits, &mut reference, transfer(0, false),
        &mut buffer, &mut chunks, Duration::from_secs(1));
    assert_kind(&result, kind::ISO_INVALID_TELEGRAM);
    assert_eq!(buffer, [0xaa; 4]);
}

#[test]
fn read_item_shorter_than_header_is_rejected() {
    assert_kind(&validate_read_response(&ack(1, 4, &[0xff, 4, 0]), 1, 2, 4), kind::ISO_INVALID_TELEGRAM);
}

#[test]
fn write_ack_with_extra_data_is_not_success() {
    for data in [&[0xff, 0x00][..], &[][..]] {
        assert_kind(&validate_write_response(&ack(1, 5, data), 1), kind::ISO_INVALID_TELEGRAM);
    }
}
```

- [ ] **Step 3: Uruchom** `cargo test --offline --lib -- read_payload_length read_item_shorter write_ack_with_extra` → 3 PASS (testy przypinające istniejące zachowanie).

- [ ] **Step 4: Red przez mutację** — dla każdej mutacji uruchom `cargo test --offline --lib`, zanotuj czerwony test, cofnij `git checkout -- src/client/protocol.rs`:
  - usuń warunek `|| data.len() - 4 != expected_payload` w `validate_read_response` → `read_payload_length_must_match_bits_field` FAIL (panika w `copy_from_slice`);
  - usuń blok `if data.len() < 4 { return Err(error::iso_invalid_telegram()); }` w `validate_read_response` → `read_item_shorter_than_header_is_rejected` FAIL;
  - zmień `if data.len() != 1` na `if data.is_empty()` w `validate_write_response` → `write_ack_with_extra_data_is_not_success` FAIL.

- [ ] **Step 5: Napisz property testy** w module `mod properties` na końcu `src/client/protocol/tests.rs` (`use proptest::prelude::*;`).
  `fn coherent_ack() -> impl Strategy<Value = Vec<u8>>` składa PDU: `0x32`, ROSCTR z `prop_oneof![Just(2), Just(3), any::<u8>()]`, `[0, 0]`, referencja z `prop_oneof![Just(1u16), any::<u16>()]`, pola długości obliczone z rzeczywistych parametrów i danych, błąd z `prop_oneof![Just([0, 0]), any::<[u8; 2]>()]`, parametry z `prop_oneof![Just(vec![4, 1]), Just(vec![5, 1]), vec(any::<u8>(), 0..=4)]`, dane z `prop_oneof![` element odczytu (`status ∈ {0xff, dowolny}`, `transport ∈ {3, 4, dowolny}`, `bits: u16` dowolne lub `payload.len() * 8`, payload `vec(any::<u8>(), 0..=40)`) `, vec(any::<u8>(), 0..=40)]`. Długość parametrów i danych zawsze zgodna z treścią, więc ramka przechodzi kontrolę `12 + params + data == len`.

```rust
proptest! {
    #[test]
    fn coherent_ack_never_panics(pdu in coherent_ack(), n in 1usize..=462, bit in any::<bool>()) {
        let (wl, n) = if bit { (1, 1) } else { (2, n) };
        match validate_read_response(&pdu, 1, wl, n) {
            Ok(data) => prop_assert_eq!(data.len(), n),
            Err(e) => prop_assert!(e.get_type().is_some_and(|k| kind::ALL.contains(&k))),
        }
        if let Err(e) = validate_write_response(&pdu, 1) {
            prop_assert!(e.get_type().is_some_and(|k| kind::ALL.contains(&k)));
        }
    }

    #[test]
    fn read_transfer_never_panics_and_keeps_buffer_on_error(pdu in coherent_ack(), n in 1usize..=462) {
        let limits = PduLimits::new(480).unwrap();
        let mut io = Scripted::pieces([data_frame(&pdu)]);
        let (mut reference, mut chunks) = (0, 0);
        let mut buffer = vec![0xaa; n];
        if read_transfer(&mut io, &limits, &mut reference, transfer(0, false),
            &mut buffer, &mut chunks, Duration::from_secs(1)).is_err() {
            prop_assert!(buffer.iter().all(|b| *b == 0xaa));
        }
    }
}
```

- [ ] **Step 6: Uruchom** `cargo test --offline --lib properties` → PASS. Red: powtórz pierwszą i drugą mutację z kroku 4 → `coherent_ack_never_panics` FAIL (z minimalnym kontrprzykładem); cofnij.

- [ ] **Step 7: Uruchom** `cargo fmt --all` i `cargo clippy --offline --all-targets -- -D warnings` → bez ostrzeżeń; `cargo test --offline` → wszystko PASS.

- [ ] **Step 8: Commit** `test: pin response length checks and add ACK property tests` (pliki: `Cargo.toml`, `Cargo.lock`, `src/client/protocol/tests.rs`).

### Task 2: Jeden komunikat timeoutu odczytu (R3-05)

**Files:**
- Modify: `src/client/protocol.rs` (`read_full`), `src/client.rs` (rustdoc `kind::IO` w `read_area`/`write_area`), `doc/Documentation.md` (te same listy i `set_timeout`), `CHANGELOG.md`
- Test: `src/client/protocol/tests.rs`, `tests/tcp_smoke.rs`

**Interfaces:**
- Produces: prywatne `fn timed_out(timeout: Duration) -> AnyError` w `protocol.rs`, zwracające `error::io(io::Error::new(io::ErrorKind::TimedOut, format!("no complete response frame within {} ms", timeout.as_millis())))`.
- Produces: `Scenario::LateReply(bool)` w `tests/tcp_smoke.rs` (`true` = zapis); zadanie 4 dopisuje do niego asercje.

- [ ] **Step 1: Przepisz testy**
  - `read_tpkt_propagates_eof_and_timeout`: dla `TimedOut` i `WouldBlock` oczekuj `kind::IO` oraz `err.to_string() == "rust7::Io: no complete response frame within 1000 ms"`; EOF bez zmian.
  - `frame_deadline_bounds_slow_peer`: zamiast tekstu `TimedOut` oczekuj `err.to_string().contains("no complete response frame within 100 ms")`.
  - Nowy `huge_timeout_with_wouldblock_reports_unified_message`: `Scripted::pieces([vec![3, 0, 0, 22]])` plus `Err(WouldBlock)`; `read_tpkt(&mut io, 487, Duration::from_millis(u64::MAX))` → `kind::IO`, komunikat zawiera `within 18446744073709551615 ms`.
  - Nowy `interrupted_read_is_retried`: odczyty `Err(Interrupted)`, potem cała ramka `data_frame(&[0x32; 20])` → `Ok(frame)`.

- [ ] **Step 2: Uruchom** `cargo test --offline --lib -- read_tpkt_propagates frame_deadline huge_timeout_with interrupted_read` → 3 FAIL (stary tekst), `interrupted_read_is_retried` PASS.

- [ ] **Step 3: Zaimplementuj** w `read_full`: gałąź `limit.is_zero()` oraz `Err(e)` z `e.kind()` równym `WouldBlock` lub `TimedOut` zwracają `timed_out(timeout)`. `Interrupted` i pozostałe błędy bez zmian.

- [ ] **Step 4: Uruchom** testy z kroku 2 → PASS.

- [ ] **Step 5: tcp_smoke** — zamień `Scenario::LateReply` na `LateReply(bool)`. Serwer: gdy `request[17] == 5`, sprawdź `assert_eq!(&request[35..], &[1, 2, 3, 4])` (część z błędem dotarła do PLC), potem po 400 ms wyślij `reply(&request[11..13], request[17], &data)` z `data = [0xff]` dla zapisu i dotychczasowymi danymi odczytu. Klient: `writing` → `client.write_db(100, 0, &[1, 2, 3, 4])`, inaczej obecny `read_db`. Asercje: `kind::IO`, komunikat zawiera `no complete response frame within 200 ms`, `client.chunks == 1`, rozłączenie jak dotąd. `late_reply_disconnects_client` uruchamia `for writing in [false, true]`.

- [ ] **Step 6: Uruchom** `cargo test --offline --test tcp_smoke -- --ignored` → 10 PASS (brak gniazd = NOT RUN, zgłoś).

- [ ] **Step 7: Dokumentacja** — rustdoc `read_area`/`write_area` i odpowiadające listy w `doc/Documentation.md`: `kind::IO`: „network I/O error; a response not received within the read timeout reports `no complete response frame within <rd_timeout_ms> ms`”. CHANGELOG `### Changed`: „A read timeout always reports `no complete response frame within <N> ms` (`kind::IO`), whether the socket or the frame deadline expired”.

- [ ] **Step 8: Commit** `fix: report read timeouts with one message` (pliki z sekcji Files).

### Task 3: Kody błędów S7 i przyczyna przy negocjacji (R3-06, R3-07)

**Files:**
- Modify: `src/error.rs`, `src/client/protocol.rs` (`ack_data`, `item_status`, `negotiate`), `src/client.rs` (rustdoc `connect_tsap`, `read_area`, `write_area`), `doc/Documentation.md`, `CHANGELOG.md`
- Test: `src/error/tests.rs`, `src/client/protocol/tests.rs`, `src/client/tests.rs`

**Interfaces:**
- Produces (w `src/error.rs`, `pub(crate)`):
  - `fn s7_header_error(class: u8, code: u8) -> AnyError` — kategoria `S7_UNSPECIFIED`, komunikat `S7 unspecified error (error class {class:#04x}, code {code:#04x})`;
  - `fn s7_item_error(code: u8) -> AnyError` — kategoria `S7_UNSPECIFIED`, komunikat `S7 unspecified error (item return code {code:#04x})`;
  - `fn pdu_negotiation_failed_by(source: AnyError) -> AnyError` — kategoria `PDU_NEGOTIATION_FAILED`, komunikat `S7 PDU negotiation failed`, `source()` = przekazany błąd (prywatny typ `Caused { msg: &'static str, source: AnyError }` z `Display` = `msg`, `Error::source` = `Some(&self.source)`; `AnyError::new(&caused).with_type(Some(kind::PDU_NEGOTIATION_FAILED))` klonuje przyczynę razem z jej kategorią).
- Removes: `s7_unspecified()`.

- [ ] **Step 1: Napisz testy** w `src/error/tests.rs`:
  - z tabeli `constructors_set_known_kind` usuń wiersz `s7_unspecified` (tablica `[Case; 11]`);
  - `s7_error_messages_carry_codes`: `s7_header_error(0x81, 4).to_string() == "rust7::S7Unspecified: S7 unspecified error (error class 0x81, code 0x04)"`, `s7_header_error(0, 4)` → `(error class 0x00, code 0x04)`, `s7_item_error(3).to_string() == "rust7::S7Unspecified: S7 unspecified error (item return code 0x03)"`, oba bez `source()`;
  - `negotiation_failure_keeps_cause`: `let e = pdu_negotiation_failed_by(iso_fragmented_packet());` → `e.get_type() == Some(kind::PDU_NEGOTIATION_FAILED)`, `e.source().and_then(|s| s.downcast_ref::<AnyError>()).and_then(|s| s.get_type()) == Some(kind::ISO_FRAGMENTED_PACKET)`, `e.to_string() == "rust7::PduNegotiationFailed: S7 PDU negotiation failed; source: rust7::IsoFragmentedPacket: Fragmented ISO Packet"`.

  W `src/client/protocol/tests.rs`:
  - `terminal_error_ack_reports_plc_error`: dla każdego `error` sprawdź też, że komunikat zawiera `format!("(error class {:#04x}, code {:#04x})", error[0], error[1])` (odczyt i zapis);
  - `response_preserves_plc_errors`: dla statusu 7 komunikat zawiera `(item return code 0x07)`;
  - nowy `negotiation_keeps_framing_error_as_source`: `negotiation_response(480)` z `bad[5] = 0` → kategoria `PDU_NEGOTIATION_FAILED`, przyczyna `ISO_INVALID_HEADER`; z `bad[6] = 0` → przyczyna `ISO_FRAGMENTED_PACKET`.

  W `src/client/tests.rs` (`only_transport_errors_break_the_session`): zamiast `error::s7_unspecified()` użyj `error::s7_header_error(0x81, 4)` i `error::s7_item_error(3)`.

- [ ] **Step 2: Uruchom** `cargo test --offline --lib` → FAIL kompilacji (brak nowych funkcji).

- [ ] **Step 3: Zaimplementuj** konstruktory i `Caused` w `src/error.rs`; usuń `s7_unspecified`. W `protocol.rs`: obie ścieżki błędu nagłówka w `ack_data` → `error::s7_header_error(pdu[10], pdu[11])`; `item_status` gałąź `_` → `code => Err(error::s7_item_error(code))`; `negotiate` → `data_payload(&frame).map_err(error::pdu_negotiation_failed_by)?`.

- [ ] **Step 4: Uruchom** `cargo test --offline` → PASS; `cargo test --offline --test tcp_smoke -- --ignored` → PASS.

- [ ] **Step 5: Dokumentacja** — rustdoc `read_area`/`write_area` (`kind::S7_UNSPECIFIED`: „the message carries the S7 error class and code, or the item return code”), `connect_tsap` (`kind::PDU_NEGOTIATION_FAILED`: „a framing error in the reply is the error's `source()`”); te same zdania w `doc/Documentation.md`. CHANGELOG `### Changed`: „`kind::S7_UNSPECIFIED` messages include the S7 error class and code or the item return code; a framing error during PDU negotiation is kept as the `source()` of `kind::PDU_NEGOTIATION_FAILED`”.

- [ ] **Step 6: Uruchom** `cargo clippy --offline --all-targets -- -D warnings` i `cargo doc --offline --no-deps` → bez ostrzeżeń.

- [ ] **Step 7: Commit** `fix: keep S7 error codes and negotiation causes in errors`.

### Task 4: Pole `confirmed` i dokumentacja części z błędem (R3-04)

**Files:**
- Modify: `src/client/protocol.rs` (`Progress`, `read_transfer`, `write_transfer`), `src/client.rs` (pole, `new`, `*_impl`, rustdoc), `README.md` („Transfer behavior”), `doc/Documentation.md` (pola klienta, `read_area`, `write_area`, zachowanie transferu), `AGENTS.md` („Ograniczenia projektu”), `CHANGELOG.md`
- Test: `src/client/protocol/tests.rs`, `src/client/tests.rs`, `tests/public_errors.rs`, `tests/tcp_smoke.rs`

**Interfaces:**
- Produces (w `protocol.rs`): `#[derive(Default)] pub(super) struct Progress { pub(super) chunks: usize, pub(super) confirmed: usize }`.
- Changes: `read_transfer(io, limits, reference, transfer, buffer: &mut [u8], progress: &mut Progress, timeout)` oraz `write_transfer(..., buffer: &[u8], progress: &mut Progress, timeout)` — parametr `chunks: &mut usize` zastąpiony przez `progress` (osobny parametr przekroczyłby limit Clippy `too_many_arguments`). `progress.chunks += 1` przed wysłaniem części (jak dotąd), `progress.confirmed = offset + count` po zaakceptowaniu części.
- Produces (publiczne): `S7Client::confirmed: usize` z rustdoc: „Number of buffer bytes confirmed in the last read/write: copied from verified responses (read) or acknowledged by the PLC (write). Reset with `chunks`. After an error, bytes `[0, confirmed)` are confirmed; the chunk starting at `confirmed` may or may not have been written by the PLC; later bytes were not sent.”

- [ ] **Step 1: Napisz testy**
  - W `src/client/protocol/tests.rs` zamień we wszystkich wywołaniach `&mut chunks` na `&mut progress` (`let mut progress = Progress::default();`), a asercje `chunks` na `progress.chunks`.
  - `second_write_chunk_failure_stops_without_retry`: dodaj `assert_eq!(progress.confirmed, 212)` (PDU 240 → 212 B na część).
  - Nowy `read_progress_counts_only_accepted_chunks`: PDU 240 (222 B na część), bufor 600 × `0xaa`, odpowiedzi: `read_ack(1, &[7; 222], false)`, potem `ack(2, 4, &[5, 0, 0, 0])` → `S7_INVALID_ADDRESS`, `(progress.chunks, progress.confirmed) == (2, 222)`, `buffer[..222] == [7; 222]`, `buffer[222..]` same `0xaa`.
  - Nowy `bit_write_confirms_one_byte`: `write_transfer` bitowy z buforem `[1, 2, 3]` i ACK `[0xff]` → `progress.confirmed == 1`.
  - Nowy `empty_byte_transfer_confirms_nothing`: `write_transfer` bajtowy z `&[]` → `Ok`, `progress.confirmed == 0`, `progress.chunks == 0`.
  - `src/client/tests.rs`, `invalid_public_inputs_clear_statistics_and_return_errors`: ustaw `client.confirmed = 9` i sprawdzaj `(client.last_time, client.chunks, client.confirmed) == (0.0, 0, 0)`; w `public_bit_helpers…` po `NOT_CONNECTED` `client.confirmed == 0`.
  - `tests/public_errors.rs`, nowy `progress_fields_are_public`: `let c = S7Client::new(); let _: (usize, usize) = (c.chunks, c.confirmed);`.
  - `tests/tcp_smoke.rs`: Normal — po `read_db` 65536 B i po `write_db` `client.confirmed == 65536`, po `read_bit` `== 1`; `AckError(_)` — `client.confirmed == 0`; `LateReply(_)` — `client.confirmed == 0`.

- [ ] **Step 2: Uruchom** `cargo test --offline` → FAIL kompilacji (brak `Progress`, `confirmed`).

- [ ] **Step 3: Zaimplementuj** `Progress` i nowe sygnatury w `protocol.rs`; w `S7Client` pole `confirmed` (zainicjowane 0 w `new`), zerowanie razem z `chunks` w `read_area_impl`, `write_area_impl`, `read_bit_impl`, `write_bit_impl`; w `*_area_impl` lokalne `let mut progress = protocol::Progress::default();` przekazane do transferu, a przed `self.finish(...)` skopiowane do `self.chunks` i `self.confirmed`.

- [ ] **Step 4: Uruchom** `cargo test --offline` i `cargo test --offline --test tcp_smoke -- --ignored` → PASS.

- [ ] **Step 5: Dokumentacja**
  - Rustdoc `write_area`, `write_db`, `write_bit`: nota „Writes are not atomic. After an error, bytes `[0, confirmed)` were acknowledged by the PLC; the chunk starting at `confirmed` may or may not have been written, also for a single-chunk write or `write_bit`; later bytes were not sent. The client never retries a write.” `read_area`/`read_db`: „On error only `[0, confirmed)` of the buffer is a result.”
  - `README.md` „Transfer behavior” i `doc/Documentation.md` (opis pól klienta przy `chunks`, notes `write_area`, sekcja zachowania transferu): ta sama treść.
  - `AGENTS.md`, „Ograniczenia projektu”: zdanie o nieatomowym zapisie uzupełnij o „także część, przy której wystąpił błąd; potwierdzone bajty to `[0, confirmed)`”.
  - CHANGELOG `### Added`: „`S7Client::confirmed` reports the buffer bytes confirmed in the last read or write; after a write error the chunk starting there may have been applied”; w „Migration … Unchanged” dopisz odwołanie do `confirmed`.

- [ ] **Step 6: Uruchom** `cargo fmt --all`, `cargo clippy --offline --all-targets -- -D warnings`, `cargo doc --offline --no-deps` → bez ostrzeżeń.

- [ ] **Step 7: Commit** `feat: report confirmed transfer progress`.

### Task 5: Targety Cargo przykładów (R3-13, R3-14, R3-15)

**Files:**
- Modify: `Cargo.toml`, `examples/sdk/client.rs`, `AGENTS.md` (tabela komend), `examples/README.md`, `examples/sdk/README.md`, `CHANGELOG.md`

**Interfaces:** brak zmian API. Produces: przykład Cargo `sdk_client`.

- [ ] **Step 1: Pokaż stan wyjściowy** `cargo metadata --offline --no-deps --format-version 1 | python3 -c 'import json,sys; print([t["name"] for p in json.load(sys.stdin)["packages"] for t in p["targets"]])'` → zawiera `docker`, nie zawiera `sdk_client`.

- [ ] **Step 2: `Cargo.toml`** — w `[package]` `autoexamples = false`; nowa sekcja `[[example]]` z `name = "sdk_client"`, `path = "examples/sdk/client.rs"`.

- [ ] **Step 3: Uruchom** metadata z kroku 1 → `['rust7', 'sdk_client', 'public_errors', 'tcp_smoke']` (bez `docker`).

- [ ] **Step 4: Oczyść przykład** — `cargo fmt --all`, potem `cargo clippy --offline --all-targets -- -D warnings`; popraw zgłoszenia w `examples/sdk/client.rs` bez zmiany zachowania (np. `println!("")` → `println!()`). R3-15: etykieta `Attempt to read DB100.DBX47.5`, zgodna z `read_bit(…, 47, 5)`.

- [ ] **Step 5: Dowód objęcia** — dopisz tymczasowo na końcu `examples/sdk/client.rs` linię `fn broken() -> u8 { "x" }`; `cargo check --offline --all-targets` → FAIL w `examples/sdk/client.rs`; cofnij zmianę.

- [ ] **Step 6: Dokumentacja** — `AGENTS.md`: wiersz tabeli „Kontrola kompilacji wszystkich targetów” uzupełnij o „(w tym przykładu `sdk_client`)”; zdanie o `examples/docker` jako osobnym pakiecie zostaje (teraz prawdziwe). `examples/README.md` i `examples/sdk/README.md`: budowanie `cargo build --offline --example sdk_client`; uruchamianie tylko przeciw wskazanemu urządzeniu. CHANGELOG `### Fixed`: „The SDK example builds as `sdk_client` and is checked by `cargo check --all-targets`; `examples/docker` is no longer compiled as an example of the `rust7` package”.

- [ ] **Step 7: Uruchom** `cargo fmt --all -- --check`, `cargo clippy --offline --all-targets -- -D warnings`, `cargo test --offline` → PASS.

- [ ] **Step 8: Commit** `fix: build the SDK example and stop compiling the Docker example in the main package`.

### Task 6: Dokumentacja zgodna z kodem (R3-16…R3-22)

**Files:**
- Modify: `src/client.rs` (rustdoc, komentarze CR), `src/error.rs` (opis stałej `CONNECTION_CLOSED`), `doc/Documentation.md`, `CHANGELOG.md`

**Interfaces:** brak zmian kodu wykonywalnego.

- [ ] **Step 1: Wprowadź zmiany** (treść zgodnie ze spec, sekcja G):
  - R3-16: rustdoc `set_connection_type` — `CT_PG`, `CT_OP`, `CT_S7` zamiast `S7_PG`, `S7_OP`, `S7_BASIC`.
  - R3-17: Documentation `set_connection_port` — parametr `port`: TCP connection port (1..65535); sygnatura `write_area` — `buffer: &[u8]`.
  - R3-18: Documentation — `kind::INVALID_FUN_PARAMETER` w listach „High level” `read_area` i `write_area`; nota bitowa „Bit access sends one bit taken from the first buffer byte (true if != 0)” zamiast „number of bytes to write = buffer size” dla bitów; lista błędów `connect_tsap` identyczna z rustdoc (z dopiskami o opcji TPDU i przekroczeniu oferty).
  - R3-19: Documentation, pętla — `client` zamiast `Client`, `if let Err(e) = client.connect_s71200_1500(ip_address) { eprintln!("{e:#}"); }`, `thread::sleep(Duration::from_millis(100))`.
  - R3-20: komentarze CR w `connect_tsap_impl` — `0x11`: COTP header length (LI = 17); `0xC0`: TPDU size parameter; `0x01`: parameter length; `0x0A`: TPDU size exponent (2^10 = 1024).
  - R3-21: rustdoc `set_timeout` i Documentation — `wr_timeout_ms`: socket send timeout for each request (ms); the PLC acknowledgement of a write is bounded by `rd_timeout_ms`.
  - R3-22: rustdoc `read_area`/`write_area` i Documentation — `kind::CONNECTION_CLOSED`: the peer closed the connection (end of stream while reading); a connection reset is reported as `kind::IO`. Opis stałej w `src/error.rs` uzupełnij tym samym zdaniem.

- [ ] **Step 2: Sprawdź**
  - `grep -nE 'S7_OP|S7_BASIC|S7_PG|Client\.connected|let _ = client\.connect|PDU Max Length|Write Connection timeout' src/client.rs doc/Documentation.md` → brak trafień;
  - `grep -n 'connection_type' doc/Documentation.md` → tylko sekcja `set_connection_type`;
  - `cargo doc --offline --no-deps` → bez ostrzeżeń; `cargo test --offline --doc` → PASS.

- [ ] **Step 3: CHANGELOG** `### Fixed`: „Documentation and comments match the code: connection type constants, `set_connection_port` and `write_area` parameters, error lists, timeout and `CONNECTION_CLOSED` descriptions, the reconnect loop and Connection Request byte labels”.

- [ ] **Step 4: Commit** `docs: align documentation and comments with the code`.

### Task 7: Weryfikacja końcowa

- [ ] **Step 1:** `cargo fmt --all -- --check` → exit 0.
- [ ] **Step 2:** `set -o pipefail; rtk bypass cargo clippy --offline --all-targets --message-format=short --color=never 2>&1 | reviewdog -f=clippy -reporter=local -filter-mode=nofilter -fail-level=warning` → exit 0; `cargo clippy --offline --all-targets -- -D warnings` → exit 0.
- [ ] **Step 3:** `cargo test --offline` i `cargo test --offline --release` → PASS (property testy w obu).
- [ ] **Step 4:** `cargo test --offline --test tcp_smoke -- --ignored` → 10 passed (brak gniazd = NOT RUN, nie PASS).
- [ ] **Step 5:** `cargo doc --offline --no-deps` → bez ostrzeżeń.
- [ ] **Step 6:** `unshare -rn cargo test --offline` (jeśli dostępne) → PASS.
- [ ] **Step 7:** metadata z zadania 5, krok 1 → `sdk_client` obecny, `docker` nieobecny.
- [ ] **Step 8:** Mutacje z przeglądu (F1–F7, S7-01…S7-08, R3-01…03) w tymczasowym worktree w scratchpadzie: każda daje czerwony test; worktree usunięty po sprawdzeniu.
- [ ] **Step 9:** `CHANGELOG.md` `[0.2.0] - Unreleased`: po jednej linii z zadań 2–6; `git status --short` czysty poza nieśledzonym planem z 2026-11-04.
