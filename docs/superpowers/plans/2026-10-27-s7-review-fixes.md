# Rust7 Review Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task, or superpowers:subagent-driven-development if the user selects delegation. Steps use checkbox (`- [x]`) syntax for tracking. Execution method: sequential inline execution, authorized by the user on 2026-10-27.

**Goal:** Naprawić siedem błędów klienta S7 i zapewnić trwałe testy regresyjne.

**Architecture:** Zachować publiczny S7Client. Wydzielić prywatne ramkowanie,
walidację i operacje na transporcie Read + Write, tak aby testy bez sieci używały
tego samego kodu co produkcja. Nie przebudowywać pozostałej biblioteki.

**Tech Stack:** Rust edition 2021, std::io, std::net, Cargo, rustfmt, Clippy,
lokalny reviewdog. Bez nowych zależności runtime.

**Spec:** [Projekt napraw](../specs/2026-10-27-s7-review-fixes-design.md).
Wykonanie projektu i planu zatwierdzone przez użytkownika 2026-10-27.

## Global Constraints

- Zachować istniejące publiczne sygnatury metod i warianty S7Error.
- Biblioteka pozostaje w edition 2021 i z `#![forbid(unsafe_code)]`.
- Bez nowych zależności runtime i bez deklarowania nieuzgodnionego MSRV.
- Nie zmieniać znaczenia start: indeks bajtu dla BYTE, indeks bitu dla BIT.
- Do obliczeń adresów używać u32, do długości całych buforów i offsetów usize.
- Nie wykonywać automatycznych ponowień zapisu po błędzie transmisji.
- Testy jednostkowe mają działać bez sieci, PLC, Dockera i podmiany tekstu źródeł.
- Nie dodawać async, FFI, obsługi wieloelementowych żądań ani COTP fragmentation.

## Review Focus

- EOF/timeout po części ramki: błąd I/O bez sukcesu ani automatycznego retry (zadanie 1).
- Prawidłowa odpowiedź błędu PLC: zachować S7NotFound/S7InvalidAddress (zadanie 3).
- Odpowiedź innej operacji/referencji: odrzucić nawet z bajtem statusu 0xFF (zadanie 3).
- Ostatni dopuszczalny adres oraz zakres przekraczający go o jeden bajt: brak zawijania i częściowej wysyłki nieprawidłowego zakresu (zadanie 4).
- Błąd drugiego fragmentu zapisu: Err, brak kolejnych fragmentów i powtórzenia pierwszego (zadanie 4).

## Pliki i odpowiedzialności

- `src/client.rs`: publiczne API, połączenie, delegowanie operacji i statystyki.
- Nowy prywatny `src/client/protocol.rs`: ramkowanie, parsowanie, walidacja,
  budowanie żądań i transfer fragmentów; deklaracja `mod protocol` w client.rs.
- Nowy `src/client/tests.rs`: regresje API i połączenia, widoczność prywatnych
  helperów poprzez moduł potomny `#[cfg(test)] mod tests`.
- Nowy `src/client/protocol/tests.rs`: przypadki ramek i skryptowany transport
  Read + Write; rejestruje wysłane bajty i zwraca zadane fragmenty/błędy.
- Nowy `tests/tcp_smoke.rs`: osobno uruchamiany test loopback z imitacją PLC.
- `src/lib.rs`, `README.md`, `doc/Documentation.md`: zgodność eksportów,
  dokumentacji i formatowania; bez zmian publicznych eksportów.
- `examples/sdk/client.rs`, `examples/docker/main.rs`: kontrola zgodności wywołań;
  poprawki tylko jeśli niezbędne dla wskazanych kontroli.

## Kolejność i sposób pracy

Zadania wykonywać 1 → 2 → 3 → 4 → 5. Współdzielą parser i client.rs, dlatego
rekomendowana jest realizacja sekwencyjna w tej sesji. Każde zadanie kończy się
przeglądem diffu i osobnym lokalnym commitem, jeśli środowisko pozwala na zapis Git.
Na etapie wykonania sprawdzić instrukcje repozytorium, stan drzewa i potrzebę
izolowanego worktree. Nie usuwać istniejących nieśledzonych plików skilli.

