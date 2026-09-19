# Trzy strumienie naraz — etap D, katalog silników, porządki

Data: 19.09.2026. Ciąg dalszy [podziału z 18.09](parallel-streams-2026-09-18.md),
którego wszystkie osiem wierszy jest dowiezionych. Ten dokument dzieli to, co
zostało **poza IAM-03**, na strumienie, które mogą iść równolegle, i mówi
wprost, gdzie się zderzą. Prompty są na końcu, jeden na strumień, każdy
samodzielny — można je odpalić w osobnych sesjach.

**Warunek wstępny dla wszystkich: IAM-03 prowadzi osobna sesja i pracuje w tym
samym drzewie roboczym.** W chwili pisania ma brudnych szesnaście plików kodu, w
tym dziesięć modułów `crates/aiwatcher-api/src/**` — z `context.rs`, `auth.rs` i
`error.rs` włącznie, czyli dokładnie to, przez co przechodzi każda nowa trasa.
Lista rośnie, więc **przeczytaj `git status` na starcie swojej sesji**; tabela
niżej mówi, co jest czyje. Sekcja „Gdzie się zderzą"
mówi, co z tym zrobić; **worktree na strumień jest tu mocniejszą rekomendacją
niż 18.09**, bo tamte strumienie dzieliły pliki, a ten dzieli je z sesją, która
zmienia ich kształt.

## Mapa

| Strumień | Co robi | Zależy od | Kiedy |
|---|---|---|---|
| **FTI-D** | Etap D z planu FTI: D2 alerty okienkowe, D3 diagnostyka zdolności, D4 serwerowe widoki i raport | — | **teraz** |
| **FLOW-02** | `prompt` i `variant` jako kolumny katalogu silników zapytań | — | **teraz** |
| **PORZ-02** | Limit komentarzy w CI i trzy dokumenty, które kłamią o stanie | — | kiedykolwiek |
| *(IAM-03/M0)* | *Pierwsze demo klienckie — **nie** ten dokument* | — | *w toku* |

**FTI-D da się rozbić na trzy sesje**, bo D2, D3 i D4 nie zależą od siebie: D2
to `aiwatcher-alerts` plus watcher w `aiwatcher-server`, D3 to jedna trasa obok
`system.rs`, D4 to nowy rejestr autorski z własną rodziną tras. Zderzają się
tylko na `routes.rs`, `openapi.rs`, `lib.rs`, `navigation.ts` i kontrakcie —
czyli tam, gdzie reguła niżej i tak obowiązuje. Prompt jest napisany tak, żeby
sesja mogła wziąć całość albo jedną literę; jeśli bierze jedną, czyta tylko jej
akapit i „Co obowiązuje każdy strumień".

## Gdzie się zderzą, i co z tym zrobić

Najpierw to, czego **nie dotyka nikt z tego dokumentu**, bo ma to sesja IAM-03:

| Ścieżka | Dlaczego |
|---|---|
| `crates/aiwatcher-auth/**`, `crates/aiwatcher-iam/**` | D4 z IAM-03: rola instancyjna przestaje być domyślna (`Identity::role() -> Option<Role>`) |
| `crates/aiwatcher-api/src/auth.rs`, `context.rs`, `error.rs` | tam siedzi odmowa dla kogoś bez roli instancyjnej |
| `crates/aiwatcher-api/src/{annotations,artifacts,conversations,executions,imports,schedules}.rs`, `integrations/hubs.rs` | trasy instancyjne, którym IAM-03 dokłada sprawdzenie roli |
| `crates/aiwatcher-api/tests/http/iam.rs`, `tests/http/instance_reach.rs` | testy tamtej pracy |
| `apps/panel/src/features/account/**`, `app/reach-notice.tsx`, `app/scope-selector.tsx` | panel IAM |
| `deploy/helm/**`, `scripts/iam-permission-check.py`, `docs/iam-03-projects.md` | M0: chart, matryca, plan |
| `crates/aiwatcher-migration/**` | IAM-03/M5 |

