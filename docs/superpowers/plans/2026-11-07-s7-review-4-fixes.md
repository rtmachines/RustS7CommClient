# Rust7 — naprawy po przeglądzie 4 (R4-01…R4-05) — plan implementacji

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Przypiąć testami adresowanie i wartość bitu w publicznej warstwie `S7Client`, stałe bajty żądań S7 i rozłączanie po błędach ramkowania, oraz uzupełnić CHANGELOG o naprawy F1–F7 względem 0.1.2.

**Architecture:** Mapowanie argumentów publicznych na `protocol::Transfer` przechodzi do czystych funkcji w `src/client/protocol.rs` (seam testowalny bez sieci); `S7Client::*_area_impl` przyjmują gotowy `Transfer`. Testy jednostkowe (`Scripted`, bez sieci) pokrywają seam, bajty żądań i `finish`; `tests/tcp_smoke.rs` dostaje asercje stałych nagłówka, rejestr żądań i nowe scenariusze. Zachowanie i publiczne API bez zmian.

**Tech Stack:** Rust edition 2021, std, `anyerror` 0.1.13; dev: `serde_json`, `proptest` 1. Cargo, rustfmt, Clippy, reviewdog, cargo-mutants 27.1.0.

**Spec:** [docs/superpowers/specs/2026-11-07-s7-review-4-fixes-design.md](../specs/2026-11-07-s7-review-4-fixes-design.md) (zaakceptowany 2026-11-07); źródło znalezisk: [raport R4](../specs/2026-11-07-s7-review-4-report.md). Wykonawca czyta spec i plan.

## Global Constraints

