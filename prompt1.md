# Strumień A — trwałe właścicielstwo i izolacja wykonań IAM-01

Jesteś agentem implementującym strumień A migracji IAM w repozytorium AIWatcher. Wprowadź kod i testy, nie poprzestawaj na propozycji. Pracuj etapami; nie ogłaszaj całego IAM jako ukończonego.

## Przygotowanie i współpraca

- Przeczytaj instrukcje repozytorium, `docs/ux-migration-plan-2026-09-14.md`, szczególnie ostatnie kontynuacje, oraz `crates/aiwatcher-iam/README.md`.
- Przeczytaj ADR_0025, ADR_0026 i odpowiednie ADR-y dotyczące workerów, claimów i credentiali prób. Zweryfikuj aktualny kod: dokumentacja może opisywać wcześniejszy etap.
- Pracuj na osobnej gałęzi/worktree utworzonej z tego samego snapshotu co pozostałe strumienie. Snapshot musi zawierać również dotychczasowe niezacommitowane i nowe pliki; worktree z samego starego HEAD nie wystarczy. Nie resetuj, nie stashuj i nie usuwaj cudzych zmian. Jeśli nie masz właściwego snapshotu, zgłoś brak zamiast odtwarzać poprzednią implementację.
- Wspólne typy to istniejące `aiwatcher_iam::ProjectScope` oraz dokładny `Principal { provider, subject }`. Nie wprowadzaj drugiego modelu zakresu ani principalu opartego na emailu.
- B implementuje katalog artefaktów/cache; C projektowe ewaluacje sędziowskie/zewnętrzne; D migrację. Nie edytuj ich plików. Zapisz potrzebne kontrakty w swoim raporcie, zamiast zakładać, że ich kod już istnieje.

## Punkt wyjścia

Istnieją projektowe deklaracje nagrań, admission i biblioteczny `ScoreExecutor::for_project` z `ProjectAuthority`. Ten ostatni sprawdza grant przed odczytem i przed publikacją, lecz obecnie otrzymuje authority od zaufanego kodu wywołującego. Nie ma trwałego właścicielstwa ani projektowego dispatchera. Projektowy `/start` pozostaje zamknięty. `Artifacts::for_project` izoluje już bajty i receipty, ale nie cały katalog, workflow store ani strumienie.

## Cel i kolejność

1. Ustal i zaimplementuj trwały, zaufany zapis właścicielstwa wykonania: scope, principal oraz powiązanie z uruchamianą deklaracją/planem. Utrwal go atomowo z utworzeniem wykonania, nie jako osobny zapis w object store przed lub po starcie.
2. Tożsamość wykonania musi rozróżniać projekt/organizację oraz zachowywać idempotencję. Zachowaj istniejące globalne ID, historyczne strumienie i hashe planów/definicji. Przypadek identycznego klucza startu w dwóch projektach ma tworzyć niezależne wykonania.
3. Właścicielstwo jest niezmienne. Kolejne komendy, replay, retry i duplikat startu nie mogą przepiąć wykonania do innego principalu lub zakresu. Właścicielstwo nie pochodzi z parametrów planu, `requested_by`, autora deklaracji, nazwy workera czy claimu klienta.
4. Odizoluj workflow history/projection, odczyty i komendy oraz wybór claimów. Globalny reactor, worker, launcher, timer, outbox i inne istniejące procesory nie mogą przypadkowo obsłużyć projektowego wykonania. Zinwentaryzuj te wejścia; brak obsługi oznacza odmowę, nie globalny fallback.
5. Dopiero na tym fundamencie przygotuj dispatcher pobierający authority z trwałego zapisu. Sprawdzaj aktualne granty przy podejmowaniu pracy i przed publikacją; błąd IAM nie może oznaczać zgody. Nie omijaj autoryzacji przez wcześniejszy cache lookup.
6. Zdefiniuj politykę odebrania dostępu. Wykorzystaj istniejące stop/cancel/Committing: dopuszczony commit ma jawne reguły zakończenia, a następna próba ponownie wymaga dostępu. Nie deklaruj transakcji IAM + object store, której nie ma.