**Konsekwencja, którą łatwo przegapić, a jest najdroższa:** IAM-03 zmienia
właśnie to, **czym jest sprawdzenie roli na trasie instancyjnej**. Każda nowa
trasa z tego dokumentu (D3, D4) musi wziąć swój `Caller` i swoją odmowę z tego,
czym `context.rs` i `auth.rs` będą **po** scaleniu tamtej pracy, a nie z ich
dzisiejszej kopii. Pisz trasę tak, jak piszą ją sąsiednie moduły, a przed
scaleniem przebazuj i przeczytaj różnicę w tych dwóch plikach — nie kopiuj
wzorca sprzed zmiany.

Pliki wspólne w obrębie tego dokumentu, reguła bez zmian od 18.09:

| Plik | Kto go dotknie | Jak nie zrobić konfliktu |
|---|---|---|
| `contracts/openapi.json`, `apps/panel/src/api/generated/**` | FTI-D | **Nigdy ręcznie.** `just openapi` na końcu, w osobnym commicie. Przy konflikcie: swoja wersja + regeneracja |
| `crates/aiwatcher-api/src/routes.rs`, `src/openapi.rs` → `document()`, `src/lib.rs` | FTI-D (D3, D4) | Jedna linia **na końcu** listy, moduł alfabetycznie. Nie przestawiaj cudzych |
| `crates/aiwatcher-api/src/openapi.rs` → `components(schemas(…))` | FTI-D | **Jedna globalna przestrzeń nazw.** Typ, którego nazwy mogłaby użyć inna domena, dostaje `#[schema(as = …)]` — `Withdrawal` raz już cicho nadpisał cudzy |
| `apps/panel/src/app/navigation.ts`, `commands.ts` | FTI-D | Jeden wpis na końcu; `commands.test.ts` sprawdza je wobec schematu zod trasy. Nowy obszar deklaruje też `reach` |
| `docs/FTI_IMPLEMENTATION_PLAN.md` | FTI-D | Zrobiona praca **znika** z tego pliku; zostaje wyłącznie otwarte |
| `docs/ux-migration-plan-2026-09-14.md`, `docs/parallel-streams-2026-09-18.md` | PORZ-02 | **Dopisz sekcję na końcu pliku.** Konflikt append-only rozwiązuje się w sekundę |

**Katalogi są własnością strumienia.** `crates/aiwatcher-alerts/`,
`crates/aiwatcher-server/src/alerts/`, `apps/panel/src/features/alerts/` i nowy
katalog raportów należą do FTI-D; `services/query/**` do FLOW-02.

**Checkout albo worktree.** `cargo`, `just openapi` i `npm run build` wołane
naraz przez dwie sesje depczą sobie po blokadach — umówcie się, kto trzyma
toolchain. FLOW-02 i PORZ-02 są tanie w worktree; FTI-D płaci zimnym `cargo
build`.

**Commituj po ścieżkach, nigdy `git add -A`.** Jeden krótki nagłówek
konwencjonalny, bez trailera współautora. Przed commitem sprawdź `git status` —
w drzewie są cudze zmiany i one mają zostać, gdzie są.

## Co obowiązuje każdy strumień

- **Nowy zasób autorski dostaje swoją zakresową rodzinę tras od urodzenia** —
  `ProjectAuthorization` plus `<prefix>/scopes/<organization>/<project>/registry/`,
  `X-AIWatcher-IAM: 1` na mutacji i **ponowne** sprawdzenie grantu po odebraniu
  ciała, `Cache-Control: no-store`, zakres **nigdy** w hashu treści, rejestr
  odmawiający przepięcia do innego projektu. Dotyczy D4 wprost. Wzór:
  `crates/aiwatcher-prompts` + `crates/aiwatcher-api/src/prompt_scope.rs`,
  najświeższy pełny przykład: `crates/aiwatcher-labs`.
- **Nie opisuj wdrożenia jako multi-tenant safe.** Do M7 z IAM-03 to nieprawda.
- **Nie zmieniaj istniejących migracji SQL.** Nowe są addytywne, z rolling
  upgrade i rollbackiem starego binarium.
- **Nie zakresuj alertów, logów ani foldów.** Alerty czytają stronę globalną
  świadomie (jeden kanał na wdrożenie, ADR_0035); zakresowanie to IAM-03/M6–M7.
