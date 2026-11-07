# Rust7: przegląd 4 (R4) — raport zweryfikowany

Data: 2026-11-07. Tryb: tylko odczyt (kod nie był naprawiany). Jedyny plik zapisany w repo: ten raport.
Dowody (testy, skrypty, logi) leżą poza repo, w scratchpadzie sesji
`/tmp/claude-1000/-home-rtm-Dokumenty-ai-works-snap-7-evo/09f74ec3-46d6-4b03-86f4-2837975034c3/scratchpad/`
(dalej `$S`), w osobnych git worktree tego samego HEAD.

## 1. Zakres i stan

| Pozycja | Wartość |
|---|---|
| Crate / HEAD | `rust7` 0.2.0 (Unreleased), `5c09959` (gałąź `test/tcp-client-mutants`, commit 2026-11-07 10:49 +0100) |
| `git status` | czysto poza nieśledzonym `docs/superpowers/plans/2026-11-04-s7-review-2-fixes.md` |
| Toolchain | rustc 1.99.1 (58cf8f9c3 2026-10-13), cargo 1.99.1; nightly (miri, llvm-cov `--branch`) |
| Kod produkcyjny | `src/lib.rs` 11, `src/error.rs` 212, `src/client.rs` 853, `src/client/protocol.rs` 446 linii |
| Graf code-review-graph | przebudowany przyrostowo, `head_matches_build: true`; huby/mosty to wyłącznie helpery testów; największe funkcje produkcyjne: `connect_tsap_impl` (72 l.), `request` (68 l.) |
| Worktree dowodowe | `$S/wt-evidence` (A+C), `$S/wt-b` (B), `$S/wt-df` (D+F i G — współdzielony), `$S/wt-mutants` (E, mutacje), `$S/wt-v1..v3` (weryfikatorzy) |
| Orkiestracja | workflow `wf_69a2c63a-024`: 5 agentów aspektowych + 3 niezależnych weryfikatorów = 8 agentów, 0 błędów |

Narzędzia niedostępne / ograniczenia:
- `cargo-fuzz` — niezainstalowany; użytkownik wybrał proptest. Krok libFuzzer: **niewykonany**.
- Target `i686-unknown-linux-gnu` — brak std; arytmetyka 32-bit sprawdzona ręcznie (wszystkie granice ≤ 0x200000 / u16).
- Prawdziwy PLC — nie użyto (zgodnie z AGENTS.md); transport tylko przez lokalny mock i skryptowany Read+Write.
- Sieć: użyta za zgodą użytkownika wyłącznie przez `cargo audit`/`cargo deny` (baza advisory) i `cargo semver-checks` (baseline z crates.io).
- Treść R3-08…R3-12 i R3-23…R3-25 nie jest zapisana w repo — nie dało się wykluczyć nakładania się z nimi; znaleziska zgłoszone niżej mają nowe dowody (mutanty/testy na HEAD 5c09959).

## 2. Bramki automatyczne

| Komenda | Wynik | Uwagi |
|---|---|---|
| `cargo fmt --all -- --check` | exit 0 | — |
| `cargo check --all-targets --offline` | exit 0 | — |
| `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --offline` | exit 0 | `-W missing_docs` (informacyjnie, agent G) zgłasza brak docs dla 9 stałych, `S7Client`, modułu `client` |
| `cargo clippy --all-targets -- -D warnings` → reviewdog `-f=clippy -reporter=local -filter-mode=nofilter -fail-level=warning` | exit 0 | 0 diagnostyk |
| clippy `pedantic` + `unwrap_used,expect_used,indexing_slicing,arithmetic_side_effects,panic` (`--lib`, informacyjnie) | exit 0, **108 ostrzeżeń** | 35 indexing, 25 arithmetic, 20 slicing, 9 doc_markdown, 4 missing_errors_doc, 7 cast truncation, 3 needless_pass_by_value, 3 unreadable_literal, 1 unused_self, 1 must_use. Każde indeksowanie/arytmetyka sprawdzone przez agenta A+C: poprzedzone kontrolą długości/zakresu (patrz §4) |
| `cargo test --offline` | exit 0 | 93 unit + 11 public_errors + 6 doctest pass; 18 tcp_smoke ignored |
| `cargo test --test tcp_smoke -- --ignored` | exit 0 | 18/18 pass (loopback) |
| `cargo llvm-cov --all-targets -- --include-ignored` (stable) | exit 0 | linie: client.rs 96.88%, protocol.rs 90.80%, error.rs 91.76%; gałęzie niedostępne na stable (0/…) |
| `cargo +nightly llvm-cov --branch -- --include-ignored` | exit 0 | client.rs: linie 96.31%, gałęzie 20/20; protocol.rs: linie 99.43%, gałęzie 136/140 (97.14%); niepokryte linie: client.rs 342, 659; protocol.rs 107, 403 |
| `cargo audit` | exit 0 | 0 podatności (1422 advisories, 29 zależności) |
| `cargo deny check advisories bans licenses` (bez `deny.toml`) | **exit 4** | advisories ok, bans ok, **licenses FAILED** — brak polityki licencji w repo (pusta lista allow odrzuca wszystko; znane, świadome ograniczenie z przeglądu 1) |
| `cargo deny --config $S/gates/deny.toml …` (tymczasowa lista allow MIT/Apache-2.0/Unicode-3.0/BSD/ISC/Zlib, informacyjnie) | exit 0 | advisories ok, bans ok, licenses ok — **nie jest** to polityka projektu |
| `cargo machete` | exit 0 | brak nieużywanych zależności |
| `typos` | **exit 2** | `endianess`, `ciclically` (doc/Documentation.md:4, 47), `Distribuited` (README.md:133) — realne; `pn` ×5 (tests/tcp_smoke.rs) — fałszywy alarm (nazwa zmiennej). Zob. R4-20 |
| `cargo +nightly miri test --offline --lib` | exit 0 | 93/93 pass, 2417 s, brak UB (informacyjnie; crate ma `forbid(unsafe_code)`, tcp_smoke pominięte — gniazda) |
| `cargo semver-checks` (baseline: rust7 0.1.2 z crates.io) | exit 0 | „major change”, 0 checks / 260 skip — podbicie major dopuszcza łamanie API; ocena ręczna CHANGELOG w R4-05 |
| `cargo mutants -f src/client/protocol.rs -f src/client.rs -- -- --include-ignored` (worktree, 27.1.0) | exit 2 (są przeżywające) | 288 mutantów, 61 s: **283 caught, 4 missed, 1 unviable, 0 timeout**. Historyczny `mutants.out/` (10:06) był starszy niż HEAD (10:49; 37 missed) i nie był użyty |
| `ocr delegate preview` (skill delegate-review) | exit 0 | tryb workspace: 1 plik (`.md`, wykluczony `unsupported_ext`), 0 do przeglądu — przegląd całego crate'a wykonano workflow; automatyczne poprawki skilla wyłączone poleceniem „Nie naprawiaj kodu” |

