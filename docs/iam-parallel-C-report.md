# Strumień C — projektowe pomiary sędziowskie i zewnętrzne (IAM-01)

Raport gałęzi równoległej. **Nie edytowano wspólnego planu migracji ani
`crates/aiwatcher-iam/README.md`** — integrator scali raporty.

Punkt wyjścia: snapshot roboczy z niezacommitowanymi zmianami i nowymi plikami
(`HEAD = 87b1f88` plus praca w toku pozostałych strumieni). Niczego nie
resetowano, nie stashowano ani nie nadpisywano.

## 1. Co zostało ukończone

Oba piony są w kodzie i w testach, w warstwie rejestru i resolvera. **Żadna
ścieżka wykonania nie została otwarta**: nie ma projektowego `/start`, nie
zarejestrowano żadnego executora, a produkcyjny `ScoreExecutor` nadal odmawia
projektowego rejestru jako `FailureClass::Policy` zanim przeczyta deklarację.

### Pion 1 — projektowy pomiar sędziowski nad nagraniem i natywną kohortą

Działa pełny łańcuch, w całości wewnątrz jednego projektu: rubryka → karta z
`Scorer::Judge` → wynik producenta → oceny ludzi → zamrożona kalibracja →
nagranie → natywna kohorta → deklaracja → admission (project admin) →
pomiar z lokalnym sędzią → publikacja dowodu z agreement.

### Pion 2 — projektowe metryki frameworkowe (zewnętrzne scorery)

Ten sam wzorzec: karta projektu może nazwać metrykę adaptera, a publikacja
przypina do wersji karty opis z **deploymentowego** katalogu (release, model,
unit, direction, aggregation, `reads`, parametry). Odpowiedzi usługi są trzymane
pod deklaracją w projekcie. Kalibrowana metryka frameworkowa wymaga zbioru
zamrożonego w tym samym projekcie.

## 2. Inwentaryzacja zależności: zasób projektu kontra konfiguracja deploymentu

| Zależność | Rodzaj | Gdzie leży | Stan przed | Stan po |
|---|---|---|---|---|
| Rubryka (wersja) | zasób projektu | `…/registry/evaluation-rubrics/` | izolowana (5. granica) | bez zmian |
| Karta (`Scorer::Judge` / `Scorer::External`) | zasób projektu | `…/registry/evaluation-scorecards/` | izolowana; karta zewnętrzna **odrzucana** | karta zewnętrzna dopuszczona i przypięta do katalogu |
| Oceny ludzi (standing assessments) | zasób projektu | `…/registry/evaluation-assessments/` | izolowane | bez zmian |
| Wynik producenta (źródło kalibracji) | zasób projektu | `…/registry/evaluations/` | izolowany (11. granica) | bez zmian |
| Zbiór kalibracyjny | zasób projektu | `…/registry/evaluation-judges/calibrations/` | izolowany (12. granica) | bez zmian |
| **Konfiguracja sędziego przypięta przez kontekst** | zasób projektu (adresowana treścią) | `evaluation-judges/settings/<digest>.json` | **odmawiana przez `ProjectStore`** | dopuszczona w prefiksie projektu |
| **Przechowywane odpowiedzi sędziego** | zasób projektu | `evaluation-judges/replies/<declaration>/<question>.json` | **odmawiane** | dopuszczone w prefiksie projektu |
| **Przechowywane odpowiedzi scorer service** | zasób projektu | `evaluation-scorers/replies/<declaration>/<question>.json` | **odmawiane** | dopuszczone w prefiksie projektu |
| Nagranie odpowiedzi | zasób projektu | `…/registry/evaluation-recordings/` | izolowane (9. granica) | bez zmian |
| Natywna kohorta | zasób projektu | dataset/anotacje `for_project` | izolowana (8. granica) | bez zmian |
| Approval + pliki pakietu | zasób projektu | `…/registry/evaluations/approvals/`, `evaluation-scopes/<o>/<p>/bundles/` | izolowane (10./11.) | bez zmian |
| **Katalog możliwości scorer service** | **konfiguracja deploymentu** | `evaluation-scorers/catalog.json` (globalny klucz) | niedostępny z projektu | **odczyt** z projektu, zapis nadal tylko dla roli `work` |
| Profil/model/adres/credential sędziego | **konfiguracja deploymentu** | `AIWATCHER_JUDGE_PROVIDER`, `AIWATCHER_JUDGE_URL` | — | bez zmian; deklaracja nazywa tylko profil i model |
| Adres/token scorer service | **konfiguracja deploymentu** | `AIWATCHER_SCORER_URL`, token | — | bez zmian; nic z tego nie trafia do projektu ani do planu |