- **Nie pushuj.** Na `main` jest 118 commitów przed `origin/main` (15.09) — to
  decyzja właściciela repozytorium, nie sesji.
- **Nigdy nie wypełniaj ekranu danymi zastępczymi.** Slot bez kontraktu mówi
  `no contract`; prawdopodobna atrapa czyta się jak działające oprogramowanie.

Testy na koniec każdego kroku: panel `npm run test`, `npm run lint` i
`npm run build` (to `check:architecture` + `vite build` + pełne `tsc -b`),
przejście klawiaturą, 375 px i szeroki ekran, motyw jasny i ciemny, bez
poziomego przewijania strony. Rust, przy dotknięciu któregokolwiek crate'a:
`cargo test --workspace --all-targets`,
`cargo clippy --workspace --all-targets --all-features -- -Dwarnings`,
`cargo fmt --all --check`, `git diff --check`,
`python3 scripts/check-rust-boundaries.py`. Po zmianie trasy lub typu:
`just openapi` i commit obu stron. PostgreSQL wyłącznie na osobnej, jednorazowej
bazie. **Jawnie wypisz, czego nie uruchomiłeś.**

`python3 scripts/lint-comments.py` **wychodzi dziś 1** — czternaście bloków nad
limitem, i to jest praca PORZ-02. Pozostałe strumienie mają nie dokładać do tej
listy; liczbę przed i po widać uruchomieniem skryptu.

Środowisko lokalne, jeśli potrzebujesz tożsamości: `just authentik-up`,
`just authentik-seed`, `just authentik-secret`, `just postgres-up`,
`just run-sso-iam`, `just panel`. Ludzie: `teacher` / `teacher-dev` (admin
instancji), `student` / `student-dev` (viewer). Druga sesja bez drugiej
przeglądarki: `python3 scripts/sso-session.py <kto>`.

---

## Prompt — FTI-D: alerty okienkowe, diagnostyka i raport

