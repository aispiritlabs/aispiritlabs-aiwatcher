# Strumień B — projektowy katalog artefaktów, lineage, cache i retencja IAM-01

**Etap biblioteczny. Nie otwiera projektowych wykonań i nie zamyka IAM-01.**
Gałąź: `claude/iam-parallel-B` w osobnym worktree
`.claude/worktrees/iam-parallel-B`, ze snapshotu zawierającego niezacommitowane
i nowe pliki z `main@87b1f88` (24 zmodyfikowane + 23 untracked — stan zgodny z
kopią roboczą w chwili startu). Nic cudzego nie zostało zresetowane ani
zastashowane; wspólny checkout nie był dotykany. `cargo fmt --all` uruchamiane
było **wyłącznie w tym worktree**, a porównanie patchy przed/po potwierdza, że
zmieniło tylko pliki z sekcji 9 — pliki A/C/D ze snapshotu były już
sformatowane.

Punkt wyjścia: `Artifacts::for_project` izolował już **bajty i receipty**
(`artifacts/scopes/<org>/<project>/registry/`). Ten etap dokłada drugą połowę —
**metadane, lineage i cache** — mierzy je osobno i jawnie odmawia kolekcji.

---

## 1. Layout kluczy — jedno miejsce

`aiwatcher_execution::artifact::layout` jest teraz jedynym miejscem, które mówi,
jak wygląda klucz pod prefiksem artefaktów. Czyta go **katalog** (ten sam crate)
i **writer bajtów** (`aiwatcher-server`), więc manifest i opisywany przez niego
obiekt nie mogą trafić pod dwie różne reguły.

```text
artifacts/<kind>/<aa>/<sha256>/data                  bajty            (writer)
artifacts/<kind>/<aa>/<sha256>/manifest.json         co to jest       (katalog)
artifacts/cache/<sha256(cache_key)>.json             co dał klucz     (katalog)
artifacts/lineage/<sha256(execution_id)>/<sha256>.json  co dał przebieg (katalog)
artifacts/receipts/<sha256(idempotency_key)>.json    co dała próba    (writer)

artifacts/scopes/<organization>/<project>/registry/…  te same pięć rodzin
```

- `<organization>` i `<project>` to UUID zapisane dokładnie tak, jak zapisuje je
  ten kod (hyphenated, lowercase). Inny zapis tego samego identyfikatora to
  `Unattributed`, nigdy „ten projekt”.
- `scopes` nie jest żadnym `ArtifactKind` ani rodziną (`cache`, `lineage`,
  `receipts`), więc rozdział globalnego od projektowego jest alfabetem, a nie
  kontrolą, o której trzeba pamiętać.
- **Zakres jest poza digestem.** Te same bajty w dwóch projektach to jeden
  content address i dwa URI. Klucze globalne nie zmieniły się o znak:
  `layout::prefix(None) == "artifacts"`.

### API modułu

| Funkcja | Odpowiada na |
|---|---|
| `prefix(Option<ProjectScope>) -> String` | gdzie mieszka jeden zakres |
| `data_key / data_uri / manifest_key` | kanoniczny klucz bajtów i manifestu |
| `cache_entry_key / lineage_prefix / lineage_key` | pozostałe rodziny katalogu |
| `is_content_address(&str) -> bool` | `ArtifactRef::has_digest` zapytane o goły string (pożyczona odpowiedź core, nie druga implementacja) |
| `addresses(prefix, &ArtifactRef) -> bool` | czy to dokładnie ten wskaźnik, który ten prefiks by wyemitował (URI + digest + kind razem) |
| `scoped_area() / reaches_a_project(&str)` | czy referencja sięga w obszar projektów |
| `owner_of(&str) -> KeyOwner` | `Global \| Project(scope) \| Unattributed \| Elsewhere` |

Kierunek zależności zachowany: `layout` leży w `aiwatcher-execution` (który już
zależy od `aiwatcher-iam`), a `aiwatcher-server` zależy od execution.
**Execution nie importuje server.** Nie powstał żaden uniwersalny framework dla
wszystkich rejestrów — to jest rodzina kluczy artefaktów i nic poza nią.

---

## 2. API konstrukcji dla A