### Task 1: Poprawne ramkowanie TCP — F4

**Files:** client.rs, client/protocol.rs, client/protocol/tests.rs.

**Interfaces:**
- `read_tpkt<R: Read>(reader: &mut R, max_total: usize) -> Result<Vec<u8>, S7Error>`;
  zwraca pełną ramkę razem z 4 bajtami TPKT.
- `data_payload(frame: &[u8]) -> Result<&[u8], S7Error>`; sprawdza COTP Data/EOT.
- `validate_connection_confirm(frame: &[u8]) -> Result<(), S7Error>`;
  sprawdza nagłówek, typ CC i granice parametrów COTP.

- [x] Dodać testy `read_tpkt_accepts_every_split`, `read_tpkt_leaves_next_frame`,
  `read_tpkt_rejects_invalid_lengths`, `read_tpkt_propagates_eof_and_timeout`.
  Fragmentować ramki 22, 27 i 26 bajtów w każdym punkcie oraz po jednym bajcie.
  Dla dwóch sklejonych ramek dwa odczyty mają zwrócić dokładnie dwie ramki.
  Długości 0..3, błędna wersja TPKT i przekroczenie max_total mają dać Err.
- [x] Uruchomić `cargo test --locked protocol::tests::read_tpkt`; potwierdzić
  czerwony wynik wskazujący nieobsłużony przypadek, a nie błąd środowiska.
- [x] Wprowadzić helpery z odczytem nagłówka i ograniczonej długości przez
  read_exact. Dla CC zastosować limit ramki 1028 bajtów odpowiadający
  reklamowanemu TPDU 1024; dla danych limit negotiated PDU + 7.
  Kontrolować LI COTP i zakres TLV, nie zakładać każdego CC o długości 22.
- [x] Zastąpić wszystkie cztery miejsca pojedynczego read w connect_tsap,
  read_area i write_area. Nie mylić krótkich odczytów TCP z fragmentacją COTP.
- [x] Ponownie uruchomić wskazane testy; oczekiwany PASS dla wszystkich podziałów,
  również Interrupted obsługiwany przez read_exact, oraz brak nadmiarowego odczytu.
- [x] Przejrzeć diff; commit niewykonany z powodu braku tożsamości Git: `fix: read complete bounded TPKT frames`.

### Task 2: Bezpieczna negocjacja — F6

**Files:** client.rs, client/protocol.rs, client/protocol/tests.rs, client/tests.rs.

**Interfaces:**
- `PduLimits { pdu_length: u16, read_payload: usize, write_payload: usize }`
  z widocznością ograniczoną do modułów klienta.
- `PduLimits::new(pdu_length: u16) -> Result<Self, S7Error>`.
- `negotiate<T: Read + Write>(io: &mut T) -> Result<PduLimits, S7Error>`;
  wysyła istniejące żądanie Setup Communication 480 i waliduje odpowiedź.

- [x] Dodać `pdu_limits_reject_invalid_sizes`: 0, 1, 18, 27, 28, 481, 65535
  dają PduNegotiationFailed; 29 → limity 11/1, 240 → 222/212, 480 → 462/452.
  Dodać `negotiation_rejects_bad_header_or_error`: błędna funkcja, referencja,
  długość, error class lub error code nie mogą ustawiać connected.
- [x] Uruchomić `cargo test --locked pdu_limits` i `cargo test --locked negotiation`;
  udokumentować czerwone wyniki na obecnym zachowaniu.
- [x] Sprawdzać zakres przed odejmowaniem. Negocjację prowadzić na lokalnym
  strumieniu; dopiero sukces zapisuje strumień, limity i connected. Limity
  wewnętrzne pozostają autorytatywne mimo modyfikowalnego pub pdu_length.
- [x] Dodać test `write_chunk_is_never_zero` dla minimalnego PDU i niepustego
  bufora. Uruchomić powyższe filtry także z `--release`; oczekiwany PASS.
- [x] Przejrzeć diff; commit niewykonany z powodu braku tożsamości Git: `fix: validate negotiated PDU before deriving limits`.

### Task 3: Ścisłe odpowiedzi S7 — F2 i F5

**Files:** client.rs, client/protocol.rs, client/protocol/tests.rs.