> Pracujesz w repozytorium AIWatcher nad **etapem D z planu FTI** — D2, D3 i D4.
> Przeczytaj `CLAUDE.md`, `docs/FTI_IMPLEMENTATION_PLAN.md` (sekcja „Etap D"
> oraz „41. Runda świadka… — co zostaje"),
> `docs/ADR/ADR_0035_ALERT_DELIVERY.md`, decyzję 27 i sekcję „Saying something
> out loud" w `CLAUDE.md`, `crates/aiwatcher-alerts/` (zwłaszcza `rule.rs`,
> `delivery.rs`, `signal.rs`), `crates/aiwatcher-server/src/alerts/`,
> `apps/panel/CLAUDE.md` oraz w tym pliku sekcje „Gdzie się zderzą" i „Co
> obowiązuje każdy strumień". Etap D bierze **pierwszy wolny numer sekcji po
> 41**. Trzy litery nie zależą od siebie — możesz wziąć całość albo jedną.
>
> **Punkt wyjścia.** D1 stoi: dwa wyzwalacze (`ExecutionFailed`,
> `EvaluationRegressed`), jeden kanał z konfiguracji, klucz dedup
> `sha256(wersja reguły ‖ co się stało)` zakładany przez `ObjectStore::create`,
> retry z `aiwatcher_jobs::after_failure`, pierwszy przebieg nie podnosi
> niczego. Obie dzisiejsze reguły czytają **cudzy werdykt** — fakt z logu i
> słowo bramki. D2 jest pierwszą, która **decyduje sama**, i to jest cała jego
> trudność.
>
> **D2 — alerty okienkowe metryk.** Reguła niesie okres, minimalną próbę,
> cooldown, zachowanie przy braku danych i odzyskanie poprawnego stanu.
> Rozstrzygnij **zanim** cokolwiek napiszesz: czym jest „to samo zdarzenie" dla
> warunku, który trwa. Klucz dedup D1 nazywa wystąpienie, a próg przekroczony
> przez godzinę to nie jedno wystąpienie w oczach zegara sprawdzającego co
> minutę. Rekomendacja: kluczuj **zamkniętym okresem** foldu, a nie chwilą
> sprawdzenia — wtedy dedup D1 działa bez zmian, a cooldown jest tym, czym ma
> być, zamiast być ukrytym kluczem. Okno poniżej minimalnej próby mówi
> „za mało danych", **nigdy „ok"** — to jest inna odpowiedź niż cisza.
> Odzyskanie stanu ma własny klucz, bo „znowu jest dobrze" to inna wiadomość niż
> „jest źle". Harmonogram **uruchamia sprawdzenie i nie definiuje reguły**.
> Tu należy też alert na ciszę, który sekcja 41 odłożyła do etapu D: margines
> dziennika do luki jest już liczony (`aiwatcher-projector`, `journal`), więc to
> jest sygnał do podniesienia, a nie nowy pomiar. Alerty czytają **stronę
> globalną** świadomie — nie zakresuj ich.
>
> **D3 — diagnostyka zdolności instancji.** `GET /api/v1/system` (SYS-01) już
> odpowiada, **czym ta instancja jest**: 28 zdolności, rola `admin`, nazwa
> zmiennej zamiast wartości, żadnego adresu i żadnego poświadczenia. SYS-01
> **świadomie odmówił sondy** — „inwentarz świecący na czerwono, bo ktoś inny
> się restartuje, czyta się jak awaria tej instancji". D3 jest dokładnie tą
> drugą połową, więc **osobna trasa** (rekomendacja:
> `GET /api/v1/system/diagnostics`) z własnym timeoutem, własną kadencją i
> nazwanym trybem awarii; nie wkładaj sondy do inwentarza. Do tego „link do
> pierwszego odebranego sygnału": dla każdej zdolności, którą karmi producent —
> kiedy ostatnio coś przyszło i co to było. **Najpierw sprawdź, czy to nie jest
> już policzone**: `readmodel`, `journal`, `asked` i `measured` trzymają
> dokładnie takie fakty, a drugi licznik obok istniejącego to dług, nie funkcja.
> Regresja na sekrety jest w `crates/aiwatcher-server/tests/system.rs` (27 igieł
> przepuszczonych przez `aiwatcher_server::build`) — **dopisz do niej nową
> trasę**, inaczej powstaje trasa poza jedynym testem, który tej reguły
> pilnuje. Żadnego UI konfiguracji: ustawienie zmienia się zmienną i restartem.
>
> **D4 — serwerowe zapisane widoki i prosty raport.** To jest **nowy zasób
> autorski**, więc obowiązuje reguła z „Co obowiązuje każdy strumień" — zakresowa
> rodzina tras od urodzenia, w całości. Dwie rzeczy z odbioru etapu D są
> rozstrzygnięciem projektowym, nie detalem: **raport trzyma przypięte wyniki po
> zmianie etykiety produkcyjnej**, więc przypina id albo digest, nigdy etykietę;
> i **link do raportu nie nadaje dostępu do obiektów źródłowych**, więc raport
> trzyma referencje, a treść rozwiązuje się prawami czytającego, przy każdym
> odczycie. Lokalne widoki przeglądarki
> (`apps/panel/src/shared/lib/local-views.ts`) **zostają** — serwerowy widok to
> inny obiekt, z właścicielem, a nie cicha migracja tamtych. Właściciela i prawa
> zapisu/odczytu nazwij wprost.
>
> **Czego nie budować.** Trasowania per reguła i drugiego kanału — kanał jest
> konfiguracją, `/system` mówi, że jest i czy podpisuje, nigdy adresu. Agregacji
> powiadomień, jeśli nie wynika wprost z D2: to jest nazwane ograniczenie D1, a
> nie brak. Sondy zdrowia w inwentarzu. UI do edycji ustawień. Zakresowania
> alertów.
>
> **Granice.** Twoje katalogi: `crates/aiwatcher-alerts/`,
> `crates/aiwatcher-server/src/alerts/`, nowe moduły API,
> `apps/panel/src/features/alerts/` i nowy katalog raportów. Nie dotykasz
> niczego z tabeli w sekcji „Gdzie się zderzą" — te pliki ma sesja IAM-03, która
> **zmienia właśnie to, czym jest sprawdzenie roli na trasie instancyjnej**, więc
> swój `Caller` i swoją odmowę bierz z `context.rs` i `auth.rs` **po**
> przebazowaniu na tamtą pracę, nie z dzisiejszej kopii. `just openapi` na samym
> końcu, w osobnym commicie.
>
> **Na koniec** uruchom pełny zestaw z „Co obowiązuje każdy strumień". Trwałe
> reguły zapisz w ADR i `CLAUDE.md`, odbiór w opisie commita, a w
> `docs/FTI_IMPLEMENTATION_PLAN.md` zostaw **wyłącznie** to, co nadal otwarte —
> to jest zasada tamtego pliku. Jawnie wypisz, czego nie uruchomiłeś.

