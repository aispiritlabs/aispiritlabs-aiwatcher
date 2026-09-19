# Cztery strumienie naraz — System, Flow, Learning, IAM

Data: 18.09.2026. Ten dokument dzieli to, co zostało z
[planu migracji UX](ux-migration-plan-2026-09-14.md), na strumienie, które mogą
iść **równolegle**, i mówi wprost, gdzie się zderzą. Prompty są na końcu, jeden
na strumień, każdy samodzielny.

Kroki 0–4 ze „Ścieżki do pierwszego testu permissionów" są zrobione na gałęzi
`iam-01/local-sso-and-panel` (10 commitów). **Warunek wstępny dla wszystkich
strumieni: ta gałąź trafia na `main`.** Dopóki nie trafi, każdy strumień
odbija się od jej czubka (`8a8209a`), a nie od `main` — inaczej LEARN-02
nie zobaczy `shared/lib/iam.ts`, a FLOW-01 poprawki fokusu w `shell.tsx`.

## Mapa

| Strumień | Co robi | Zależy od | Kiedy |
|---|---|---|---|
| **SYS-01** | Obszar System: runtimes, integracje, konfiguracja instancji | — | **teraz** |
| **FLOW-01** | Agenci jako obiekty, lineage, porównania przedziałów, wspólne filtry | — | **teraz** |
| **LEARN-02** | Treść warsztatu: brief, testy, wynik | — | **teraz** |
| **IAM-02/A** | E1 + E2: projekt na kopercie, fold zna zakres | — | **teraz** |
| **IAM-02/B** | E3 + E4: odczyty i żywy strumień po grantach — **bramka M1** | A | po A |
| **IAM-02/C** | Aktywacja selektora organizacja/projekt w panelu | B (M1) | po M1 |
| **IAM-02/D** | E6 + E7: reszta data plane, dispatcher, projektowy `/start`, cutover | C | po C |
| **PORZ-01** | eslint w panelu, eksport i retencja audytu IAM | — | kiedykolwiek |