**Interfaces:**
- `validate_read_response(pdu: &[u8], reference: u16, wordlen: u8,
  expected_payload: usize) -> Result<&[u8], S7Error>`.
- `validate_write_response(pdu: &[u8], reference: u16) -> Result<(), S7Error>`.
- Prywatny licznik referencji w kliencie, wspólny dla kolejnych fragmentów;
  nadawanie przez `next_reference(counter: &mut u16) -> u16`, z jawnym wrapping_add.
  Request i response kodują/dekodują referencję w tym samym porządku bajtów.

- [x] Dodać `read_response_requires_exact_payload`: żądane 4 bajty i odpowiedź
  z 1 lub 5 bajtami → Err; dokładnie 4 → właściwy slice. Sprawdzać deklarację
  długości oraz faktyczny rozmiar. Osobno BYTE i BIT, w tym 1 bit w 1 bajcie.
- [x] Dodać `response_rejects_wrong_header`, mutujący kolejno S7 ID, ROSCTR,
  reference, function, item count, parameter length, data length, transport size
  i błędy nagłówka. Wyzerowany nagłówek z 0xFF na pozycji statusu → Err.
- [x] Dodać `response_preserves_plc_errors` z prawidłowymi odpowiedziami 0x05
  i 0x0A oraz nieznanym statusem. Oczekiwane S7InvalidAddress, S7NotFound,
  S7Unspecified; nie wymagać payloadu sukcesu od odpowiedzi błędu.
- [x] Uruchomić `cargo test --locked response_`; potwierdzić czerwone regresje.
- [x] Zaimplementować parser z kontrolą granic przed odczytem pól. Zidentyfikować
  i zweryfikować nagłówek ACK_DATA, parametry i dane; żadne kopiowanie do bufora
  użytkownika przed powodzeniem walidacji fragmentu. Usunąć uznawanie samego
  response[14] za wystarczający dowód sukcesu.
- [x] Uruchomić filtry w debug/release. Dodać test zawinięcia licznika referencji
  oraz odpowiedzi z referencją poprzedniego fragmentu — musi być odrzucona.
- [x] Przejrzeć diff; commit niewykonany z powodu braku tożsamości Git: `fix: validate S7 replies and exact read payloads`.

### Task 4: Adresy, pełne bufory i postęp — F1, F3, F7

**Files:** client.rs, client/protocol.rs, client/tests.rs, client/protocol/tests.rs.

**Interfaces:**
- Prywatne `read_area_impl(&mut self, area: u8, db_number: u16, start: u32,
  wordlen: u8, buffer: &mut [u8]) -> Result<(), S7Error>` i analogiczne
  `write_area_impl(..., buffer: &[u8]) -> Result<(), S7Error>`.
- `validate_transfer(start: u32, wordlen: u8, len: usize) -> Result<usize, S7Error>`
  zwraca efektywną liczbę elementów; dla BIT jeden, dla BYTE pełne len.
- `Transfer { area: u8, db_number: u16, start: u32, wordlen: u8 }`.
- `read_transfer<T: Read + Write>(io: &mut T, limits: &PduLimits,
  reference: &mut u16, transfer: Transfer, buffer: &mut [u8], chunks: &mut usize)
  -> Result<(), S7Error>`; `write_transfer` analogicznie z `buffer: &[u8]`.
  Helpery wykorzystują parsery i next_reference z zadania 3; nie powielać
  implementacji przyrostu licznika.

- [x] Dodać `bit_address_does_not_wrap`: bajty 8191, 8192, 65535, bity 0/7;
  dla 8192.0 adres w telegramie to [0x01,0x00,0x00], dla 65535.7
  [0x07,0xff,0xff]. bit_idx=8 → InvalidFunParameter. Sprawdzić read i write.
- [x] Dodać `large_transfer_is_complete`: długości 0, 1, limit-1, limit,
  limit+1, 65535, 65536, 100000 dla odczytu i zapisu, PDU 240 i 480.
  Weryfikować wszystkie bajty, adresy kolejnych fragmentów, liczbę chunks
  i dodatnią długość każdego wysłanego fragmentu. Wzorzec danych ma różnić
  się między fragmentami, aby wykryć powtarzanie lub pomijanie danych.
