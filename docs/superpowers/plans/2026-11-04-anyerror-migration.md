# Migracja Rust7 do anyerror — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Zastąpić enum `S7Error` błędem `anyerror::AnyError` w całej bibliotece `rust7`,
z publicznymi kategoriami w `get_type()`, kontekstem operacji i serializacją JSON.

**Architecture:** Nowy publiczny moduł `rust7::error` jest jedynym miejscem tworzenia
błędów (konstruktory per kategoria) i decyzji o zerwaniu sesji (`breaks_session`).
Protokół i klient zwracają `AnyError`; 13 publicznych metod dodaje jeden kontekst.

**Tech Stack:** Rust 2021, anyerror 0.1.13 (bez funkcji opcjonalnych), serde_json 1
(dev), Cargo, rustfmt, Clippy, reviewdog.

**Spec:** [Projekt migracji](../specs/2026-11-04-anyerror-migration-design.md) — przeczytaj
przed zadaniem; plan nie powtarza uzasadnień.

**Wykonanie (decyzja użytkownika):** subagent-driven; implementerzy na modelu `sonnet`,
recenzenci zadań i przegląd końcowy na modelu `opus`. Praca na branchu
`feat/anyerror-migration` od `main` (`9b7bb15`); spec i plan commitowane jako pierwszy
commit brancha.

## Global Constraints

- Edition 2021, `#![forbid(unsafe_code)]`, MSRV niedeklarowany.
- `anyerror = { version = "0.1.13", default-features = false }`; `serde_json = "1"` tylko
  w `[dev-dependencies]`; nie włączać `anyhow`, `backtrace`, `rkyv`.
- Kategorie: stałe `rust7::error::kind::*` o wartościach `rust7::<Wariant>` z tabeli w specu;
  `kind::ALL` zawiera dokładnie 13 stałych.
- Błędy tworzone wyłącznie konstruktorami `pub(crate)` z `src/error.rs`; poza
  `src/error.rs` i `src/error/tests.rs` zakaz `AnyError::error(` i `AnyError::new(`.
- `breaks_session` → `true` dla `IO`, `CONNECTION_CLOSED`, `ISO_INVALID_HEADER`,
  `ISO_INVALID_TELEGRAM`, `ISO_FRAGMENTED_PACKET`, dla braku kategorii i kategorii spoza `ALL`.
- Bez zmian protokołu S7, kolejności walidacji, czasów, liczników; bez ponowień zapisu.
- Testy bez prawdziwego PLC; TCP tylko lokalny mock, testy `#[ignore]`.
- Asercje testów: nigdy nie spłaszczać do samego `is_err()`.
- Nie edytować `docs/superpowers/plans/2026-11-04-s7-review-2-fixes.md`.

## Review Focus

1. Peer zamyka gniazdo w środku ramki → `CONNECTION_CLOSED`, klient rozłączony,
   kolejna operacja `NOT_CONNECTED` (zadanie 2, nowy test TCP).
2. Błąd walidacji (zły obszar) przy aktywnej sesji nie rozłącza klienta
   (zadanie 2, `invalid_area_keeps_active_session`).
3. Błąd odebrany z JSON w innym procesie nadal porównuje się ze stałymi `kind`
   (zadanie 1, `io_error_round_trip` sprawdza `get_type()` po dekodowaniu).
4. Skrót (`read_db`, `write_bit`, `connect_rack_slot`) ma dokładnie jeden kontekst
   z nazwą wywołanej metody (zadanie 3, `shortcuts_have_single_context`).
5. Miejsce I/O zmapowane na kategorię domenową zamiast `IO` nie zrywa sesji
   (zadanie 2: grep kontrolny `.map_err(error::io)` + test `LateReply` z kategorią `IO`).

## Pliki

