# Rust7: migracja obsługi błędów do anyerror — projekt

Status: projekt zatwierdzony sekcjami w rozmowie 2026-11-04; czeka na przegląd
pisemny. Zastępuje wcześniejszy, niezatwierdzony projekt „granicy API”.

## Cel

Błędy `rust7` mają być:

1. **serializowalne** — przesyłane i zapisywane przez aplikację (serde/JSON),
2. **diagnostyczne** — z nazwą operacji, parametrami wywołania i łańcuchem przyczyn,
3. **zgodne z aplikacją**, która już używa `anyerror::AnyError`, bez własnych konwersji,
4. **rozróżnialne programowo** po kategorii (np. utrata połączenia vs błędny adres PLC).

Zmiana łamiąca API jest dopuszczalna: wydanie `0.2.0`. Szczegóły `io::ErrorKind`
po konwersji nie są potrzebne.

## Wybrane podejście: AnyError w całej bibliotece

Enum `S7Error` znika. Protokół, transport, walidacja i publiczne API operują
bezpośrednio na `AnyError`; kategoria błędu jest przenoszona w `get_type()`.

Odrzucone: (A) typowany prywatny `S7Error` z konwersją na granicy API — bezpieczniejszy
dla logiki sesji, ale utrzymuje dwa typy błędów; (C) publiczny typowany błąd
z konwersją do `AnyError` — wymaga `map_err` po stronie aplikacji, więc słabo
realizuje cel zgodności.

Świadomy koszt: decyzja o rozłączeniu sesji opiera się na porównaniu napisów,
a nie na wyczerpującym `match`. Kompensują to: kategorie tylko jako stałe,
konstruktory jako jedyne źródło błędów, domyślne rozłączenie dla nieznanej
kategorii oraz test tabelaryczny (sekcja „Testy”).

## Stan wyjściowy

HEAD `9b7bb15` (`main`), `rust7 0.1.2`, edition 2021, brak zależności,
`#![forbid(unsafe_code)]`. `S7Error` (14 wariantów, ręczne `Display`,
`From<io::Error>`, `source()`, `breaks_session()`) w `src/client.rs`; odwołania:
`src/client.rs`, `src/client/protocol.rs` (50), testy (59), `doc/Documentation.md` (46).
`cargo test --offline`: 38 testów jednostkowych i 1 doctest zaliczone,
9 testów TCP `#[ignore]` (nieuruchomione podczas projektowania).

## Moduł `rust7::error`

Nowy plik `src/error.rs`, publiczny moduł `error`:

```rust
pub use anyerror::AnyError;
pub type Result<T> = std::result::Result<T, AnyError>;

pub mod kind {
    pub const IO: &str = "rust7::Io";
    pub const NOT_CONNECTED: &str = "rust7::NotConnected";
    pub const TCP_CONNECTION_FAILED: &str = "rust7::TcpConnectionFailed";
    pub const CONNECTION_CLOSED: &str = "rust7::ConnectionClosed";
    pub const ISO_CONNECTION_FAILED: &str = "rust7::IsoConnectionFailed";
    pub const ISO_FRAGMENTED_PACKET: &str = "rust7::IsoFragmentedPacket";
    pub const ISO_INVALID_HEADER: &str = "rust7::IsoInvalidHeader";
    pub const ISO_INVALID_TELEGRAM: &str = "rust7::IsoInvalidTelegram";
    pub const PDU_NEGOTIATION_FAILED: &str = "rust7::PduNegotiationFailed";
    pub const INVALID_FUN_PARAMETER: &str = "rust7::InvalidFunParameter";
    pub const S7_NOT_FOUND: &str = "rust7::S7NotFound";
    pub const S7_INVALID_ADDRESS: &str = "rust7::S7InvalidAddress";
    pub const S7_UNSPECIFIED: &str = "rust7::S7Unspecified";
    pub const ALL: &[&str] = &[/* 13 powyższych stałych */];
}
```

`src/lib.rs` reeksportuje `error::{kind, AnyError, Result}` w korzeniu crate'a.
Wartości stałych są publicznym, stabilnym kontraktem. Wariant `Other(String)`
(nigdzie nie konstruowany) zostaje usunięty.

### Konstruktory

Jedynym źródłem błędów biblioteki są funkcje `pub(crate)` modułu `error`:

- po jednej funkcji na kategorię domenową (`not_connected()`,
  `iso_invalid_header()`, …, 12 funkcji), zwracającej `AnyError` z kategorią
  i dotychczasowym komunikatem `Display` (np. „Invalid ISO Header”);
  wspólna prywatna funkcja pomocnicza paruje stałą z komunikatem w jednym miejscu;
- `io(e: io::Error) -> AnyError`: `AnyError::new(&e).with_type(Some(kind::IO))`;
  komunikat to tekst `io::Error`, jego zagnieżdżone przyczyny stają się łańcuchem `source()`.

Poza `src/error.rs` i `src/error/tests.rs` kod nie wywołuje `AnyError::error(`
ani `AnyError::new(`.

### Błędy I/O i `?`

`AnyError` nie ma ogólnego `From<io::Error>` (tylko `From<&E>`), więc każde
miejsce I/O w `protocol.rs` i `client.rs` otrzymuje jawne `.map_err(error::io)`.
Timeout w `read_full` tworzy `io::Error` `TimedOut` i przechodzi przez `error::io`.
`FrameRead::limit_next_read` pozostaje `io::Result<()>`; sprawdzenie
`ErrorKind::Interrupted` w `read_full` następuje przed konwersją i nie zmienia się.

### Decyzja o sesji

`pub(crate) fn breaks_session(e: &AnyError) -> bool` w module `error`:
`true` dla `IO`, `CONNECTION_CLOSED`, `ISO_INVALID_HEADER`, `ISO_INVALID_TELEGRAM`,
`ISO_FRAGMENTED_PACKET`; `false` dla pozostałych kategorii z `kind::ALL`;
`true` dla błędu bez kategorii lub z kategorią spoza `kind::ALL`.
`S7Client::finish` używa tej funkcji tak jak dotąd.

## Publiczne API

Zmienia się wyłącznie typ błędu. `read_bit` zwraca `rust7::Result<bool>`;
`rust7::Result<()>` zwracają: `set_connection_type`, `set_timeout`,
`set_connection_port`, `connect_s71200_1500`, `connect_s7300`, `connect_rack_slot`,
`connect_tsap`, `read_area`, `write_area`, `read_db`, `write_db`, `write_bit`.
Parametry, wartości sukcesu oraz `new`, `disconnect`, `Default`, `Drop` bez zmian.
Znikają `rust7::S7Error` i `rust7::client::S7Error`.

### Kontekst

Każda z 13 metod publicznych dodaje dokładnie jeden kontekst przez
`add_context(|| ...)` (formatowanie tylko dla `Err`): nazwa wywołanej metody
i parametry z kluczami `db=`, `start=`, `len=`, `area=`, `wordlen=`,
`byte_num=`, `bit_idx=`, `ip=`, `port=`, `rack=`, `slot=`, `local_tsap=`,
`remote_tsap=` — odpowiednio do parametrów metody; settery podają przekazaną
wartość. Długość bufora tak, zawartość bufora i dane PLC nigdy.

Skróty nie wywołują innych metod publicznych: `read_db`/`write_db` →
`read_area_impl`/`write_area_impl`; `connect_s71200_1500`/`connect_s7300` →
`connect_rack_slot_impl`; `connect_rack_slot` → `connect_tsap_impl`;
`read_bit`/`write_bit` → `read_bit_impl`/`write_bit_impl`. Rdzenie `*_impl`
nie dodają kontekstu. `finish` pozostaje wewnątrz `read_area_impl`/`write_area_impl`
(decyzja o sesji przed dodaniem kontekstu); wrapper dodaje kontekst do wyniku rdzenia.

Przykładowy `Display`:
`rust7::NotConnected: Not connected while: read_db db=7 start=3 len=4`.
Stabilne są kategorie i obecność pól kontekstu, nie pełny tekst.

### Zachowanie bez zmian

- Rozłączenie po błędach z listy w „Decyzja o sesji”; inne błędy nie rozłączają.
  Nieudana próba połączenia pozostawia klienta rozłączonego.
- Nieprawidłowy rack/slot odrzucony przed próbą połączenia zachowuje istniejącą sesję.
- Kolejność walidacji, czasy, liczniki; błędny bit resetuje `last_time` i `chunks`.
- Brak ponowień zapisu i atomowości transferów wieloczęściowych; po błędzie odczytu
  bufor nie jest pełnym wynikiem, wcześniejsze fragmenty zapisu mogły dotrzeć do PLC.
- Brak zmian protokołu S7, async, retry, telemetrii; `#![forbid(unsafe_code)]`.

