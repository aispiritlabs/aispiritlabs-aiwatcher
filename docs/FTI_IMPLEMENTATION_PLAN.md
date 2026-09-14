# FTI — co zostało do zrobienia

Stan: 2026-09-14. Etapy A, B i C są dostarczone i zamknięte na tablicy jako
[AW-6](specs/AW-6-compare-kept-evaluation-evidence-and-run-evaluations/06-deploy.md).
Ten plik trzyma wyłącznie pracę otwartą: etap D, rundę świadka z sekcji 41 i to,
co odłożono na później.

- **Co już działa i dlaczego tak:** [ADR 0030](ADR/ADR_0030_EVALUATION_EVIDENCE.md),
  decyzja 8 i Guardrails w [`CLAUDE.md`](../CLAUDE.md), README
  [`crates/aiwatcher-evaluation`](../crates/aiwatcher-evaluation/README.md) i
  [`services/scorers`](../services/scorers/README.md).
- **Historia dostawy** (sekcje 1–40, weryfikacje, commity):
  `git show a40bb1f:docs/FTI_IMPLEMENTATION_PLAN.md`. Odwołania w rodzaju „40.1”
  czy „37.4” wskazują tamtą wersję.
- **Zasada tego pliku:** zrobiona praca stąd znika. Trwałe reguły trafiają do ADR
  i `CLAUDE.md`, weryfikacja do opisu commita, a tu zostaje tylko „Co zostaje”.
  Przed scaleniem: `rtk just check`, `just openapi` przy zmianie kontraktu.

Założenie robocze bez zmian: instancja self-hosted dla jednej grupy ze wspólnym
dostępem do danych. Izolacja wielu zespołów nie jest potwierdzonym wymaganiem;
jeśli się pojawi, zakres danych i autoryzacja idą przed współdzielenie i nowe
zasoby serwerowe (D4). Sam filtr `project` nie zapewnia izolacji.

## Etap D — operacje i współpraca

Status: nie rozpoczęty. Warunki wejścia są spełnione — uruchamialna ewaluacja
(C0–C3) i trwałe wyniki (B) istnieją.

**Rezultat:** platforma informuje o wyniku lub problemie, a zespół może wrócić do uzasadnienia decyzji.

- **D1 — alerty v1:** terminalny błąd wykonania oraz regresja zakończonej, porównywalnej ewaluacji. Jeden kanał, najlepiej konfigurowany webhook. Trwała kolejka dostarczeń/outbox, retry z ograniczeniem, deduplikacja po zdarzeniu i wersji reguły, historia oraz test kanału. Odbiorca może otrzymać powtórzenie po niejednoznacznym błędzie sieci; przekazywać klucz deduplikacji zamiast obiecywać exactly-once.
- **D2 — alerty okienkowe metryk**, później: okres, minimalna próba, cooldown, odzyskanie poprawnego stanu i jawne zachowanie przy braku danych. Harmonogram uruchamia sprawdzenie, ale sam nie definiuje reguły alertu.
- **D3 — diagnostyka możliwości instancji** i integracja pierwszego źródła: dostępność registry/workera/oceny oraz link do pierwszego odebranego sygnału. Nie udostępniać sekretów w odpowiedzi diagnostycznej. UI konfiguracji tylko dla rzeczywiście edytowalnych ustawień.
- **D4 — serwerowe zapisane widoki i prosty raport:** opis decyzji, przypięte warianty/oceny, kilka wykresów i referencje artefaktów. Osobno określić właściciela i uprawnienia zapisu/odczytu. Link do raportu nie nadaje dostępu do obiektów źródłowych. Lokalny zapis widoków z etapu A zostaje.

**Odbiór D:** powtórzone zdarzenie nie tworzy nowego logicznego alertu; awaria kanału jest widoczna i ponawiana; raport zachowuje przypięte wyniki po zmianie etykiety produkcyjnej; osoba bez prawa do źródła nie dostaje jego treści przez raport.

## Później — poza etapem D

Rozmiar: S — lokalna zmiana; M — jeden przepływ; L — kilka warstw lub trwałość; XL — osobna inicjatywa. To nie estymaty kalendarzowe.

| Funkcja | Kiedy | Granica pierwszej wersji | Rozmiar |
| --- | --- | --- | --- |
| Globalny lineage i katalog artefaktów | Później | Rozszerzenie sprawdzonych relacji, wyszukiwanie i paginacja | L |
| Playground | Później | Klient istniejącego procesu oceny, wspólne wejścia | M–L |
| Prompty chat i format odpowiedzi | Później | Rozszerzenie tekstowego registry wdrożyć z migracją | M–L |
| Ocena sesji i trajektorii | Później | Najpierw zapisane sesje i kroki; przypięty zakres oraz kompletność | M–L |
| Sweeps / HPO | Na żądanie | Ograniczona liczba prób istniejącego workflow; najpierw prosty grid/random | L |
| Monitoring jakości online i budżety | Później | Scorery na próbkowanym ruchu; budżety na istniejącej tabeli cen | L |
| Organizacje, zespoły, projekty, klucze, audyt | Warunkowo | Tylko przy wymaganiu wielu odizolowanych zespołów | XL |
| Global search, komentarze, wzmianki | Później | Gdy będzie dość zapisanych analiz i użytkowników | M–L |
| Szeroki katalog integracji | Według popytu | Jedna rzeczywiście używana integracja z testem pierwszego sygnału | M na adapter |
| Asystent, HiveMind, zarządzane endpointy | Poza roadmapą | Odrębne produkty | XL |
| Własny routing dostawców LLM | Poza roadmapą | Bramka świadka `aiwatcher_sdk.gateway` jest czymś innym i już istnieje | XL |