- Edition 2021, `#![forbid(unsafe_code)]`, kod synchroniczny, bez nowych zależności (także dev).
- Bez zmian zachowania i publicznego API: sygnatury, kategorie `kind`, reguły rozłączania, konteksty błędów, kolejność kontroli parametrów. `tests/public_errors.rs` bez modyfikacji.
- Testy jednostkowe bez sieci; TCP tylko w `tests/tcp_smoke.rs` (`#[ignore]`). Bez wstrzykiwania transportu do `S7Client`. Bez kontaktu z PLC.
- Wszystkie komendy Cargo z `--offline`, uruchamiane z katalogu `RustS7CommClient`.
- Gałąź `fix/s7-review-4` z `5c09959` (`test/tcp-client-mutants`). Commit na zadanie, konwencja `test:`/`refactor:`/`docs:`, autor jak w repo:
  `git -c user.name="Bad badgers" -c user.email="207913709+rtmachines@users.noreply.github.com" commit`, wiadomość zakończona trailerem `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- CHANGELOG zmienia tylko zadanie 5; testy i seam nie zmieniają zachowania.
- Nie dodawać do commitów nieśledzonego `docs/superpowers/plans/2026-11-04-s7-review-2-fixes.md`. Spec, raport R4 i ten plan commituje zadanie 0.
- Numery linii odnoszą się do `5c09959`; po wcześniejszych zadaniach szukaj miejsca po cytowanej treści.
- Dowód mutacyjny: mutację wprowadź ręcznie, uruchom wskazaną komendę (oczekiwany FAIL), cofnij `git checkout -- <plik>` przed commitem i potwierdź PASS. Mutacji nie commituje się.

## Review Focus

- `write_area(…, S7_WL_BIT, &[5, 0])` przez warstwę publiczną: wysyłany payload `[1]` pod adresem bitowym `start` (test w zadaniu 4, C3).
- `read_area(…, S7_WL_BIT, &mut [0xaa, 0xbb])`: `buffer[0]` dostaje bajt z PLC, `buffer[1]` zostaje `0xbb` (test w zadaniu 4, C3).
- Największy adres bitu `Transfer::bit(S7_AREA_DB, 1, 65535, 7)` → `start == 524287` bez zawijania (test w zadaniu 3, B1).
- Skrajne wartości DB `Transfer::db(u16::MAX, u16::MAX)` → `db_number == 65535`, `start == 65535` (test w zadaniu 3, B1).

## Pliki

| Plik | Zadania |
|---|---|
| `src/client/protocol.rs` | 3 (konstruktory `Transfer`, `bit_payload`, `bit_value`, derive) |
| `src/client.rs` | 3 (`*_area_impl(Transfer, …)`, wywołania w metodach publicznych i `*_bit_impl`) |
| `src/client/protocol/tests.rs` | 1 (B2), 3 (B2 na konstruktorach) |
| `src/client/tests.rs` | 2 (B3), 3 (B1) |
| `tests/tcp_smoke.rs` | 1 (C1), 2 (C4), 4 (C2, C3) |
| `CHANGELOG.md` | 5 |

---

### Task 0: Gałąź i dokumenty

**Files:** `docs/superpowers/specs/2026-11-07-s7-review-4-report.md`, `docs/superpowers/specs/2026-11-07-s7-review-4-fixes-design.md`, `docs/superpowers/plans/2026-11-07-s7-review-4-fixes.md`

- [ ] **Step 1:** `git switch -c fix/s7-review-4 5c09959`; `git status --short` pokazuje tylko trzy powyższe pliki i `2026-11-04-s7-review-2-fixes.md` jako nieśledzone.
- [ ] **Step 2:** Baseline: `cargo test --offline` → 93 + 11 + 6 pass; `cargo test --offline --test tcp_smoke -- --ignored` → 18 pass. Brak gniazd → zapisz NOT RUN i zatrzymaj się (zadania 1, 2, 4 wymagają tcp_smoke).
- [ ] **Step 3:** Commit trzech dokumentów: `docs: add review 4 report, fix design and plan`.

### Task 1: Bajty referencyjne żądań (R4-03)

**Files:**
- Test: `src/client/protocol/tests.rs` (nowy test na końcu modułu, poza `mod properties`/`mod invariants`)
- Test: `tests/tcp_smoke.rs` (`serve`, pętla żądań, zaraz po `assert_eq!(&request[11..13], …)`)

**Interfaces:**
- Consumes: `request(transfer: &Transfer, offset: usize, count: usize, reference: u16, payload: Option<&[u8]>) -> Vec<u8>` (`protocol.rs:301`), `Transfer { area, db_number, start, wordlen }`.
- Produces: test `requests_match_reference_bytes` (zadanie 3 zamienia w nim literały `Transfer` na konstruktory).

- [ ] **Step 1: Test B2 `requests_match_reference_bytes`.** Cztery `assert_eq!` całych ramek; `Transfer` jako literał struktury. Każde pole ramki opisane komentarzem (TPKT, COTP DT, nagłówek Job, parametry, nagłówek danych). Komentarz testu: układ S7comm (Snap7, dissector Wireshark s7comm), bez pcap z PLC to referencja, nie dowód zgodności z firmware.

```rust
// odczyt bajtów: Transfer { area: 0x84, db_number: 0x1234, start: 0x0102, wordlen: 2 }, offset 0, count 3, ref 0x0a0b, None
[3,0,0,0x1f, 2,0xf0,0x80, 0x32,1,0,0,0x0a,0x0b,0,0x0e,0,0,
 4,1,0x12,0x0a,0x10,2,0,3,0x12,0x34,0x84,0,0x08,0x10]
// odczyt bitu: Transfer { area: 0x83, db_number: 0, start: 83, wordlen: 1 }, count 1, ref 0x0102, None
[3,0,0,0x1f, 2,0xf0,0x80, 0x32,1,0,0,0x01,0x02,0,0x0e,0,0,
 4,1,0x12,0x0a,0x10,1,0,1,0,0,0x83,0,0,0x53]
