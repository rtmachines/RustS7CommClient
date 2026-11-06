# Rust7: naprawy po przeglądzie 3 (R3) — projekt

Status: projekt zatwierdzony dwiema częściami w rozmowie 2026-11-06; czeka na
przegląd pisemny. Źródło wymagań: raport przeglądu runda 3 na HEAD
`1ac916efb7783c5dd95cb1e898a073a2ea649510` (rozmowa z użytkownikiem).
Poprzednie decyzje: [F1–F7](2026-10-27-s7-review-fixes-design.md),
[anyerror](2026-11-04-anyerror-migration-design.md), `AGENTS.md`.

## Cel

1. Przypiąć testami kontrole długości, których usunięcie daje panikę lub
   fałszywy sukces zapisu na danych z sieci (R3-01…03).
2. Pozwolić użytkownikowi ustalić, które bajty zapisu PLC potwierdził, i
   opisać, że część, na której wystąpił błąd, mogła zostać wykonana (R3-04).
3. Poprawić diagnostykę: jeden komunikat timeoutu odczytu (R3-05), kody błędów
   S7 w komunikacie (R3-06), przyczyna błędu ramkowania przy negocjacji (R3-07).
4. Uporządkować targety Cargo przykładów (R3-13, R3-14) i wyrównać dokumentację
   z kodem (R3-15…R3-22).

## Znaleziska w zakresie

| ID | Miejsce (HEAD 1ac916e) | Wymaganie |
|---|---|---|
| R3-01 | `src/client/protocol.rs:226` | Test: ACK odczytu z `bits` zgodnym z oczekiwaniem i inną długością danych daje `ISO_INVALID_TELEGRAM`, bez paniki w `:380` |
| R3-02 | `src/client/protocol.rs:210` | Test: spójny ACK odczytu z polem danych < 4 B daje `ISO_INVALID_TELEGRAM`, bez paniki |
| R3-03 | `src/client/protocol.rs:235` | Test: ACK zapisu z 0 lub ≥ 2 B danych daje `ISO_INVALID_TELEGRAM`, nie sukces |
| R3-04 | `src/client.rs:571`, `:748`; `README.md:64-66`; `doc/Documentation.md`; `AGENTS.md` | Pole `pub confirmed: usize` i dokumentacja części, na której wystąpił błąd |
| R3-05 | `src/client/protocol.rs:100`, `:107` | Timeout odczytu zawsze daje ten sam komunikat z wartością `rd_timeout` |
| R3-06 | `src/client/protocol.rs:179-186`, `:194-200` | Klasa i kod błędu nagłówka S7 oraz kod zwrotny elementu w komunikacie |
| R3-07 | `src/client/protocol.rs:50` | Oryginalny błąd ramkowania odpowiedzi Setup Communication w łańcuchu `source()` |
| R3-13 | `Cargo.toml`, `AGENTS.md:60-61`, `examples/README.md:10` | `examples/docker` nie jest przykładem głównego pakietu |
| R3-14 | `examples/sdk/client.rs` | Przykład SDK kompilowany przez `cargo check --all-targets` |
| R3-15…R3-22 | rustdoc, `doc/Documentation.md`, przykład SDK | Dokumentacja i komentarze zgodne z kodem (tabela w sekcji G) |

Poza zakresem: R3-08…R3-12, R3-23…R3-25 oraz timeout zapisu (`write_all` przy
`wr_timeout` nadal zwraca tekst systemowy, np. EAGAIN; `protocol.rs` nie zna tej
wartości, a przegląd nie dał dowodu problemu w praktyce). Bez zmian pozostają
H1–H3 z poprzednich rund.

## Ograniczenia

- Edition 2021, `#![forbid(unsafe_code)]`, synchroniczny kod, bez nowych
  zależności runtime. Jedyna nowa zależność: `proptest = "1"` w
  `[dev-dependencies]` (1.12 w lokalnym cache, więc `--offline` działa).
- Kategorie `kind` i ich reguły rozłączania bez zmian. Zmienia się tylko treść
  komunikatów, której CHANGELOG nie obiecuje jako stabilnej.
- Każda publiczna metoda nadal dodaje dokładnie jeden kontekst. Przyczyny
  (`source()`) i treść komunikatu nie są kontekstem.