---

## Prompt — FLOW-02: prompt i wariant w katalogu silników zapytań

> Pracujesz w repozytorium AIWatcher nad **jedną rzeczą, którą FLOW-01 nazwał
> osobną pracą**: `prompt` i `variant` jako kolumny katalogu silników zapytań.
> Przeczytaj `CLAUDE.md`, `services/query/CLAUDE.md`,
> `docs/ADR/ADR_0028_QUERY_ENGINES.md`, `docs/ADR/ADR_0008_FLOW_QUERY_SURFACE.md`,
> akapit „Czego świadomie nie zrobiono" w sekcji „FLOW-01 dowieziony"
> (`docs/ux-migration-plan-2026-09-14.md`) oraz w tym pliku sekcje „Gdzie się
> zderzą" i „Co obowiązuje każdy strumień".
>
> **Co jest, a czego nie ma.** Read model niesie oba fakty:
> `SpanRow::prompt_name` i `prompt_version`
> (`crates/aiwatcher-projector/src/spans.rs`) oraz `RunSummary::variant_id`
> (`readmodel.rs`), a `/dimensions` ma oś `prompt` od FLOW-01.
> `services/query/contract/catalog.json` **nie projektuje żadnego z nich**:
> `spans` i `corpus_spans` kończą się na `step_type`, a `runs` nie ma wariantu.
> Czyli to jest projekcja, katalog i testy — **nie nowa telemetria**.
>
> **Zrób.** Kolumny w katalogu, ta sama trójka w każdym silniku (`flow` w PHP,
> `datafusion` i `duckdb` we wspólnym workspace'ie `uv`), atrybut w budowniczym
> zapytań w panelu i **zgodność**: `just query-conformance` pyta każdy silnik
> tymi samymi pytaniami i porównuje z wierszami Flow, więc nowa kolumna bez
> pytania w tym zestawie to kolumna, o której dowiesz się od użytkownika.
> Sprawdź po drodze, czy dokumentacja katalogu (`services/query/README.md` i
> `CLAUDE.md` tamtej usługi) wymienia kolumny — jeśli tak, to jest druga kopia
> do ruszenia razem.
>
> **Czego nie robić.** Nie dokładaj `prompt` ani `variant` jako osi **żywego
> strumienia**: zdarzenie nie niesie faktu spanowego (ADR_0003) i to jest ten sam
> powód, dla którego nie ma tam dziś wariantu. Nie zmieniaj trasy Rusta, jeśli
> nie musisz — sprawdź najpierw, czy `just openapi` ma w ogóle co regenerować.
>
> **Weryfikacja:** `just query-check`, `just query-contract-check`,
> `just query-conformance` (raz na silnik), panel `npm run test`, `npm run lint`
> i `npm run build`. Jawnie wypisz, czego nie uruchomiłeś.

---

## Prompt — PORZ-02: limit komentarzy w CI i trzy nieaktualne dokumenty

> Dwie niezależne, małe rzeczy w repozytorium AIWatcher. Rób je osobnymi
> commitami i nie łącz w jedną zmianę. Przeczytaj `CLAUDE.md` i w tym pliku
> sekcje „Gdzie się zderzą" oraz „Co obowiązuje każdy strumień".
>
> **1. `scripts/lint-comments.py` wychodzi 1.** Czternaście bloków przekracza
> limit 25 linii prozy: `crates/aiwatcher-api/src/system.rs` (57),
> `crates/aiwatcher-server/src/execution/measure.rs` (39),
> `apps/panel/src/features/system/screens/overview/page.tsx` (38),
> `crates/aiwatcher-server/src/execution/artifacts.rs` (34),
> `crates/aiwatcher-execution/src/artifact/object.rs` (32),
> `crates/aiwatcher-iam/src/export.rs` (31),
> `crates/aiwatcher-iam/src/retention.rs` (31),
> `apps/panel/src/features/learning/lib/enrollment.ts` (30),
> `crates/aiwatcher-migration/src/lib.rs` (30),
> `crates/aiwatcher-projector/src/pipeline.rs` (30),
> `crates/aiwatcher-execution/src/artifact/mod.rs` (28),
> `crates/aiwatcher-projector/src/readmodel.rs:449` (28),
> `apps/panel/src/features/learning/screens/overview/page.tsx` (26),
> `apps/panel/src/shared/lib/iam.ts` (26). W IAM-01 §6 było ich **sześć** —
> urosło do czternastu, bo tego skryptu **nie ma w CI**: `.github/workflows/ci.yml:45`
> woła wyłącznie `check-rust-boundaries.py`. Zrób obie połowy: skróć bloki i
> dołóż skrypt do CI obok tamtego, żeby nie mógł znowu odjechać. Reguła
> skracania jest w samym skrypcie: *argument, historia i odrzucone warianty
> należą do `docs/ADR/`* — więc uzasadnienie **przenosisz**, nie kasujesz, a przy
> blokach cudzych strumieni zostawiasz ich sens nietknięty. `crates/aiwatcher-iam/**`
> zostaw w spokoju, jeśli sesja IAM-03 nadal ma te pliki brudne — `git status`
> powie; wtedy zrób resztę i nazwij pominięte.
>
> **2. Trzy dokumenty kłamią o stanie.** W
> `docs/ux-migration-plan-2026-09-14.md`, tabela „Stan implementacji": wiersz
> IAM-01 mówi „Brak dispatchera i projektowego `/start`" (oba są od IAM-02/D),
> wiersz IAM-02 mówi „Niewdrożone" (E1–E7 dowiezione, bramka M1 51/51 na żywym
> serwerze), wiersz LEARN-01 mówi, że treść, testy i wyniki nie mają kontraktu
> (`crates/aiwatcher-labs` je ma, razem z notatnikiem i `context_id` jako
> złączeniem). W `docs/parallel-streams-2026-09-18.md` są sekcje „zrobione" dla
> IAM-02/B i /C, a **nie ma dla /D** — dopisz na końcu pliku, w kształcie tamtych
> dwóch, na podstawie sekcji 12 w `docs/iam-02-data-plane.md`. Nie ruszaj
> `docs/iam-03-projects.md`: prowadzi go inna sesja. Popraw **stan**, nie
> przepisuj argumentów — te dokumenty są rejestrem tego, co i dlaczego zrobiono.