| Plik | Rola |
|---|---|
| `src/error.rs` (nowy) | `AnyError`, `Result`, `kind`, konstruktory, `breaks_session`; od Task 2 `assert_kind` (test) |
| `src/error/tests.rs` (nowy) | Tabela sesji, konstruktory, round-trip JSON |
| `src/lib.rs` | `pub mod error`, reeksporty, usunięcie `S7Error` |
| `src/client.rs` | Migracja, rdzenie `*_impl`, kontekst, rustdoc metod |
| `src/client/protocol.rs` | Migracja na `AnyError` |
| `src/client/tests.rs`, `src/client/protocol/tests.rs` | Migracja asercji |
| `tests/public_errors.rs` (nowy) | Sygnatury, Send/Sync, kontekst |
| `tests/tcp_smoke.rs` | Migracja asercji, scenariusz `MidFrameClose` |
| `Cargo.toml`, `Cargo.lock` | Zależności, wersja 0.2.0 |
| `CHANGELOG.md`, `README.md`, `doc/Documentation.md` | Migracja i opis błędów |
| `examples/sdk/client.rs`, `examples/docker/*` | Raportowanie, zależność 0.2 |

---

### Task 1: Moduł `rust7::error`

**Files:**
- Create: `src/error.rs`, `src/error/tests.rs`
- Modify: `src/lib.rs`, `Cargo.toml`, `Cargo.lock`

**Interfaces:**
- Produces (w `src/error.rs`, publiczny moduł `error`):
  - `pub use anyerror::AnyError;`, `pub type Result<T, E = AnyError> = std::result::Result<T, E>;` (domyślne `E` dodane po przeglądzie końcowym)
  - `pub mod kind` — 13 stałych `&str` + `pub const ALL: &[&str]` (nazwy jak w specu).
  - `pub(crate) fn io(e: std::io::Error) -> AnyError`
  - `pub(crate) fn <nazwa>() -> AnyError` dla: `not_connected`, `tcp_connection_failed`,
    `connection_closed`, `iso_connection_failed`, `iso_fragmented_packet`,
    `iso_invalid_header`, `iso_invalid_telegram`, `pdu_negotiation_failed`,
    `invalid_fun_parameter`, `s7_not_found`, `s7_invalid_address`, `s7_unspecified`.
  - `pub(crate) fn breaks_session(e: &AnyError) -> bool`
- `src/lib.rs`: `pub mod error;` oraz `pub use error::{kind, AnyError, Result};`
  (obok istniejących eksportów; `S7Error` jeszcze zostaje).

- [ ] **Step 1: Branch i zależności.** `git switch -c feat/anyerror-migration`
  (jeśli koordynator nie utworzył go wcześniej). Dodaj zależności z Global Constraints;
  uruchom `cargo check --offline`, a przy braku cache `cargo check` (sieć). Oczekiwane:
  kod 0, `Cargo.lock` zawiera `anyerror 0.1.13`, `serde`, `serde_json`.

- [ ] **Step 2: Napisz testy w `src/error/tests.rs`** (podłączone `#[cfg(test)] mod tests;`):