- Bez automatycznych ponowień zapisu; zapis wieloczęściowy pozostaje nieatomowy.
- Testy jednostkowe bez sieci; transport TCP tylko w `tests/tcp_smoke.rs`
  (`#[ignore]`). Bez kontaktu z PLC; przykłady tylko kompilowane.
- Komendy Cargo z `--offline`. Zmiany widoczne dla użytkownika w `CHANGELOG.md`,
  sekcja `[0.2.0] - Unreleased`.

## Projekt

### A. Testy przypinające (R3-01…03)

W `src/client/protocol/tests.rs`, każdy z asercją kategorii (`assert_kind`):

- `read_payload_length_must_match_bits_field`: `bits=32`, 5 B danych, oczekiwane
  4 B → `ISO_INVALID_TELEGRAM`; bufor docelowy nietknięty.
- `read_item_shorter_than_header_is_rejected`: `ack(1, 4, &[0xff, 4, 0])` →
  `ISO_INVALID_TELEGRAM`.
- `write_ack_with_extra_data_is_not_success`: dane ACK zapisu `[0xff, 0x00]`
  oraz `[]` (przy `params=2`) → `ISO_INVALID_TELEGRAM`.

Property testy (`proptest`):

- Generator spójnego ACK: dowolny ROSCTR, referencja, klasa i kod błędu,
  parametry (w tym `[function, 1]` z niezerowym prawdopodobieństwem) i dane
  o długości 0..=40 B; pola długości zgodne z rzeczywistą zawartością, więc
  ramka przechodzi kontrolę `12 + params + data == len`.
- Własność 1: `validate_read_response(pdu, ref, wl, n)` dla `wl ∈ {1, 2}`,
  `n ∈ 1..=462` (dla `wl = 1` tylko `n = 1`) i `validate_write_response(pdu, ref)`
  zwracają `Ok` albo błąd z `get_type() ∈ kind::ALL`, nigdy panikę. Przy `Ok`
  z odczytu długość wyniku wynosi dokładnie `n`.
- Własność 2: `read_transfer` z dowolną ramką odpowiedzi (losowe bajty po
  poprawnym nagłówku TPKT/COTP) nie panikuje, a przy błędzie bufor poza
  potwierdzonymi częściami pozostaje niezmieniony.

Dowód skuteczności: dla każdego testu przykładowego tymczasowa mutacja
odpowiedniej linii (`:210`, `:226`, `:235`) daje wynik czerwony; po cofnięciu
zielony. Własność 1 musi zawieść przy mutacji `:210` lub `:226`.

### B. Postęp transferu (R3-04)

- Nowe pole `S7Client::confirmed: usize`: liczba bajtów bufora potwierdzonych
  w ostatniej operacji odczytu lub zapisu. Odczyt: bajty zweryfikowane i
  skopiowane do bufora. Zapis: bajty, których zapis PLC potwierdził ACK.
- Zerowane razem z `chunks` na początku `read_area_impl`, `write_area_impl`,
  `read_bit_impl`, `write_bit_impl`, także gdy operacja kończy się błędem
  parametru lub `NOT_CONNECTED`. Udany transfer bajtowy: `confirmed ==
  buffer.len()`. Udany transfer bitowy: `confirmed == 1`.
- `read_transfer` i `write_transfer` dostają parametr `confirmed: &mut usize`,
  ustawiany na `offset + count` po każdej zaakceptowanej części.
- Zmiana addytywna: `S7Client` ma pola prywatne, więc użytkownicy nie tworzą go
  literałem struktury.
- Dokumentacja (rustdoc `write_area`, `write_db`, `write_bit`, `read_area`;
  README „Transfer behavior”; Documentation.md; AGENTS.md „Ograniczenia
  projektu”): po błędzie bajty `[0, confirmed)` są potwierdzone; część
  zaczynająca się od `confirmed` mogła zostać wykonana przez PLC albo nie,
  także przy zapisie jednoczęściowym i `write_bit`; pozostałe bajty nie zostały
  wysłane. Przy odczycie bufor poza `[0, confirmed)` nie jest wynikiem.
- Testy: jednostkowy (błąd 2. części zapisu i odczytu → `confirmed` = rozmiar
  1. części); `src/client/tests.rs` (zerowanie przy błędzie parametru);
  tcp_smoke (`late_reply…`: timeout po wysłaniu jednoczęściowego zapisu →
  `confirmed == 0`, `chunks == 1`; scenariusz normalny → `confirmed ==
  buffer.len()`); `tests/public_errors.rs` (pole istnieje i ma typ `usize`).

