# Rust7: naprawy po przeglądzie 4 (R4-01…R4-05) — projekt

Status: projekt zatwierdzony w trzech sekcjach w rozmowie 2026-11-07; czeka na
przegląd pisemny. Źródło wymagań: [raport R4](2026-11-07-s7-review-4-report.md)
na HEAD `5c09959`. Poprzednie decyzje: [F1–F7](2026-10-27-s7-review-fixes-design.md),
[anyerror](2026-11-04-anyerror-migration-design.md),
[R3](2026-11-06-s7-review-3-fixes-design.md), `AGENTS.md`.

## Cel

1. Testy mają wykrywać regresje, w których publiczna warstwa `S7Client` wysyła
   zapis pod zły DB, obszar lub bit albo złą wartość bitu (R4-01, R4-02).
2. Stałe bajty nagłówka żądania S7 są przypięte testem (R4-03).
3. Rozłączenie po każdym błędzie rozsynchronizowującym strumień jest
   sprawdzane w `finish` i na transporcie, nie tylko w predykacie (R4-04).
4. CHANGELOG 0.2.0 opisuje naprawy F1–F7 widoczne dla użytkownika 0.1.2 (R4-05).

Kryterium sukcesu: każda mutacja z sekcji „Mutacje odbioru” jest wykrywana
przez wskazany suite; CHANGELOG zawiera wpisy z sekcji E.

## Znaleziska w zakresie

| ID | Miejsce (HEAD 5c09959) | Wymaganie |
|---|---|---|
| R4-01 | `src/client.rs:603`, `:686`, `:748-752`, `:775`, `:838-839` | DB, obszar, adres i indeks bitu przekazywane przez metody publiczne są sprawdzane testem |
| R4-02 | `src/client.rs:752`, `:839` | `write_bit(false)` wysyła payload `[0]`; `read_bit` zwraca `false` dla bitu 0 |
| R4-03 | `src/client/protocol.rs:310-366` | Pełne bajty żądań odczytu/zapisu × bajt/bit przypięte literałami |
| R4-04 | `src/client.rs:653-666` | `finish` rozłącza dla każdej kategorii „Breaks the session” i tylko dla nich; tcp_smoke dla `ISO_INVALID_HEADER` i `ISO_FRAGMENTED_PACKET` |
| R4-05 | `CHANGELOG.md` sekcja `[0.2.0] - Unreleased` | Wpisy F1–F7 i zmiana walidacji odpowiedzi |

Poza zakresem: R4-06…R4-23 (P3), w tym deduplikacja `check_area`/`valid_area`
i literału `0x84` w `request()` (R4-12). Bez zmian pozostają H1–H3, timeout
zapisu oraz decyzje „poza zakresem” z rund R3 i wcześniejszych.

## Ograniczenia

- Edition 2021, `#![forbid(unsafe_code)]`, kod synchroniczny, bez nowych
  zależności (także dev).
- Bez zmian zachowania i publicznego API: sygnatury, kategorie `kind`, reguły
  rozłączania, konteksty błędów i kolejność kontroli parametrów pozostają takie
  same. `tests/public_errors.rs` bez modyfikacji.
- Testy jednostkowe bez sieci; transport TCP tylko w `tests/tcp_smoke.rs`
  (`#[ignore]`). Bez wstrzykiwania transportu do `S7Client` (odrzucone w tej
  rundzie). Bez kontaktu z PLC.
- Komendy Cargo z `--offline`.

## Projekt

### A. Seam: mapowanie argumentów publicznych (R4-01, R4-02)

Całe przekształcenie argumentów metod publicznych na adres transferu trafia
do czystych funkcji obok `Transfer` w `src/client/protocol.rs`:

| Funkcja | Wynik |
|---|---|
| `Transfer::area(area: u8, db_number: u16, start: u16, wordlen: u8) -> Transfer` | pola bez zmian, `start` rozszerzony do `u32` |
| `Transfer::db(db_number: u16, start: u16) -> Transfer` | `area = S7_AREA_DB`, `wordlen = S7_WL_BYTE` |
| `Transfer::bit(area: u8, db_number: u16, byte_num: u16, bit_idx: u8) -> Result<Transfer>` | `start = bit_start(byte_num, bit_idx)?`, `wordlen = S7_WL_BIT` |
| `bit_payload(value: bool) -> [u8; 1]` | `[u8::from(value)]` |
| `bit_value(byte: u8) -> bool` | `byte != 0` |

- Stałe `S7_AREA_DB`, `S7_WL_BYTE`, `S7_WL_BIT` przez `super::`; nowe funkcje
  nie wprowadzają literałów obszaru ani długości słowa.
- `Transfer` dostaje `#[derive(Clone, Copy, Debug, PartialEq, Eq)]` (typ
  `pub(super)`, poza publicznym API).