// zapis bajtów: Transfer { area: 0x84, db_number: 0x1234, start: 0x0102, wordlen: 2 }, count 3, ref 0x0a0c, Some(&[0xde,0xad,0xbe])
[3,0,0,0x26, 2,0xf0,0x80, 0x32,1,0,0,0x0a,0x0c,0,0x0e,0,7,
 5,1,0x12,0x0a,0x10,2,0,3,0x12,0x34,0x84,0,0x08,0x10, 0,4,0,0x18,0xde,0xad,0xbe]
// zapis bitu: Transfer { area: 0x82, db_number: 0, start: 83, wordlen: 1 }, count 1, ref 0x0a0d, Some(&[0])
[3,0,0,0x24, 2,0xf0,0x80, 0x32,1,0,0,0x0a,0x0d,0,0x0e,0,5,
 5,1,0x12,0x0a,0x10,1,0,1,0,0,0x82,0,0,0x53, 0,3,0,1,0]
```

Wartości sprawdzone przez koordynatora na `5c09959` (sonda w scratch-worktree, wynik `ok`).

- [ ] **Step 2: PASS:** `cargo test --offline --lib requests_match_reference_bytes` (unit: 94). Dowód literału: zmiana jednego bajtu w oczekiwanej ramce → FAIL; cofnij.
- [ ] **Step 3: Dowód mutacyjny B2** (komenda jak w kroku 2, każda → FAIL): w `request()` M01 `{ 3 } else { 4 }` → `{ 3 } else { 9 }` (`:361`); M03 `0x10` → `0x11` (`:332`); M04 `0x12` → `0x13` (`:330`); M05 `0x0a` → `0x0b` (`:331`); M06 item count `1` → `2` (`:329`); M07 `14` → `15` (`:325`); M08 ROSCTR `1` → `7` (`:319`); M09 `0x32` → `0x33` (`:318`). Cofnij każdą.
- [ ] **Step 4: C1 w mocku.** Po kontroli referencji, dla każdego żądania:

```rust
assert_eq!(&request[4..11], &[2, 0xf0, 0x80, 0x32, 1, 0, 0]);
assert_eq!(&request[13..15], &[0, 14]);
assert_eq!(&request[18..22], &[1, 0x12, 0x0a, 0x10]);
// odczyt: [15..17] == [0, 0]; zapis: [15..17] == (count + 4) as u16,
// [31] == 0, [32] == if bit { 3 } else { 4 }, [33..35] == if bit { 1 } else { count * 8 } as u16
```

- [ ] **Step 5: PASS:** `cargo test --offline --test tcp_smoke -- --ignored` → 18 pass. Dowód: M01 i M09 → ten sam suite FAIL; cofnij.
- [ ] **Step 6:** `cargo fmt --all`, commit `test: pin S7 request bytes`.

### Task 2: Reguła sesji dla każdej kategorii (R4-04)

**Files:**
- Test: `src/client/tests.rs` (nowy test obok `finish_preserves_session_only_for_valid_error_ack`)
- Test: `tests/tcp_smoke.rs` (`Scenario`, `serve`, `run`, dwa nowe testy)

**Interfaces:**
- Consumes: `S7Client::finish(&mut self, result: Result<()>, time: Instant) -> Result<()>` (`client.rs:653`); konstruktory z `src/error.rs`: `io`, `not_connected`, `tcp_connection_failed`, `connection_closed`, `iso_connection_failed`, `iso_fragmented_packet`, `iso_invalid_header`, `iso_invalid_telegram`, `pdu_negotiation_failed`, `invalid_fun_parameter`, `s7_not_found`, `s7_invalid_address`, `s7_header_error(0x81, 4)`.
- Produces: `Scenario::BadHeader(bool)`, `Scenario::Fragment(bool)` (bool = zapis).

- [ ] **Step 1: Test B3 `finish_applies_the_session_rule_of_every_kind`.** Tabela 13 pozycji `(kind, AnyError, breaks: bool)`; `breaks == true` wyłącznie dla `IO`, `CONNECTION_CLOSED`, `ISO_INVALID_HEADER`, `ISO_INVALID_TELEGRAM`, `ISO_FRAGMENTED_PACKET` (wpisane ręcznie, nie przez `breaks_session`). Asercje:
  - posortowany zbiór kind z tabeli == posortowany `kind::ALL`;
  - dla każdej pozycji świeży klient: `connected = true`, `pdu_length = 125`, `limits = Some(protocol::PduLimits::new(125).unwrap())`; `let e = client.finish(Err(err), Instant::now()).unwrap_err();` → `e.get_type() == Some(kind)`; gdy `breaks`: `!connected`, `limits.is_none()`, `pdu_length == 0`; inaczej `connected`, `limits.is_some()`, `pdu_length == 125`;
  - `AnyError::error("x")` (bez kategorii) i `AnyError::error("x").with_type(Some("other::X"))` → rozłączenie.
- [ ] **Step 2: PASS:** `cargo test --offline --lib finish_applies_the_session_rule_of_every_kind` (unit: 95). Dowód: w `finish` (`client.rs:660`) M13 `if error::breaks_session(&e) && e.get_type() != Some(error::kind::ISO_FRAGMENTED_PACKET)` → FAIL; M14 to samo z `ISO_INVALID_HEADER` → FAIL; cofnij.
- [ ] **Step 3: Scenariusze C4 w mocku.** Pierwsza odpowiedź na żądanie (`reference == 1`) budowana jak zwykle (`reply(…)`, także zapis do `written`), potem psuta przed wysłaniem: `BadHeader(_)` → `response[0] = 2`; `Fragment(_)` → `response[6] = 0`. Wysyłana w całości (`send_split`), więc w gnieździe zostają resztki; dalej mock kończy przy EOF jak dotąd.
- [ ] **Step 4: Strona klienta C4 w `run`.** Rozszerz gałąź `AckError(writing) | MalformedAck(writing)` o `BadHeader(writing) | Fragment(writing)`: bufor `[0xaa; 600]`, `write_db(100, 0, &buffer)` lub `read_db(100, 0, &mut buffer)`; asercje jak dla `MalformedAck`: bufor bez zmian, `chunks == 1`, `confirmed == 0`, `last_time == 0.0`, kategoria `ISO_INVALID_HEADER` / `ISO_FRAGMENTED_PACKET`, `!connected`, `pdu_length == 0`, `write_db(100, 0, &[1])` → `NOT_CONNECTED`.
- [ ] **Step 5: Testy** `bad_tpkt_header_disconnects_client` i `fragmented_reply_disconnects_client` (`#[ignore = "requires permission to open loopback sockets"]`, pętla `for writing in [false, true]`).
- [ ] **Step 6: PASS:** `cargo test --offline --test tcp_smoke -- --ignored` → 20 pass. Dowód: M13 → `fragmented_reply_disconnects_client` FAIL; M14 → `bad_tpkt_header_disconnects_client` FAIL; cofnij.
- [ ] **Step 7:** `cargo fmt --all`, commit `test: check disconnect after every framing error`.