```rust
use super::*;
use std::error::Error as _;

#[test]
fn constructors_set_known_kind() {
    let table: [(fn() -> AnyError, &str, &str); 12] = [
        (not_connected, kind::NOT_CONNECTED, "Not connected"),
        (tcp_connection_failed, kind::TCP_CONNECTION_FAILED, "TCP connection failed"),
        (connection_closed, kind::CONNECTION_CLOSED, "TCP connection closed by the peer"),
        (iso_connection_failed, kind::ISO_CONNECTION_FAILED, "ISO-on-TCP connection failed"),
        (iso_fragmented_packet, kind::ISO_FRAGMENTED_PACKET, "Fragmented ISO Packet"),
        (iso_invalid_header, kind::ISO_INVALID_HEADER, "Invalid ISO Header"),
        (iso_invalid_telegram, kind::ISO_INVALID_TELEGRAM, "Invalid ISO Telegram"),
        (pdu_negotiation_failed, kind::PDU_NEGOTIATION_FAILED, "S7 PDU negotiation failed"),
        (invalid_fun_parameter, kind::INVALID_FUN_PARAMETER,
            "Invalid parameter supplied to the function"),
        (s7_not_found, kind::S7_NOT_FOUND, "S7 Resource not found in the CPU"),
        (s7_invalid_address, kind::S7_INVALID_ADDRESS, "S7 Invalid address"),
        (s7_unspecified, kind::S7_UNSPECIFIED, "S7 unspecified error"),
    ];
    for (make, expected, msg) in table {
        let err = make();
        assert_eq!(err.get_type(), Some(expected));
        assert!(kind::ALL.contains(&expected));
        assert_eq!(err.to_string(), format!("{expected}: {msg}"));
        assert!(err.source().is_none());
    }
    assert_eq!(io(std::io::ErrorKind::TimedOut.into()).get_type(), Some(kind::IO));
    assert_eq!(kind::ALL.len(), 13);
}
```

  `breaks_session_table`: tablica `[(kind, bool); 13]` pokrywająca każdą stałą z `kind::ALL`
  dokładnie raz (asercja: zbiór kluczy == `kind::ALL`), dla każdej
  `breaks_session(&AnyError::error("x").with_type(Some(k))) == expected`; dodatkowo
  `AnyError::error("x")` (brak typu) i typ `"other::Kind"` → `true`.

  `io_error_round_trip`: własny typ `Outer` (Display `"outer cause"`, `source()` → `Inner`
  z Display `"root cause"`); `let err = io(std::io::Error::other(Outer)).add_context(|| "read_db db=7");`
  Asercje: `get_type() == Some(kind::IO)`, `to_string()` zawiera `"outer cause"` i
  `"read_db db=7"`, `err.source().unwrap().to_string()` zawiera `"root cause"`;
  po `serde_json::to_string` → `from_str::<AnyError>`: `decoded == err`,
  `decoded.get_type() == Some(kind::IO)`, ten sam tekst `source()`.

- [ ] **Step 3: Uruchom** `cargo test --offline error::tests` — oczekiwane: błąd kompilacji
  (brak konstruktorów / `kind`).

- [ ] **Step 4: Zaimplementuj `src/error.rs`.** Konstruktory domenowe przez jedną prywatną
  funkcję `fn domain(kind: &'static str, msg: &'static str) -> AnyError`
  (`AnyError::error(msg).with_type(Some(kind))`); `io` przez
  `AnyError::new(&e).with_type(Some(kind::IO))`. `breaks_session` przez `matches!` na
  `e.get_type()` ze stałymi jako wzorcami, `None`/nieznane → `true`.
  Na czas tego zadania na początku pliku `#![cfg_attr(not(test), allow(dead_code))]`
  z komentarzem `// removed in Task 2`.

- [ ] **Step 5: Weryfikacja.** `cargo test --offline` — 3 nowe testy + dotychczasowe
  38 jednostkowych i 1 doctest zaliczone. Bramka jakości (koniec planu) bez ostrzeżeń.

- [ ] **Step 6: Commit** `Cargo.toml Cargo.lock src/lib.rs src/error.rs src/error/tests.rs`:
  `feat(error): add anyerror-based error module`.

---

### Task 2: Migracja biblioteki i testów na `AnyError`

**Files:**
- Modify: `src/client/protocol.rs`, `src/client.rs`, `src/lib.rs`, `src/error.rs`,
  `src/client/tests.rs`, `src/client/protocol/tests.rs`, `tests/tcp_smoke.rs`
- Create: `tests/public_errors.rs`

**Interfaces:**
- Consumes: wszystko z Task 1.
- Produces: 13 metod publicznych z sygnaturami ze specu (`read_bit -> crate::Result<bool>`,
  pozostałe `-> crate::Result<()>`); wszystkie funkcje `protocol` i `finish`
  zwracają `crate::Result<_>` zamiast `Result<_, S7Error>`. Kontekstu jeszcze brak.
- Produces (w `src/error.rs`): `#[cfg(test)] #[track_caller] pub(crate) fn assert_kind<T>(result: &Result<T>, expected: &str)`
  — panikuje z faktycznym `get_type()` i `Display`, gdy wynik to `Ok` lub inna kategoria.

