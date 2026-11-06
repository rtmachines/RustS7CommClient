# RustS7CommClient — instrukcje dla agentów

Biblioteka `rust7`: natywny klient protokołu S7 w Rust, komunikujący się z PLC Siemens.
Ten plik jest wspólnym źródłem instrukcji projektu; nie twórz kopii w CLAUDE.md.

## Środowisko i komendy

Uruchamiaj komendy z katalogu zawierającego główny Cargo.toml. Potrzebujesz
toolchainu Rust obsługującego edition 2021, Cargo oraz komponentów rustfmt i clippy. Uruchomienie testów (dev-dependency proptest) wymaga
Rust ≥ 1.88; sama biblioteka wymaga tylko edition 2021.

| Cel | Komenda |
| --- | --- |
| Formatowanie zmian Rust | `cargo fmt --all` |
| Kontrola formatowania | `cargo fmt --all -- --check` |
| Kontrola kompilacji wszystkich targetów (w tym przykładu `sdk_client`) | `cargo check --all-targets` |
| Budowanie biblioteki | `cargo build` |
| Testy jednostkowe i dokumentacji | `cargo test` |
| Wybrany test | `cargo test <fragment_nazwy>` |
| Testy TCP na lokalnym mocku | `cargo test --test tcp_smoke -- --ignored` |
| Lint | `cargo clippy --all-targets -- -D warnings` |
| Dokumentacja API | `cargo doc --no-deps` |

Cargo obsługuje tutaj `--offline`; używaj go, gdy potrzebne zależności są lokalnie.
Po zmianach Rust uruchom kontrolę formatowania, Clippy oraz testy. Dla zmian
transportu uruchom również ignorowane testy TCP; wymagają dostępu do lokalnych
gniazd, więc w sandboksie bez sieci mogą nie przejść z przyczyn środowiskowych.
Po zmianach dokumentacji API sprawdź także generowanie dokumentacji.

## Nawigacja

Narzędzia code-review-graph i reviewdog opisują instrukcje globalne użytkownika;
tutaj obowiązuje tylko to, co specyficzne dla Rust. Definicje, implementacje
traitów i referencje weryfikuj przez rust-analyzer/LSP (lub Serenę, jeśli jest
dostępna — najpierw aktywuj w niej ten projekt). Zmiany nazw rób semantycznym
rename i sprawdź wynikowy diff.

## Kontrakt błędów

- Publiczne metody zwracają `rust7::Result<T>` z `AnyError`. Kategorie z modułu
  `kind` w src/error.rs to stabilny kontrakt: używaj istniejących stałych, a nową
  kategorię traktuj jako zmianę API (dopisz ją do `kind::ALL` i dokumentacji).
- Każda publiczna metoda dodaje jeden kontekst: nazwę metody i parametry, nigdy
  zawartość bufora.
- Błąd, po którym strumień może być rozsynchronizowany (transport, ramkowanie),
  musi rozłączyć klienta; błędy zgłoszone przez PLC zachowują połączenie. Opis
  każdej stałej w `kind` mówi, która reguła ją obowiązuje.
- Sygnatury publicznego API pilnuje tests/public_errors.rs. Zmiany zachowania
  widoczne dla użytkownika biblioteki opisz w CHANGELOG.md, w sekcji niewydanej wersji.

## Ograniczenia projektu

- Zachowaj implementację bez unsafe oraz walidację ramek i granic adresowania.
  Zmiany protokołu sprawdzaj wraz z testami w src/client/tests.rs,
  src/client/protocol/tests.rs i tests/tcp_smoke.rs.
- Zapis nie jest atomowy, także przy zapisie jednoczęściowym i `write_bit`:
  wcześniejsze części mogą dotrzeć do PLC przed błędem, a także część, przy której
  wystąpił błąd; potwierdzone bajty to `[0, confirmed)`. Nie dodawaj automatycznych ponowień zapisów bez rozstrzygnięcia
  ich semantyki. Po błędzie odczytu bufor nie jest kompletnym wynikiem.
- Nie uruchamiaj przykładów przeciw rzeczywistemu PLC bez wyraźnego zlecenia
  i wskazania urządzenia; do testów transportu używaj lokalnego mocka.
- examples/docker to osobny pakiet, poza głównymi komendami Cargo; zależy od
  opublikowanego rust7, więc jego uruchomienie nie weryfikuje lokalnych zmian biblioteki.
- Opis protokołu i API: [dokumentacja](doc/Documentation.md).
  Przykłady: [instrukcje przykładów](examples/README.md).