## 41. Runda świadka po sekcji 40 — co zostaje

Status: zrobione 2026-09-14 (`96a8032`…`4849c27`, bez pusha), a tego samego
dnia domknięte resztki: licznik z zegara klienta i ze spoolu zabitego klienta,
spool w TypeScript, liczniki pomiaru w indeksie pytań, `asked_since_seconds` w
polityce bramki i skrót kodu narzędzia pod URL-em i na hoście. Reguły są w ADR
0001, 0002, 0012 i 0030 (poprawki z 2026-09-14) oraz w `CLAUDE.md`; odbiór w
opisach commitów: 51 twierdzeń `just e2e-generate` i sprawdzenie dziennika na
własnym Iggy i RustFS. Tu zostaje tylko to, czego te rundy nie zamknęły.

Z ograniczeń sekcji 40, które nie weszły do tej rundy, bez zmian:

- reguła pytania zadanego gdzie indziej jest ostrożna: ruch produkcyjny z tym
  samym pytaniem na przypiętym prompcie i modelu, równoległy baseline na tym
  samym prompcie i ponowiona próba generowania odbierają wymianę — a z
  `asked_since_seconds` także wcześniejsze pomiary tych samych przypadków;
- host z kluczem świadka może wytworzyć każdy skrót w domenie tego klucza — jest
  zaufany tak jak bramka;
- rola `journal` działa tylko na Laserze;
- runy klienta zaczętego w pierwszej minucie po starcie foldu nie są liczone.

### Co zostaje

- **Kolejność sędziego.**
  - `both_ways` odwraca kolejność, więc dla więcej niż dwóch kandydatów nie
    sprawdza każdego miejsca.
  - `witnessed` jest stałe dla danych wartości: aplikacja nie ustawi go z góry,
    ale nie jest też losowe między przypadkami.
  - Bez `order` wybór sędziego nadal jest wymianą, tylko nazwaną.
- **Indeks pytań.**
  - Parafraza nie jest wykrywana, z założenia.
  - `asked_since` przypina deklaracja albo polityka bramki
    (`asked_since_seconds`, a `aiwatcher-gate` deklaruje run z co najmniej tyle).
    Start wersji kohorty nie jest punktem odczytu: przypadek mógł być znany
    wcześniej.
  - Odczyt czyta każdą stronę okna, co przy dużym ruchu i długim oknie kosztuje.
  - Każda replika roli `serve` pisze własne strony, a czytelnik je deduplikuje.
  - Normalizacja jest bajt w bajt tylko dla znaków znanych wersji Unicode każdego
    języka (Python 3.13: 15.1).
  - TypeScript ma samą funkcję `normalized`, bez bramki.
- **Narzędzia.**
  - Obliczenie w procesie aplikacji zostaje jej słowem, z założenia.
  - Skrót kodu narzędzia pod URL-em to słowo usługi (`Aiwatcher-Tool-Code`), a
    na hoście z `ToolWitness` słowo hosta (`code`). Usługa, która go nie poda,
    zostawia wartości bez rozliczenia tam, gdzie wariant przypina kod.
  - Skrót obejmuje plik funkcji, a nie moduły, które ona importuje.
- **Luka dłuższa niż retencja.**
  - Żaden adapter nie czyta retencji brokera; zapas liczy się od konfiguracji.
  - Zapas zgłasza tylko działający dziennik, a wyłączony milczy. Alert na ciszę
    i pozycja w diagnostyce czekają na etap D (D1, D3).
  - Odzysk okresów z VictoriaTraces nie jest zrobiony (decyzja 6).
  - Sprawdzenie na brokerze wstrzymało magazyn zamiast skrócić retencję tematu,
    więc samo usunięcie przez broker nie było odtworzone.
- **Ostatni zgubiony run klienta.**
  - Klient zabity bez zamknięcia i bez spoolu nie mówi o runach od ostatniego
    licznika (zegar mówi co pięć minut). Ze spoolem licznik jest zapisywany przy
    starcie każdego runu, kosztem zapisu pliku, a wysyła go dopiero następny
    transport uruchomiony na tym samym katalogu.
  - Spool jest w Pythonie (`spool_dir`) i w TypeScript na Node (`fileSpool` z
    `@aiwatcher/sdk/node`); w przeglądarce go nie ma.
- **Licznik runów pomiaru.**
  - Podział na „zgubiony” i „nieznany” liczy się z liczników, a nie z
    odpowiedzi. Gdy są oba, które odpowiedzi są którymi, jest przypisaniem.
  - Liczniki trzyma indeks pytań obok swojego zasięgu, więc restart ich nie
    gubi. Wdrożenie bez magazynu obiektów czyta je z read modelu i gubi przy
    restarcie bez odtworzenia logu. Przetrwanie restartu sprawdza test zapisu i
    odczytu indeksu, a nie restart na brokerze.
  - Numer próby przekazuje tylko `Generation` w Pythonie. TypeScript liczy próby,
    gdy wywołujący poda `generationAttempt`.
- **Limit jednej krawędzi.** Liczba ukończeń jest liczona ostrożnie: limit
  wewnątrz cyklu nie jest nazywany, nawet gdy cykl ma własny limit rund.

Etap D (wyżej) prowadzi osobna sesja; bierze pierwszy wolny numer sekcji po tej.
