# Rust7: asynchroniczny klient S7 na tokio — projekt

Status: zatwierdzony 2026-11-07 (wraz z aktualizacją `AGENTS.md`); zaimplementowany. Obowiązują: `AGENTS.md` (komendy, kontrakt błędów,
ograniczenia) oraz decyzje z [R4](2026-11-07-s7-review-4-fixes-design.md) i
[anyerror](2026-11-04-anyerror-migration-design.md).

## Cel i kryterium sukcesu

Async odpowiednik `S7Client` z tymi samymi operacjami (setery, `connect_*`,
`disconnect`, `read_area`/`write_area`, `read_db`/`write_db`, `read_bit`/`write_bit`)
i tymi samymi publicznymi polami stanu (`connected`, `pdu_length`, `last_time`,
`chunks`, `confirmed`). API synchroniczne i jego zachowanie się nie zmieniają.

Zadanie jest skończone, gdy komendy z `AGENTS.md` (fmt check, clippy `-D warnings`,
test, `tcp_smoke --ignored`, doc) przechodzą bez feature'a i z `--features tokio`,
a `cargo tree -e normal` bez feature'a nie zawiera tokio.

## 1. Typ, ścieżka i feature

- Typ: `AsyncS7Client` w `src/client/async_client.rs`. Publiczne ścieżki:
  `rust7::client::AsyncS7Client` i reeksport `rust7::AsyncS7Client`, tak samo jak
  dla `S7Client`. Oba wpisy mają `#[cfg(feature = "tokio")]`.
  Odrzucona alternatywa: `rust7::tokio::S7Client`. Ta sama nazwa co typ sync myli
  przy imporcie obu typów, a moduł `tokio` zasłania crate `tokio` wewnątrz `rust7`.
- `Cargo.toml`:
  ```toml
  [features]
  tokio = ["dep:tokio"]

  [dependencies]
  tokio = { version = "1.48", optional = true, default-features = false,
            features = ["net", "time", "io-util"] }

  [dev-dependencies]
  tokio = { version = "1.48", features = ["net", "time", "io-util", "rt", "macros", "test-util"] }

  [package.metadata.docs.rs]
  features = ["tokio"]
  ```
  Bez feature'a drzewo `-e normal` się nie zmienia, bo dev-dependencies do niego
  nie należą. `Cargo.lock` zyska wpisy tokio (optional i dev). Przy implementacji
  sprawdzę ten warunek komendą `cargo tree -e normal`.
- Zgodność z MSRV: tokio 1.54.1 (najnowsze w lokalnym cache) ma `rust-version = "1.85"`,
  a MSRV crate'a to 1.99. Lokalny toolchain to dokładnie rustc 1.99.1, więc kompilacja
  potwierdza zgodność całego drzewa (mio, socket2, bytes, libc, pin-project-lite).
  Dolną granicę `1.48` sprawdzę jednorazowo: `cargo update -p tokio --precise 1.48.0`
  i `cargo check --features tokio --all-targets` (offline). Potem przywracam 1.54.1.
  Jeśli 1.48 nie przejdzie, podniosę wymaganie do najniższej działającej wersji z cache.

## 2. Granica sync/async I/O: jedna logika protokołu

Zasada: cała logika bez I/O zostaje w `src/client/protocol.rs` i jest wspólna.
Osobno, raz dla sync i raz dla async, istnieją tylko pętle I/O, każda na kilka linii.

### Wydzielane z istniejącego kodu (sans-I/O, `protocol.rs`)