### Task 3: Seam mapowania argumentów (R4-01, R4-02, domyślny suite)

**Files:**
- Modify: `src/client/protocol.rs:264-276` (`Transfer`, nowe funkcje obok `bit_start`)
- Modify: `src/client.rs:485`, `:494-531`, `:603`, `:612-649`, `:686`, `:738-753`, `:775`, `:827-840`
- Test: `src/client/tests.rs` (B1), `src/client/protocol/tests.rs` (B2 na konstruktorach)

**Interfaces:**
- Produces (w `protocol.rs`, widoczność `pub(super)`):
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] struct Transfer` (pola bez zmian)
  - `impl Transfer { fn area(area: u8, db_number: u16, start: u16, wordlen: u8) -> Transfer; fn db(db_number: u16, start: u16) -> Transfer; fn bit(area: u8, db_number: u16, byte_num: u16, bit_idx: u8) -> Result<Transfer> }`
  - `fn bit_payload(value: bool) -> [u8; 1]`, `fn bit_value(byte: u8) -> bool`
  - w `client.rs`: `fn read_area_impl(&mut self, transfer: protocol::Transfer, buffer: &mut [u8]) -> Result<()>`, `fn write_area_impl(&mut self, transfer: protocol::Transfer, buffer: &[u8]) -> Result<()>`

- [ ] **Step 1: Test B1 `public_arguments_map_to_transfer`** (`src/client/tests.rs`):
  - dla `a` ∈ `[S7_AREA_PE, S7_AREA_PA, S7_AREA_MK, S7_AREA_DB]`: `Transfer::area(a, 9, 0x1234, S7_WL_BYTE) == Transfer { area: a, db_number: 9, start: 0x1234, wordlen: S7_WL_BYTE }`;
  - `Transfer::db(7, 3) == Transfer { area: S7_AREA_DB, db_number: 7, start: 3, wordlen: S7_WL_BYTE }`; `Transfer::db(u16::MAX, u16::MAX)` → `db_number == 65535`, `start == 65535`;
  - `Transfer::bit(S7_AREA_MK, 5, 10, 0).unwrap() == Transfer { area: S7_AREA_MK, db_number: 5, start: 80, wordlen: S7_WL_BIT }`; `Transfer::bit(S7_AREA_DB, 12, 10, 3)` → `start == 83`, `db_number == 12`; `Transfer::bit(S7_AREA_DB, 1, 65535, 7)` → `start == 524287`; `assert_kind(&Transfer::bit(S7_AREA_DB, 1, 0, 8), kind::INVALID_FUN_PARAMETER)`;
  - `bit_payload(false) == [0]`, `bit_payload(true) == [1]`; `!bit_value(0)`, `bit_value(1)`, `bit_value(5)`.
- [ ] **Step 2: FAIL (kompilacja):** `cargo test --offline --lib public_arguments_map_to_transfer` → brak `Transfer::area` itd.
- [ ] **Step 3: Implementacja w `protocol.rs`.** `Transfer::db` i `Transfer::bit` używają `super::S7_AREA_DB`, `super::S7_WL_BYTE`, `super::S7_WL_BIT`; `Transfer::bit` woła istniejące `bit_start`. Bez nowych literałów obszaru/długości słowa.
- [ ] **Step 4: Przepięcie `client.rs`.** `*_area_impl` przyjmują `Transfer`; wewnątrz `self.check_area(transfer.area)?`, `protocol::validate_transfer(transfer.start, transfer.wordlen, buffer.len())?`, `transfer` przekazany do `read_transfer`/`write_transfer`. Wywołania: `read_area` → `Transfer::area(area, db_number, start, wordlen)`; `read_db` → `Transfer::db(db_number, start)`; `write_area`, `write_db` analogicznie; `read_bit_impl`: zerowanie → `let transfer = protocol::Transfer::bit(area, db_number, byte_num, bit_idx)?;` → `read_area_impl(transfer, &mut buffer)?` → `Ok(protocol::bit_value(buffer[0]))`; `write_bit_impl`: zerowanie → `Transfer::bit(…)?` → `write_area_impl(transfer, &protocol::bit_payload(value))`. Konteksty `add_context` bez zmian.
- [ ] **Step 5: B2 na konstruktorach** (`requests_match_reference_bytes`): odczyt bajtów `Transfer::area(S7_AREA_DB, 0x1234, 0x0102, S7_WL_BYTE)`; odczyt bitu `Transfer::bit(S7_AREA_MK, 0, 10, 3).unwrap()`; zapis bajtów `Transfer::db(0x1234, 0x0102)`; zapis bitu `Transfer::bit(S7_AREA_PA, 0, 10, 3).unwrap()` z `bit_payload(false)`. Literały ramek bez zmian (stałe przez `use crate::client::{S7_AREA_DB, S7_AREA_MK, S7_AREA_PA, S7_WL_BYTE};`).
- [ ] **Step 6: PASS:** `cargo test --offline` (96 unit + 11 + 6) i `cargo test --offline --test tcp_smoke -- --ignored` (20). `git diff -- tests/public_errors.rs` pusty.
- [ ] **Step 7: Dowód mutacyjny B1** (`cargo test --offline --lib`, każda → FAIL, cofnij): M17 `Transfer::db` ustawia `db_number: 1`; M19 `bit_payload` zwraca `[1]`; M20 `Transfer::bit` ustawia `area: S7_AREA_DB`; M24 `Transfer::bit` używa `bit_start(byte_num, 7)`; M21 `Transfer::area` ustawia `db_number + 1` (użyj `wrapping_add`); M22 `Transfer::area` ustawia `area: S7_AREA_DB`; M23 `bit_value` zwraca `true`.
- [ ] **Step 8:** `cargo fmt --all`, Clippy jak w zadaniu 6, commit `refactor: map public arguments to transfers in pure functions`.

### Task 4: Rejestr żądań i scenariusz `Addressing` (R4-01, R4-02, tcp_smoke)

**Files:**
- Test: `tests/tcp_smoke.rs` (`serve`, `run`, nowy `Scenario::Addressing`, nowy test)

**Interfaces:**
- Consumes: publiczne API `rust7` (bez zmian).
- Produces: `#[derive(Debug, PartialEq)] struct Seen { function: u8, wordlen: u8, db: u16, area: u8, address: u32, payload: Vec<u8> }`; `fn serve(stream, scenario) -> io::Result<Vec<Seen>>`; `fn run(scenario) -> Vec<Seen>` (dotychczasowe wywołania ignorują wynik).

