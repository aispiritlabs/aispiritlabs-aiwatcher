# Review pipeline — 2026-09-08

Zakres: niezacommitowane zmiany względem `3117259`, w tym nowe pliki schedulera,
oraz zgodność implementacji z `PIPELINE_ARCHITECTURE.md` i `KICKOFF.md`.
To przegląd i propozycja kolejności prac; kod produkcyjny nie został zmieniony.

**Ocena: fundament managed execution jest użyteczny, ale scheduler nie powinien
jeszcze otrzymać deklarowanych gwarancji pracy bez nadzoru i na wielu replikach.**
Najpilniejsze problemy dotyczą decyzji opartych na nieaktualnej projekcji,
trwałości slotów oraz równoczesnych zapisów harmonogramu. Zielone testy obecnie
nie obejmują tych scenariuszy. Lista „one defect” w KICKOFF jest zbyt wąska.

## 1. Wyniki review zmian

P1 oznacza naprawę przed poleganiem na danym zachowaniu we wdrożeniu.
P2 oznacza konkretny błąd do zaplanowania, a nie sugestię stylistyczną.

### R1 — P1: `overlap=skip` nie działa w rozdzieleniu `serve/work`

Miejsce: [scheduler.rs:255](/Users/mkubaszek/Projects/ai_spirit/aiwatcher/crates/aiwatcher-server/src/execution/scheduler.rs:255).

Scheduler uruchamia się w roli `work` (`execution/mod.rs:177–197`), ale aktywnych
runów szuka w `state.read_model`. Ten read model jest lokalny dla procesu.
`bin/aiwatcher.rs:89–102` kończy ścieżkę `work` przed uruchomieniem projectora.
W rezultacie zapytanie zwraca pustą listę również wtedy, gdy poprzedni run trwa.

W trybie łączonym pozostaje drugi problem: odczyt asynchronicznej projekcji
poprzedza start, więc przy nadrabianiu kilku slotów lub opóźnieniu outboxa kolejny
start może nie zobaczyć poprzedniego. Idempotencja identyfikatora slotu chroni
przed dwoma runami tego samego slotu, ale nie przed overlapem różnych slotów.

**Naprawa:** atomowo egzekwować ograniczenie aktywnego wykonania dla definicji
w transactional store. Ustalić, czy obejmuje również ad-hoc i `run_now`.
Nie wymaga to przeniesienia listy produktowej do PostgreSQL ani realizacji
całej Phase 8 — to warunek dopuszczenia pracy, nie drugi widok historii.

**Test akceptacyjny:** dwa workery, opóźniona projekcja i dwa zaległe sloty;
przy `skip` najwyżej jeden aktywny run. Dodatkowo paused/awaiting-input i ad-hoc.

### R2 — P1: przejściowy błąd startu trwale konsumuje slot

Miejsce: [scheduler.rs:127](/Users/mkubaszek/Projects/ai_spirit/aiwatcher/crates/aiwatcher-server/src/execution/scheduler.rs:127).

Każdy błąd `fire()` staje się `Refused`, po czym globalny checkpoint jest
przesuwany do `now` (`:167–172`). `fire()` obejmuje także odczyt definicji i zapis
wykonania, a błędy API są zamieniane na tekst, więc tracona jest ich klasyfikacja.

Przykład: podczas startu o 09:00 baza lub object store chwilowo nie odpowiada;
zapis checkpointu chwilę później działa. Run nie istnieje, lecz następny tick
nie obejmuje już jego slotu. To przeczy komentarzowi, że niedostępny store kosztuje
wyłącznie opóźnienie. Błąd zapisu `last` również jest ignorowany, więc obiecana
naprawa notatki w następnym ticku nie jest zapewniona.

**Naprawa:** zachować rozróżnienie błędu trwałego i przejściowego. Walidacyjna
odmowa może zamknąć slot, awaria infrastruktury musi pozostawić trwałą pracę do
ponowienia. Najprostsze zatrzymanie checkpointu wymaga też sprawdzenia replayu
`skip`; docelowo wynik obsługi/pending slot powinien być zapisany per slot.

**Test akceptacyjny:** awaria odczytu/zapisu tylko podczas `fire`, sprawny zapis
checkpointu, następnie powrót store’a; ten sam slot ostatecznie ma jeden run.

### R3 — P1: zapis `last` może cofnąć edycję albo przywrócić usunięty harmonogram

Miejsce: [scheduler.rs:152](/Users/mkubaszek/Projects/ai_spirit/aiwatcher/crates/aiwatcher-server/src/execution/scheduler.rs:152).