| Nowa funkcja/typ | Pochodzenie | Cel |
|---|---|---|
| `connection_request(local_tsap, remote_tsap) -> [u8; 22]` | literał `iso_cr` i makra `hi_part!`/`lo_part!` z `client.rs` | CR wspólny dla obu klientów |
| `negotiation_request(tpdu_limit) -> ([u8; 25], u16)` + `accept_negotiation(frame, requested) -> Result<PduLimits>` | treść `negotiate` | Setup Communication bez I/O |
| `tpkt_length(header: &[u8; 4], max_total) -> Result<usize>` | środek `read_tpkt` | walidacja nagłówka TPKT |
| `timed_out(timeout)` (widoczność `pub(super)`) | bez zmian | identyczny komunikat `no complete response frame within <N> ms` |
| `TransferPlan` + `Chunk` | pętle `read_transfer`/`write_transfer` | podział na telegramy, referencja, budowa żądania, walidacja odpowiedzi, kopiowanie danych, `Progress` |

`TransferPlan` w zarysie:

```rust
pub(super) struct Chunk { pub(super) request: Vec<u8>, reference: u16, offset: usize, count: usize }

impl TransferPlan {
    fn read(limits: &PduLimits, transfer: Transfer, len: usize) -> Result<Self>;   // valid_area + validate_transfer
    fn write(limits: &PduLimits, transfer: Transfer, buffer: &[u8]) -> Result<Self>;
    fn next_read(&mut self, reference: &mut u16) -> Option<Chunk>;
    fn next_write(&mut self, reference: &mut u16, buffer: &[u8]) -> Option<Chunk>;
    fn frame_limit(&self) -> usize;                                                // pdu_length + 7
    fn accept_read(&mut self, chunk: &Chunk, frame: &[u8], buffer: &mut [u8], progress: &mut Progress) -> Result<()>;
    fn accept_write(&mut self, chunk: &Chunk, frame: &[u8], progress: &mut Progress) -> Result<()>;
}
```

Pętla (sync; async różni się tylko `.await` i timeoutami):

```rust
let mut plan = TransferPlan::read(limits, transfer, buffer.len())?;
while let Some(chunk) = plan.next_read(reference) {
    progress.chunks += 1;
    io.write_all(&chunk.request).map_err(error::io)?;
    let frame = read_tpkt(io, plan.frame_limit(), timeout)?;
    plan.accept_read(&chunk, &frame, buffer, progress)?;
}
```

Istniejące `negotiate`, `read_tpkt`, `read_transfer` i `write_transfer` zachowują
sygnatury i zachowanie, ale korzystają z powyższych funkcji. Dzięki temu
`src/client/protocol/tests.rs`, `src/client/tests.rs` i `tests/tcp_smoke.rs`
nie zmieniają asercji.

### Wspólna warstwa klienta (`src/client/shared.rs`)

Prywatne pola konfiguracji zostają w `S7Client` w obecnej postaci, bo testy
jednostkowe czytają `client.port`, `client.co_timeout_ms` i inne. Wspólne są
tylko funkcje:
- walidacja seterów (`check_connection_type`, `check_timeouts`, `check_port`)
  oraz `check_area`, która deleguje do `protocol::valid_area`. To usuwa duplikat
  odnotowany w R4-12, w zakresie, w jakim dotyka tej pracy;
- formatowanie kontekstów (`ctx::read_area(...)`, `ctx::connect_tsap(...)` itd.).
  Gwarantuje to identyczny kontekst w obu klientach, jeden na metodę publiczną.

Glue po stronie każdego klienta zostaje zdublowane, bo zależy od pól i typu
strumienia: zerowanie statystyk, `finish` (`last_time` albo `breaks_session` →
`disconnect`) i kolejność kontroli (parametry → `connected` → I/O).

### Async I/O (`src/client/protocol/async_io.rs`, `cfg(feature = "tokio")`)

Generyczne po `T: AsyncRead + AsyncWrite + Unpin + Send`, żeby testy jednostkowe
mogły używać `tokio::io::duplex`:
- `read_tpkt(io, max_total, timeout)`: odczyt nagłówka i treści w całości wewnątrz
  `tokio::time::timeout(timeout, …)`, więc timeout obejmuje całą ramkę. Po
  przekroczeniu zwraca `protocol::timed_out(timeout)` (`kind::IO`, ten sam komunikat
  co sync). `read` zwracające 0 daje `error::connection_closed()`, pozostałe błędy
  dają `error::io`. Za duży timeout nie powoduje paniki, bo tokio przechodzi wtedy
  na „far future”.