- [ ] **Step 1: Rejestr.** Każde żądanie odczytu/zapisu obsłużone w gałęzi `match (request[17], request[22])` dopisuje `Seen` (`db` z `[25..27]`, `area` z `[27]`, `address` z `[28..31]`, `payload` = `request[35..]` dla zapisu, pusty dla odczytu). Odpowiedź na odczyt bitu: `vec![0xff, 3, 0, 1, (address % 2) as u8]` (dla `0x07ffff` daje 1, więc `Normal` bez zmian).
- [ ] **Step 2: Założenia tylko poza `Addressing`.** `assert_eq!(address, 0x07ffff)` dla bitu, sekwencyjne zapisy (`address / 8 == written.len()`, kontrola 65536 B) i `assert_eq!(&request[35..], &[1])` dla zapisu bitu obowiązują przy `scenario != Scenario::Addressing`.
- [ ] **Step 3: Test C3 `public_methods_send_their_addresses_and_bit_values`** (`#[ignore]`). Klient po `connect_s71200_1500`:
  1. `write_db(7, 3, &[0xde, 0xad])`; `read_db(11, 4, &mut [0; 2])`;
  2. dla `a` ∈ `[0x81, 0x82, 0x83, 0x84]`: `write_area(a, 9, 5, S7_WL_BYTE, &[a])`, `read_area(a, 9, 5, S7_WL_BYTE, &mut [0])`;
  3. `write_bit(S7_AREA_MK, 9, 10, 0, true)`; `write_bit(S7_AREA_DB, 12, 10, 3, false)`;
  4. `assert!(!read_bit(S7_AREA_MK, 9, 10, 0)?)`; `assert!(read_bit(S7_AREA_PE, 9, 10, 3)?)`;
  5. `write_area(S7_AREA_MK, 9, 83, S7_WL_BIT, &[5, 0])`; `let mut b = [0xaa, 0xbb]; read_area(S7_AREA_PE, 9, 83, S7_WL_BIT, &mut b)` → `b == [1, 0xbb]`.

  Rejestr (`run(Scenario::Addressing)`) == dokładnie 16 pozycji `Seen { function, wordlen, db, area, address, payload }`:

| # | f | wl | db | area | address | payload |
|---|---|---|---|---|---|---|
| 1 | 5 | 2 | 7 | 0x84 | 24 | `[0xde, 0xad]` |
| 2 | 4 | 2 | 11 | 0x84 | 32 | `[]` |
| 3–10 | 5 / 4 naprzemiennie | 2 | 0 (dla 0x84: 9) | a | 40 | zapis `[a]`, odczyt `[]` |
| 11 | 5 | 1 | 0 | 0x83 | 80 | `[1]` |
| 12 | 5 | 1 | 12 | 0x84 | 83 | `[0]` |
| 13 | 4 | 1 | 0 | 0x83 | 80 | `[]` |
| 14 | 4 | 1 | 0 | 0x81 | 83 | `[]` |
| 15 | 5 | 1 | 0 | 0x83 | 83 | `[1]` |
| 16 | 4 | 1 | 0 | 0x81 | 83 | `[]` |

- [ ] **Step 4: PASS:** `cargo test --offline --test tcp_smoke -- --ignored` → 21 pass.
- [ ] **Step 5: Dowód mutacyjny w miejscu wywołania** (`cargo test --offline --test tcp_smoke -- --ignored`, każda → FAIL, cofnij): `write_db` → `Transfer::db(1, start)`; `read_db` → `Transfer::db(1, start)`; `write_area` → `Transfer::area(S7_AREA_DB, db_number, start, wordlen)`; `write_bit_impl` → `bit_payload(true)`; `read_bit_impl` → `Ok(true)` zamiast `bit_value(buffer[0])`.
- [ ] **Step 6:** `cargo fmt --all`, commit `test: check addresses and bit values sent by public methods`.