Klasyfikacja przeżywających mutantów (agent E, potwierdzone przez A+C i D+F):

| Mutant | Klasa | Uzasadnienie |
|---|---|---|
| `client.rs:851` `Drop::drop` → `()` | równoważny | upuszczenie `Option<TcpStream>` zamyka deskryptor; brak `try_clone`, więc `shutdown(Both)` nie zmienia nic obserwowalnego |
| `protocol.rs:282:25`, `:282:39` `\|` → `^` w `remote_tsap` | równoważny | po walidacji rack ≤ 7, slot ≤ 31 pola bitowe (8–15, 5–7, 0–4) są rozłączne, więc OR ≡ XOR |
| `protocol.rs:288:14` strażnik `start <= 0x1fffff` → `true` | równoważny | `last = start + len-1 ≥ start`, więc kontrola `last > 0x1fffff` (`:292`) odrzuca to samo; strażnik redundantny |
| `protocol.rs:43:5` `negotiate` → `Ok(Default::default())` | unviable | `PduLimits` nie implementuje `Default` |

Żaden przeżywający mutant cargo-mutants nie jest luką ani ukrytym błędem. Luki wykazały dopiero **ręczne mutacje**, których cargo-mutants nie generuje (31 mutacji, `$S/manual-mutants/`, R4-01…R4-04).

## 3. Znaleziska (od najwyższej wagi)

Brak P0 i P1. Harness wrogich danych (A+C, 5 własności, `PROPTEST_CASES=200000`, 41 s, wszystkie pass) nie znalazł paniki, zawijania, niekontrolowanej alokacji, zawieszenia ani błędnego żądania zapisu w warstwie protokołu. Wszystkie P2 to luki w testach chroniących właściwości klasy P0/P1 w **publicznej warstwie `S7Client`** oraz niekompletny CHANGELOG.

### P2