Reguła, którą to utrwala: **wszystko, co jest orzeczeniem o treści, jest zasobem
projektu; deploymentowe pozostaje tylko to, co opisuje usługę, którą ten
deployment uruchamia.** Katalog jest opisem usługi — nie zawiera danych
projektu, adresu ani poświadczenia — a karta projektu przypina to, co katalog
mówił w chwili publikacji.

## 3. Zmiany w kodzie

### `crates/aiwatcher-evaluation/src/store.rs`
Trzy rodziny kluczy dostały nazwane stałe (`JUDGE_SETTINGS`, `JUDGE_REPLIES`,
`EXTERNAL_REPLIES`), z których budowane są ich klucze — tak, żeby lista rodzin
w `scope.rs` i konstruktory kluczy nie mogły się rozjechać.

### `crates/aiwatcher-evaluation/src/scope.rs`
- `AUTHORED` i `MEASURED` to teraz dwie nazwane listy rodzin zamiast dwóch
  literałów w środku `key()`. `MEASURED` zyskało trzy rodziny z tabeli powyżej.
- `DEPLOYMENT_READS` — jeden klucz (`evaluation-scorers/catalog.json`),
  **wyłącznie w `get`**, przez osobne `read_key()`. `put`, `create`, `list` i
  `delete` nadal idą przez `key()`, więc projekt nie zapisze, nie wylistuje i
  nie usunie katalogu. Listowanie `evaluation-scorers/` z projektu jest nadal
  odmawiane.

### `crates/aiwatcher-evaluation/src/registry.rs`
- `declare_scoring_run` rozwiązuje **najpierw** oba zbiory kalibracyjne
  (wspólne `declared_calibration`: ten rejestr, ta nazwa, te rubryki, nie z
  własnego wyniku), **potem** wykonuje kontrolę projektową, i dopiero na końcu
  zapisuje `keep_settings`. Odrzucona deklaracja projektowa nadal nie zostawia
  konfiguracji sędziego (istniejący test to sprawdza).
- `check_project_declaration` przyjmuje rozwiązane zbiory; przestaje odrzucać
  sędziego, zewnętrzną kalibrację i kartę pytającą usługę; dalej wymaga
  nagrania i natywnej kohorty; **jawnie odmawia zbioru zamrożonego z dowodów
  rozmów** (`from_archive`) po obu stronach.
- `scoring_run_view` woła tę samą kontrolę po rozwiązaniu kalibracji, więc
  pełny widok i deklaracja nie mogą orzec inaczej.
- `admit_scoring` (gałąź projektowa) rozdzielono na dwie reguły: rodzaj kohorty
  (curation/annotations) i **wywiedziony** `reads_archive` sędziego oraz pinu
  metryk. Drugą sprawdza się z kontekstu, więc ręcznie napisany kontekst jest
  odrzucany tak samo jak wywiedziony.
- `publish_scorecard` nie odrzuca już kart zewnętrznych w projekcie —
  `declared_against` przypina opis z katalogu (brak katalogu = odmowa).
- `record_scorer_catalog` odmawia w zakresie projektowym, z nazwanym powodem.

### `crates/aiwatcher-server/src/evaluation/project_evidence.rs`
`ProjectEvidence::resolve` dopuszcza sędziego i zewnętrzną kalibrację
**wyłącznie** dla pomiaru tego deploymentu (`scored_here`) nad natywną kohortą,
i odmawia, gdy którykolwiek `reads_archive` jest prawdziwy. Reszta odmów bez
zmian: producencki sędzia nadal nie ma adaptera nigdzie (ADR_0030), archiwum
rozmów i katalog hosta pozostają odcięte.

## 4. Granice, które zostały zamknięte (i dlaczego)

- **Brak projektowego `/start`, generowania odpowiedzi, approval lines,
  globalnych obserwacji i integracji strumieni.** Nie dodano żadnej trasy HTTP.
- **Archiwum rozmów pozostaje zamknięte.** Kohorta rozmów jest odrzucana przez
  rodzaj datasetu, a zbiór kalibracyjny z dowodów rozmów — jawnie, po nazwie, w
  trzech miejscach: deklaracja, admission rejestru i resolver serwera.
- **Odpowiedzi.** `JudgeReply::kept` i `KeptScore` są bez zmian: trwale zapisana
  odpowiedź sędziego to wartość ze skali (albo nazwany zastępnik), a odpowiedź
  scorera to liczba albo powód porażki. Test czyta zapisane bajty i sprawdza, że
  zdania modelu w nich nie ma. Przypięta konfiguracja sędziego zostaje.