- `negotiate`, `read_transfer`, `write_transfer`: te same pętle co w sync, na
  `TransferPlan` / `negotiation_request` / `accept_negotiation`.
- Każdy zapis żądania ma limit `wr_timeout_ms` (`tokio::time::timeout` wokół
  `write_all` i `flush`). Po przekroczeniu zwraca `kind::IO` z komunikatem
  `request not sent within <N> ms`. Tak jak w sync timeout zapisu jest ustalany
  przy łączeniu (pole `session_wr_timeout`), a timeout odczytu działa od razu po
  `set_timeout` (poprawka po code review, 2026-11-07).

### `AsyncS7Client` (`src/client/async_client.rs`)

- Pola jak w `S7Client`, z `stream: Option<tokio::net::TcpStream>`.
- `new()`, `Default` i synchroniczne setery (bez I/O) z tą samą walidacją i tym
  samym kontekstem.
- `connect_*` jako `async fn`. Najpierw `disconnect()`, potem rozwiązanie adresu:
  literał IP jest parsowany bez DNS, nazwa idzie przez `tokio::net::lookup_host`.
  Bierzemy pierwszy adres; jego brak daje `kind::TCP_CONNECTION_FAILED`.
  Dalej `timeout(co_timeout, TcpStream::connect)`, `set_nodelay(true)`, CR/CC
  i negocjację PDU z `rd_timeout_ms` na każdą ramkę. Strumień trafia do `self`
  dopiero po obu handshake'ach, tak jak w sync.
- `disconnect(&mut self)` pozostaje **synchroniczne**: zrzuca strumień (zamyka
  gniazdo), zeruje `limits`, `connected` i `pdu_length`. Async `shutdown` nie
  nadaje się do `Drop` ani do strażnika anulowania, a zrzucenie zamyka połączenie
  tak samo jak `shutdown(Both)` + drop w sync.
- `read_*`/`write_*` jako `async fn(&mut self, …)`, bez blokad wewnętrznych.
- `Drop` wywołuje `disconnect()`.

## 3. Cancel-safety

Mechanizm: strażnik `InFlight<'a> { client: &'a mut AsyncS7Client, progress: Progress, done: bool }`.
Metoda tworzy go przed pierwszym zapisem do gniazda, czyli po kontroli parametrów
i `connected`, które wykonują się synchronicznie w pierwszym `poll`. I/O odbywa się
przez rozłączne pożyczki pól `guard.client` i `guard.progress`. Po zakończeniu
pętli strażnik ustawia `done = true`, przepisuje `progress` do `chunks`/`confirmed`
i woła `finish`.

`Drop` strażnika z `done == false` oznacza, że future porzucono w trakcie I/O.
Wtedy przepisuje `chunks`/`confirmed` i woła `client.disconnect()`. Skutek:
`connected == false`, `pdu_length == 0`, strumień zamknięty, `last_time == 0`,
a następne wywołanie zwraca `kind::NOT_CONNECTED`. Przy anulowanym zapisie
semantyka `confirmed` jest taka sama jak po błędzie: `[0, confirmed)` potwierdzone,
porcja od `confirmed` mogła zostać zapisana albo nie, a dalsze nie zostały wysłane.
Nic nie jest ponawiane.

`connect_*` nie potrzebuje strażnika. `disconnect()` wykonuje się w pierwszym
`poll`, a nowy strumień jest lokalny aż do końca handshake'u, więc porzucenie
w trakcie zostawia klienta rozłączonego.