- [ ] **Step 1: Napisz `tests/public_errors.rs`:** test `public_methods_return_anyerror`
  przypisujący każdą z 13 metod do wskaźnika funkcji, np.
  `let _: fn(&mut S7Client, u8, u16, u16, u8, &mut [u8]) -> rust7::Result<()> = S7Client::read_area;`
  oraz `let _: fn(&mut S7Client, u8, u16, u16, u8) -> rust7::Result<bool> = S7Client::read_bit;`;
  sprawdź `rust7::Result<()>` == `Result<(), rust7::AnyError>` przez przypisanie typów.
  Test `public_errors_are_send_sync`: `fn bound<E: std::error::Error + Send + Sync + 'static>() {}`
  wywołane dla `rust7::AnyError`. Test `read_db_without_connection_is_not_connected`:
  `S7Client::new().read_db(7, 3, &mut [0; 4])` → `get_type() == Some(rust7::kind::NOT_CONNECTED)`.

- [ ] **Step 2: Uruchom** `cargo test --offline --test public_errors` — oczekiwane: błąd
  kompilacji (niezgodny typ błędu wskaźników).

- [ ] **Step 3: Migracja `protocol.rs` i `client.rs`.** `use crate::error::{self, Result}`
  (lub ścieżki pełne). Każde `Err(S7Error::X)` → `Err(error::x())`; `.ok_or(S7Error::X)` →
  `.ok_or_else(error::x)`; `map_err(|_| S7Error::PduNegotiationFailed)` →
  `map_err(|_| error::pdu_negotiation_failed())`. Każde `?` na `io::Result` →
  `.map_err(error::io)?` (protokół: `write_all`, `limit_next_read`, gałąź `Err(e)` w
  `read_full`, timeout `io::Error::from(io::ErrorKind::TimedOut)`; klient: `to_socket_addrs`,
  `connect_timeout`, `set_read_timeout`, `set_write_timeout`, `set_nodelay`, `write_all`).
  `finish` woła `error::breaks_session(&e)`. Usuń enum `S7Error`, jego `Display`, `From`,
  `Error`, `breaks_session`, import `fmt` jeśli zbędny; z `lib.rs` usuń `S7Error` z eksportu.
  Usuń `#![cfg_attr(not(test), allow(dead_code))]` z `src/error.rs`. Rustdoc metod: każde
  `S7Error::X` → `` `kind::X` `` (stała z `rust7::kind`), treść opisów bez zmian.

- [ ] **Step 4: Migracja testów jednostkowych.** `matches!(r, Err(S7Error::X))` →
  `assert_kind(&r, kind::X)`; dla wartości błędu (np. `finish(...).unwrap_err()`) użyj
  `assert_eq!(e.get_type(), Some(kind::X))`. Wyjątki:
  - `read_tpkt_propagates_eof_and_timeout`: dla `TimedOut`/`WouldBlock` asercja
    `kind::IO` oraz `err.to_string().contains(&io::Error::from(kind).to_string())`.
  - `frame_deadline_bounds_slow_peer`: `kind::IO` oraz komunikat zawiera
    `io::Error::from(io::ErrorKind::TimedOut).to_string()`.
  - `only_transport_errors_break_the_session`: te same listy kategorii, budowane
    konstruktorami (`error::io(io::ErrorKind::TimedOut.into())`, `error::connection_closed()`, …),
    asercje przez `error::breaks_session`.
  - Usuń `s7_error_is_std_error_with_io_source` (zastąpiony `io_error_round_trip`).
  - Tablica `("address", S7Error::S7InvalidAddress) | …` w `protocol/tests.rs` → pary
    `(&str, &str)` z `kind::*`.

- [ ] **Step 5: Dodaj test `invalid_area_keeps_active_session`** w `src/client/tests.rs`:
  klient z `connected = true`, `pdu_length = 125`, `limits = Some(PduLimits::new(125).unwrap())`
  (jak w `finish_preserves_session_only_for_valid_error_ack`); `read_area(0x99, 1, 0, S7_WL_BYTE, &mut [0; 2])`
  → `kind::INVALID_FUN_PARAMETER`, nadal `connected`, `pdu_length == 125`, `limits.is_some()`.