Tick czyta wszystkie harmonogramy, wykonuje pracę, a potem zapisuje cały
`ScheduledDefinition` ze starego snapshotu. `ScheduleStore::set` robi zwykłe
`ObjectStore::put`, bez wersji i compare-and-swap.

Jeżeli między odczytem a zapisem użytkownik wyłączy, zmieni lub usunie schedule,
tick nadpisze tę decyzję. Po DELETE może ponownie pojawić się aktywny schedule.
Dwie repliki mogą też nadpisać nowszy `last` starszym wynikiem.

**Potwierdzenie:** wykonano kolejno `all → clear → set(stary_snapshot)` na
rzeczywistym `ScheduleStore`; usunięty harmonogram ponownie istniał.
To dokładnie kolejność operacji dopuszczana przez pętlę schedulera.

**Naprawa:** oddzielić zapis konfiguracji od zapisu wyniku slotu; konfigurację
wersjonować i warunkowo aktualizować. Kolejne `get` przed `put` nie usuwa wyścigu.
Port musi zapewniać wymaganą atomowość albo kontrolne dane schedulera powinny
trafić do transactional store.

**Test akceptacyjny:** zatrzymać tick po odczycie, wykonać PUT/DELETE, wznowić;
edycja zostaje zachowana, usunięcie nie jest cofane.

### R4 — P1: migracja 0003 łamie działające binaria poprzedniej wersji

Miejsce: [0003_drop_dead_timestamps.sql:18](/Users/mkubaszek/Projects/ai_spirit/aiwatcher/crates/aiwatcher-execution/migrations/0003_drop_dead_timestamps.sql:18).

To, że wartości były NULL, nie oznacza kompatybilności schematu. Kod w HEAD
czyta `started_at, ended_at` i wymienia je w `INSERT ... ON CONFLICT`.
Nowy proces usuwa kolumny przy starcie. Stary proces od tego momentu nie może
czytać/zapisywać projekcji. Chart workerów stosuje `RollingUpdate`
(`deploy/helm/aiwatcher/templates/worker.yaml:37–42`), więc okres mieszanych
wersji jest normalną ścieżką wdrożenia. Rollback obrazu również nie przywraca kolumn.

**Naprawa:** najpierw wydać kod, który nie używa kolumn, a ich usunięcie odłożyć
do kolejnego, kontrolowanego etapu. Alternatywnie jawnie wymagać zatrzymania
wszystkich starych procesów i opisać ograniczenia rollbacku. Sam advisory lock
serializuje migracje, lecz nie chroni starych zapytań.

**Test akceptacyjny:** stary i nowy klient na jednej bazie podczas upgrade’u
oraz sprawdzony scenariusz rollbacku; test świeżej bazy i dwukrotnego `apply`
tego nie dowodzi.

### R5 — P2: zmiana czasu łamie niezależność wyniku od częstotliwości ticka

Miejsce: [rule.rs:215](/Users/mkubaszek/Projects/ai_spirit/aiwatcher/crates/aiwatcher-execution/src/schedule/rule.rs:215).

`local_slot_at` buduje lokalną godzinę na `OffsetDateTime` z offsetem pochodzącym
z punktu startowego. Odczyt offsetu dla tego obiektu nie stanowi jednoznacznego
rozwiązania powtórzonej godziny lokalnej.

**Potwierdzona reprodukcja, prawdziwe `Schedule::slots_between`:**

```text
daily 02:30, Europe/Warsaw
przedział: 2026-10-25 00:00Z → 02:00Z

jedno wywołanie:   [00:30Z]
tick co 20 minut:  [00:30Z, 01:30Z]
```

Dwa różne timestampy dają dwa execution IDs. Ta sama dzienna intencja może
zatem uruchomić się dwukrotnie. Istniejący test podziału przedziału używa UTC;
testy Warszawy sprawdzają 09:00, więc omijają powtórzoną godzinę.

**Naprawa:** jawna polityka dla niejednoznacznej i nieistniejącej lokalnej godziny,
rozwiązanie lokalnej daty i czasu przez reguły strefy oraz testy podziału
przedziałów obejmujących obie zmiany czasu. `next_after` musi stosować tę samą politykę.

### R6 — P2: panel traktuje odmowy komend jak sukces

Miejsce: [managed-run.tsx:121](/Users/mkubaszek/Projects/ai_spirit/aiwatcher/apps/panel/src/components/managed-run.tsx:121).