### C. Timeout odczytu (R3-05)

W `read_full` każdy z przypadków: termin wygasł przed `read`, `read` zwrócił
`WouldBlock` lub `TimedOut`, daje
`error::io(io::Error::new(io::ErrorKind::TimedOut, format!("no complete response frame within {} ms", timeout.as_millis())))`.
`timeout` to wartość przekazana do `read_tpkt` (czyli `rd_timeout`). Kategoria
`kind::IO`, klient rozłącza się jak dotąd. Pozostałe błędy I/O bez zmian.

Testy: `read_tpkt_propagates_eof_and_timeout` sprawdza identyczny tekst dla
`WouldBlock`, `TimedOut` i wygaśnięcia terminu; `frame_deadline_bounds_slow_peer`
sprawdza tekst; tcp_smoke `late_reply…` sprawdza, że komunikat zawiera
`within 200 ms`.

### D. Kody błędów S7 (R3-06)

W `src/error.rs` dwa konstruktory z kategorią `kind::S7_UNSPECIFIED` i
dynamicznym komunikatem (`AnyError::error(String).with_type(...)`):

- `s7_header_error(class: u8, code: u8)`:
  `S7 unspecified error (error class 0x81, code 0x04)`. Używany w `ack_data`
  dla ROSCTR 3 z niezerowym błędem oraz header-only error ACK przy ROSCTR 2.
- `s7_item_error(code: u8)`: `S7 unspecified error (item return code 0x03)`.
  Używany w `item_status` w gałęzi `_`.

`S7_NOT_FOUND` (0x0A) i `S7_INVALID_ADDRESS` (0x05) bez zmian.
`s7_unspecified()` zostaje usunięte: wszystkie trzy użycia produkcyjne
(`protocol.rs:180`, `:186`, `:199`) przechodzą na nowe konstruktory, a pozostawienie
go tylko dla testów dałoby `dead_code` w buildzie bez testów. Testy:
`response_preserves_plc_errors` i `terminal_error_ack_reports_plc_error`
sprawdzają kategorię i obecność kodów w `to_string()`; `constructors_set_known_kind`
(`src/error/tests.rs:61`) i `only_transport_errors_break_the_session`
(`src/client/tests.rs:97`) używają nowych konstruktorów.

### E. Przyczyna przy negocjacji (R3-07)

`AnyError` 0.1.13 nie ma metody dołączającej przyczynę, ale `AnyError::new(&e)`
przejmuje łańcuch `e.source()`. W `src/error.rs` powstaje prywatny typ
`Caused { msg: &'static str, source: AnyError }` z `Display` = `msg` i
`Error::source` = `Some(&self.source)` oraz konstruktor
`pdu_negotiation_failed_by(source: AnyError) -> AnyError`
(`AnyError::new(&Caused { .. }).with_type(Some(kind::PDU_NEGOTIATION_FAILED))`).
`negotiate` używa go zamiast `map_err(|_| pdu_negotiation_failed())`.

Pierwszy krok zadania: test sprawdzający, jak odczytać kategorię przyczyny
(`source()` + `downcast_ref::<AnyError>()` albo tekst `{e:#}`), zanim powstanie
kod produkcyjny. Test docelowy: odpowiedź Setup z zepsutym bajtem COTP 5
(`0xf0`) lub 6 (EOT) → kategoria `PDU_NEGOTIATION_FAILED`, przyczyna
`ISO_INVALID_HEADER` lub `ISO_FRAGMENTED_PACKET`. Rustdoc `connect_tsap`:
„the framing error, if any, is the error's `source()`”.

### F. Targety Cargo (R3-13, R3-14)

`Cargo.toml` w `[package]`: `autoexamples = false`; nowa sekcja
`[[example]] name = "sdk_client"`, `path = "examples/sdk/client.rs"`.
Skutek: `examples/docker/main.rs` przestaje być przykładem głównego pakietu
(zdanie w AGENTS.md staje się prawdziwe), a `cargo check --all-targets` i Clippy
kompilują przykład SDK. AGENTS.md: dopisek w tabeli komend, że
`--all-targets` obejmuje `sdk_client`. `examples/README.md`,
`examples/sdk/README.md`: budowanie przez `cargo build --example sdk_client`;
uruchomienie tylko przeciw wskazanemu urządzeniu.