| ID | Waga | Miejsce | Opis | Scenariusz awarii | Dowód | Werdykt | Rekomendacja |
|---|---|---|---|---|---|---|---|
| R4-01 | P2 | `src/client.rs:775` (też `:603`, `:838-839`) | Żaden test nie sprawdza, że publiczne `write_db`/`write_area`/`write_bit` przekazują do żądania właściwy `db_number`, `area` i indeks bitu. Mock tcp_smoke nie asertuje `request[25..28]`, zawsze używa DB100/start 0; `write_bit` wołany tylko dla (DB,100,65535,7), gdzie `\| 7` jest równoważne; `write_area` nie ma udanego wywołania | Regresja: `write_db(5,0,d)` wysyła DB1, `write_area(MK,…)` trafia w DB, `write_bit(MK,0,10,0,true)` ustawia M10.7 — zapis pod zły adres z `Ok` i `confirmed == len`; cały zestaw (z `--ignored`) zielony | Ręczne mutacje M17, M20–M24 przeżyły (`$S/manual-mutants/`); niezależnie V1–V4 przeżyły w `$S/wt-v2` (`python3 -I $S/verify-v2/mut.py`, 8× „test result: ok”, 93+11+18+6) | CONFIRMED | W mocku tcp_smoke rejestrować i asertować DB, obszar i adres każdego żądania; scenariusze publiczne z niedomyślnymi wartościami: `write_db(db≠100, start≠0)`, `write_area` dla PE/PA/MK/DB (DB=0 poza DB), `write_bit`/`read_bit` dla bitów 0 i 3 |
| R4-02 | P2 | `src/client.rs:839`, `:752` | Brak testu `write_bit(…, false)` z kontrolą payloadu oraz `read_bit` zwracającego `false` po udanym odczycie. Mock zawsze odpowiada bitem 1 (`tests/tcp_smoke.rs:282`) i asertuje payload `[1]` (`:283-285`) | Regresja wysyła 1 zamiast 0: polecenie wyłączenia wyjścia/zerowania flagi ustawia je w PLC, wynik `Ok` (błędne dane + fałszywy sukces). Albo `read_bit` zawsze `true` | M19 (`&[u8::from(value)]` → `&[1]`) i M23 (`buffer[0] != 0` → `buffer[0] == buffer[0]`) przeżyły u agenta E i weryfikatora (`$S/v3-proofs/M19.log`, `M23.log`). **Powtórzone przez koordynatora** w `$S/wt-v1`: `cargo test --offline -q -- --include-ignored` → 93/11/18/6 pass, rc=0 | CONFIRMED | Mock odpowiada 0 dla jednego adresu i 1 dla innego; testy `write_bit(false)` → payload `[0]` i `write_bit(true)` → `[1]`; asercja obu wyników `read_bit` |
| R4-03 | P2 | `src/client/protocol.rs:318-332`, `:361` | Stałe bajty nagłówka żądania S7 nie są przypięte: protocol ID 0x32 `[7]`, ROSCTR=1 `[8]`, długość parametrów 14 `[13..15]`, liczba elementów 1 `[18]`, var-spec 0x12/0x0a/0x10 `[19..22]`, transport size danych 4 dla zapisu bajtowego `[32]` | Refaktor zmienia transport size zapisu bajtowego 4 → 9: PLC interpretuje pole długości w bajtach zamiast bitów, czyli inny zakres zapisu; albo item count 2 / ROSCTR 7. Testy zielone | 8/8 mutacji przeżyło: `$S/wt-v1/proof-v1/mutate_header.sh` (128 testów z `--include-ignored`, `SURVIVED: 361s/{ 3 } else { 4 }/{ 3 } else { 9 }/` itd.); u agenta E M01, M03–M09 | CONFIRMED | Test „golden bytes”: pełne `assert_eq!` całych żądań (odczyt/zapis × bajt/bit) względem referencji (Snap7/Wireshark) albo asercja całego `packet[..35]` w `transfers_split_into_consecutive_bounded_chunks`; mock tcp_smoke sprawdza stałe nagłówka |
| R4-04 | P2 | `src/client.rs:660` | Rozłączenie po `ISO_INVALID_HEADER` i `ISO_FRAGMENTED_PACKET` w transferze chroni tylko test predykatu `breaks_session` (`src/client/tests.rs:92`). `finish_preserves_session_only_for_valid_error_ack` używa wyłącznie `ISO_INVALID_TELEGRAM` i `S7_UNSPECIFIED`; tcp_smoke pokrywa IO, CONNECTION_CLOSED, ISO_INVALID_TELEGRAM | Zmiana w `finish` (np. osobna ścieżka dla kategorii) zostawia klienta połączonego po złym nagłówku TPKT (zużyte 4 B, reszta w gnieździe): następny `read_db` parsuje resztki poprzedniej ramki (desynchronizacja, P1) | M13/M14 (`breaks_session(&e) && kind != FRAG/HDR`) przeżyły; niezależnie V5/V6 przeżyły (`$S/verify-v2/V5_finish_keep_FRAG.log`, `V6_finish_keep_HDR.log`). Kontrola: M15 (CONNECTION_CLOSED) i M16 (IO) wykryte | CONFIRMED | Test parametryzowany po `kind::ALL` przez `client.finish` z asercją `connected`/`limits`/`pdu_length` wg reguły kategorii; scenariusze tcp_smoke (zła wersja TPKT, DT bez EOT) dla odczytu i zapisu z `!connected` i `NOT_CONNECTED` w kolejnym wywołaniu |
| R4-05 | P2 | `CHANGELOG.md:23` (sekcje `### Fixed`/`### Changed` 0.2.0) | CHANGELOG nie wymienia widocznych dla użytkownika napraw F1–F7 względem 0.1.2: w 0.1.2 `byte_num*8` liczone w u16 (zawija w release → zapis innego bitu dla byte_num ≥ 8192), bufor > 65535 B obcinany z `Ok`, krótka odpowiedź kopiowana częściowo z `Ok`; ścisła walidacja odpowiedzi teraz kończy się `ISO_INVALID_TELEGRAM` + rozłączeniem; `disconnect()` zeruje `pdu_length` | Integrator migrujący 0.1.2 → 0.2.0 wg CHANGELOG nie dowiaduje się, że wcześniejsze zapisy mogły trafić pod inny adres, że ten sam `write_db` z 70000 B zapisze teraz więcej danych, ani że tolerowane dotąd urządzenie może być rozłączane | `$S/wt-v1/proof-v1/changelog_gap.sh`: kod 0.1.2 (`~/.cargo/registry/src/*/rust7-0.1.2/src/client.rs:573,652,740,900,964`), commit F1–F7 `c739ed4` nie dotknął CHANGELOG, grep CHANGELOG pod kątem 65535/8192/byte_num/truncat/partial → brak | CONFIRMED | Dopisać w `### Fixed` F1, F2/F5, F3, F6, F7 (z jawnym ostrzeżeniem, że 0.1.2 w release mógł pisać pod inny bit), w `### Changed` transfer > 65535 B i ścisłą walidację z rozłączeniem; w Migration — zalecenie przeglądu zapisów bitowych ≥ 8192 wykonanych na 0.1.2 |