```rust
// aiwatcher-execution
ObjectArtifactCatalog::new(store)                      // zakres deploymentu
ObjectArtifactCatalog::for_project(scope) -> Result<Self, StoreError>
ObjectArtifactCatalog::project_scope() -> Option<ProjectScope>
ObjectArtifactCatalog::prefix() -> &str

// aiwatcher-server — obie połowy naraz
ProjectArtifacts::bind(&Arc<dyn ObjectStore>, ProjectScope) -> Result<Self, ActivityError>
ProjectArtifacts::artifacts() -> &Artifacts          // bajty i receipty
ProjectArtifacts::catalog()   -> &Arc<dyn ArtifactCatalog>
ProjectArtifacts::scope()     -> ProjectScope
Artifacts::prefix() -> &str                          // nowe, do sparowania
```

`bind` buduje obie połowy z jednego zakresu i **porównuje prefiksy po fakcie**,
zamiast ufać, że dwa razy przekazał ten sam scope. Każdą połowę nadal można
związać osobno (robią tak istniejące testy magazynu) — to kształt, który dostaje
caller, a nie mur wokół części.

Przepięcie związanego katalogu na drugi projekt to `StoreError::OutOfScope`;
przypięcie do już trzymanego zakresu jest idempotentne.

### Zasady bezpiecznego użycia (dla przyszłego dispatchera)

1. **Scope pochodzi z zaufanego, trwałego właścicielstwa wykonania** — nigdy
   z planu, parametru żądania, nazwy workera ani autora deklaracji.
2. **Grant i lease sprawdza caller, PRZED zapytaniem — łącznie z cache
   lookup.** Trafienie w cache jest odpowiedzią o danych projektu niezależnie od
   tego, czy potem cokolwiek się wykona. Katalog nie czyta IAM.
3. **Nie wolno podać zakresowego katalogu globalnemu reactorowi.**
   `Reactor::cached` zamienia błąd katalogu na `None` i loguje `warn`, więc
   odmowa `Policy` byłaby tam niewidoczna — wykonałby pracę i zapisał wynik
   globalnie.
4. **Nie wolno parować zakresowego magazynu bajtów z globalnym katalogiem.**
   Stąd `ProjectArtifacts::bind`.
5. Wynik projektu nie może trafić do globalnego outboxa, API ani strumienia —
   to pozostaje po stronie A.

---

## 3. Co jest izolowane

Wszystkie sześć metod `ArtifactCatalog`, w obu kierunkach:

| Metoda | Izolacja | Kontrola referencji |
|---|---|---|
| `record` | klucz manifestu i lineage pod prefiksem zakresu | **przed zapisem** + na ścieżce idempotentnej (podmieniony manifest nie zostaje „już zapisany”) |
| `by_digest` | jw. | **po odczycie**: digest i kind zgodne z kluczem, wskaźnik z tej przestrzeni |
| `produced_by` | prefiks lineage z hasha execution ID | klucz z listowania sprawdzany **przed otwarciem**, wskaźnik musi opisywać własny klucz, provenance musi nazywać pytany przebieg |
| `cached` | klucz z hasha cache key pod prefiksem | **po odczycie**: `cache_key` zgodny z pytanym, każdy artefakt z tej przestrzeni |
| `remember` | jw. | **przed zapisem**: każdy artefakt z tej przestrzeni |
| `invalidate` | jw. | przez te same kontrole co trafienie — wpisu, którym ta przestrzeń nie może odpowiedzieć, nie przepisuje |

Identyczny digest, identyczny cache key i identyczny execution ID w dwóch
projektach jednej organizacji, w dwóch organizacjach i globalnie **nie dzielą
metadanych, provenance, wyniku cache ani invalidation**. Brak fallbacku w obie
strony.

**Znajomość URI lub hasha nie daje prawa odczytu**, a odmowa nie wykonuje cudzej
operacji I/O: referencja jest sprawdzana przed jakimkolwiek `get`/`put`, a klucz
z listowania przed jego otwarciem (to ostatnie było błędem w pierwszej wersji —
czytanie klucza, żeby sprawdzić, czy wolno go czytać, jest dokładnie tym I/O,
któremu odmowa ma zapobiec).

**Odmowa vs. miss.** Rekord, którego ten build nie umie sparsować, pozostaje
**miss** — indeks jest z założenia porzucalny. Rekord, który *da się* sparsować i
opisuje coś innego (inny klucz, inny przebieg, inna przestrzeń), jest
**odmową** (`StoreError::OutOfScope`). To reguła receiptów z poprzedniego etapu,
rodzinę dalej; cicho skrócona lista byłaby tym, czemu ta reguła zapobiega.

---

## 4. Zachowana semantyka

Nie ruszona przy okazji izolacji:

- **dane → receipt** (writer, bez rereadu i bez transakcji na zapis receiptu),
- **manifesty → cache** (`record_outputs`: wpis indeksu dopiero po opisach),
- **invalidation znaczy, nie kasuje** — ani wpisu, ani artefaktów, które nazywa,
- **pierwszy zapis provenance wygrywa** — retry deterministyczny nie nadpisuje
  ani `produced_by`, ani `inputs`,
- **wygaśnięcie i brak wpisu to jedna odpowiedź** (`None`), polityka zostaje w
  katalogu, a nie w każdym callerze,
- weryfikacja hasha odczytanych bajtów i reguła kanonicznej referencji writera.

---

## 5. Pomiar, retencja i kolekcja

`crates/aiwatcher-server/src/execution/measure.rs`:

- `summarise` zwraca teraz `Storage` — **partycję** przejścia po prefiksie:
  `global`, `projects` (jeden kubełek na projekt, który cokolwiek trzyma),
  `unattributed` (pod `artifacts/scopes/`, bez rozpoznawalnego projektu) i
  `elsewhere` (klucz spoza pytanego prefiksu — błąd adaptera). Każdy klucz trafia
  do dokładnie jednego kubełka; suma serii to całe przejście.
- Metryki: istniejąca seria `aiwatcher.artifacts.{objects,bytes,oldest_days}`
  z `prefix="artifacts"` liczy **tylko zakres deploymentu**. Projekt dostaje
  własną serię z `prefix`, `organization` i `project`; `unattributed` — serię z
  `prefix="artifacts/scopes"` i **bez** identyfikatorów. `elsewhere` nie dostaje
  serii, tylko `warn` w logu: to fakt o magazynie, nie o tym, co w nim leży.
- Projekt, którego przejście nie znalazło, jest **nieobecny, a nie zerowy** — to
  przejście nie wie, jakie projekty istnieją.
- `walk` zwraca `None`, gdy listowanie się nie powiodło. **Błąd odczytu nie
  udaje pustego magazynu** — zero narysowałoby urwisko w wykresie przy każdej
  gorszej minucie bucketu, a dla serii projektu czytałoby się jak projekt,
  którego magazyn zniknął.

**Kolekcja: jawna odmowa.** Nie ma kolektora artefaktów i ten etap żadnego nie
dodaje. Retencja workflow (`store.prune`) dotyczy `WorkflowStore`, nie object
store'a, i nie chodzi za artefaktami. Dla obszaru zakresowego **nie istnieje
scoped źródło prawdy o osiągalności** — projektowa historia wykonań jeszcze nie
istnieje (bramka A). Pass, który usuwałby to, czego nie nazywa żadna *globalna*
historia, skasowałby bajty projektu za brak globalnej historii; to jest dokładnie
ta awaria, której ten podział ma zapobiec, i dlatego pomiar tylko czyta.

---

## 6. Zmiany kompatybilności starych referencji

| Zmiana | Wpływ |
|---|---|
| Klucze globalne | **Bez zmian.** `layout::prefix(None) == "artifacts"`. |
| Globalny katalog odmawia referencji z digestem, który nie jest sha256 | Digest jest segmentem ścieżki; `../secret` budował klucz poza prefiksem. Produkcyjnie wszystkie referencje pochodzą z `Artifacts::put_*`, więc realnych rekordów to nie dotyczy. |
| Globalny katalog odmawia referencji sięgającej w obszar projektów | Druga połowa reguły bajtów. Obejmuje też inne schematy (`s3://`, `file://`) nazywające `artifacts/scopes/` — kontrola podciągiem, celowo tępa, bo to reguła *deny* dla przestrzeni permisywnej. |
| Globalny katalog **nadal** przyjmuje `file://`, `s3://` i niekanoniczne `object://` | Zawsze zapisywał wskaźniki, których nie stworzył. Zawężenie tego gubiłoby wiersze zamiast cokolwiek izolować. |
| `by_digest` z digestem, który nie jest content addressem | `Ok(None)` zamiast budowania klucza. Żadnego I/O. |
| Odczyt odrzuca parsowalny rekord opisujący co innego | Nowe. W praktyce nieosiągalne bez podmiany w magazynie. |
| `StoreError::OutOfScope` | **Nowy wariant wspólnego enuma.** `says_the_same_next_time() == true`. `aiwatcher-api/src/error.rs` mapuje go istniejącą gałęzią `HandleError::Store(_)` → **503 `workflow_store_unavailable`**, co dla trwałej odmowy jest obietnicą powrotu. Zostawione bez zmian (nie mój plik, i nieosiągalne dziś); **do rozważenia przez integratora** — 502 pasowałoby lepiej. |
| `summarise` zwraca `Storage` zamiast `Stored` | Sygnatura publiczna. `Stored` zostaje jako kubełek. |
| Znaczenie `aiwatcher.artifacts.*` z `prefix="artifacts"` | Z „cały prefiks” na „zakres deploymentu”. Dziś liczby są identyczne, bo żadna ścieżka produkcyjna nie zapisuje artefaktów projektowych. |
| Kontrakt HTTP, OpenAPI, klient panelu | **Bez zmian.** Nie regenerowano. |