- `read_area_impl(&mut self, transfer: protocol::Transfer, buffer: &mut [u8])`
  i `write_area_impl(&mut self, transfer: protocol::Transfer, buffer: &[u8])`
  zamiast czterech argumentów adresu.
- Metody publiczne i `*_bit_impl` tylko przekazują swoje argumenty, np.
  `self.write_area_impl(protocol::Transfer::db(db_number, start), buffer)`;
  `write_bit_impl` używa `Transfer::bit(..)?` i `bit_payload(value)`,
  `read_bit_impl` zwraca `bit_value(buffer[0])`.
- Kolejność bez zmian: zerowanie `last_time`/`chunks`/`confirmed` →
  `bit_start` (dla metod bitowych) → `check_area` → `validate_transfer` →
  `NOT_CONNECTED`. Istniejące testy (`invalid_area_keeps_active_session`,
  `invalid_public_inputs_clear_statistics_and_return_errors` i in.) przechodzą
  bez zmian.

Ryzyko rezydualne: mutacja w samym miejscu wywołania (np. `Transfer::db(1, start)`
w `write_db`) przeżywa domyślny `cargo test`; wykrywa ją tylko scenariusz C3
w `--ignored`.

### B. Testy jednostkowe (domyślny `cargo test`)

**B1. `public_arguments_map_to_transfer`** (`src/client/tests.rs`, R4-01/02):

- `Transfer::area(a, 9, 0x1234, S7_WL_BYTE)` dla `a` ∈ {PE, PA, MK, DB} ==
  `Transfer { area: a, db_number: 9, start: 0x1234, wordlen: S7_WL_BYTE }`;
- `Transfer::db(7, 3)` == `{ S7_AREA_DB, 7, 3, S7_WL_BYTE }`;
- `Transfer::bit(S7_AREA_MK, 5, 10, 0)` → `start = 80`;
  `Transfer::bit(S7_AREA_DB, 12, 10, 3)` → `start = 83`, `db_number = 12`;
  `Transfer::bit(S7_AREA_DB, 1, 0, 8)` → `INVALID_FUN_PARAMETER`;
- `bit_payload(false) == [0]`, `bit_payload(true) == [1]`;
  `bit_value(0) == false`, `bit_value(1)` i `bit_value(5) == true`.

**B2. `requests_match_reference_bytes`** (`src/client/protocol/tests.rs`, R4-03).
Cztery pełne ramki z `request()` porównane `assert_eq!` z literałami zapisanymi
ręcznie, z komentarzem przy każdym polu (TPKT, COTP DT, nagłówek S7 Job:
0x32, ROSCTR 1, referencja, długość parametrów 14, długość danych; parametry:
funkcja, item count 1, 0x12/0x0a/0x10, wordlen, count, DB, obszar, adres
24-bit; dla zapisu nagłówek danych: return code 0, transport size 4 lub 3,
długość w bitach):

| Przypadek | Transfer | Pozostałe |
|---|---|---|
| odczyt bajtów | `Transfer::area(S7_AREA_DB, 0x1234, 0x0102, S7_WL_BYTE)` | count 3, referencja `0x0a0b` |
| odczyt bitu | `Transfer::bit(S7_AREA_MK, 0, 10, 3)?` | count 1 |
| zapis bajtów | `Transfer::db(0x1234, 0x0102)` | payload `[0xde, 0xad, 0xbe]` |
| zapis bitu | `Transfer::bit(S7_AREA_PA, 0, 10, 3)?` | payload `bit_payload(false)` |

Literały pochodzą z układu S7comm (Snap7, dissector Wireshark s7comm), nie
z kodu biblioteki. Bez pcap z prawdziwego PLC to układ referencyjny, nie dowód
zgodności z firmware; komentarz testu mówi to wprost.

**B3. `finish_applies_the_session_rule_of_every_kind`** (`src/client/tests.rs`, R4-04).
Tabela `(kind, konstruktor z error.rs, rozłącza)` z oczekiwaniem wpisanym
ręcznie według opisów stałych w `kind`: rozłączają `IO`, `CONNECTION_CLOSED`,
`ISO_INVALID_HEADER`, `ISO_INVALID_TELEGRAM`, `ISO_FRAGMENTED_PACKET`;
pozostałe osiem nie. Test:

- sprawdza, że zbiór kategorii w tabeli jest równy `kind::ALL` (nowa kategoria
  wymusza aktualizację tabeli);
- dla każdej pozycji tworzy „połączonego” klienta (`connected = true`,
  `pdu_length = 125`, `limits = Some(PduLimits::new(125))`), woła
  `finish(Err(e), Instant::now())` i sprawdza: kategoria błędu bez zmian;
  przy rozłączeniu `!connected`, `limits.is_none()`, `pdu_length == 0`;
  inaczej wszystkie trzy bez zmian;