- **Ponowne użycie odpowiedzi** jest możliwe tylko w obrębie projektu i
  deklaracji: klucz to `<projekt>/…/replies/<declaration>/<digest pytania>`, a
  deklaracja jest lokalna. Cache odpowiedzi nie jest ścieżką obejścia kontroli
  dostępu, bo sięga się po niego dopiero po tym, jak caller uzyskał projektowy
  rejestr — a ten powstaje tylko z aktualnego grantu (HTTP) albo z jawnego
  `ProjectAuthority` (biblioteka).
- **Executor nie został podłączony.** Odmowa nieobsługiwanego runtime'u jest
  jawna i pokryta testem w obu pionach.

## 5. Kontrakt dla A

Czego A potrzebuje, żeby otworzyć projektowe wykonanie pomiaru sędziowskiego
lub zewnętrznego — po spełnieniu własnej bramki (trwałe właścicielstwo,
izolacja historii/komend/claimów/artefaktów/strumieni):

1. **Jak uzyskać scoped rejestr.** Bez zmian względem poprzedniego etapu:
   `Registry::for_project_evidence(scope)` na rejestrze instancji. Jest
   idempotentny dla tego samego zakresu i odmawia przepięcia. Zwrócony rejestr
   sam **nie autoryzuje**: `Registry::project_scope()` mówi tylko, do jakiego
   magazynu jest związany.
2. **Zależności rozwiązują się same.** Deklaracja, admission i publikacja
   pomiaru sędziowskiego/zewnętrznego nie wymagają od dispatchera żadnego
   dodatkowego uchwytu: rubryki, karty, oceny, kalibracje, konfiguracje i
   odpowiedzi są pod prefiksem projektu, a katalog scorerów jest deploymentowy i
   czytany przez rejestr. Dispatcher nie przekazuje adresu ani tokenu usługi.
3. **Co A musi zmienić, żeby wykonanie było możliwe** (oba miejsca należą do A):
   - `crates/aiwatcher-server/src/execution/scoring/project.rs` —
     `ProjectAuthority::authorize` dopuszcza dziś wyłącznie
     `RuntimeBinding::ScoreEvaluation`. Plan pomiaru sędziowskiego to
     `RuntimeBinding::JudgeEvaluation`, a karty z metryką frameworkową —
     `RuntimeBinding::ExternalEvaluation` (`aiwatcher-api/src/scoring.rs`,
     `plan_for`). Warunek powinien dopuszczać te trzy warianty, nadal
     porównując `spec.declaration` z przypiętą deklaracją.
   - `crates/aiwatcher-server/src/execution/scoring.rs` — pierwszy strażnik w
     `ScoreExecutor::execute` odmawia, gdy `project.is_some()` i ustawiony jest
     sędzia, scorer **lub** `artifacts`. Odmowa dla `artifacts` musi zostać
     (globalny czytnik artefaktów). Odmowa dla sędziego i scorera może zostać
     zdjęta, ponieważ oba są klientami deploymentu, a nie cudzymi danymi — pod
     warunkiem, że `ProjectAuthority::authorize` już dopuszcza dany runtime.
     Sugerowany kształt: refuse tylko `self.artifacts.is_some()`.
   - Pozostała część `execute` nie wymaga zmian: `prepare` już woła
     `authority.authorize(...)` przed odczytem deklaracji, a druga kontrola IAM
     przed `Committing` już tam jest.
4. **Czego A nie musi budować.** Drugiego modelu scorerów, drugiego magazynu
   odpowiedzi ani projektowego katalogu usług. Nie ma też potrzeby przekazywania
   `subject` — autorem deklaracji jest `declared_by` zapisany w rejestrze i jest
   to pochodzenie, nie uprawnienie.
5. **Czego nie wolno.** Zarejestrowania `ScoreExecutor::for_project` w
   `ExecutorRegistry` przed bramką: `Reactor` robi cache lookup **przed**
   wykonaniem, a projektowy wynik nie jest cacheowalny właśnie dlatego, że
   wcześniejszy wynik nie może zastąpić bieżącej autoryzacji.

## 6. Nowe layouty dla D

Do inwentaryzacji migracji dochodzą trzy rodziny pod istniejącym prefiksem
`evaluation-scopes/<organization>/<project>/registry/`:

| Klucz | Zawartość | Uwagi dla migracji |
|---|---|---|
| `evaluation-judges/settings/<sha256>.json` | kanoniczne bajty `JudgeSettings` | adresowane treścią; `create`-only, idempotentne; przypięte przez `context.judge.configuration.digest` |
| `evaluation-judges/replies/<declaration>/<sha256 pytania>.json` | `JudgeReply` **w postaci `kept`** | `create`-only, pierwszy zapis wygrywa; **nie jest to indeks pochodny** — jego utrata zmienia bajty ponownego pomiaru, więc nie wolno go pominąć jako „odtwarzalny cache" |
| `evaluation-scorers/replies/<declaration>/<sha256 pytania>.json` | `KeptScore` | jak wyżej |

Poza projektem, bez zmian: `evaluation-scorers/catalog.json` pozostaje jednym
obiektem deploymentu, nadpisywanym przez rolę `work`. **Nie należy go kopiować
do zakresu projektu** — projekt go czyta, a kart nie przelicza się przy migracji
(opublikowana wersja karty już przypina to, co katalog mówił).

Nie zmieniono adresów treści, identyfikatorów ani historycznych referencji.
Identyczna deklaracja w dwóch projektach ma ten sam `id`, a niezależnego
pierwszego autora — test krzyżowo-organizacyjny to sprawdza.

## 7. Zmienione i dodane pliki

Zmodyfikowane:
- `crates/aiwatcher-evaluation/src/store.rs`
- `crates/aiwatcher-evaluation/src/scope.rs`
- `crates/aiwatcher-evaluation/src/registry.rs`
- `crates/aiwatcher-evaluation/tests/scope.rs`
- `crates/aiwatcher-server/src/evaluation/project_evidence.rs`
- `crates/aiwatcher-server/tests/evaluation.rs` (dwa `mod`)
- `crates/aiwatcher-server/tests/evaluation/project_declarations.rs` (asercja
  odmowy sędziego zmienia się na odmowę po nazwie zależności)
- `crates/aiwatcher-server/tests/evaluation/project_results.rs` (asercja o
  odpowiedziach sędziego przechodzi na rejestr `authored`)
- `crates/aiwatcher-api/tests/http/iam.rs` (jeden `mod`)

Dodane:
- `crates/aiwatcher-server/tests/evaluation/project_judge.rs`
- `crates/aiwatcher-server/tests/evaluation/project_external.rs`
- `crates/aiwatcher-api/tests/http/project_judged.rs`
- `docs/iam-parallel-C-report.md`

**Nie dotknięto** `crates/aiwatcher-server/src/execution/scoring.rs`,
`crates/aiwatcher-server/src/execution/scoring/project.rs`, workflow store'a,
dispatchera, katalogu artefaktów, modułów migracji, panelu, kontraktu OpenAPI
ani wygenerowanego klienta.

## 8. Testy

Nowe (7 scenariuszy):

`aiwatcher-server --test evaluation` — produkcyjny resolver `LocalSource` +
plikowy/pamięciowy object store:
1. `a_project_judged_run_reads_only_its_own_rubric_card_people_and_settings` —
   pełny pion sędziowski: lokalne zależności, przypięta konfiguracja pod
   prefiksem projektu i **brak jej w magazynie instancji**, brak dostępu u
   sąsiada i w instancji, admission przez project admin, odmowa executora,
   pomiar z lokalnym sędzią, agreement i `reproducible: false`, wynik tylko w
   projekcie, oraz **druga organizacja z identycznymi bajtami**: te same adresy
   (rubryka, kalibracja, deklaracja), inny pierwszy autor, żadnego przecieku.
2. `kept_project_replies_answer_a_retry_without_asking_again_and_hold_no_words`
   — retry nie pyta ponownie i publikuje te same bajty; zapisane odpowiedzi nie
   zawierają zdania modelu i mają wyłącznie `{"value": …}`; nic nie leży w
   prefiksie instancji ani sąsiada; rejestr `authored` nie otwiera deklaracji.
3. `a_project_judge_is_refused_the_archive_a_neighbours_dependency_and_a_corrupted_pin`
   — zbiór z dowodów rozmów odrzucony po nazwie; obca karta; **rubryka bez ocen
   ludzi** (`no human judgement`); uszkodzona i usunięta konfiguracja po
   admission → odczyt kończy się błędem, nigdy `admitted: true`; kontekst
   producenckiego scorera nie przechodzi.
4. `a_project_card_pins_the_deployment_catalog_and_keeps_its_replies_in_the_project`
   — karta bez katalogu odrzucona, projekt nie zapisze katalogu, publikacja
   przypina release/unit/direction/aggregation, `measured_by` w manifeście,
   odmowa executora, pomiar, izolacja odpowiedzi i wyniku, retry bez pytania,
   reopen magazynu, nowy katalog nie zmienia opublikowanej karty.