Rustdoc (sekcja `# Cancel safety` na typie i odnośnik w każdej metodzie I/O):
future, który nigdy nie był `poll`-owany, nie ma żadnego efektu. Porzucenie
future'a po rozpoczęciu I/O, np. przez `select!` albo `timeout`, rozłącza klienta.
Metody nie są cancel-safe w sensie tokio, bo nie da się ich bezpiecznie wznowić.
Po porzuceniu trzeba ponownie się połączyć.

`Send`: future'y przechowują tylko `&mut AsyncS7Client`, `&str`, bufory, `Progress`
i future'y tokio, a wszystkie te typy są `Send`. Sprawdza to test kompilacyjny
(sekcja 5).

## 4. Zmiany publicznego API

- Nowe (tylko z `--features tokio`): feature `tokio` i typ `AsyncS7Client` w obu
  ścieżkach. Typ ma `new`, `Default`, `Drop`, setery `set_connection_type`,
  `set_timeout`, `set_connection_port` (sync, `Result<()>`), `async`
  `connect_s71200_1500`, `connect_s7300`, `connect_rack_slot`, `connect_tsap`,
  `read_area`, `write_area`, `read_db`, `write_db`, `read_bit` (`Result<bool>`),
  `write_bit`, sync `disconnect` oraz pola `pdu_length`, `connected`, `last_time`,
  `chunks`, `confirmed`.
- Kategorie `kind` bez zmian (bez nowych stałych). Reguły rozłączania, konteksty
  i kolejność kontroli są takie same jak w sync.
- Różnice względem sync, opisane w rustdoc:
  1. timeout zapisu obejmuje całe żądanie, a nie pojedyncze `send`; komunikat
     `request not sent within <N> ms`;
  2. upływ czasu połączenia daje `kind::IO` z komunikatem
     `TCP connection not established within <N> ms`;
  3. wymagany runtime tokio 1 z `enable_io()` i `enable_time()`. Rozwiązywanie
     nazw hosta używa puli blokującej tokio, więc proces musi mieć tokio z feature
     `rt`. Każda aplikacja z runtime'em tokio 1 go ma, bo feature'y się unifikują.
     Adres IP podany literałem nie używa puli.
- API sync: bez zmian w sygnaturach i zachowaniu. `tests/public_errors.rs`
  (część sync) zostaje bez zmian.

## 5. Testy

Unit, bez `#[ignore]`, `--features tokio`, w `src/client/protocol/async_io/tests.rs`
na `tokio::io::duplex`:
- ramka rozbita na wiele zapisów (w tym po 1 bajcie) jest składana;
- timeout ramki: przy wstrzymanym czasie (`start_paused`) kawałki przychodzą
  częściej niż co `timeout`, ale cała ramka nie mieści się w limicie. Wynik to
  `kind::IO` z dokładnym komunikatem, co dowodzi, że limit dotyczy całej ramki;
- EOF w środku ramki daje `kind::CONNECTION_CLOSED`;
- test zgodności sync/async: te same transfery (wiele PDU, `PduLimits::new(29)`,
  bajty i bity, odczyt i zapis) wysyłają identyczne bajty żądań i dają te same
  `Progress`/bufory co sync `read_transfer`/`write_transfer` na skryptowanym peerze;
- błąd PLC (item code 10, header-only error ACK) daje tę samą kategorię co w sync.

TCP, `#[ignore = "requires permission to open loopback sockets"]`, w
`tests/tcp_smoke/async_client.rs`, włączanym przez
`#[cfg(feature = "tokio")] #[path = "tcp_smoke/async_client.rs"] mod async_client;`
w `tests/tcp_smoke.rs`. Komenda
`cargo test --test tcp_smoke --features tokio -- --ignored` uruchamia wtedy oba
zestawy. Mock używa `tokio::net::TcpListener` i reużywa budowania ramek z tcp_smoke:
1. happy path: connect, a potem `read_db`/`write_db` przez wiele PDU (mała
   negocjowana PDU). Sprawdzane są dane, `chunks`, `confirmed`, `last_time > 0`
   i żądania widziane przez mocka;