---

## 7. Wymagania migracji dla D

- **Content address się nie zmienia; zmienia się prefiks klucza.** Migracja
  „dopisz projekt” to skopiowanie `artifacts/<kind>/<aa>/<digest>/…` pod
  `artifacts/scopes/<org>/<proj>/registry/<kind>/<aa>/<digest>/…` bajt w bajt.
- **Ale referencja jest częścią rekordu.** `ArtifactRef.uri` zmienia się razem z
  prefiksem, a URI siedzi w *treści* manifestu, wpisu cache i receiptu — i poza
  katalogiem: w strumieniach wykonań, w zdarzeniach `artifact.produced` na logu
  zdarzeń i w wersjach datasetów. Czysta kopia katalogów daje rekordy, które
  odczyt odrzuci jako `OutOfScope` (kontrola po odczycie), a nie wskaźniki „ze
  starego świata”. **Nie migrować rodziny artefaktów jako zwykłego katalogu
  plików.**
- Pięć rodzin na zakres; ich klucze mówi wyłącznie `layout`. Do inwentarza
  dry-run należy użyć `layout::owner_of`, a nie własnego parsera prefiksu.
- `lineage` hashuje execution ID, a `produced_by` wymaga, by
  `manifest.produced_by.execution_id` nazywał przebieg, pod którego prefiksem
  leży wskaźnik. Migracja, która przeniesie manifesty bez wskaźników (albo
  odwrotnie), da odpowiednio pusty `produced_by` albo odmowę.
- Receipt: klucz to `sha256(idempotency_key)` **wewnątrz** prefiksu, a treść
  trzyma referencję sprawdzaną przy odczycie.
- **Rekomendacja: nie migrować historycznych artefaktów w pierwszym cutoverze.**
  Przestrzeń globalna pozostaje czytelna i nic z niej nie „wypada” — fallbacku
  nie ma w żadną stronę, więc pozostawienie ich na miejscu nic nie kosztuje,
  a przepisanie URI w strumieniach i na logu zdarzeń kosztuje dużo.

---

## 8. Ograniczenia i procesy niewdrożone

- **To nie jest autoryzacja.** Żaden grant, lease ani principal nie jest tu
  czytany. Posiadanie `ProjectArtifacts` nie jest uprawnieniem.
- **Nic nie jest podłączone.** `execution/mod.rs`, `wiring.rs` i reactor
  pozostają globalne i niezmienione. Brak projektowego `/start`, dispatchera,
  claimów, historii, strumieni i selektorów UI.