- dodatkowo błąd bez kategorii (`AnyError::error("x")`) i z kategorią spoza
  `kind::ALL` (`"other::X"`) rozłączają.

### C. tcp_smoke (`cargo test --test tcp_smoke -- --ignored`)

**C1. Stałe nagłówka w mocku** (R4-03). W `serve` każde żądanie każdego
scenariusza przechodzi asercje: `[4..7] == [2, 0xf0, 0x80]`, `[7] == 0x32`,
`[8] == 1`, `[9..11] == [0, 0]`, `[13..15] == [0, 14]`,
`[18..22] == [1, 0x12, 0x0a, 0x10]`; `[15..17]` == 0 dla odczytu i `count + 4`
dla zapisu; dla zapisu `[31] == 0`, `[32] == 3` (bit) lub `4` (bajty),
`[33..35]` == 1 (bit) lub `count * 8` (bajty).

**C2. Rejestr żądań.** `serve` zwraca `io::Result<Vec<Seen>>`, gdzie
`Seen { function: u8, wordlen: u8, db: u16, area: u8, address: u32, payload: Vec<u8> }`
dla każdego obsłużonego żądania odczytu/zapisu. Założenia mocka obecnie
wspólne dla wszystkich scenariuszy przechodzą do scenariusza `Normal`:
adres bitu `0x07ffff`, zapisy sekwencyjne od adresu 0 z kontrolą 65536 B,
payload bitu `[1]`.

**C3. Scenariusz `Addressing`, test `public_methods_send_their_addresses_and_bit_values`**
(R4-01, R4-02). Mock odpowiada na odczyt bajtów jak dziś (`i % 251` od offsetu),
na odczyt bitu wartością `address % 2` i potwierdza zapisy. Klient wykonuje
kolejno:

1. `write_db(7, 3, &[0xde, 0xad])`, `read_db(11, 4, &mut [0; 2])`;
2. dla `a` ∈ {PE, PA, MK, DB}: `write_area(a, 9, 5, S7_WL_BYTE, &[a])` i
   `read_area(a, 9, 5, S7_WL_BYTE, &mut [0])`;
3. `write_bit(S7_AREA_MK, 9, 10, 0, true)`, `write_bit(S7_AREA_DB, 12, 10, 3, false)`;
4. `read_bit(S7_AREA_MK, 9, 10, 0)` == `false` (adres 80),
   `read_bit(S7_AREA_PE, 9, 10, 3)` == `true` (adres 83).

Test porównuje cały rejestr z oczekiwaną listą literałów `Seen`, w tym:
`db == 0` poza obszarem DB, adres bajtowy `start * 8`, payload bitu `[1]` dla
`true` i `[0]` dla `false`.

**C4. Złe ramki odpowiedzi** (R4-04). Scenariusze `BadHeader(writing)`
(odpowiedź z wersją TPKT 2; wysłane nagłówek i ciało, więc w gnieździe zostają
resztki) i `Fragment(writing)` (COTP DT z `0x00` zamiast EOT `0x80`), na wzór
`MalformedAck`, dla odczytu i zapisu 600 B. Oczekiwane: kategoria
`ISO_INVALID_HEADER` lub `ISO_FRAGMENTED_PACKET`, `chunks == 1`,
`confirmed == 0`, bufor odczytu nietknięty, `last_time == 0.0`, `!connected`,
`pdu_length == 0`, następne wywołanie → `NOT_CONNECTED`. Testy:
`bad_tpkt_header_disconnects_client`, `fragmented_reply_disconnects_client`.
`malformed_ack_disconnects_client` bez zmian.

### D. Mutacje odbioru

Definicje z raportu R4 (ręczne mutacje M01–M31). Każda wprowadzana tymczasowo,
po sprawdzeniu cofana.

| Mutacja | Miejsce po zmianie | Wykrywa domyślny `cargo test` | Wykrywa `--ignored` |
|---|---|---|---|
| M01 transport size zapisu bajtowego 4 → 9 | `request()` | B2 | C1 |
| M03 0x10 → 0x11, M04 0x12 → 0x13, M05 0x0a → 0x0b | `request()` | B2 | C1 |
| M06 item count 1 → 2, M07 długość parametrów 14 → 15 | `request()` | B2 | C1 |
| M08 ROSCTR 1 → 7, M09 0x32 → 0x33 | `request()` | B2 | C1 |
| M13 `finish` pomija rozłączenie dla `ISO_FRAGMENTED_PACKET` | `finish` | B3 | C4 |
| M14 `finish` pomija rozłączenie dla `ISO_INVALID_HEADER` | `finish` | B3 | C4 |
| M17 `write_db` ignoruje `db_number` | `Transfer::db` | B1 | C3 |
| M19 `write_bit` zawsze `true` | `bit_payload` | B1, B2 | C3 |
| M20 `write_bit` zawsze obszar DB, M24 ignoruje indeks bitu | `Transfer::bit` | B1, B2 | C3 |
| M21 `write_area` przekazuje `db_number + 1`, M22 wymusza DB | `Transfer::area` | B1 | C3 |
| M23 `read_bit` zawsze `true` po udanym odczycie | `bit_value` | B1 | C3 |
| odczyt: `read_db` ignoruje `db_number`, `read_area` wymusza DB | `Transfer::db`, `Transfer::area` | B1 | C3 |
| dowolna z M17–M24 w miejscu wywołania (np. `Transfer::db(1, start)`) | `client.rs` | — (ryzyko rezydualne) | C3 |