### Task 5: CHANGELOG (R4-05)

**Files:** Modify `CHANGELOG.md`, sekcja `## [0.2.0] - Unreleased`

- [ ] **Step 1:** Dopisz dokładnie wpisy ze specu, sekcja E: sześć w `### Fixed` (na końcu listy), dwa w `### Changed`, jeden w `### Migration 0.1 to 0.2` (przed linią „Unchanged: writes are not atomic…”). Tekst bez zmian względem specu; bez twierdzeń o walidacji ACK zapisu w 0.1.2.
- [ ] **Step 2:** `typos CHANGELOG.md` bez nowych zgłoszeń; `git diff --stat` dotyczy tylko `CHANGELOG.md`.
- [ ] **Step 3:** Commit `docs: list 0.1.2 behavior fixes in changelog`.

### Task 6: Weryfikacja końcowa

- [ ] **Step 1:** `cargo fmt --all -- --check` → exit 0.
- [ ] **Step 2:** `set -o pipefail; cargo clippy --offline --all-targets --message-format=short --color=never -- -D warnings 2>&1 | reviewdog -f=clippy -reporter=local -filter-mode=nofilter -fail-level=warning` → exit 0, brak diagnostyk.
- [ ] **Step 3:** `cargo test --offline` i `cargo test --offline --release` → 96 unit + 11 + 6 pass; `cargo test --offline --test tcp_smoke -- --ignored` → 21 pass (brak gniazd = NOT RUN, nie PASS).
- [ ] **Step 4:** `RUSTDOCFLAGS="-D warnings" cargo doc --offline --no-deps` → exit 0.
- [ ] **Step 5:** `CARGO_NET_OFFLINE=true cargo mutants -f src/client/protocol.rs -f src/client.rs -- -- --include-ignored` (najlepiej w osobnym worktree) → missed wyłącznie: `client.rs` Drop → `()`, dwa `remote_tsap` `|` → `^`, strażnik `start <= 0x1fffff` w `validate_transfer`; żaden mutant `Transfer::area/db/bit`, `bit_payload`, `bit_value`.
- [ ] **Step 6:** `git diff 5c09959 -- tests/public_errors.rs` pusty; `git diff 5c09959 --stat -- src/client.rs src/client/protocol.rs` obejmuje tylko seam z zadania 3; `git log --oneline 5c09959..` = 6 commitów (zadania 0–5).