- `MemoryArtifactCatalog` **nie jest zakresowalny**. Produkcyjnie nie jest
  wiązany nigdzie (`execution/mod.rs` i `wiring.rs` budują wyłącznie
  `ObjectArtifactCatalog` z object store'a), więc deployment bez object store'a
  nie ma katalogu w ogóle — dzielenie metadanych między projektami jest tam
  niemożliwe, a nie „niesprawdzone”.
- Zakresowy katalog zapisuje **tylko kanoniczne referencje własnej
  przestrzeni**. Krok, który oddaje dalej wskaźnik `file://`, nie zostanie w
  projekcie skatalogowany (reactor loguje i idzie dalej). Świadome i opisane.
- **Zastana luka, której nie zmieniałem:** `record` przy trafieniu w istniejący
  manifest wraca wcześnie i nie zapisuje wskaźnika lineage dla *drugiego*
  wykonania tych samych bajtów; awaria między zapisem manifestu a zapisem
  wskaźnika gubi ten wskaźnik na stałe. Reguła „pierwsza provenance wygrywa” jest
  udokumentowana, a kontrola krzyżowa w `produced_by` na niej stoi — zmiana tego
  jest osobną decyzją, nie skutkiem ubocznym izolacji.
- **Kardynalność metryk** rośnie o jeden zestaw serii na projekt trzymający
  artefakty. Ograniczone liczbą takich projektów, raz na godzinę.
- `inputs` w manifeście **nie są walidowane** jako content addressy: to notatka
  lineage, nigdy klucz, a odmowa zapisu manifestu z powodu digestu *wejścia*
  kosztowałaby opis wyjścia.
- **Nie użyto** produkcyjnego S3/RustFS, PostgreSQL ani klastra. Testy jadą na
  `FileObjectStore` (prawdziwy katalog na dysku, z reopenem) i
  `MemoryObjectStore`. Żaden test pamięciowy nie jest przedstawiany jako test S3.

---

## 9. Pliki

Nowe:

- `crates/aiwatcher-execution/src/artifact/layout.rs`
- `crates/aiwatcher-execution/tests/artifact_scope.rs`
- `docs/iam-parallel-B-report.md`

Zmienione:

- `crates/aiwatcher-execution/src/artifact/mod.rs` — `pub mod layout`, dokumentacja granicy
- `crates/aiwatcher-execution/src/artifact/object.rs` — zakres, kontrole referencji, klucze z `layout`
- `crates/aiwatcher-execution/src/error.rs` — `StoreError::OutOfScope` (+ `says_the_same_next_time`)
- `crates/aiwatcher-server/src/execution/artifacts.rs` — klucze z `layout`, `Artifacts::prefix`, `ProjectArtifacts`
- `crates/aiwatcher-server/src/execution/measure.rs` — `Storage`, podział przejścia, `walk`
- `crates/aiwatcher-server/tests/project_artifacts.rs` — dwa testy dołożone do pięciu istniejących

Nietknięte: workflow store/handler/claim/dispatcher (A), ewaluacje (C), migracja
(D), panel, `crates/aiwatcher-iam/README.md`, wspólny plan, kontrakt OpenAPI i
wygenerowany klient. Zmiany eksportów rootowych: **żadne** — `layout` jest
dostępny jako `aiwatcher_execution::artifact::layout`.

---

## 10. Wyniki testów

```text
cargo test --workspace --all-targets      1715 passed, 0 failed, 7 ignored
cargo test -p aiwatcher-execution           300 passed   (285 przed etapem)
cargo test -p aiwatcher-server              306 passed, 1 ignored (RustFS/S3, istniejący)
                                                        (300 + 1 przed etapem)
cargo clippy --workspace --all-targets --all-features -- -Dwarnings   czysto
cargo fmt --all && git diff --check        czysto
```

Nowe pokrycie:

- `tests/artifact_scope.rs` (8): cztery przestrzenie nad prawdziwym magazynem
  plikowym z reopenem i jednym digestem/kluczem cache/execution ID; brak
  fallbacku w obie strony; odmowa przepięcia; wskaźniki cudzych przestrzeni,
  traversal, zakodowane separatory, podwójny ukośnik, `?query`, klucz innej
  rodziny, niezgodny kind/digest, fałszywy digest — wszystkie **bez I/O**;
  permisywność przestrzeni deploymentu z zamkniętym przejściem do projektu;
  podmieniony manifest / wpis cache / wskaźnik lineage i listowanie spoza
  prefiksu; zachowana idempotencja, wygaśnięcie i brak kasowania.
- `artifact/layout.rs` (5) — prefiksy, rozłączność z rodzajami, `owner_of`,
  kanoniczność referencji, content address.
- `artifact/object.rs` (+2) — globalny katalog nie opisuje artefaktu projektu;
  digest, który nie jest content addressem, nie staje się kluczem.
- `execution/measure.rs` (+4) — bajty projektu nie są liczone jako
  deploymentu; partycja kubełków i nazwy serii; projekt bez obiektów nieobecny,
  nie zerowy; nieosiągalny prefiks nie udaje pustego magazynu.
- `tests/project_artifacts.rs` (+2) — `ProjectArtifacts::bind` wiąże obie
  połowy; cały ślad jednej próby (bajty, receipt, manifest, lineage, cache)
  mierzony jako projektu i przez nic nie kasowany.

---

## 11. Następna bramka

Trwałe właścicielstwo wykonania (principal + scope zapisane razem z
wykonaniem), izolacja claimów, historii, komend i strumieni, sprawdzanie
bieżącego grantu przy podejmowaniu pracy i przy publikacji — dopiero wtedy
dispatcher, który zbuduje `ProjectArtifacts` i **przed** lookupem w cache
sprawdzi grant. Do tego czasu ta biblioteka nie ma callera na ścieżce
produkcyjnej i to jest stan zamierzony.