### E. CHANGELOG (R4-05)

Wpisy w sekcji `[0.2.0] - Unreleased`. Fakty o 0.1.2 sprawdzone w źródle
`rust7-0.1.2/src/client.rs` (:474-475, :573, :637-652, :740, :900, :964).
Nie dodawać twierdzeń o walidacji ACK zapisu w 0.1.2 (niezweryfikowane).
Testy i seam nie zmieniają zachowania i nie trafiają do CHANGELOG.

`### Fixed`:

- `read_bit`/`write_bit` with `byte_num` >= 8192 no longer overflow the bit address. 0.1.2 computed it in `u16`: a release build read or wrote byte `byte_num % 8192` (e.g. DBX8192.0 became DBX0.0), a debug build panicked
- Transfers larger than 65535 bytes are no longer truncated. 0.1.2 transferred only the first 65535 bytes and returned `Ok`
- A short or truncated read response is no longer reported as success. 0.1.2 could return `Ok` with a partly filled buffer
- A response split across several TCP reads is assembled instead of being rejected
- A negotiated PDU below 29 bytes is rejected with `kind::PDU_NEGOTIATION_FAILED` instead of underflowing the chunk size
- Bit access with an empty buffer returns `kind::INVALID_FUN_PARAMETER` before any I/O

`### Changed`:

- Responses are checked against the request (reference, function, item count, transport size, lengths). A mismatch is `kind::ISO_INVALID_TELEGRAM` and disconnects, so a device that 0.1.2 tolerated may now be rejected
- `disconnect()` resets `pdu_length` to 0

`### Migration 0.1 to 0.2`:

- Bit writes with `byte_num` >= 8192 made by a 0.1.2 release build reached byte `byte_num % 8192`: check the affected PLC data. Writes of more than 65535 bytes wrote only the first 65535; the same call in 0.2.0 writes the whole buffer.

## Kolejność zadań

Gałąź `fix/s7-review-4` z HEAD `5c09959` (`test/tcp-client-mutants`, 5 commitów
przed `main`). Jeden commit na zadanie (`test:`/`refactor:`/`docs:`).

1. `test:` B2 i C1 (R4-03).
2. `test:` B3 i C4 (R4-04).
3. `refactor:` seam A i B1 (R4-01, R4-02 w domyślnym suite).
4. `test:` C2 i C3 (R4-01, R4-02 w tcp_smoke).
5. `docs:` CHANGELOG, sekcja E (R4-05).
6. Weryfikacja końcowa.

Zadania 1–2 nie zmieniają kodu produkcyjnego; zabezpieczają refaktor z zadania 3.
B2 w zadaniu 1 buduje `Transfer` literałem struktury i payload bitu literałem
`[0]`; po zadaniu 3 przechodzi na konstruktory i `bit_payload` z tabeli w B2.

## Dowody i odbiór

- Red–green przez mutację: dla każdego nowego testu mutacja z sekcji D daje
  wynik czerwony we wskazanym suite, a po cofnięciu zielony. Dla B2 i C1
  dodatkowo czerwony wynik przy zmianie jednego bajtu literału.
- Weryfikacja końcowa, z `--offline`:
  - `cargo fmt --all -- --check`,
  - Clippy przez reviewdog (`-f=clippy`, `-filter-mode=nofilter`,
    `-fail-level=warning`) z `-D warnings`,
  - `cargo test` i `cargo test --release`,
  - `cargo test --test tcp_smoke -- --ignored` (brak gniazd to NOT RUN, nie PASS),
  - `cargo doc --no-deps` bez ostrzeżeń.
- `cargo mutants -f src/client/protocol.rs -f src/client.rs -- -- --include-ignored`:
  przeżywają tylko 4 równoważne mutanty z raportu R4 (Drop, dwa w `remote_tsap`,
  strażnik w `validate_transfer`); żaden mutant nowych funkcji z sekcji A.
- `tests/public_errors.rs` bez zmian i zielony; `git diff` w `src/` dotyczy
  tylko seam z sekcji A.