Mutacje `pause/resume/cancel`, `retryStep`, `provideInput` i `clearSchedule`
zwracają bezpośrednio wynik wygenerowanego SDK. Domyślnie klient zwraca
`{error, response}` zamiast rzucać wyjątek (`client/client.gen.ts:210`), a globalna
konfiguracja w `lib/api.ts` ustawia wyłącznie `baseUrl`.

403, 409 i 5xx uruchamiają zatem `onSuccess`. `command.isError` i
`answerIt.isError` nie pokazują powodu odmowy. Usunięcie harmonogramu może
wyzerować formularz mimo odrzuconego DELETE.

Dodatkowo GET schedule zamienia każdy brak `data` na „brak harmonogramu”,
a `managed.isError` jest prezentowane jako „No run under this id”. Awaria
usługi lub sesji nie powinna udawać usunięcia danych.

**Naprawa:** `throwOnError: true` lub wspólna kontrola wyniku w mutacjach;
404 obsługiwać oddzielnie od innych błędów odczytu. Dla DELETE sprawdzać sukces
HTTP, ponieważ poprawne 204 nie ma `data`.

**Test akceptacyjny:** odpowiedzi 403/409/503 są widoczne w UI, odrzucony DELETE
nie resetuje formularza, rzeczywiste 404 zachowuje zwykły widok pustego stanu.

### R7 — P2: nowy harmonogram może uruchamiać sloty sprzed jego utworzenia

Miejsce: [scheduler.rs:123](/Users/mkubaszek/Projects/ai_spirit/aiwatcher/crates/aiwatcher-server/src/execution/scheduler.rs:123).

Każdy aktualny schedule otrzymuje cały przedział globalnego checkpointu.
`updated_at` nie ogranicza go i nie ma osobnego `effective_from`.
Jeśli worker był wyłączony przez trzy dni, a użytkownik utworzył schedule
przed jego powrotem, tick zastosuje nową regułę także do trzech minionych dni.
Zmiana cadence podczas przerwy analogicznie przelicza przeszłość nową regułą.

**Naprawa:** zdefiniować moment obowiązywania wersji harmonogramu oraz politykę
misfire/catch-up. Nie używać bezrefleksyjnie każdego `updated_at` jako granicy,
bo edycja niezwiązana z cadence mogłaby wtedy zgubić prawidłowy zaległy slot.

**Test akceptacyjny:** harmonogram utworzony w trakcie przerwy nie wywołuje
slotów sprzed aktywacji; edycja i ponowne włączenie mają jawnie określone zasady.

## 2. Istniejąca luka architektury, niezależna od tego diffu

### A1 — P1: file store nie odtwarza częściowo zapisanego commitu

Miejsce: [file.rs:306](/Users/mkubaszek/Projects/ai_spirit/aiwatcher/crates/aiwatcher-execution/src/store/file.rs:306).

Historia, projekcja, outbox i attempts są zapisywane w oddzielnych operacjach.
Po błędzie lub przerwaniu między nimi historia może zawierać ID wejścia,
a kolejka wykonania i outbox jeszcze nie istnieć. Ponowienie kończy się wtedy
na deduplikacji w `ExecutionHandler`, bez naprawienia brakujących zapisów.
`open()` również ich nie rekonstruuje. Wbrew komentarzom nie odtwarza też
automatycznie projekcji ze streamu; `projection()` czyta gotowy JSON.

**Potwierdzona reprodukcja:** w tymczasowym store wymuszono błąd rename outboxa
po zapisie historii/projekcji, usunięto przyczynę błędu i ponowiono identyczny start.

```text
Partial file commit error: Is a directory (os error 21)
Retry duplicate=true, stream records=5, outbox=0, attempts file exists=false
After reopening: outbox=0, attempts file exists=false
```

Run nie ma pracy do podjęcia ani faktów do opublikowania. To sprzeczne z celem
„awaria daje recovery albo jawny stan końcowy” z §29.3. Ograniczenie adaptera
do jednego procesu nie usuwa problemu częściowych zapisów.

**Naprawa:** atomowy journal całego commitu i recovery jego pochodnych albo
jedno transakcyjne lokalne storage. Dodać fault injection po każdej granicy zapisu,
a nie tylko test kompletnie udanego `append`. Osobno sprawdzić restart po SIGKILL:
obecny `workflow.lock` jest usuwany przez `Drop`, który w takim przypadku nie działa.

## 3. Co już jest wartościowe