Dowód: `cargo metadata --offline --no-deps --format-version 1` zawiera
`sdk_client`, nie zawiera `docker`; celowy błąd składni w przykładzie łamie
`cargo check --offline --all-targets` (sprawdzone tymczasowo, cofnięte).

### G. Dokumentacja (R3-15…R3-22)

| ID | Miejsce | Zmiana |
|---|---|---|
| R3-15 | `examples/sdk/client.rs:61` | Etykieta `DB100.DBX47.5`, zgodna z `read_bit(…, 47, 5)` |
| R3-16 | `src/client.rs:115`, `:122` | `CT_PG`, `CT_OP`, `CT_S7` zamiast `S7_PG`, `S7_OP`, `S7_BASIC` |
| R3-17 | `doc/Documentation.md:237`, `:388` | Parametr `port` (1..65535) w `set_connection_port`; `buffer` w sygnaturze `write_area` |
| R3-18 | `doc/Documentation.md:323-324`, `:374-379`, `:430-440` | `INVALID_FUN_PARAMETER` w listach „High level”; dostęp bitowy wysyła 1 bit z pierwszego bajtu; brak atomowości i `confirmed` przy `write_area`; lista błędów `connect_tsap` jak w rustdoc |
| R3-19 | `doc/Documentation.md:62-77` | Pętla: `client`, logowanie błędu connect zamiast `let _ =`, `thread::sleep(Duration::from_millis(100))` |
| R3-20 | `src/client.rs:355`, `:363-364` | Komentarze CR: `0x11` = długość nagłówka COTP (LI), `0x01` = długość parametru C0, `0x0A` = wykładnik rozmiaru TPDU (2^10) |
| R3-21 | `src/client.rs:153`, `doc/Documentation.md:217` | `wr_timeout_ms`: timeout wysłania pojedynczego żądania; ACK zapisu ogranicza `rd_timeout_ms` |
| R3-22 | `src/client.rs:443`, `:554`, Documentation.md | `CONNECTION_CLOSED`: peer zamknął połączenie (EOF przy odczycie); reset połączenia daje `kind::IO` |

Rustdoc i Documentation.md opisują także zmiany B–E. Numery linii odnoszą się
do `1ac916e` i przesuną się po zadaniach 1–5; w planie wskazywać je przez
treść.

## Kolejność zadań

Gałąź `fix/s7-review-3` z `main` (`1ac916e`). Jeden commit na zadanie
(`test:`/`fix:`/`feat:`/`docs:`), jedna linia w CHANGELOG na zadanie zmieniające
zachowanie lub dokumentację.

1. Testy R3-01…03 i property testy (sam kod testów i dev-dependency).
2. Timeout odczytu R3-05.
3. Diagnostyka S7 i negocjacji R3-06, R3-07.
4. Pole `confirmed` i dokumentacja R3-04 (zmienia sygnatury transferów, więc po
   zadaniach 2–3).
5. Targety Cargo R3-13, R3-14.
6. Dokumentacja R3-15…R3-22.
7. Weryfikacja końcowa.

## Dowody i odbiór

- Każdy nowy test przechodzi cykl red–green: test zmian zachowania (zadania
  2–4) najpierw czerwony; test przypinający (zadanie 1) czerwony przy mutacji.
- Weryfikacja końcowa, z `--offline`:
  - `cargo fmt --all -- --check`,
  - Clippy przez reviewdog (`-f=clippy`, `-filter-mode=nofilter`,
    `-fail-level=warning`) oraz `cargo clippy --all-targets -- -D warnings`,
  - `cargo test` i `cargo test --release`,
  - `cargo test --test tcp_smoke -- --ignored` (brak gniazd to NOT RUN, nie PASS),
  - `cargo doc --no-deps` bez ostrzeżeń,
  - `unshare -rn cargo test`, jeśli przestrzenie użytkownika są dostępne,
  - `cargo metadata` zgodne z sekcją F.
- Mutacje z przeglądu (F1–F7, S7-01…S7-08) nadal wykrywane; mutacja F2
  (usunięcie kontroli długości payloadu) teraz wykrywana.