- [x] Dodać `empty_bit_buffer_rejected_before_io` i `byte_empty_is_noop`;
  skryptowany transport ma zero wysłanych żądań. BIT z większym buforem
  przetwarza wyłącznie pierwszy bajt.
- [x] Dodać `address_range_checked_before_io`: ostatni legalny bajt 0x1fffff
  plus len=1 przechodzi w prywatnym helperze, len=2 daje Err bez I/O;
  overflow obliczeń usize także daje Err. Przetestować granicę poprzez
  publiczny start=65535 i odpowiednio długi bufor.
- [x] Dodać `second_write_chunk_failure_stops_without_retry`: pierwszy ACK
  sukces, drugi błąd; dokładnie dwa żądania, Err i brak retransmisji.
- [x] Uruchomić `cargo test --locked`; potwierdzić regresje przed poprawką.
- [x] Poszerzyć adresy wewnętrzne do u32, offsety do usize; usunąć min(u16::MAX)
  z długości transferu. Publiczne metody delegują bez zmiany sygnatur.
  read_bit/write_bit obliczają start bez zawężania do u16. Zweryfikować cały
  zakres przed pierwszym zapisem; walidować pusty BIT przed jakimkolwiek I/O.
- [x] Wpiąć helpery transferu w produkcyjny S7Client i w skryptowane testy.
  Offset zwiększać wyłącznie o poprawnie potwierdzony, dodatni fragment.
  Zachować last_time=0 przy Err; testy nie mogą wymagać atomowości całego zapisu.
- [x] Uruchomić `cargo test --locked --lib` oraz `cargo test --locked --release --lib`;
  wszystkie regresje F1–F7 muszą przejść, niezależnie od overflow-checks.
- [x] Przejrzeć diff; commit niewykonany z powodu braku tożsamości Git: `fix: preserve full transfer lengths and bit addresses`.

### Task 5: Kontrole projektu, dokumentacja i integracja

**Files:** src/client.rs, src/lib.rs, README.md, doc/Documentation.md,
tests/tcp_smoke.rs; pozostałe pliki tylko dla niezbędnych poprawek kontroli.

**Interfaces:** Publiczne eksporty pozostają zgodne; nowy test loopback używa
publicznego S7Client i TcpListener na 127.0.0.1:0. Bez zewnętrznych usług.

- [x] Poprawić 4 akapity rustdoc interpretowane jako kod; zachować przykład README
  jako kompilowalny doctest. Naprawić 6 istniejących diagnostyk Clippy minimalnie
  (m.in. Default delegujący do new), następnie wykonać cargo fmt --all.
- [x] Opisać pełne transfery, kontrolę zakresu adresów, puste bufory, walidację
  odpowiedzi i możliwość częściowego wykonania zapisu przed Err. Uzgodnić
  deklaracje chunks z rzeczywistym zachowaniem także przy błędach.
- [x] Dodać `tcp_smoke` jako jawnie uruchamiany test #[ignore]: negocjacja,
  wielofragmentowy odczyt/zapis, dzielenie odpowiedzi między write po stronie
  symulatora i zamknięcie połączenia. Użyć skończonych timeoutów, zakończyć
  wątek serwera również przy błędzie klienta. Brak gniazd to błąd uruchomienia,
  nie ciche pominięcie wewnątrz testu.
- [x] Uruchomić z katalogu repozytorium:
  `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`,
  `cargo test --workspace --all-features --locked`,
  `cargo test --workspace --all-features --locked --release`.
  Oczekiwane PASS, niezerowa liczba regresji i wszystkie doctesty poprawne.
- [x] Zachować surowy, niekolorowany output Clippy i jego exit code; wyświetlić
  przez reviewdog z parserem clippy, reporter=local, filter-mode=nofilter.
  Nie uznawać pustego raportu reviewdog za zastępstwo kontroli Clippy.
- [x] Uruchomić `cargo test --locked --test tcp_smoke -- --ignored` tam, gdzie
  dozwolone są lokalne gniazda. Jeśli środowisko nadal blokuje bind, oznaczyć
  integrację NOT RUN i pozostawić ją jako warunek końcowego odbioru integracji.