---

## Co świadomie zostaje poza tym dokumentem

- **IAM-03 w całości**: M0 (w toku), M1–M5, M6 (trasy artefaktów w projekcie,
  retencja deklaracji/ustawień sędziego/trzymanych odpowiedzi, kolektor
  artefaktów, reszta rodziny `executions` razem z poświadczeniem workera na
  projekt) i M7 (30 rodzin tras wyłącznie instancyjnych). To jest plan
  [IAM-03](iam-03-projects.md) i praca tamtej sesji.
- **Adaptery migracji** dla pięciu rodzin, które mają zakresowy układ i nie mają
  adaptera (`artifacts`, `workflows`, `evaluation-judges`, `evaluation-reviews`,
  `evaluation-variant-artifacts`), oraz przebieg migracji end-to-end po
  S3/RustFS. To IAM-03/M5 i E7 — brać wyłącznie po uzgodnieniu z tamtą sesją.
- **Ograniczenia z sekcji 41 planu FTI** (kolejność sędziego, indeks pytań,
  skróty kodu narzędzi, ostatni zgubiony run klienta). To są **zapisane
  granice**, nie zaległości; rusza się tylko to, co etap D nazywa po imieniu.
- **Push 118 commitów** przed `origin/main` — decyzja właściciela repozytorium.