Cztery pierwsze startują jednocześnie. E5 z planu IAM-02 („dzielenie projektu")
jest **w większości już zrobione** — zaproszenia, roster, granty i okno
udostępnienia dowiózł krok 2, a strona `/account/access` krok 1. Zostaje z niego
wyłącznie selektor w górnym pasku, i to jest IAM-02/C, świadomie za bramką M1.

## Gdzie się zderzą, i co z tym zrobić

Osiem plików dotyka więcej niż jeden strumień. Każdy prompt to powtarza, ale
reguła jest jedna i warto ją mieć w jednym miejscu.

| Plik | Kto go dotknie | Jak nie zrobić konfliktu |
|---|---|---|
| `contracts/openapi.json` | SYS, LEARN, IAM-02/A i B | **Nigdy ręcznie.** `just openapi` na końcu, w osobnym commicie. Przy konflikcie: weź swoją wersję i przegeneruj — to artefakt pochodny, nie źródło |
| `apps/panel/src/api/generated/**` | ci sami | To samo, tym samym poleceniem. `.prettierignore` już je chroni |
| `crates/aiwatcher-api/src/routes.rs` | SYS, LEARN | Jedna linia **na końcu** listy. Nie przestawiaj istniejących |
| `crates/aiwatcher-api/src/openapi.rs` → `document()` | SYS, LEARN | To samo: jedna linia na końcu |
| `crates/aiwatcher-api/src/openapi.rs` → `components(schemas(…))` | SYS, LEARN | **Jedna globalna przestrzeń nazw.** Każdy typ, którego nazwy mogłaby użyć inna domena, dostaje `#[schema(as = …)]`. `Withdrawal` już raz cicho nadpisał cudzy — patrz `apps/panel/CLAUDE.md` |
| `crates/aiwatcher-api/src/lib.rs` | SYS, LEARN | Jedna deklaracja modułu, alfabetycznie |
| `apps/panel/src/app/navigation.ts` | SYS, FLOW, LEARN | Jeden wpis obszaru albo widoku. Nie zmieniaj cudzych blurbów |
| `apps/panel/src/app/commands.ts` | SYS, FLOW, LEARN | Dopisz swoje komendy na końcu; `commands.test.ts` sprawdza je wobec schematu zod trasy |
| `docs/ux-migration-plan-2026-09-14.md` | wszyscy | **Dopisz sekcję na końcu pliku.** Konflikt append-only rozwiązuje się w sekundę |
| `apps/panel/CLAUDE.md` | wszyscy, którzy dodają obszar | Jeden punkt w Konwencjach, ewentualnie jedna Odmowa |

**Katalogi są własnością strumienia.** `features/system/`, `features/agents/`,
`features/learning/` — nikt nie edytuje cudzego. To samo po stronie Rusta:
moduł API na strumień.

**Checkout albo worktree.** Cztery sesje w jednym katalogu roboczym już raz
skończyły się commitowaniem cudzych plików. Worktree na strumień to czysty
rozdział, ale każdy ma własny `target/` i pierwszy `cargo build` jest zimny;
FLOW-01 jest wyłącznie panelowy i worktree kosztuje go nic. Jeśli katalog jest
wspólny: **`cargo`, `just openapi` i `npm run build` wołane naraz przez dwie
sesje deptią sobie po blokadach** — umówcie się, kto trzyma toolchain.

**Commituj po ścieżkach, nigdy `git add -A`.** Jeden krótki nagłówek
konwencjonalny, bez trailera współautora.

## Co obowiązuje każdy strumień

- **Nowy zasób autorski dostaje swoją zakresową rodzinę tras od urodzenia** —
  `ProjectAuthorization` plus `<prefix>/scopes/<organization>/<project>/registry/`.
  Dotyczy LEARN-02 wprost. Nie dotyczy SYS-01, bo konfiguracja instancji nie jest
  zasobem projektu.
- **Nie opisuj wdrożenia jako multi-tenant safe.** Do M1 to nie jest prawda.
- **Nie zmieniaj istniejących migracji SQL.** Nowe są addytywne, z rolling
  upgrade i rollbackiem starego binarium.
- **Nie zakresuj logu, strumieni ani foldów, jeśli nie jesteś IAM-02/A lub B.**
- **Nie otwieraj projektowego `/start` i nie rejestruj dispatchera w
  produkcyjnym `spawn`.** Ten zakaz zdejmuje wyłącznie IAM-02/D.
- **Nigdy nie wypełniaj ekranu danymi zastępczymi.** `AreaPlaceholder` istnieje
  po to; prawdopodobna atrapa czyta się jak działające oprogramowanie.
- `npm run lint` **nie działa w tym repozytorium i nie działał** — `apps/panel`
  nie ma eslinta ani jego konfiguracji, a CI woła `build` i `test`. Nie goń
  tego; naprawia to PORZ-01.

Testy na koniec każdego kroku: panel `npm run test` i `npm run build` (to
`check:architecture` + `vite build` + pełne `tsc -b`), przejście klawiaturą, 375 px
i szeroki ekran, motyw jasny i ciemny, bez poziomego przewijania strony. Rust, przy
dotknięciu któregokolwiek crate'a: `cargo test --workspace --all-targets`,
`cargo clippy --workspace --all-targets --all-features -- -Dwarnings`,
`cargo fmt --all --check`, `git diff --check`,
`python3 scripts/check-rust-boundaries.py`. Po zmianie trasy lub typu: `just openapi`
i commit obu stron. PostgreSQL wyłącznie na osobnej, jednorazowej bazie.
**Jawnie wypisz, czego nie uruchomiłeś.**

Środowisko lokalne, jeśli potrzebujesz tożsamości: `just authentik-up`,
`just authentik-seed`, `just authentik-secret`, `just postgres-up`,
`just run-sso-iam`, `just panel`. Ludzie to `teacher` / `teacher-dev` (admin
instancji) i `student` / `student-dev` (viewer). Dwie sesje naraz bez dwóch
przeglądarek daje `python3 scripts/sso-session.py <kto>`, całą matrycę
`python3 scripts/iam-permission-check.py` (28/28).

---

## Prompt — SYS-01: obszar System

> Pracujesz w repozytorium AIWatcher nad **obszarem System**: runtimes,
> integracje i konfiguracja instancji. Przeczytaj `CLAUDE.md`,
> `apps/panel/CLAUDE.md`, `docs/parallel-streams-2026-09-18.md` (sekcje „Gdzie
> się zderzą" i „Co obowiązuje każdy strumień") oraz sekcję „Kontynuacja — shell
> sprawdzony i Learning nad grantami" w `docs/ux-migration-plan-2026-09-14.md`,
> gdzie ten brak został nazwany.
>
> **Punkt wyjścia, i jest lepszy niż zapisano w planie.** Plan mówi „System nie
> ma kontraktu", i to prawda — żadna z 226 ścieżek w `contracts/openapi.json` nie
> odpowiada, co ta instancja ma skonfigurowane. Ale **fakty istnieją**:
> `crates/aiwatcher-api/src/state.rs` trzyma je niemal w komplecie — czy jest
> rejestr promptów, datasetów, anotacji, archiwum rozmów, harmonogramy,
> definicje workflow, szablony podów (`AIWATCHER_POD_TEMPLATES`), silnik zapytań
> (`AIWATCHER_QUERY_ENGINE`) i jego timeout, profil sędziego i jego
> współbieżność, serwis scorerów, huby datasetów, magazyn wykonań. Do tego
> `auth::config` już raportuje tryb i issuer, a osobno stoją tablica cen
> (`AIWATCHER_MODEL_PRICES`), świadkowie (`AIWATCHER_WITNESSES`),
> `AIWATCHER_WORKFLOW_RUNNER_URL` i `AIWATCHER_POD_RUNTIME`. To jest trasa do
> napisania nad stanem, który już jest, a nie ekran do wymyślenia.
>
> **Zbuduj.** Moduł `crates/aiwatcher-api/src/system.rs` — facade jak każdy inny
> w tym crate: `pub fn router()` i `pub fn openapi()`, handlery prywatne,
> `#[derive(OpenApi)]` obok routera. Jedna trasa odczytu, `GET
> /api/v1/system`, odpowiadająca **czym ta instancja jest**: dla każdej
> zdolności jej stan (`configured` / `not configured`), **nazwa zmiennej
> środowiskowej**, która o tym decyduje, i wartość tam, gdzie wartość nie jest
> sekretem — silnik zapytań, runtime podów, nazwy szablonów, tryb auth, issuer.
> W panelu obszar `system` w `navigation.ts` i strona, która to rysuje.
>
> **Czego nie wolno ujawnić, i to jest cała trudność tej pracy.** Sekrety nigdy:
> żadnych URL-i z poświadczeniem, tokenów, kluczy, connection stringów,
> `AIWATCHER_AUTH_CLIENT_SECRET`, `AIWATCHER_POD_CREDENTIAL_SECRET`,
> `AIWATCHER_SCORER_TOKEN`. **Fakt „skonfigurowane" nie jest sekretem, wartość
> bywa.** Adres bazy i adres object store'u to rekonesans dla kogoś, kto już jest
> w środku — raportuj obecność i nazwę zmiennej, nie treść. Napisz regresję,
> która to pilnuje: odpowiedź nie zawiera żadnego skonfigurowanego sekretu,
> sprawdzana przez wstrzyknięcie rozpoznawalnej wartości do każdej wrażliwej
> zmiennej i szukanie jej w ciele. Ta trasa **wymaga roli** — zdecyduj której i
> zapisz dlaczego; rekomendacja: `admin`, bo to inwentarz wdrożenia, a nie
> pomoc dla czytelnika przebiegu.
>
> **Czego nie budować.** Żadnego zapisu — to jest odczyt konfiguracji, nie
> panel administracyjny; zmiana ustawienia to zmienna środowiskowa i restart, i
> tak ma zostać. Żadnego zdrowia usług zewnętrznych (to jest sonda, nie
> inwentarz) i żadnego zgadywania: zdolność, o której `AppState` nic nie mówi,
> nie pojawia się na liście.
>
> **Granice z innymi strumieniami.** Dotykasz `routes.rs`, `openapi.rs`,
> `lib.rs` i `navigation.ts` — po jednej linii na końcu list, nie przestawiaj
> cudzych. Twój katalog panelu to `apps/panel/src/features/system/`. `just
> openapi` na samym końcu, w osobnym commicie.

---

## Prompt — FLOW-01: pełne przejścia i lineage

> Pracujesz w repozytorium AIWatcher nad **FLOW-01: pełnymi przejściami i
> lineage**. Przeczytaj `CLAUDE.md`, `apps/panel/CLAUDE.md`,
> `docs/parallel-streams-2026-09-18.md` (sekcje „Gdzie się zderzą" i „Co
> obowiązuje każdy strumień"), wiersz FLOW-01 w tabeli „Stan implementacji" i
> punkt 6 „Kolejności wdrażania" w `docs/ux-migration-plan-2026-09-14.md`, oraz
> ADR_0007 (wymiary) i ADR_0012 (graf workflow).
>
> **Co ma powstać.** Cztery rzeczy, nazwane w planie:
> **(1) Dedykowane strony agentów.** Dziś agent jest wymiarem w Explore —
> `dimensions::compute` odpowiada siedmioma osiami z jednym kształtem wiersza —
> ale nie ma strony obiektu. Agent ma historię, modele, prompty, narzędzia,
> koszt i przebiegi; to jest strona, nie komórka w tabeli.
> **(2) Powiązania wersji prompt/model/dataset.** Span niesie `prompt_name` i
> `prompt_version` (`PromptRef::from_data`, atrybuty `aiwatcher.prompt.*`), a
> wersja modelu nazywa przebieg treningu i eksport za nim. Te złączenia **istnieją
> w danych** i nie są klikalne. `apps/panel/src/shared/components/lineage-reference.tsx`
> już stoi i praktycznie nikt go nie woła — zacznij od sprawdzenia, czy to jest
> ten element, czy relikt.
> **(3) Porównania przedziałów.** Dwa okna czasu obok siebie, na tych samych
> filtrach.
> **(4) Wspólne filtry tabel i wykresów.** Dziś czas i agent wybierają przebiegi,
> a model zawęża wyłącznie wywołania LLM i **nie** zawęża liczników run/tool/step
> — to jest kontrakt opisany w UX-02 i zmiana go jest właśnie tą pracą. Zacznij
> od zapisania, co dokładnie ma znaczyć wspólny filtr obiektów, zanim cokolwiek
> napiszesz.
>
> **Zacznij od inwentaryzacji, nie od kodu.** Co z tych czterech da się zrobić
> **wyłącznie w panelu nad dzisiejszymi trasami**, a co wymaga nowej? Rekomendacja:
> zrób najpierw całą część panelową i dopiero potem, osobnym commitem, dopisz
> trasę, jeśli okaże się konieczna — bo każda nowa trasa to `just openapi` i
> kolizja z SYS-01 oraz LEARN-02.
>
> **Reguły, które tu obowiązują i łatwo je złamać.** Nigdy nie licz niczego w
> przeglądarce, co liczy serwer: wymiary, porównania i ceny są jego. Nigdy nie
> rysuj wywnioskowanej krawędzi jako wiadomości — zadeklarowana krawędź to
> obietnica orkiestratora, `agent.message` to to, co powiedziano. Filtry
> **wyłącznie w URL**, nigdy w stanie komponentu. Każda lista, która rośnie z
> retencją, to `useInfiniteQuery` nad `VirtualList` i niesie okno czasu. Przebieg
> bez zdarzenia końcowego zostaje `Running` — to panel rysuje linię na
> `STALLED_AFTER_MS`, a nie projektor zgaduje śmierć.
>
> **Granice z innymi strumieniami.** Twoje katalogi to
> `apps/panel/src/features/observability/` i nowy `apps/panel/src/features/agents/`
> (albo strona agenta w istniejącym — zdecyduj i zapisz dlaczego). Dotykasz
> `navigation.ts` i `commands.ts` — dopisuj na końcu. Jeśli dopiszesz trasę,
> `just openapi` na samym końcu, w osobnym commicie.

---

## Prompt — LEARN-02: treść warsztatu

> Pracujesz w repozytorium AIWatcher nad **treścią warsztatu**: brief, testy,
> ewaluacja i wynik laboratorium. Przeczytaj `CLAUDE.md`, `apps/panel/CLAUDE.md`
> (punkt o obszarze `learning`), `docs/parallel-streams-2026-09-18.md` i sekcję
> „Kontynuacja — shell sprawdzony i Learning nad grantami" w
> `docs/ux-migration-plan-2026-09-14.md`.
>
> **Co już stoi, i czego nie wolno tknąć.** Połowa dostępowa działa i jest
> zbudowana na jednym zdaniu: **warsztat to projekt, uczestnik to grant, zapis to
> zrealizowane zaproszenie.** W backendzie nie ma i nie ma powstać pojęcia
> „warsztatu". `/learning` rysuje listę warsztatów, uczestników z fazą okna
> każdego grantu i dziewięć **pustych** slotów laboratoriów. Ta reguła zostaje:
> faza czytana z **jednego** grantu, nigdy sumowana w dostęp osoby.
>
> **Pierwsze zadanie nie jest kodem — jest decyzją, i od niej zależy cała
> reszta.** Zanim napiszesz crate, ustal, **czy te cztery rzeczy nie istnieją już
> tutaj pod innymi nazwami**. To repozytorium konsekwentnie odmawia budowania
> drugiej kopii czegoś, co ma. Trop:
> – **testy laboratorium** wyglądają jak `evaluation-scorecards` plus kohorta;
> – **wynik** wygląda jak wynik scoring runu, z kierunkiem metryki z karty;
> – **oddanie pracy** wygląda jak nagranie albo jak wygenerowana odpowiedź;
> – **brief** nie wygląda na nic z tego i jest jedynym pewnym nowym bytem —
> tekst autorski, wersjonowany, poza retencją, czyli dokładnie kształt rejestru
> promptów (ADR_0011).
> Napisz, co z czego wynikło, **zanim** zaczniesz. Jeśli trzy z czterech da się
> złożyć z istniejących rejestrów, ta praca jest o rząd wielkości mniejsza, niż
> wygląda — i o to chodzi.
>
> **Zasada, która wiąże wszystko, co jednak napiszesz.** Nowy zasób autorski
> dostaje swoją **zakresową rodzinę tras od urodzenia**: `ProjectAuthorization`
> plus klucze `<prefix>/scopes/<organization>/<project>/registry/`. Zakres nigdy
> nie wchodzi do hasha treści — identyczna treść w dwóch projektach ma ten sam
> identyfikator wersji i osobne obiekty. Rejestr odmawia przepięcia do innego
> projektu. Każda mutacja wymaga `X-AIWatcher-IAM: 1` i **ponownego** sprawdzenia
> grantu po odebraniu ciała. Odpowiedzi niosą `Cache-Control: no-store`. Wzorem
> jest `crates/aiwatcher-prompts` i `crates/aiwatcher-api/src/prompt_scope.rs`.
>
> **Czego nie budować.** Postępu, punktów, rankingu ani terminu oddania, dopóki
> nie ma kontraktu, który je niesie — i **żadnej atrapy**: slot bez treści mówi
> `no contract` i tyle. Żadnego drugiego pojęcia dostępu: kto widzi laboratorium,
> decyduje grant na projekcie, i nic innego.
>
> **Granice z innymi strumieniami.** Twój katalog panelu to
> `apps/panel/src/features/learning/` — cały jest twój. Po stronie Rusta: nowy
> crate (jeśli decyzja wyżej go uzasadni) plus moduł API; `routes.rs`,
> `openapi.rs`, `lib.rs`, `Cargo.toml` workspace'u i `navigation.ts` po jednej
> linii na końcu. `just openapi` na samym końcu, w osobnym commicie. Każdy typ,
> którego nazwy mogłaby użyć inna domena, dostaje `#[schema(as = …)]`.

---

## Prompt — IAM-02/A: projekt na kopercie i fold, który go zna

> Pracujesz w repozytorium AIWatcher nad **etapami E1 i E2 z IAM-02**.
> Przeczytaj `docs/iam-02-data-plane.md` **w całości** — sekcja 2 („Jedna
> decyzja") jest tu ważniejsza niż wszystko inne — potem `CLAUDE.md`,
> `crates/aiwatcher-iam/README.md`, ADR_0033, ADR_0001 i ADR_0003, oraz
> `docs/parallel-streams-2026-09-18.md`.
>
> **Decyzja jest podjęta i nie jest twoja do zmiany:** `EventEnvelope` niesie
> opcjonalny `ProjectScope`, serializowany, domyślnie nieobecny — brak znaczy
> stronę globalną, więc nic z dotychczasowych danych się nie rusza. Log per
> projekt jest odrzucony, bo `AIWATCHER_LASER_PARTITIONS > 1` jest zakazane,
> dopóki skalarny `Checkpoint` nie stanie się kursorem per partycja.
>
> **E1 — projekt na kopercie.** `IngestToken` zyskuje zakres, `Identity` go
> niesie, rola zostaje **twardo `Editor`**. Trasa `POST /api/v1/events`
> **zawsze nadpisuje** pole wartością z poświadczenia; wartość podana przez
> producenta jest odrzucana, nie honorowana. To jest cała różnica między polem
> koperty a prymitywką do podszywania się pod cudzy projekt — i uwaga, tego
> **nie** wolno skopiować z `published_by`, które jest `#[serde(skip)]`, bo tam
> czytelnikiem jest ten sam proces, a tutaj czytelnikiem jest projektor czytający
> z szyny. Zakres **musi** przeżyć serializację. Zaktualizuj
> `contracts/envelope.schema.json`. **SDK bez zmian** — nigdy tego pola nie
> wysyłają.
>
> **E2 — fold zna zakres.** **Jeden fold z kluczem zakresu w wierszu, nie fold
> per tenant** — to jest różnica między „dodatkowy wymiar" a „przesłanka do
> rewizji ADR_0033". Obejmuje `RunSummary`, `dimensions::compute` (siedem osi),
> spany, `period_fold`, `asked`, `measured` i journal. **Identyfikatory globalne
> nie drgną co do bajtu**: `TraceId::derive` i `SpanId::derive` są czystymi
> funkcjami `run_id`. `AIWATCHER_MAX_SPANS_TOTAL` zachowuje znaczenie, ale
> **przebiegnij `just load-test` ponownie** i razem z nim rusz limit kontenera —
> metryki rosną o jedną serię na projekt, który cokolwiek zapisał. Retencja logu
> zostaje instancyjna na tym etapie.
>
> **Czego nie robić.** Nie filtruj odczytów po grantach — to jest E3 i robi to
> IAM-02/B po tobie. Nie dotykaj SSE ani WebSocketu — to E4. Nie aktywuj żadnego
> selektora w panelu. Nie zmieniaj istniejących migracji SQL.
>
> **Testy, które są tu właściwą robotą:** producent nazywający projekt w ciele
> jest ignorowany; token projektu A nie zapisze do B; koperta bez pola czyta się
> jak każda historyczna; redelivery ląduje na tym samym spanie co dziś; fold nad
> mieszanką zdarzeń globalnych i projektowych daje dokładnie to, co dziś, dla
> globalnych.
>
> **Granice z innymi strumieniami.** Dotykasz `aiwatcher-core`, `aiwatcher-bus`,
> `aiwatcher-projector` i `aiwatcher-api/src/ingest.rs` — żaden inny strumień
> tam nie wchodzi. Wspólne są `contracts/openapi.json`,
> `contracts/envelope.schema.json` i wygenerowany klient: `just openapi` na
> samym końcu, w osobnym commicie.

---

## Prompt — IAM-02/B: odczyty i żywy strumień, czyli M1

> **Nie zaczynaj, dopóki IAM-02/A nie jest na `main` i zielone.** Pracujesz nad
> **E3 i E4 z IAM-02**, czyli nad bramką **M1**. Przeczytaj
> `docs/iam-02-data-plane.md` (sekcje E3, E4 i „Bramka M1"), ADR_0004 (wznowienie
> strumienia), ADR_0013 (sesja jako ciasteczko) i `docs/parallel-streams-2026-09-18.md`.
>
> **E3 — odczyty odpowiadają pytającemu.** Każda lista i każdy szczegół filtruje
> po **efektywnych grantach pytającego**, liczonych **świeżo na żądanie**:
> `ProjectAccess` to migawka z `evaluated_at`, nie zdolność na sesję. Odmowa
> zakresu to **404**, nie 403 i nie 503 — przebieg, do którego ktoś nie sięga, to
> przebieg, którego nie ma (precedens `StoreError::OutOfScope`). Jedna decyzja
> jest do podjęcia, nie techniczna: **co widzi członek projektu na przebiegu
> nieprzypisanym (globalnym)?** Rekomendacja planu: bez zmian — dane globalne
> zostają pod autoryzacją instancji, projektowe są addytywne. Zapisz, co
> wybrałeś, i dlaczego.
>
> **E4 — żywy odczyt.** SSE i WebSocket niosą zakres w subskrypcji, hub filtruje.
> Tożsamością jest **podpisane ciasteczko sesji**, bo przeglądarka nie ustawi
> nagłówka na żadnej z tych dwóch tras — to cała racja bytu ADR_0013. Dwie rzeczy
> decydują o tym, czy to jest bezpieczne: **ponowne sprawdzenie grantu co ~30 s na
> strumień, z zamknięciem połączenia przy odmowie** (dziś TTL ciasteczka *jest*
> oknem odwołania, a to za mało dla strumienia żyjącego godzinami), oraz
> **`Last-Event-ID` sprawdzający grant przed odtworzeniem** — bez tego wznowienie
> strumienia jest sposobem na czytanie po odebraniu dostępu. Żywy filtr zostaje
> po stronie serwera; `llm.chunk` to większość logu i zawężanie w przeglądarce
> znaczyłoby odebrać wszystko, żeby wyrzucić.
>
> **Bramka M1, weryfikowana rzeczywistym HTTP, nie testem bibliotecznym:**
> zalogowanie → lista wyłącznie moich projektów → przebiegi, spany i metryki
> wyłącznie mojego projektu → żywy strumień wyłącznie mojego projektu →
> odwołanie grantu zamyka strumień i odcina odczyty. Rozszerz
> `scripts/iam-permission-check.py` o te scenariusze, tak jak urósł do 28 pytań o
> połowę autorską.
>
> **Czego nie robić.** Nie aktywuj selektora organizacja/projekt w górnym pasku —
> to IAM-02/C, po tobie. Nie ruszaj zapytań, notebooków, workerów, harmonogramów
> ani retencji — to E6.
>
> **Granice.** `aiwatcher-projector`, `aiwatcher-api` (`runs`, `metrics`, `live`,
> `stream`) i panel tylko tam, gdzie odczyt musi nazwać projekt.

---

## Prompt — IAM-02/C: selektor organizacja/projekt

> **Nie zaczynaj przed bramką M1.** Pracujesz nad tym, co zostało z E5:
> **aktywacją selektora organizacja/projekt** w panelu. Przeczytaj
> `apps/panel/CLAUDE.md` (punkty o `/account` i o `learning`),
> `docs/iam-02-data-plane.md` sekcja E5 i `docs/parallel-streams-2026-09-18.md`.
>
> **Większość E5 jest już zrobiona** i nie buduj jej drugi raz: zaproszenia
> (jednorazowy wygasający token, realizowany po SSO w jednej transakcji z
> grantem), roster, granty projektu, okno udostępnienia i strona
> `/account/access` stoją od kroków 1 i 2. Learning rysuje to samo jako warsztaty.
>
> **Została jedna rzecz i jest celowo trudna.** Do dziś obowiązywała reguła:
> *„nie ma przełącznika organizacji w nagłówku, bo selektor zakresujący cały
> panel byłby ogłoszeniem multi-tenancy, którego płaszczyzna danych nie utrzyma"*.
> **Po M1 ta przesłanka znika i regułę wolno zdjąć — ale tylko wtedy, gdy M1
> naprawdę trzyma.** Zanim cokolwiek aktywujesz, przebiegnij matrycę uprawnień
> na żywym serwerze i zapisz wynik. Jeśli któryś odczyt albo strumień nadal
> odpowiada globalnie, **to jest odpowiedź: selektor jeszcze nie**.
>
> Potem: wybór zakresu w URL i w nagłówku, zachowany przy przejściach między
> obszarami (to jest `NavArea.carries`, tyle że globalne), pusty stan dla kogoś
> bez żadnego grantu, i **jawne rozróżnienie danych nieprzypisanych** — przebieg
> globalny nie może wyglądać jak przebieg projektu. Zaktualizuj `apps/panel/CLAUDE.md`:
> zdejmujesz regułę, więc zapisz, co zajęło jej miejsce.

---

## Prompt — IAM-02/D: reszta data plane i cutover

> **Nie zaczynaj przed IAM-02/C.** Pracujesz nad **E6 i E7 z IAM-02**. Przeczytaj
> `docs/iam-02-data-plane.md` (E6, E7, sekcja 5 „Ryzyka"),
> `docs/iam-migration-runbook.md`, ADR_0033 i `docs/parallel-streams-2026-09-18.md`.
>
> **E6 — reszta data plane.** Silniki zapytań (`services/query`), runtime
> notebooków (`services/ml_pipeline`), poświadczenia i kolejki workerów,
> harmonogramy (dziś odmawiane po nazwie na związanym magazynie), archiwum rozmów
> (zamknięte po obu stronach), zakresowy sweep retencji, katalog artefaktów,
> eksport do VictoriaTraces i VictoriaMetrics. **Tu i tylko tu zdejmuje się dwa
> zakazy:** wolno zarejestrować `ProjectDispatcher` w produkcyjnym `spawn` i
> wolno otworzyć projektowy `/start`. Oba są zbudowane i przetestowane; brakuje
> wyłącznie okablowania — i ono musi brać właściciela z **trwałego
> `ExecutionOwnership`**, nigdy z planu, parametru, `requested_by`, nazwy workera
> ani autora deklaracji.
>
> **E7 — migracja i cutover.** Runbook mówi, ile z tego jest: 4 z 16 rodzin mają
> adapter, 11 nie ma, 1 jest zablokowana (`conversations` — kopia bajtów ich nie
> otworzy, ADR_0021), S3/RustFS niezweryfikowane. I rzecz, której żadne narzędzie
> nie załatwi: **kto ma dostęp po skopiowaniu, nie jest zdecydowane** —
> przypisanie danych do projektu nikomu nie nadaje do nich dostępu. To jest
> decyzja do podjęcia z użytkownikiem, nie do domyślenia się.
>
> Cutover: snapshot, czasowe zatrzymanie zapisów i ingestu, wykonanie manifestu,
> weryfikacja liczników i referencji, wznowienie. Nie zmieniaj hashy treści, ID
> wersji ani historycznych referencji. Nieprzypisane dane zostają niedostępne, a
> **rollback interfejsu nie może przywrócić globalnego dostępu do danych**.

---

## Prompt — PORZ-01: porządki

> Dwie niezależne, małe rzeczy w repozytorium AIWatcher. Rób je osobnymi
> commitami i nie łącz w jedną zmianę.
>
> **1. eslint w panelu.** `npm run lint` jest w `package.json`, ale `apps/panel`
> nie ma ani konfiguracji eslinta, ani samego eslinta w zależnościach — skrypt
> nie działa i nie działał, a CI woła `build` i `test`. Dodaj konfigurację
> pasującą do tego, co ten kod **naprawdę robi** (TypeScript, React, hooki), i
> **nie przeformatowuj panelu przy okazji** — reguła stylistyczna, która chce
> przepisać sto plików, jest wyłączana, nie stosowana; `apps/panel/.prettierrc`
> pilnuje formatu i to on zostaje autorytetem. Zielony `npm run lint` na czystym
> drzewie jest kryterium; dopiero potem rozważ dołożenie go do CI.
>
> **2. Eksport i retencja audytu IAM.** Tabela `iam_audit` rośnie bez końca i
> czyta się wyłącznie stronicowanym odczytem dla ownera/admina organizacji.
> Brakuje eksportu i retencji, i obie rzeczy mają tu precedens, którego trzeba
> się trzymać: **eksport jest zadaniem asynchronicznym, którego kursor przesuwa
> się dopiero po zapisaniu sharda** (`aiwatcher-jobs`, ADR_0022), a retencja
> nazywa własny zegar, nie cudzy. Przeczytaj `crates/aiwatcher-iam/README.md` i
> ADR_0022 zanim cokolwiek napiszesz. Migracja SQL **addytywna**, z rolling
> upgrade i rollbackiem starego binarium; istniejących nie ruszaj.

---

## IAM-02/B — zrobione (19.09.2026)

E3 i E4 stoją, bramka **M1** trzyma 44/44 na żywym serwerze.
[Plan IAM-02](iam-02-data-plane.md), sekcja 9, jest pełnym zapisem; ADR_0033 ma
aneks z tą datą. Trzy rzeczy, które dotykają innych strumieni:

- **`aiwatcher-projector` i `aiwatcher-api` (runs, metrics, live, stream)** —
  `ReadScope` jest argumentem każdego odczytu foldu przebiegów, a `runs`,
  `metrics` i `live` serwują jeden router dwa razy: pod `/api/v1` i pod
  `/api/v1/orgs/{organization}/projects/{project}`. Kontrakt urósł o **10
  ścieżek**; nic nie zniknęło.
- **Panel nietknięty ręcznie.** Zmienił się wyłącznie wygenerowany klient, bo
  nic w panelu nie musi jeszcze nazywać projektu — selektor to IAM-02/C. Jedna
  rzecz jest do dorobienia **razem z selektorem**, i lepiej, żeby nie została
  odkryta: `apps/panel/src/shared/lib/live.ts` nie zna ramki `revoked`. Dziś to
  nieosiągalne — strumień globalny nigdy jej nie dostaje — ale w chwili, w
  której panel otworzy strumień zakresowy, odwołanie grantu zamknie połączenie,
  `EventSource` spróbuje się wznowić, dostanie 404 i **nic tego nie narysuje**.
  Jeden wariant w `liveFrameSchema`, jeden `addEventListener('revoked', …)`
  i jedna faza strumienia.
- **Zastane na `main`, naprawione osobnym commitem:** merge IAM-02/A zostawił
  `cargo test --workspace --all-targets` niekompilujące się (fikstura
  `RunSummary` bez pola `project`) i trzy pliki poza `cargo fmt --check`.

**Dla IAM-02/C:** fold workflow i fold ewaluacji **nie mają projektu w wierszu**
— `/api/v1/workflows`, `/api/v1/workflow-executions`, `/api/v1/evaluations` i
`/api/v1/experiments` nadal odpowiadają instancyjnie, a strumień wykonania
workflow odpowiada stroną globalną. To jest ta odpowiedź, o którą pyta prompt
IAM-02/C: **selektor jeszcze nie**, dopóki te foldy nie zostaną okluczowane —
to reszta E2, nie E3.

---

## IAM-02/C — zrobione (19.09.2026)

Selektor organizacja/projekt jest w nagłówku, a reguła, która go tam nie
wpuszczała, została zdjęta i zastąpiona. [Plan IAM-02](iam-02-data-plane.md),
sekcja 10, jest pełnym zapisem; `apps/panel/CLAUDE.md` ma regułę, która zajęła
miejsce starej.

- **Najpierw pomiar, potem aktywacja.** `scripts/iam-permission-check.py` urosło
  z 44 do **47 pytań** i trzy nowe są tymi, które rozstrzygnęły kształt: zadane
  po odwołaniu grantu pokazują, że principal bez grantu **nadal czyta** graf
  workflow tego projektu, jego wykonanie i raport ewaluacji z tras
  instancyjnych. To reszta E2, nie awaria E3 — i to jest powód, dla którego
  selektor nazywa swój zasięg zamiast zakresować cały panel.
- **Panel.** `?scope=` na trasie root z `retainSearchParams`, jedno przepisanie
  w transporcie (`shared/lib/scope.ts`, zbiór tras sprawdzany testem wobec
  `contracts/openapi.json`), `reach` na każdym obszarze w `navigation.ts`,
  `reach-notice.tsx` mówiący, gdzie granica nie sięga, pusty stan dla kogoś bez
  grantu i brak jakiejkolwiek kontrolki na instancji bez organizacji.
- **Ramka `revoked` jest narysowana.** To była ta jedna rzecz, którą IAM-02/B
  zostawił na później: zbudowana na serwerze i nieosiągalna z panelu. Zmierzone
  w przeglądarce — odwołanie grantu przy otwartym `/observability/live`
  przestawia odznakę na `access revoked` po 30 s.
- **Rust nietknięty.** Żadnej nowej trasy, żadnego `just openapi`; kontrakt ma
  te same 251 ścieżek.

**Dla IAM-02/D:** `SCOPED_ROUTES` w `apps/panel/src/shared/lib/scope.ts` i
`reach` w `apps/panel/src/app/navigation.ts` są tym, co trzeba zmienić, gdy
okluczujesz kolejny fold — a test przy `SCOPED_ROUTES` sam powie, że czas to
zrobić, bo przestanie zgadzać się z kontraktem.