### P3

| ID | Waga | Miejsce | Opis | Scenariusz awarii | Dowód | Werdykt | Rekomendacja |
|---|---|---|---|---|---|---|---|
| R4-06 | P3 | `src/client.rs:174` (też `:145`, `:212`; doc `:142`, `:172`, `:210`; Documentation.md:206/230/250) | `set_timeout`/`set_connection_type`/`set_connection_port` zwracają `Ok` na połączonym kliencie, mimo że dokumentacja mówi „must not be connected”. `set_timeout` działa częściowo: `rd_timeout_ms` od razu (budowany przy każdym transferze), `wr_timeout_ms` dopiero po ponownym połączeniu | `set_timeout(3000, 50, 7)` w trakcie sesji → `Ok`; odczyty kończą się po 50 ms, ale `write_all` dalej ma limit 500 ms. Limit pozostaje skończony | Testy `v1_set_timeout_while_connected_applies_only_read_timeout` (`$S/wt-v1/src/client/tests.rs`), `v3_setters_accepted_while_connected` (`$S/wt-v3/tests/proof_v3.rs`), `df_set_timeout_while_connected_is_partially_applied` (`$S/wt-df`) | CONFIRMED (3 niezależne zgłoszenia scalone) | Albo egzekwować warunek (`INVALID_FUN_PARAMETER`, gdy `stream.is_some()`, + CHANGELOG), albo opisać, co działa od razu, i ewentualnie stosować `set_write_timeout` na aktywnym gnieździe |
| R4-07 | P3 | `src/client.rs:338-343` | `to_socket_addrs()` (blokujący getaddrinfo) nie jest objęty `co_timeout_ms`; próbowany jest tylko pierwszy adres (`.next()`) | Nazwa hosta zamiast literału IPv4: wolny DNS blokuje connect na czas resolvera; `localhost` → `[::1]` pierwszy, a PLC/mock nasłuchuje tylko na IPv4 → `Connection refused` mimo działającego adresu | `$S/wt-v3/tests/proof_v3.rs::v3_localhost_only_first_address_tried`: `resolved: [[::1]:…, 127.0.0.1:…]`, `server accepted IPv4 connection: false`. Część DNS — tylko ścieżka kodu (wymaga sieci) | CONFIRMED (kolejność adresów); timeout DNS — ścieżka kodu | Udokumentować, że obsługiwane są literały IPv4 i że `co_timeout_ms` nie obejmuje rozwiązywania nazw; albo parsować `Ipv4Addr`/`SocketAddr` i odrzucać inne wartości, albo próbować wszystkich adresów w jednym terminie |
| R4-08 | P3 | `src/client/protocol.rs:53-56` | Odrzucenie Setup Communication przez PLC poprawnym nagłówkiem błędu S7 (ROSCTR 2/3, klasa/kod ≠ 0) daje ogólne `PDU_NEGOTIATION_FAILED` bez klasy, kodu i bez `source()`; ścieżki odczytu/zapisu zachowują kody od R3-06 | CPU odmawia z klasą 0x81 kod 0x04 → „S7 PDU negotiation failed”, `source() == None`; operator nie odróżni odmowy PLC od zniekształconej odpowiedzi | `v1_negotiation_plc_error_header_loses_class_and_code` (`$S/wt-v1`) i `proof_b_negotiation_…` (`$S/wt-b`): `source_is_none=true`, brak 0x81/0x04 w tekście | CONFIRMED | Przy spójnym nagłówku S7 z błędem zwracać `pdu_negotiation_failed_by(s7_header_error(class, code))`; kategoria bez zmian |
| R4-09 | P3 | `src/client/protocol.rs:148-170` | COTP Disconnect Request (DR, z kodem przyczyny) lub ER w odpowiedzi na CR daje ten sam komunikat `ISO_CONNECTION_FAILED` co uszkodzone CC | `connect_rack_slot(ip, 0, 3)` do CPU w slocie 2 → DR, reason 0x81 → komunikat identyczny jak dla CC ze złym dst-ref; nic nie wskazuje na rack/slot/TSAP | `$S/wt-v2/tests/verify_v2.rs::v2_dr_vs_bad_cc` (mock loopback): oba komunikaty identyczne bajt w bajt | CONFIRMED | Zachować kategorię, dopisać typ TPDU (CC/DR/ER) i przyczynę DR w komunikacie; test z DR |
| R4-10 | P3 | `tests/public_errors.rs:30-33`, `src/client.rs:50` | `S7Client` nie implementuje `Debug`; `Send`/`Sync`/`Unpin` wynikają tylko z auto-traitów i nie pilnuje ich strażnik API | Przyszłe pole `Rc`/`Cell` cicho usuwa `Send` (zmiana łamiąca), testy zielone; `#[derive(Debug)] struct Plc { c: S7Client }` → E0277 | `$S/wt-v2/tests/verify_v2.rs::v2_s7client_auto_traits_today` ok; tymczasowy test z derive → `error[E0277]` | CONFIRMED | `bound::<S7Client>()` (Send+Sync+Unpin) w public_errors.rs; ręczne `impl Debug` pokazujące tylko konfigurację i stan, nigdy bufory |
| R4-11 | P3 | `src/client/protocol/tests.rs:1355` (`coherent_ack`, `short_item` `:1307`) | Generator proptest prawie nigdy nie trafia w gałąź `Ok` (≈3·10⁻⁴ na przypadek), więc wyrocznia `buffer == ogon pdu` prawie się nie wykonuje; `short_item` zawsze daje status 0xff (brak 5/10/innych kodów zapisu) | Regresja w ścieżce sukcesu wykrywana przez tę własność „przypadkiem”; ścieżkę chronią inne testy, więc ryzyko niskie | Sonda `zz_v3_probe_ok_rates` (`$S/v3-proofs/probe.rs`): 25 600 próbek → read_ok=4, transfer_ok=7, write_ok=266; brak S7_INVALID_ADDRESS/S7_NOT_FOUND | CONFIRMED | Rozdzielić na własność „poprawne z konstrukcji” (Ok + wyrocznia) i „dokładnie jedno pole zepsute” (oczekiwany kind); losować status z {0xff, 5, 10, inne} |
| R4-12 | P3 | `src/client/protocol.rs:370-375`, `:345` vs `src/client.rs:108-114` | Walidacja obszaru/zakresu w dwóch warstwach (`check_area` ze stałymi, `valid_area` z literałami; `validate_transfer` dwukrotnie); literał `0x84` decyduje o wysłaniu `db_number` | Dodanie nowego obszaru wymaga zmian w 3–4 miejscach; przeoczenie warunku `0x84` w `request()` dla obszaru „jak DB” cicho wyzeruje db_number. Dziś fail-safe | Ścieżka: client.rs:505 → :506 → protocol.rs:393 → :394 → :345; clippy pedantic `unused_self` (client.rs:108) | PLAUSIBLE (hipotetyczny przyszły błąd utrzymania) | Jedno źródło prawdy: protocol.rs używa `S7_AREA_*`/`S7_WL_*` lub prywatnego typu `Area`/`WordLen` sparsowanego na granicy klienta |
| R4-13 | P3 | `src/client.rs:66` (doc) vs `:281-284` | Doc `last_time`: „If an error occurred the value will be 0”, ale `connect_rack_slot` z rack/slot poza zakresem zwraca `INVALID_FUN_PARAMETER` przed `connect_tsap_impl` (gdzie jest zerowanie) | Po udanym `read_db` (`last_time = 1.2`) `connect_rack_slot(ip, 8, 0)` → błąd, `last_time` nadal 1.2 | `$S/wt-v2/tests/verify_v2.rs::v2_last_time_not_reset_by_rack_slot_param_error` | CONFIRMED | Zawęzić opis albo zerować `last_time` na początku `connect_rack_slot_impl` |
| R4-14 | P3 | `src/client.rs:440` (też Documentation.md:360) | Nota bitowa `read_area` skopiowana z `write_area` opisuje bufor jako wejście („considered true if != 0”); przy odczycie `buffer[0]` dostaje surowy bajt z PLC | Użytkownik oczekuje normalizacji do 0/1; dostaje np. 5 | `$S/wt-v3/tests/proof_v3.rs::v3_read_area_bit_raw_byte`: `buf=[05, bb, cc]` | CONFIRMED | „Only the first byte receives the bit value returned by the PLC; other bytes are not modified.” |
| R4-15 | P3 | `src/client.rs:705`, `:795` | Przykłady `read_bit`/`write_bit` to inline code bez odbiorcy i z ukrytym `Result` (nie są doctestami) | Skopiowany przykład się nie kompiluje albo daje `Result` zamiast `bool` | `cargo test --doc -- --list`: 6 doctestów, żaden z client.rs | CONFIRMED | Prawdziwe doctesty `no_run` z `client.read_bit(…)?` |
| R4-16 | P3 | `src/client.rs:449`, `:564` (Documentation.md:369, 426) | `ISO_INVALID_TELEGRAM` opisany jako „Inconsistent expected telegram length”, a zwracany też przy złej referencji, ROSCTR, funkcji, item count, transport size, pustym ACK | Debugowanie rozłączenia po złej referencji: szukanie błędu długości | `finish_preserves_session_only_for_valid_error_ack` asertuje ten kind dla błędu niebędącego długością | CONFIRMED | „Malformed or mismatched response (length, reference, function, item count, transport size, empty/unexpected ACK)” |
| R4-17 | P3 | `src/error.rs:5-6` (też examples/README.md:8, README:116-117, CHANGELOG:10) | Opisy `{e:#}` i kontekstu się różnią: rustdoc mówi, że kategoria i wiadomość są w osobnych liniach (w anyerror 0.1.13 `typ: msg` są w pierwszej linii); kontekst `write_bit` celowo pomija `value`, czego dokumentacja nie mówi | Użytkownik zgłasza brak `value` jako naruszenie kontraktu | `$S/wt-v3/tests/proof_v3.rs::v3_display_format`; anyerror sprawdzony przez lockdoc | CONFIRMED | Ujednolicić wg Documentation.md:129; dopisać, że wartość bitu (jak bufor) nie trafia do kontekstu |
| R4-18 | P3 | `src/client.rs:18-30`, `:50`, `src/lib.rs:4` | Publiczne stałe `CT_*`, `S7_AREA_*`, `S7_WL_*`, `S7Client` i moduł `client` bez rustdoc; znaczenie tylko w komentarzach `//` (plus „AS S7 Basic”) | Na docs.rs `S7_AREA_MK`, `CT_OP` bez opisu | `cargo rustdoc --lib -- -W missing_docs`: 9 stałych, struct, moduł | CONFIRMED | Zamienić komentarze końcowe na `///`; krótki rustdoc `S7Client` |
| R4-19 | P3 | `README.md:123`, `:126`, `:133` (via `src/lib.rs:2` `include_str!`) | Względne linki w README (doc/Documentation.md, examples/README.md, LICENSE) są martwe w rustdoc/docs.rs | Czytelnik docs.rs dostaje 404 do pełnej dokumentacji | `cargo doc` w `$S/wt-v2`: `href="doc/Documentation.md"`, plików brak w `target/doc/rust7/` | CONFIRMED | Absolutne URL repozytorium |
| R4-20 | P3 | `doc/Documentation.md:4`, `:8`, `:47`; `README.md:133`; `src/client.rs:361-362`; `examples/README.md:3`; `examples/sdk/client.rs:30` | Literówki w dokumentacji użytkownika: endianess, ciclically, Distribuited (typos), protocovl, lenght ×2, „an PLC SDK”, „Hexc Offset” | Niższa jakość publikowanej dokumentacji | `typos` (exit 2) + grep | CONFIRMED | Jeden commit `docs:` |
| R4-21 | P3 | `src/client.rs:404` | Doc `disconnect()`: „calls to read/write will return NOT_CONNECTED”, ale walidacja parametrów (`check_area`, `validate_transfer`, `bit_start`) jest przed sprawdzeniem połączenia | Kod rozpoznający „trzeba się połączyć” po `NOT_CONNECTED` dostaje `INVALID_FUN_PARAMETER` dla złych argumentów | `v1_disconnected_client_reports_param_error_before_not_connected` (`$S/wt-v1`) | CONFIRMED | „…with valid parameters return NOT_CONNECTED” lub odesłanie do `connected` (jak README) |
| R4-22 | P3 | `examples/sdk/README.md:1`; `examples/sdk/client.rs:18-22` | README przykładu SDK to notatka historyczna („# Previous documentation”); komentarze w przykładzie powtarzają `println!` | Czytelnik uznaje stronę za przestarzałą i pomija ostrzeżenie o prawdziwym PLC | `cat examples/sdk/README.md` | CONFIRMED | Tytuł „# SDK example”, tylko komenda budowania i ostrzeżenie |
| R4-23 | P3 | `Cargo.toml` (brak `include`/`exclude`) | Paczka crates.io zawiera wewnętrzne plany i specyfikacje przeglądów (`docs/superpowers/**`, 7 plików) | Publikacja 0.2.0 wysyła wewnętrzne notatki procesu przeglądu do rejestru (trwale) | `cargo package --list --offline --allow-dirty \| grep -c '^docs/superpowers'` → 7 (sprawdzone przez koordynatora; zgłoszone jako uwaga uboczna przez agenta G) | CONFIRMED | `exclude = ["docs/", "mutants.out/", …]` lub `include` w `[package]`; sprawdzić `cargo package --list` przed publikacją |

## 4. Odrzucone hipotezy

Odrzucone przez weryfikatorów:

| Hipoteza | Powód |
|---|---|
| Kodowanie CR (TSAP) wbudowane w `connect_tsap_impl`, sprawdzane tylko przez ignorowane testy TCP (D+F, P3) | Fakt prawdziwy (zamiana hi/lo TSAP nie łamie domyślnego `cargo test`), ale tcp_smoke ją wykrywa (17 porażek). AGENTS.md wymaga uruchamiania testów TCP przy zmianach protokołu/transportu; brak wstrzykiwania transportu to zaakceptowane ograniczenie. Zostaje rekomendacja refaktoryzacji, nie defekt |
| Wymienne parametry `u16` (`db_number`, `start`, `byte_num`) → zamiana argumentów pisze pod zły, poprawny adres (D+F, P3) | Opinia projektowa; kolejność to udokumentowana konwencja Snap7, przypięta w public_errors.rs; zmianę typów adresu odrzucono w projekcie 2026-10-27 (podejście 3). Brak ścieżki kodu dającej zły adres z poprawnych argumentów |

Odrzucone przez agentów aspektowych (wybór najważniejszych z 53; pełna lista w `$S/wf.json`, pole `rejected_by_reviewers`):

| Hipoteza | Powód |
|---|---|
| Przepełnienie w `request()` (`start*8`, `offset as u32`, `count as u16`, `(count+4) as u16`, `req.len() as u16`), także na 32 bitach | `validate_transfer` gwarantuje `start+len-1 ≤ 0x1fffff` ⇒ `(start+offset)*8 ≤ 0xfffff8`; `count ≤ 462`; ramka ≤ 487 B |
| Żądanie przekracza wynegocjowane PDU lub TPDU | `requested = min(480, tpdu-3)`, CC przyjmuje tylko 2^7…2^10; CPU może tylko obniżyć PDU; `read_payload = pdu-18`, `write_payload = pdu-28` |
| Panika przy indeksowaniu w `ack_data`/`validate_*`/`data_payload`/`validate_connection_confirm` (108 ostrzeżeń pedantic) | Każde indeksowanie poprzedza kontrola długości; harness 200 000 przypadków bez paniki |
| Alokacja sterowana długością z sieci (`vec![0; len]`) | `len ≤ max_total` (487 / pdu+7 / 1028) sprawdzane przed alokacją |
| Pętla bez postępu / zawieszenie w `read_full` | `Ok(0)` → `CONNECTION_CLOSED`; `Interrupted` przelicza termin; zerowy limit kończy timeoutem |
| Bardzo mały/duży `Duration` w `set_read_timeout` | zero nigdy nie trafia (`is_zero`); `checked_add` → `None` ogranicza każdy `read` |
| Zapis wysyła złe dane/adres w warstwie protokołu | Harness parsuje każde żądanie: wordlen, area, db (tylko 0x84), adres, nagłówek elementu, payload == bajty bufora — 200 000 przypadków ok; ręczne mutacje offsetu/kolejności części (M10–M12, M27, M28) wykryte |
| Obcięta ramka N „wciąga” początek ramki N+1 i daje cichy zły odczyt | Własność protokołu S7 bez sumy kontrolnej; referencja i długości muszą się zgadzać; `[0, confirmed)` zachowane. Nie błąd biblioteki |
| Desynchronizacja po błędzie PLC (S7_*) bez rozłączenia | Błędy S7 zgłaszane dopiero po wczytaniu całej ramki i sprawdzeniu referencji; dowód: błąd elementu, potem udany zapis na tym samym transporcie |
| Metoda publiczna dodaje 0 lub 2 konteksty | Wszystkie 13 metod wołają tylko rdzenie `*_impl` bez kontekstu |
| Brak `value` w kontekście `write_bit` łamie kontrakt | Świadoma decyzja (plans/2026-11-04-anyerror-migration.md:271-272); niezgodność opisu → R4-17 |
| `let _ = stream.shutdown(…)` ukrywa błąd | `disconnect`/`Drop` z założenia nie zawodzą; deskryptor i tak zamknięty |
| Utrata `io::ErrorKind`/`raw_os_error` | Udokumentowana decyzja (CHANGELOG Migration) |
| Serde/Debug `AnyError` ujawnia bufor | Żaden konstruktor nie przyjmuje danych bufora; komunikaty statyczne lub kody u8 |
| Publiczne pole `connected`/`pdu_length` pozwala wysłać bez strumienia lub zmienić limity parsera | Transfery wymagają też prywatnych `limits` i `stream`; limity z prywatnego `PduLimits` |
| `breaks_session` niespójne z opisami `kind` | Dokładnie 5 kategorii „Breaks the session”; nieznane/brak → rozłączenie |
| Brak timeoutu całej ramki dla ACK zapisu | M26/M31 wykryte przez tcp_smoke (`late_reply…`, `trickling…`) |
| Komentarze „A chunk is at most 462 bytes”, „CC validation guarantees at least 128 bytes”, bajty CR, pola ACK Setup | Zgodne z kodem (poza literówką „lenght” → R4-20) |
| Niepokryte linie llvm-cov (client.rs:342, 659; protocol.rs:107, 403) są krytyczne | 342 wymaga resolvera bez adresów (nieosiągalne bez wstrzyknięcia); pozostałe to artefakty regionów; gałęzie 156/160 pokryte |
| Ścisła walidacja AmQ, transport size 0x09, ROSCTR=2, komunikat timeoutu zapisu | Rozstrzygnięte wcześniej (H1–H3, timeout zapisu) — brak nowego dowodu |

## 5. Macierz pokrycia

| Aspekt | Użyte | Pominięte (powód) |
|---|---|---|
| 0 Inwentaryzacja | code-review-graph (build, hub, bridge, large functions, architecture — 0 communities), odczyt AGENTS/specs/plans/CHANGELOG | LSP — kod 1,5 kLOC przeczytany w całości |
| 1 Bramki | fmt, check, doc, clippy+reviewdog, clippy pedantic, test, tcp_smoke, llvm-cov (stable + nightly `--branch`), audit, deny (bez configu + config tymczasowy), machete, typos, miri, semver-checks (crates.io 0.1.2), cargo-mutants (worktree, `--include-ignored`), ocr delegate preview | cargo-fuzz (niezainstalowany, decyzja użytkownika); i686 (brak std targetu) |
| A Protokół | rust-review (checklista ręcznie — skill orkiestruje subagentów), property-based-testing, lens review-council:correctness-reviewer (stosowany przez agenta) | harness-writing (zasady zastosowane w wariancie proptest, skill nie ładowany), LSP/graph/lockdoc (zbędne przy tej skali) |
| B Błędy | pr-review-toolkit:silent-failure-hunter (typ agenta), lockdoc (anyerror 0.1.13), testy-dowody w `$S/wt-b` | code-review-graph, LSP (kod przeczytany w całości); rust-review/property-based-testing (wąski audyt kontraktu) |
| C Wrogie dane | harness proptest 5 własności (`$S/wt-evidence/src/client/protocol/hostile.rs`, 20 000 i 200 000 przypadków), sharp-edges (lens) | cargo-fuzz/libFuzzer (niewykonany), address-sanitizer (forbid(unsafe), pominięty z założenia) |
| D API | sharp-edges, rust-skills, lensy type-design-analyzer / review-council:interface-reviewer (bez spawnu agentów), porównanie z rust7-0.1.2 z rejestru | api-design-principles (REST/GraphQL — nieadekwatny); cargo semver-checks bez wartości (major bump) |
| E Testy | pr-review-toolkit:pr-test-analyzer (typ agenta), mutation-testing (klasyfikacja; kampania cargo-mutants zamiast mewt/muton), property-based-testing (sonda częstości gałęzi), lens test-evidence-reviewer, 31 ręcznych mutacji | code-review-graph, lockdoc, LSP (zbędne) |
| F Architektura | code-review-graph (hub/bridge/large functions), lens review-council:architecture-reviewer | code-maturity-assessor (framework smart-contract — nieadekwatny) |
| G Dokumentacja | pr-review-toolkit:comment-analyzer (typ agenta), lean-comments (audyt), lockdoc (Display/serde anyerror), typos, `-W missing_docs`, `cargo package --list` | rust-review (poza aspektem) |
| Weryfikacja | 3 niezależnych weryfikatorów (worktree `$S/wt-v1..v3`), repro dla każdego CONFIRMED; koordynator powtórzył R4-02 (M19) i R4-23 | — |
| Pominięte globalnie | constant-time-analysis/testing (brak kryptografii), address-sanitizer (forbid(unsafe)), api-design-principles, code-maturity-assessor, skille webowe/GraphQL/gRPC/axum/TUI (nieadekwatne), `/code-review ultra` (płatny, uruchamiany przez użytkownika) | |

Uwaga procesowa: agenci D+F i G współdzielili worktree `$S/wt-df`; agent G odnotował test agenta D+F w tym worktree i użył go tylko jako dowodu (bez wpływu na wnioski).

## 6. Ryzyka rezydualne i czego nie zweryfikowano

- **Brak prawdziwego PLC.** Zgodność ramek z firmware S7-300/400/1200/1500 (stałe nagłówka — R4-03, transport size, AmQ, ROSCTR=2, odmowy DR) sprawdzono tylko względem mocka i własnej interpretacji protokołu. Testy przypinające bajty bez referencji z pcap/Wireshark przypięłyby też ewentualny błąd. Zalecany pcap z urządzeń docelowych (jak dla H1–H3).
- **Publiczna warstwa `S7Client` jest słabiej chroniona niż `protocol.rs`.** R4-01…R4-04 pokazują, że 17 ręcznych mutacji klasy P0/P1 przeżywa cały zestaw. Do czasu ich zamknięcia zielony `cargo test` nie gwarantuje adresu, wartości bitu ani rozłączenia po każdym błędzie ramkowania na ścieżce publicznej.
- **Brak CI** (.github nie istnieje): testy tcp_smoke (`#[ignore]`) zależą od dyscypliny ręcznego uruchamiania (AGENTS.md).
- **DNS i wielu adresów** (R4-07) nie testowano z prawdziwym resolverem (brak sieci w testach).
- **Fuzzing pokryciowy** (libFuzzer) niewykonany; proptest z mutacjami strukturalnymi to słabszy zamiennik dla rzadkich ścieżek.
- **32-bit** — tylko analiza ręczna.
- **R3-08…R3-12, R3-23…R3-25** — treść niedostępna w repo; możliwe nakładanie się z R4-06…R4-09 nie zostało wykluczone.
- **Polityka licencji** — brak `deny.toml`; wynik `licenses ok` uzyskano tylko z konfiguracją tymczasową i nie jest gwarancją.
- **Dowody poza repo** — worktree i logi w scratchpadzie sesji są tymczasowe; worktree są zarejestrowane w `.git/worktrees` i warto je usunąć `git worktree remove` po lekturze raportu.