- [ ] **Step 6: Migracja `tests/tcp_smoke.rs`.** Import `rust7::{kind, S7Client, S7_AREA_DB}`;
  lokalny `#[track_caller] fn assert_kind<T>(r: &rust7::Result<T>, k: &str)`. Wszystkie
  `matches!` → `assert_kind`; w `LateReply` dodatkowo komunikat błędu niepusty po
  prefiksie kategorii. Dodaj `Scenario::MidFrameClose`: po negocjacji serwer na pierwsze
  żądanie odczytu wysyła tylko nagłówek TPKT deklarujący pełną ramkę i 3 bajty, po czym
  zamyka gniazdo. Test `#[ignore = "requires permission to open loopback sockets"]`
  `mid_frame_close_disconnects_client`: `read_db(100, 0, &mut [0; 4])` →
  `kind::CONNECTION_CLOSED`, `!client.connected`, `pdu_length == 0`, kolejny `read_db`
  → `kind::NOT_CONNECTED`.

- [ ] **Step 7: Weryfikacja.**
  - `cargo test --offline` — wszystkie zaliczone, w tym `public_errors`.
  - `cargo test --offline --test tcp_smoke -- --ignored` — 10 zaliczonych; jeśli sandbox
    blokuje gniazda, ponów poza sandboxem lub zgłoś jako niewykonane (nie jako sukces).
  - `rg -n 'S7Error' src tests` → brak wyników.
  - `rg -n 'AnyError::(error|new)\(' src tests --glob '!src/error.rs' --glob '!src/error/**'` → brak.
  - `rg -n '\?;' src/client/protocol.rs src/client.rs` przejrzyj: każde `?` na `io::Result`
    poprzedza `.map_err(error::io)`.
  - Bramka jakości bez ostrzeżeń.

- [ ] **Step 8: Commit:** `feat!: replace S7Error with anyerror::AnyError`.

---

### Task 3: Rdzenie `*_impl` i kontekst operacji

**Files:**
- Modify: `src/client.rs`, `tests/public_errors.rs`

**Interfaces:**
- Consumes: Task 2.
- Produces (prywatne w `impl S7Client`, bez kontekstu):
  - `fn connect_tsap_impl(&mut self, ip: &str, local_tsap: u16, remote_tsap: u16) -> crate::Result<()>`
  - `fn connect_rack_slot_impl(&mut self, ip: &str, rack: u16, slot: u16) -> crate::Result<()>`
  - `fn read_bit_impl(&mut self, area: u8, db_number: u16, byte_num: u16, bit_idx: u8) -> crate::Result<bool>`
  - `fn write_bit_impl(&mut self, area: u8, db_number: u16, byte_num: u16, bit_idx: u8, value: bool) -> crate::Result<()>`
  - istniejące `read_area_impl` / `write_area_impl` (`start: u32`) bez zmian sygnatur.

Format kontekstu (jeden na metodę publiczną, przez `anyerror::AddContext::add_context(|| format!(...))`
na wyniku rdzenia; klucze rozdzielone spacją, `area` i TSAP-y w hex `{:#04x}`/`{:#06x}`):

| Metoda | Kontekst |
|---|---|
| `set_connection_type` | `set_connection_type connection_type={}` |
| `set_timeout` | `set_timeout co_timeout_ms={} rd_timeout_ms={} wr_timeout_ms={}` |
| `set_connection_port` | `set_connection_port port={}` |
| `connect_s71200_1500`, `connect_s7300` | `<metoda> ip={} port={}` |
| `connect_rack_slot` | `connect_rack_slot ip={} port={} rack={} slot={}` |
| `connect_tsap` | `connect_tsap ip={} port={} local_tsap={:#06x} remote_tsap={:#06x}` |
| `read_area`, `write_area` | `<metoda> area={:#04x} db={} start={} wordlen={} len={}` |
| `read_db`, `write_db` | `<metoda> db={} start={} len={}` |
| `read_bit`, `write_bit` | `<metoda> area={:#04x} db={} byte_num={} bit_idx={}` |

`port` = `self.port` w chwili wywołania; `len` = `buffer.len()`; wartość zapisywanego bitu
i zawartość buforów nie trafiają do kontekstu.