2. odpowiedź wysłana w kilku segmentach TCP (`nodelay`, pauzy między kawałkami);
3. timeout ramki: mock wysyła część ramki i milknie. Wynik to `kind::IO`
   z komunikatem `no complete response frame within <N> ms` i `connected == false`;
4. EOF w trakcie odpowiedzi daje `kind::CONNECTION_CLOSED` i `connected == false`;
5. błąd PLC zachowuje sesję: po `kind::S7_NOT_FOUND` `connected == true`, a kolejny
   odczyt na tej samej sesji się udaje;
6. cancel-safety: `tokio::time::timeout(50 ms, client.read_db(...))` przy mocku,
   który odebrał żądanie i wysłał pół odpowiedzi. Oczekiwane: `Elapsed`,
   `connected == false`, `pdu_length == 0`, mock odczytuje EOF (strumień zamknięty),
   a następne `read_db` daje `kind::NOT_CONNECTED`. Wariant z `write_db` wielu PDU
   sprawdza, że `confirmed` równa się bajtom potwierdzonym przed porzuceniem.

API, w nowym module `#[cfg(feature = "tokio")] mod async_api` w
`tests/public_errors.rs`:
- sygnatury: setery jako wskaźniki `fn`, metody async przez kompilowaną funkcję
  `async fn`, która przypisuje `.await` do `rust7::Result<()>` / `rust7::Result<bool>`;
- `Send`: `fn assert_send<F: Future + Send>(_: F)` dla future'a każdej metody I/O
  (tworzonego, nie `poll`-owanego);
- pola publiczne, `kind::NOT_CONNECTED` bez połączenia, pojedyncze konteksty
  identyczne z sync oraz kolejność „parametry przed połączeniem”.

TDD: kolejność kroków to (1) refaktor sans-I/O w protocol.rs pod istniejącymi
testami, (2) testy async_io → implementacja, (3) testy API → klient,
(4) testy TCP → poprawki.

## 6. Dokumentacja

- `CHANGELOG.md`, `[0.2.0] - Unreleased`, `### Added`: feature `tokio`,
  `AsyncS7Client`, cancel-safety, różnice timeoutów i wymaganie runtime'u.
- `README.md`: sekcja „Async (tokio)” z przykładem `rust,no_run`. Ponieważ README
  jest dokumentacją crate'a (`include_str!`), przykład ma ukryte linie
  `# #[cfg(feature = "tokio")]` przed `#[tokio::main(flavor = "current_thread")]` oraz
  `# #[cfg(not(feature = "tokio"))] # fn main() {}`. Doctest kompiluje się wtedy bez
  feature'a, a z feature'em sprawdza API. Na GitHubie te linie są widoczne jako `#`.
- Rustdoc: dokumentacja typu z `# Cancel safety`, `# Runtime` i odniesieniami do
  dokumentacji metod `S7Client` (sekcje Errors/Notes są wspólne, więc opisuję tylko
  różnice, żeby nie kopiować ~300 linii).
- `AGENTS.md`: dopisane warianty komend z `--features tokio`, `cargo tree -e normal`
  oraz ograniczenia klienta async (zatwierdzone).
- `doc/Documentation.md` i przykłady bez zmian (poza zakresem).

## Znane ograniczenia (zamierzone)

- Jedno żądanie naraz na klienta (`&mut self`). Brak współbieżnych zapytań
  i pipeliningu.
- Testy klienta async na poziomie TCP wymagają gniazd loopback (`--ignored`).
  Warstwa protokołu jest testowana na `duplex` bez gniazd.
- Metody nie są wznawialne po anulowaniu. Anulowanie zawsze kończy sesję.
- Pętle I/O (sync i async) oraz `finish` istnieją w dwóch kopiach po kilka linii,
  a cała logika protokołu jest wspólna.
