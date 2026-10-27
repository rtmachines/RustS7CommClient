# Naprawy siedmiu znalezisk Rust7 — projekt

Status: zatwierdzony do sekwencyjnej implementacji przez użytkownika 2026-10-27.

## Cel i zakres

Naprawić siedem błędów wskazanych w review całego kodu na commicie
`6f5fd6722bb519c695394a0dc585269cc9055d4b`: F1 przepełnienie adresu bitu,
F2 sukces przy niepełnym odczycie, F3 obcinanie bufora, F4 krótkie odczyty TCP,
F5 brak walidacji S7, F6 nieprawidłowe PDU, F7 pusty bufor bitowy.
Usunąć również istniejące błędy fmt, Clippy i doctestów, aby wynik można było
zweryfikować automatycznie. Zachować synchroniczny charakter biblioteki.

## Approaches

1. Osobne poprawki w dotychczasowych metodach: najmniejszy początkowy diff,
   lecz powielanie odczytu i walidacji ramek utrudnia sprawdzenie spójności.
2. Wspólna prywatna obsługa ramek i walidacji oraz zachowanie publicznego API:
   rekomendacja autora planu. Umożliwia testowanie tego samego kodu z prawdziwym
   TcpStream i skryptowanym transportem implementującym Read + Write.
3. Zmiana publicznych typów adresów i klienta: szersza przebudowa, wymagająca
   migracji użytkowników; zbędna do naprawy tych znalezisk.

Wariant 2 zaakceptowany poleceniem wykonania sekwencyjnego 2026-10-27.

## Ograniczenia

- Zachować istniejące publiczne sygnatury metod i warianty S7Error.
- Biblioteka pozostaje w edition 2021 i z `#![forbid(unsafe_code)]`.
- Bez nowych zależności runtime i bez deklarowania nieuzgodnionego MSRV.
- Nie zmieniać znaczenia start: indeks bajtu dla BYTE, indeks bitu dla BIT.
- Do obliczeń adresów używać u32, do długości całych buforów i offsetów usize.
- Nie wykonywać automatycznych ponowień zapisu po błędzie transmisji.
- Testy jednostkowe mają działać bez sieci, PLC, Dockera i podmiany tekstu źródeł.
- Nie dodawać async, FFI, obsługi wieloelementowych żądań ani COTP fragmentation.

## Zachowanie docelowe

F1: read_bit/write_bit obsługują cały zakres byte_num: u16 i bit_idx 0..7.
Adres obliczany jest jako `u32::from(byte_num) * 8 + u32::from(bit_idx)`;
wartości 8192 i 65535 nie zawijają się. Publiczne read_area/write_area
zachowują start: u16, a wewnętrzne operacje przyjmują szerszy adres.

F2/F5: wynik Ok oznacza zweryfikowaną odpowiedź właściwej operacji oraz dokładną
ilość danych dla danego fragmentu. Walidować identyfikator i typ S7, referencję
PDU, błędy nagłówka, długości parametrów/danych, funkcję, liczbę elementów,
status elementu, transport size i długość payloadu. Zachować rozpoznawanie
S7NotFound i S7InvalidAddress dla prawidłowych odpowiedzi błędów.

F3: przetwarzać cały bufor, również powyżej 65535 bajtów. Przed pierwszą operacją
I/O odrzucać zakres przekraczający 24-bitowy adres bitowy protokołu; dla BYTE
ostatni bajt ma adres najwyżej 0x1fffff. Nie obcinać długości. Przy błędzie
późniejszego fragmentu wcześniejszy zapis może być wykonany — brak atomowości
musi pozostać jawny w dokumentacji.

F4: odczytywać nagłówek TPKT i dokładnie wskazaną resztę ramki przez read_exact;
nie konsumować następnej ramki. Sprawdzać długość przed alokacją i indeksowaniem.
Krótki odczyt TCP nie jest błędem protokołu; EOF/timeout pozostaje błędem I/O.
Walidować osobno COTP Connection Confirm oraz ramkę COTP Data z EOT.

F6: dla obecnego żądania 480 akceptować tylko rozmiary pozwalające zmieścić
nagłówki i co najmniej jeden bajt zapisu: `28 < pdu_length <= 480`.
To granica obsługi tej implementacji, nie deklaracja minimalnego PDU standardu.
Sprawdzić ją przed odejmowaniem. Nie publikować stanu connected ani limitów
operacji przed pełnym powodzeniem negocjacji. Każdy fragment musi mieć dodatnią
długość. Nie używać publicznego pola diagnostycznego pdu_length jako jedynego
źródła zaufanych limitów parsera.

F7: pusty bufor BIT zwraca InvalidFunParameter przed I/O; dla niepustego
bufora używany jest tylko pierwszy bajt. Pusty BYTE pozostaje operacją bez
telegramu po zwykłej kontroli parametrów i połączenia.

## Dowody i odbiór

Każde znalezisko ma stały test regresyjny: najpierw czerwony, następnie zielony
w debug i release. Główne kontrole: fmt, Clippy z -D warnings, cargo test oraz
doctesty. Smoke test lokalnego TCP stanowi osobną kontrolę integracji; blokada
gniazd oznacza NOT RUN, a nie PASS. Bez automatycznego kontaktowania się z PLC.
Brak deny.toml pozostaje jawnym ograniczeniem; nie tworzyć polityki licencji
i źródeł zależności tylko po to, aby uzyskać zielony wynik cargo-deny.