- [ ] **Step 1: Napisz testy w `tests/public_errors.rs`.** Pomocnik
  `fn contexts(e: &rust7::AnyError) -> Vec<String>` czytający tablicę `context` z
  `serde_json::to_value(e)`.

```rust
#[test]
fn disconnected_db_read_has_category_and_operation_context() {
    let mut client = rust7::S7Client::new();
    let err = client.read_db(7, 3, &mut [0; 4]).unwrap_err();
    assert_eq!(err.get_type(), Some(rust7::kind::NOT_CONNECTED));
    assert_eq!(contexts(&err), ["read_db db=7 start=3 len=4"]);
    assert!(std::error::Error::source(&err).is_none());
}
```

  `shortcuts_have_single_context`: dokładne listy kontekstów (jeden element) dla
  `write_db(7, 3, &[0; 2])` → `"write_db db=7 start=3 len=2"`,
  `read_bit(S7_AREA_DB, 1, 0, 8)` → `"read_bit area=0x84 db=1 byte_num=0 bit_idx=8"`
  (kategoria `INVALID_FUN_PARAMETER`), `write_bit(S7_AREA_DB, 1, 2, 3, true)` bez połączenia
  → `"write_bit area=0x84 db=1 byte_num=2 bit_idx=3"`, `connect_rack_slot("127.0.0.1", 8, 0)`
  → `"connect_rack_slot ip=127.0.0.1 port=102 rack=8 slot=0"` (`INVALID_FUN_PARAMETER`).
  `setters_report_rejected_value`: `set_connection_port(0)` → `["set_connection_port port=0"]`,
  `set_timeout(0, 1, 1)` → `["set_timeout co_timeout_ms=0 rd_timeout_ms=1 wr_timeout_ms=1"]`,
  `set_connection_type(9)` → `["set_connection_type connection_type=9"]`.
  `read_area_context_lists_transfer`: `read_area(S7_AREA_DB, 5, 10, S7_WL_BYTE, &mut [0; 6])`
  → `["read_area area=0x84 db=5 start=10 wordlen=2 len=6"]`.

- [ ] **Step 2: Uruchom** `cargo test --offline --test public_errors` — oczekiwane: FAIL
  (puste listy kontekstów).

- [ ] **Step 3: Wydziel rdzenie i dodaj kontekst.** Ciała obecnych metod przenieś do
  rdzeni z Interfaces; skróty wołają rdzenie (`read_db` → `read_area_impl(S7_AREA_DB, …, u32::from(start), S7_WL_BYTE, …)`,
  `connect_s7300` → `connect_rack_slot_impl(ip, 0, 2)`, `connect_rack_slot_impl` →
  `connect_tsap_impl`). Każda z 13 metod publicznych = wywołanie rdzenia lub walidacji
  + `.add_context(...)` z tabeli. Reset `last_time`/`chunks` w bitach zostaje w rdzeniu
  przed `bit_start`.

- [ ] **Step 4: Weryfikacja.** `cargo test --offline`, testy TCP z `--ignored`
  (jak w Task 2), bramka jakości. Oczekiwane: wszystko zaliczone, bez ostrzeżeń.

- [ ] **Step 5: Commit:** `feat(client): add operation context to public errors`.

---