- Wspólny `executions::start` ogranicza rozchodzenie się startu HTTP i schedulera.
- `AttemptWrite::Dispatch/Retire` usuwa nowe terminalne rekordy claimów, z testem
  wspólnego kontraktu adapterów. Jest to sensowna poprawa kosztu file store’a.
- `RunView.allowed`, konteksty kroków i execution ID w URL poprawiają obsługę runu
  po odświeżeniu panelu.
- Oddanie blokującego notebooka do thread poola pozwala obsługiwać lookup i UI
  podczas wykonania. Test współbieżności usługi dotyka realnego problemu.
- `CheckedClient` zachowuje informację o błędzie upstream zamiast wtórnego błędu
  „brak rows”; osobne joby CI wreszcie obejmują PHP, Python i PostgreSQL.

Te poprawy nie zastępują testów schedulera na granicach zapisu i między procesami.

## 4. Rzeczywiste braki względem planu

| Obszar | Obecny stan | Następny rezultat do dowiezienia |
|---|---|---|
| Bezobsługowy scheduler | CRUD, cadence, slot ID i ostatni wynik; R1–R5 i R7 | Trwałe rozliczenie slotów, kontrola overlapu, bezpieczne edycje i poprawny DST |
| Lokalna trwałość | File store przechodzi happy-path contract, ale A1 pozostaje | Automatyczne recovery częściowego commitu oraz test SIGKILL/restart |
| Historyczny notebook | Pin SHA i odmowa driftu; runtime czyta aktualny plik po nazwie | Snapshot treści po digest i odtworzenie starej wersji po zmianie head |
| Editor sessions | Context ID i staging istnieją; sesja z uprawnieniami/expiry nie | Otworzenie konkretnego historycznego źródła i wejścia po reloadzie |
| Canvas | RunCard pokazuje stan; bloki nie są mapowane do zarządzanego runu | Serwerowy mapping blok → krok, z informacją o zgodności rewizji draftu |
| Human input | Komenda Answer i UI istnieją; compiler nie emituje HumanInput | Jeden rzeczywisty approval gate z trwałym oczekiwaniem i autoryzacją |
| Worker / Planner | Typy i plan rozwoju nie są działającym protokołem wykonania | Claim/heartbeat/report oraz jeden pionowy przypadek użycia w Python SDK |
| Engine / hosted / container | Oddzielne, odroczone integracje | Każda za własnym gate’em; nie blokować nimi napraw obecnego schedulera |

**Ważne rozróżnienie dla notebooków:** wykrycie, że kod się zmienił, chroni przed
cichym wykonaniem innego kodu, lecz nie pozwala wykonać ani wyświetlić starego.
Dlatego SHA bez zachowanej treści nie spełnia §29.4 punkt 5. Snapshotów nie należy
sprowadzać wyłącznie do „sesji, która przetrwa reload”.

## 5. Proponowana kolejność prac

1. **Domknąć gwarancje wykonania:** R1–R3, R5 i R7 z testami schedulera; A1
   dla lokalnego recovery. Wprowadzić testowalny tick z wstrzykiwanym czasem
   i portami, tak aby dało się zatrzymać go na granicy efektu.
2. **Zapewnić bezpieczną aktualizację i prawdziwe błędy w panelu:** R4, R6,
   instrukcja migracji/rollbacku w INSTALL. Dopisać zmianę kształtu odpowiedzi
   `RunProjection → RunView` do informacji o kompatybilności API.
3. **Dowiezienie kontekstu historycznego:** snapshot notebooka, editor session,
   mapping canvasu i czytelny komunikat driftu. To bezpośrednio realizuje obietnicę
   ponownego otwierania i wznawiania historycznego wykonania.
4. **Jedna pionowa integracja workerowa:** Phase 10 i odpowiedni etap Phase 11,
   z rzeczywistym taskiem i porównaniem wyników do ścieżki bez workera.
   ContainerJob i hosted decider dopiero za potwierdzoną potrzebą następnego kroku.

Pozostałe tematy usprawnień, po powyższych:

- **Historia decyzji schedulera:** `last` nadpisuje poprzednią odmowę. Zapisywać
  per slot fakt `started/skipped/refused`, nie kopiując końcowego wyniku runu.
  Mierzyć opóźnienie startu, liczbę zaległych slotów i wiek ostatniego udanego ticka.
- **Kontekst schedule:** obecnie start dostaje puste parametry i `window=None`.
  Jeżeli intencją jest „przetwórz poprzednią dobę”, trzeba zapisać tę politykę
  i rozwiązywać okno względem slotu, również podczas nadrabiania przerw.