## Serializacja i zależności

- `anyerror = { version = "0.1.13", default-features = false }` — bez `anyhow`,
  `backtrace`, `rkyv`; pociąga obowiązkowo `serde` z `derive`.
- `serde_json = "1"` wyłącznie w `[dev-dependencies]`.
- Format JSON należy do `anyerror` (`typ`, `msg`, `source`, `context`, `backtrace`);
  nie definiujemy własnego schematu ani stabilności między wersjami `anyerror`.
  Gwarancja: round-trip JSON zachowuje kategorię, kontekst i łańcuch przyczyn.
- Po konwersji nie da się odzyskać `io::ErrorKind`, `raw_os_error` ani wykonać downcastu.
- Wersja `0.2.0`; edition 2021; MSRV niedeklarowany.
- Dokumentacja traci twierdzenie o braku zależności; „bez unsafe” dotyczy kodu `rust7`.

## Testy

Nowe, w `src/error/tests.rs`:

- `breaks_session_table` — dla każdej stałej z `kind::ALL` jawna oczekiwana wartość;
  błąd bez kategorii i z obcą kategorią → `true`.
- `constructors_set_known_kind` — każdy konstruktor daje kategorię z `kind::ALL`,
  bez `source()` dla błędów domenowych.
- `io_error_round_trip` — `error::io` z zagnieżdżoną przyczyną + kontekst → JSON → z powrotem;
  równe obiekty, zachowana kategoria, kontekst i pełny łańcuch `source()`.

Nowe, w `tests/public_errors.rs`: wskaźniki do 13 metod przypisane do typów
zwracających `rust7::Result`; `AnyError: Error + Send + Sync + 'static`;
`read_db` bez połączenia → `NOT_CONNECTED` z polami `read_db`, `db=7`, `start=3`,
`len=4`; skrót ma dokładnie jeden kontekst.

Istniejące 38 testów jednostkowych i 9 TCP: `matches!(…, S7Error::X)` zastąpione
pomocnikiem `assert_kind(&err, kind::X)` przy tej samej semantyce; bez spłaszczania
do `is_err()`. Testy sesji, częściowych transferów, braku ponowień i statystyk
zachowują sprawdzenia liczby żądań, bufora, `chunks`, `last_time`, `connected`.
Test `s7_error_is_std_error_with_io_source` zastępuje `io_error_round_trip`.

Nowy test TCP (`#[ignore]`): peer zamyka połączenie w środku ramki →
`CONNECTION_CLOSED`, `connected == false`, kolejna operacja → `NOT_CONNECTED`.
Testy TCP muszą zostać wykonane w środowisku z loopbackiem przed zamknięciem pracy.

## Dokumentacja i przykłady

- `CHANGELOG.md` 0.2.0 z poradnikiem migracji 0.1 → 0.2: nowe sygnatury,
  `matches!(e, S7Error::X)` → `e.get_type() == Some(rust7::kind::X)`,
  `client.connected` jako zalecany sygnał do ponownego połączenia,
  iteracja przez `source()`, utrata `io::ErrorKind`, granice serializacji.
- Rustdoc metod, `README.md`, `doc/Documentation.md`: nowe typy, kategorie,
  zachowanie sesji. Doctest z `rust7::Result<()>` i `?` (`set_connection_port(102)`);
  przykłady sieciowe `no_run`.
- `examples/sdk/client.rs`, `examples/docker/main.rs` raportują `AnyError`
  z kontekstem; `examples/docker/Cargo.toml` → `rust7 = "0.2"`. Oba przykłady
  kompilowane przez tymczasowy manifest w scratchpadzie wskazujący lokalną
  bibliotekę; bez uruchamiania i bez Dockera.

## Bramka jakości i odbiór

`cargo fmt --all -- --check`, `cargo check --all-targets`, `cargo test`,
Clippy z `-D warnings` przez reviewdog (reporter lokalny), `cargo doc --no-deps`,
`cargo test --test tcp_smoke -- --ignored`, kontrola grep strażnika konstruktorów.

Sukces: brak `S7Error` w repozytorium (poza historią w CHANGELOG/poradniku),
13 sygnatur z `rust7::Result`, wszystkie dotychczasowe zachowania pokryte
przechodzącymi testami, nowe testy kategorii/kontekstu/JSON zaliczone,
dokumentacja i przykłady kompilują się z lokalną wersją.