- [x] Skompilować oba przykłady przeciw lokalnej bibliotece; domyślne cargo test
  nie obejmuje zagnieżdżonego projektu Docker. Nie uznawać kompilacji zależności
  rust7 z crates.io za dowód poprawności lokalnych zmian.
- [x] Sprawdzić `git diff --check`, pełny diff oraz powiązanie F1–F7 z testami.
  Bez deny.toml raportować cargo-deny NOT RUN; Miri/Loom N/A przy zachowaniu
  bezpiecznego, synchronicznego kodu. Nie deklarować niebadanych platform.
- [x] Przejrzeć diff; commit niewykonany z powodu braku tożsamości Git: `test: verify S7 fixes and restore project checks`.

## Kryteria zakończenia

Wszystkie siedem znalezisk ma test przechodzący w debug/release, publiczne API
pozostaje zgodne, odczyt i zapis pełnego bufora 65536 bajtów działają, a błędne
odpowiedzi nie zwracają sukcesu. Fmt, Clippy i doctesty przechodzą. Test TCP ma
udokumentowany wynik; jego niewykonanie pozostaje ograniczeniem raportu.
Brak niezamówionego deploymentu, publikacji i kontaktu z rzeczywistym PLC.

## Self-review planu

- Pokrycie: F4 → 1; F6 → 2; F2/F5 → 3; F1/F3/F7 → 4; kontrole → 5.
- Pięć warunków Review Focus ma wskazane testy w zadaniach 1, 3 i 4.
- Typy: u16 pozostaje na publicznym wejściu, u32 dla adresów wewnętrznych,
  usize dla długości i offsetów; pola długości telegramu zawężane po walidacji.
- Próby review z /tmp są materiałem pomocniczym; stałe testy muszą wywoływać
  produkcyjne helpery, bez kopiowania źródeł i podmiany ich importów.
- Kolejność wynika z zależności parsera, negocjacji i transferu; nie planować
  równoległych edycji client.rs.


## Wynik wykonania — 2026-10-27

Wszystkie pięć zadań wykonano sekwencyjnie na gałęzi `fix/s7-review-findings`.
22 testy jednostkowe i 1 doctest przechodzą w debug i release. Fmt, Clippy
wszystkich targetów/features z `-D warnings`, lokalny reviewdog oraz kompilacja
obu przykładów przeciwko lokalnej bibliotece przeszły. Wszystkie 15 istniejących
publicznych sygnatur metod pozostaje zgodnych; dodano implementację Default.

Dwa testy integracyjne TCP są skompilowane i domyślnie ignorowane. Próba jawnego
uruchomienia zatrzymuje się na bind z PermissionDenied/EPERM; odbiór integracji
pozostaje NOT RUN. Ich oznaczenie na liście kroków oznacza dodanie i wykonanie
próby, nie zaliczenie testów. Cargo-deny: NOT RUN z powodu braku deny.toml.

Niezależny reviewer sprawdził wszystkie 11 plików poprawki i nie zgłosił uwag
Critical, Important ani Minor. Samodzielnie potwierdził testy debug oraz
Clippy przez reviewdog. Zakres review obejmował rzeczywisty diff względem
commita bazowego, łącznie z nowymi plikami, ponieważ Git nie ma skonfigurowanej
tożsamości autora i nie można było utworzyć commitów.

Decyzje wykonawcze: praca na osobnej gałęzi w bieżącym checkout; znane błędy
bazowe doctestów/lintów usunięte w zadaniu 5; brak zmian konfiguracji tożsamości
Git; przegląd snapshotu roboczego zamiast pustego HEAD..HEAD. Zmiany i dowody
pozostają lokalnie, bez merge/push/publikacji. Pełny dziennik i logi:
`.superpowers/sdd/2026-10-27-s7-review-fixes/`.

Poza zakresem pozostały wcześniejsze skrajne wartości rack/slot mogące
przepełnić TSAP oraz semantyka surowego BIT z bajtem innym niż 0/1. Publiczny
write_bit przekazuje bool jako 0/1. Fragmentacja COTP i żądania wieloelementowe
pozostają jawnymi ograniczeniami projektu. Brak odłożonych uwag Minor.
