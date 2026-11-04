# RustS7CommClient — instrukcje dla agentów

Biblioteka `rust7`: natywny klient protokołu S7 w Rust, komunikujący się z PLC Siemens.
Ten plik jest wspólnym źródłem instrukcji projektu; nie twórz kopii w CLAUDE.md.

## Środowisko i komendy

Uruchamiaj komendy z katalogu zawierającego główny Cargo.toml. Potrzebujesz
toolchainu Rust obsługującego edition 2021, Cargo oraz komponentów rustfmt i clippy.
Do nawigacji semantycznej używaj rust-analyzer przez LSP lub Serenę.

| Cel | Komenda |
| --- | --- |
| Formatowanie zmian Rust | `cargo fmt --all` |
| Kontrola formatowania | `cargo fmt --all -- --check` |
| Kontrola kompilacji wszystkich targetów | `cargo check --all-targets` |
| Budowanie biblioteki | `cargo build` |
| Testy jednostkowe i dokumentacji | `cargo test` |
| Wybrany test | `cargo test <fragment_nazwy>` |
| Testy TCP na lokalnym mocku | `cargo test --test tcp_smoke -- --ignored` |
| Lint | `cargo clippy --all-targets -- -D warnings` |
| Dokumentacja API | `cargo doc --no-deps` |

Cargo obsługuje tutaj `--offline`; używaj go, gdy potrzebne zależności są lokalnie.
Po zmianach Rust uruchom kontrolę formatowania, Clippy oraz testy. Dla zmian
transportu uruchom również ignorowane testy TCP; wymagają dostępu do lokalnych
gniazd. Zgłoś ograniczenia środowiska zamiast przedstawiać pominięty test jako sukces.
Po zmianach dokumentacji API sprawdź także generowanie dokumentacji.

## Nawigacja: graf, Serena i LSP

- Najpierw zawęź zakres narzędziami code-review-graph, podając bezwzględny katalog
  tego repozytorium jako `repo_root`. Brak grafu wymaga `build_or_update_graph_tool`.
- Do wyszukiwania używaj `semantic_search_nodes_tool` lub `query_graph_tool`;
  do przeglądu zmian `detect_changes_tool` i `get_review_context_tool`, a do wpływu
  zmian `get_impact_radius_tool`, `get_affected_flows_tool` oraz `tests_for`.
- Przed użyciem Sereny przeczytaj jej instrukcje inicjalne i aktywuj ten projekt
  przez jego ścieżkę. Korzystaj z przeglądu symboli, wyszukiwania definicji
  i referencji; odczytuj ciała tylko istotnych symboli.
- Definicje, typy, implementacje traitów i referencje w Rust weryfikuj przez
  rust-analyzer/LSP (także za pośrednictwem Sereny). Przy zmianie nazwy używaj
  semantycznego rename i sprawdź wynikowy diff.
- Graf i indeks LSP pomagają znaleźć kod; implementacja i testy rozstrzygają.
  Pusty wynik nie dowodzi braku zależności. Gdy narzędzie jest niedostępne,
  zgłoś to i użyj `rg` oraz odczytu źródeł, zachowując weryfikację przez Cargo.

## Reviewdog

Zbieraj diagnostykę Clippy przez reviewdog podczas review i weryfikacji zmian Rust.
Jeśli repozytorium otrzyma konfigurację reviewdog, używaj jej właściwych runnerów.
Bez konfiguracji uruchom w Bash:

```bash
set -o pipefail
cargo clippy --all-targets --message-format=short --color=never 2>&1 | reviewdog -f=clippy -reporter=local -filter-mode=nofilter -fail-level=warning
```

Przekazuj surowy output; przy przechwytywaniu Cargo przez RTK użyj
`rtk bypass cargo` zamiast `cargo` w powyższym potoku. Zachowaj status błędu
potoku i odróżnij awarię lintera od znalezionych problemów. Pusty raport
filtrowany nie potwierdza poprawności całego projektu. Publikowanie raportów
wymaga zlecenia użytkownika; domyślnie używaj reportera lokalnego.

## Ograniczenia projektu

- Zachowaj implementację bez unsafe oraz walidację ramek i granic adresowania.
  Zmiany protokołu sprawdzaj wraz z testami w src/client/tests.rs,
  src/client/protocol/tests.rs i tests/tcp_smoke.rs.
- Wieloczęściowy zapis nie jest atomowy: wcześniejsze części mogą dotrzeć do PLC
  przed błędem. Nie dodawaj automatycznych ponowień zapisów bez rozstrzygnięcia
  ich semantyki. Po błędzie odczytu bufor nie jest kompletnym wynikiem.
- Nie uruchamiaj przykładów przeciw rzeczywistemu PLC bez wyraźnego zlecenia
  i wskazania urządzenia; do testów transportu używaj lokalnego mocka.
- examples/docker to osobny pakiet, poza głównymi komendami Cargo; zależy od
  opublikowanego rust7, więc jego uruchomienie nie weryfikuje lokalnych zmian biblioteki.
- Opis protokołu i API: [dokumentacja](doc/Documentation.md).
  Przykłady: [instrukcje przykładów](examples/README.md).