### Task 4: Wersja 0.2.0, dokumentacja i przykłady

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md`, `README.md`, `doc/Documentation.md`,
  `src/lib.rs` lub `src/error.rs` (doctesty), `examples/sdk/client.rs`,
  `examples/docker/main.rs`, `examples/docker/Cargo.toml`, `examples/README.md`,
  `examples/docker/README.md`

**Interfaces:** konsumuje `rust7::{AnyError, Result, kind}`; nie rozszerza API.

- [ ] **Step 1: Doctesty w rustdoc modułu `error`:** (a) kompilowany przykład
  `fn configure(c: &mut rust7::S7Client) -> rust7::Result<()> { c.set_connection_port(102)?; Ok(()) }`
  wywołany i `assert!(... .is_ok())`; (b) przykład rozróżnienia kategorii:
  `set_connection_port(0)` → `get_type() == Some(rust7::kind::INVALID_FUN_PARAMETER)`;
  (c) dwa `compile_fail`: `use rust7::S7Error;` i `use rust7::client::S7Error;`.
  Uruchom `cargo test --offline --doc` — zaliczone.

- [ ] **Step 2: Wersja** `0.2.0` w `Cargo.toml`; `cargo check --offline` aktualizuje lockfile.

- [ ] **Step 3: `CHANGELOG.md`:** nowa sekcja `## [0.2.0] - Unreleased` z `### Changed`
  (breaking: `AnyError`, usunięte `S7Error`, 13 sygnatur, kontekst, zależność `anyerror`)
  i podsekcją „Migracja 0.1 → 0.2” zgodną ze specem: `matches!(e, S7Error::X)` →
  `e.get_type() == Some(rust7::kind::X)`, `client.connected` jako sygnał ponownego połączenia,
  iteracja `source()`, utrata `io::ErrorKind`/downcastu, JSON w formacie `anyerror` bez
  gwarancji schematu. Istniejące wpisy `[Unreleased]` odnoszące się do `S7Error`
  przenieś do `0.2.0`, przepisując nazwy wariantów na kategorie.

- [ ] **Step 4: `README.md`, `doc/Documentation.md`:** wszystkie odwołania do `S7Error`
  → kategorie `rust7::kind`; sekcja błędów z tabelą 13 kategorii (stała, wartość,
  czy zrywa sesję); usuń twierdzenia o braku zależności (`rg -in 'no dependenc|zero dependenc|without dependenc'`);
  zachowaj ostrzeżenia o częściowych zapisach/odczytach i braku ponowień. Snippety
  sieciowe `no_run`.

- [ ] **Step 5: Przykłady.** `examples/sdk/client.rs`, `examples/docker/main.rs`:
  raportowanie `eprintln!("…: {e:#}")` (alternatywny format pokazuje kontekst i przyczyny),
  bez loggera. `examples/docker/Cargo.toml`: `rust7 = "0.2"`. README przykładów: build
  docker używa wydania z rejestru, przed publikacją 0.2 nie testuje bieżącego checkoutu.

- [ ] **Step 6: Kompilacja przykładów.** W scratchpadzie sesji utwórz pakiet z
  `rust7 = { path = "<absolutna ścieżka repo>" }` i dwoma `[[bin]]` wskazującymi absolutne
  ścieżki do `examples/sdk/client.rs` i `examples/docker/main.rs`;
  `cargo check --offline --manifest-path <scratchpad>/Cargo.toml --bins` → kod 0.
  Nie uruchamiaj binariów ani Dockera; nie commituj tego pakietu.

- [ ] **Step 7: Weryfikacja końcowa.** Bramka jakości, `cargo doc --offline --no-deps`
  bez ostrzeżeń, testy TCP z `--ignored`, `rg -n 'S7Error' src tests examples README.md doc`
  → brak (CHANGELOG/poradnik dozwolone).

- [ ] **Step 8: Commit:** `docs: document AnyError migration and update examples`.

---

## Bramka jakości

Z katalogu repo (`RustS7CommClient`), po pobraniu zależności:

```bash
cargo fmt --all -- --check
cargo check --offline --all-targets
cargo test --offline
set -o pipefail
rtk bypass cargo clippy --offline --all-targets --message-format=short --color=never -- -D warnings 2>&1 | /home/rtm/.local/bin/reviewdog -f=clippy -reporter=local -filter-mode=nofilter -fail-level=warning
```

Oczekiwane: kody 0, brak diagnostyk. Awaria uruchomienia Clippy/reviewdog to nie wynik
lintowania. Testy TCP (`--ignored`) raportowane osobno.

## Odbiór

- Spec spełniony: kategorie, konstruktory, sesja, kontekst, JSON, wersja 0.2.0.
- Przegląd końcowy całego brancha (opus): `detect_changes_tool` + `get_review_context_tool`
  z `repo_root` repozytorium, potem źródła; potwierdzić 13 sygnatur, brak `S7Error`,
  strażnika konstruktorów i niezmienione zachowanie sesji/transferów.
- Merge, push i publikacja wydania wymagają osobnej decyzji użytkownika.