- **Semantyka `run_now`:** ID zależy od sekundy zegara; retry HTTP w następnej
  sekundzie może uruchomić drugi run. Run powstaje przed zapisem schedule, więc
  nieudany PUT nie oznacza, że nic nie wystartowało. Potrzebny stabilny request ID
  i jawny wynik częściowego powodzenia albo trwała wspólna intencja.
- **Upgrade file store’a:** `Retire` usuwa przyszłe zakończone próby, ale nie
  sprząta terminalnych rekordów już obecnych w starym `attempts.json`.
  PostgreSQL dostał backfill 0004; adapter plikowy potrzebuje odpowiednika.
- **Marimo pod obciążeniem:** ustalić relację timeoutu notebooka, kolejki thread
  poola i TTL `ExecutionMemory`. Aktywny run nie powinien znikać z lookup tylko
  dlatego, że minęło 900 sekund. Przy rozbudowie o repliki pamięć per proces
  przestaje być wystarczająca.
- **Koszt przechowywania:** zmierzyć przyrost artifactów, receipts i stagingu.
  Retencja workflowów nie jest automatycznie garbage collection wszystkich tych
  danych; ewentualny GC musi uwzględniać referencje z żywych runów i datasetów.
- **Flow join:** odrębna funkcja parsera sub-pipeline, nie powód do powrotu do
  ręcznej whitelisty. Realizować pod konkretny przypadek kuracji.

## 6. Korekty dokumentacji

- Zastąpić ogólne „Phases 0–7 built” tabelą capabilities i testów akceptacyjnych.
  §28 nadal rozpoczyna się statusem z 2026-09-05, a później dopisuje kolejne fazy.
- W `KICKOFF.md` zastąpić „one defect” aktualną listą problemów. Kolejność „INSTALL,
  potem canvas” nie odpowiada ryzyku utraty slotów i usuniętych harmonogramów.
- §43.31: doprecyzować, że deterministyczne ID daje deduplikację jednego slotu;
  samo nie zapewnia overlap policy, bezpiecznych edycji ani niezawodnego catch-up.
- §43.32: nazwać `last` ostatnim wynikiem, nie pełną historią; uwzględnić
  nieudany zapis notatki oraz równoległych autorów.
- §43.33: rozdzielić brak utraty danych w DROP COLUMN od kompatybilności
  działających binariów i możliwości rollbacku.
- §20 zestawia proponowane endpointy z istniejącymi. Oznaczyć planowane;
  m.in. `GET /executions` pozostaje w wykazie mimo decyzji, że lista pochodzi z foldu.
- `mode: preview`: rekomenduję usunąć z kontraktu planowanego, dopóki nie ma
  konkretnego wymagania na symulację całego wykonania bez publikacji.
- `WorkflowStore::attempt()`: pozostawić jako obserwowalność kontraktu adaptera
  i opisać ten cel; brak wywołania produkcyjnego nie unieważnia testowalności portu.

## 7. Weryfikacja wykonana w tym review

| Polecenie / próba | Wynik |
|---|---|
| `rtk cargo test -p aiwatcher-execution -p aiwatcher-api -p aiwatcher-server --lib --tests` | 334 passed, 12 suites |
| `rtk just test-postgres` | 5 passed, rzeczywisty lokalny PostgreSQL na porcie testowym 5433 |
| `rtk just flow-check` | format/lint zaliczone; 101 testów, 209 assertions; 2 komunikaty poziomu help |
| `rtk just ml-pipeline-check` | format, ruff, mypy zaliczone; 41 testów; 2 ostrzeżenia deprecation |
| `rtk proxy npm run typecheck` w panelu | zaliczone |
| Dodatkowa próba DST na `Schedule::slots_between` | potwierdzono R5 |
| Sekwencja stale-read/delete/write na `ScheduleStore` | potwierdzono R3 |
| Wymuszony częściowy commit rzeczywistego file store’a, retry i reopen | potwierdzono A1 |

Próby reprodukcyjne używały pamięci i tymczasowych plików; pomocniczy przykład
Rust został usunięty po wykonaniu. Nie wykonano pełnego `just check`, testów
przeglądarkowych ani rzeczywistego rolling upgrade’u klastra. R1, R2, R4, R6 i R7
wynikają z prześledzenia kodu i konfiguracji; nie należy przedstawiać ich jako
zaliczonych testów end-to-end. PostgreSQL testuje bieżący schemat, nie współpracę
starej i nowej wersji w trakcie aktualizacji.