5. `a_project_framework_metric_is_refused_an_unknown_name_and_a_neighbours_card`
   — metryka spoza katalogu; karta sąsiada; ten sam adres karty po lokalnej
   publikacji nadal nie wystarcza; approval nie przenosi się; **niezgodność
   release (2.2.59 ↔ 3.0.0) nazwana po obu stronach**; kalibrowana metryka
   wymaga lokalnego zbioru.

`aiwatcher-api --test http` — podpisane sesje OIDC, lokalne discovery/JWKS,
pamięciowy IAM:
6. `a_judged_declaration_resolves_its_rubric_card_and_people_in_one_project_only`
   — deklaracja sędziowska przez trasy projektowe, idempotencja, przypięty
   digest konfiguracji, brak `reads_archive`, izolacja od sąsiada/innej
   organizacji/instancji, **brak projektowego `/start` i brak globalnego
   fallbacku startu**, admission przez project admina, `no-store`.
7. `a_judged_declaration_needs_a_current_grant_the_mutation_header_and_a_project_admin`
   — rola instancji nie zastępuje projektu (odczyt, deklaracja, approval), brak
   nagłówka `X-AIWatcher-IAM`, okna grantu (przed startem / po `edit_until` /
   po `read_until`), editor nie awansuje do admina, upload kończący się po
   utracie grantu nie zapisuje, cofnięcie grantu, brak uwierzytelnienia, brak
   IAM.

Testy zaktualizowane: `tests/scope.rs` (odczyt katalogu deploymentu i odmowa
jego zapisu z projektu, publikacja karty zewnętrznej po zapisie katalogu przez
deployment), `project_declarations.rs`, `project_results.rs`.

### Wyniki

```
cargo test -p aiwatcher-evaluation      96 passed   (79 lib + 11 manifest + 6 scope)
cargo test -p aiwatcher-api            268 passed   (23 lib/kontrakt + 245 HTTP)
cargo test -p aiwatcher-server         305 passed, 1 ignored
                                       (147 lib + 153 evaluation + 5 project_artifacts;
                                        pominięty to istniejący test RustFS/S3 z `#[ignore]`)
cargo clippy -p aiwatcher-evaluation -p aiwatcher-api -p aiwatcher-server \
    --all-targets -- -D warnings       powodzenie
cargo fmt --all --check                powodzenie
git diff --check                       powodzenie
just openapi-check                     ✓ contracts/openapi.json is current
```

Przyrost względem snapshotu wejściowego: **+5 testów integracyjnych serwera** i
**+2 scenariusze HTTP**; pozostałe pakiety mają te same liczby przy zmienionej
treści dwóch asercji.

## 9. Czego nie zrobiono — jawnie

- **Nie uruchomiono żadnego E2E z Authentikiem, PostgreSQL, S3/RustFS ani
  workerem.** HTTP korzysta z pamięciowego IAM i podpisanych sesji z lokalnym
  discovery/JWKS; testy serwera z plikowego i pamięciowego object store'a;
  sędzia i scorer service to kontrolowane stuby w procesie testu — żaden płatny
  provider ani żadne rzeczywiste dane użytkownika nie były używane.
- Nie zmieniono kontraktu HTTP, więc nie regenerowano OpenAPI ani klienta
  panelu i nie uruchamiano buildu panelu (`openapi-check` potwierdza aktualność
  kontraktu). Selektory UI pozostają nieaktywne.
- Nie uruchamiano `services/scorers` — nie zmieniano tej usługi.
- Nie ma projektowego wykonania: pomiar w testach jest napędzany bezpośrednio
  (te same publiczne funkcje składania, których używa executor), a produkcyjny
  executor jest w obu pionach sprawdzony jako odmawiający.
- Kontrola grantu pozostaje dopuszczeniem operacji, a nie wspólną transakcją
  IAM i object store'a: cofnięcie dostępu w trakcie już dopuszczonego zapisu go
  nie przerywa. To niezmieniony kontrakt poprzednich etapów.
- Retencja deklaracji, konfiguracji i przechowywanych odpowiedzi nadal nie ma
  osobnej polityki. Odpowiedzi rosną z liczbą pytań na deklarację; przy
  otwieraniu wykonań warto to nazwać jako oddzielną bramkę.
- Nie dotknięto approval lines, generowania odpowiedzi, `experiments`, gate'u
  ani obserwacji wariantów w zakresie projektu — pozostają zamknięte.