Pierwszym samodzielnie odbieralnym etapem jest trwałe właścicielstwo wraz z odmową obsługi przez stare globalne ścieżki. Jeśli pełny dispatcher przekracza bezpieczny zakres tej iteracji, dostarcz ten etap i nazwij pozostałe blokady.

## Bramka integracji — obowiązkowa

Nie wystawiaj ani nie rejestruj projektowego `/start`, dopóki nie zostały zintegrowane i przetestowane:
- katalog/lineage/cache z B;
- autoryzacja wszystkich odczytów, komend, artefaktów, workerów i live streams;
- bezpieczna ścieżka faktów/outbox/ingestu/projekcji, bez ujawnienia ich globalnemu API;
- granty i ich cofnięcie na rzeczywistej ścieżce wykonania.

Nie maskuj brakującej izolacji samym prefiksem URL lub filtrem panelu. Jeśli zapis projektowego runu mógłby już uruchomić globalny outbox/reactor, nie dodawaj produkcyjnego wywołania takiego zapisu. Biblioteczny test nie jest zgodą na otwarcie trasy.

## Własność plików

- Głównie `crates/aiwatcher-execution/src/` dotyczące store, handlera, claimów, startu i reactora oraz ich testy. Katalog `src/artifact/` należy do B.
- Zmiany w `crates/aiwatcher-server/src/execution/scoring/project.rs` i nowym dispatcherze, jeśli potrzebne. Wspólny `scoring.rs` wymaga uzgodnienia z C; preferuj osobny moduł.
- Nie edytuj rejestrów ewaluacji, modułów migracji ani UI.
- Nie zmieniaj istniejących migracji SQL. Nowe migracje są addytywne; rozważ rolling upgrade i rollback starego binarium.
- Nie regeneruj kontraktu HTTP bez rzeczywistej zmiany API. Jeśli zmiana jest konieczna, uzgodnij integrację z C; wygenerowanych plików nie edytuj ręcznie.

## Testy i odbiór

- Wspólny kontrakt wszystkich dotkniętych adapterów workflow store: atomowość, restart, idempotencja, niezmienność owner/scope, kolizje identycznych kluczy między projektami i globalnym zakresem.
- Próby odczytu/komendy/claimu po ID cudzego wykonania; stary globalny claimant nie bierze projektowej pracy. Także pod/claim-by-key i próby podstawienia authority.
- Cofnięcie i wygaśnięcie grantu bez odnawiania sesji, odmowa IAM i transient failure, retry po utracie dostępu, cache nie omija IAM.
- Jeśli zmieniasz PostgreSQL, uruchom kontrakt i upgrade na osobnej, jednorazowej bazie testowej. Nie używaj znalezionej bazy projektu ani produkcyjnego klastra. Jawnie opisz nieuruchomione adaptery/feature'y.
- Uruchom odpowiednie testy execution/server, Clippy z `-D warnings`, `cargo fmt --all --check` i `git diff --check`. Gdy dotykasz HTTP, również testy API oraz generację/sprawdzenie OpenAPI i build panelu.

## Raport

Zapisz `docs/iam-parallel-A-report.md`: decyzje i ewentualny ADR, zmienione pliki, kontrakt dla B/C/D, wykonane polecenia z wynikami, ograniczenia oraz dokładna bramka do następnego etapu. Dla zmiany architektury utwórz osobny dokument decyzji strumienia zamiast wybierać numer ADR kolidujący z innym agentem.

Nie edytuj wspólnego planu migracji ani README IAM podczas równoległej pracy — integrator scali raporty. W odpowiedzi końcowej oddziel działający kod od przygotowanego kontraktu i od niewdrożonych elementów.
