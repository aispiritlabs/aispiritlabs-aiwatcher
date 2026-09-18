# Strumień B — projektowy katalog artefaktów, lineage, cache i retencja IAM-01

Jesteś agentem implementującym strumień B migracji IAM w AIWatcher. Wprowadź działający kod i testy izolacji. Nie otwieraj projektowych wykonań ani nie ogłaszaj zakończenia IAM.

## Przygotowanie i współpraca

- Przeczytaj instrukcje repozytorium, `docs/ux-migration-plan-2026-09-14.md`, ostatnią kontynuację dotyczącą bajtów artefaktów oraz `crates/aiwatcher-iam/README.md`.
- Przeczytaj ADR_0025 i ADR_0026 oraz istniejące reguły cache, lineage, retencji i kolejności zapisu.
- Pracuj na osobnej gałęzi/worktree z tego samego snapshotu co A/C/D. Snapshot musi zawierać aktualne zmiany niezacommitowane i pliki untracked, nie tylko stary HEAD. Nie resetuj, nie stashuj i nie nadpisuj cudzej pracy. Zgłoś brak snapshotu zamiast odtwarzać istniejącą implementację.
- A odpowiada za właścicielstwo wykonań, claims i dispatcher; C za ewaluacje; D za migrację. Nie zmieniaj ich modułów.
- Używaj istniejącego `aiwatcher_iam::ProjectScope`. Scope jest zaufanym kontekstem dostarczanym przez przyszłego dispatchera, nie polem planu ani swobodnym parametrem workera.

## Punkt wyjścia

`crates/aiwatcher-server/src/execution/artifacts.rs` ma już `Artifacts::for_project`, z kluczami `artifacts/scopes/<org>/<project>/registry/`. Izoluje tabele, oryginalny JSON, logi podów i receipty. Odczyt wymaga kanonicznego URI zgodnego z rodzajem i digestem; globalny czytnik odmawia projektowych URI. Testy: `crates/aiwatcher-server/tests/project_artifacts.rs`.

To NIE izoluje jeszcze `ObjectArtifactCatalog`, metadanych, lineage, cache ani raportowania retencji. Sprawdź aktualny kod przed zmianą.

## Cel

1. Dodaj projektowy zakres katalogu artefaktów, zgodny z przestrzenią bajtów. Zachowaj hashe treści, format referencji i globalne historyczne klucze. Odmów przepięcia już związanego katalogu.
2. Obejmij wszystkie metody `ArtifactCatalog`: record, by_digest, produced_by, cached, remember i invalidate. Identyczny digest, cache key lub execution ID w różnych projektach nie może współdzielić metadanych, provenance, wyniku cache ani invalidation.
3. Sprawdzaj referencje zarówno przed zapisem, jak i po odczycie trwałych rekordów. Podmieniony manifest/cache/lineage nie może odesłać do globalnego lub cudzego projektu. Znajomość URI lub hasha nie daje prawa odczytu. Nie ufaj ścieżkom zwróconym przez listowanie bez sprawdzenia zakresu.
4. Wspólną regułę kanonicznego klucza utrzymuj w jednym miejscu tam, gdzie korzystają z niej katalog i writer. Zachowaj kierunek zależności crate'ów: execution nie może importować server. Nie twórz uniwersalnego frameworka do wszystkich rejestrów.
5. Zachowaj semantykę: dane przed receiptem, manifesty przed cache, invalidation nie usuwa bajtów, pierwszy zapis provenance nie jest nadpisywany przez retry. Nie zmieniaj tych zasad przy okazji izolacji.
6. Sprawdź procesy mierzące/listujące artefakty, retencję i ewentualną kolekcję. Oddziel globalne wyniki od projektowych. Projektowe bajty nie mogą zostać uznane za globalne ani usunięte na podstawie braku globalnej historii. Nie dodawaj kasowania, jeśli nie ma scoped źródła prawdy o osiągalności.
7. Przygotuj niewielki, jawny kontrakt konstrukcji katalogu dla A. Sam katalog nie przyznaje uprawnień IAM; aktualny grant i lease muszą być sprawdzone przez caller. Nie uruchamiaj projektowego cache lookup w istniejącym globalnym reactorze.

## Bramka integracji

Nie rejestruj scoped katalogu/writera w produkcyjnym dispatcherze i nie dodawaj `/start`. Cache wymaga autoryzacji PRZED lookup, nie dopiero w executorze. Wynik projektu nie może trafić do globalnego outboxa, API czy strumienia. Brak części z A oznacza bibliotekę z testami i nazwanymi ograniczeniami, a nie fallback.

Jeżeli retencja wymaga nieistniejącego kontraktu z A, zakończ na bezpiecznej izolacji odczytu/raportowania i jawnej odmowie kolekcji tego zakresu. Nie zgaduj, które wykonania zakończono ani które artefakty są nieużywane.

## Własność plików

- `crates/aiwatcher-execution/src/artifact/` i powiązane testy.
- `crates/aiwatcher-server/src/execution/artifacts.rs`, testy projektowych artefaktów oraz moduły pomiarów/retencji artefaktów, jeśli niezbędne.
- Nowy współdzielony moduł layoutu w warstwie odpowiedniej dla zależności, jeśli rzeczywiście potrzebny.
- Bez zmian workflow store/handlera/claimów/dispatchera z A, ewaluacji z C, migracji z D i panelu.
- Nie zmieniaj wspólnego planu ani README IAM. Zmiany root eksportów ogranicz do koniecznego minimum i opisz dla integratora.

## Testy i odbiór

- Rzeczywisty plikowy object store: reopen, niezależność dwóch projektów jednej organizacji, innej organizacji i globalnego zakresu, identyczne bajty/digest/cache key/execution ID.
- Izolacja wszystkich metod katalogu, invalidation i provenance; brak globalnego fallbacku i rebindingu.
- Podmiana manifestu, wskaźnika lineage i cache; obcy URI, traversal, fałszywy digest/kind, listowanie spoza prefiksu. Odmowa nie może wykonać cudzej operacji I/O.
- Brak utraty dotychczasowej semantyki idempotencji, wygaśnięcia cache i nieusuwania bajtów przez invalidation.
- Testy retencji/raportowania: globalny pass nie obejmuje projektowych obiektów, brak źródła prawdy nie uruchamia kasowania. Błąd odczytu nie udaje pustego magazynu.
- Uruchom testy artifact/execution/server oraz Clippy dotkniętych crate'ów z `-D warnings`, format i `git diff --check`. Zostaw istniejące testy `project_artifacts` zielone.
- Nie używaj produkcyjnego S3, bazy ani klastra. Nie przedstawiaj pamięciowego testu jako testu S3.

## Raport

Zapisz `docs/iam-parallel-B-report.md`: layout kluczy i API konstrukcji dla A, zasady bezpiecznego użycia, pliki, wyniki testów, wymagania migracji dla D, ograniczenia i niewdrożone procesy. Odnotuj każdą zmianę kompatybilności starych referencji.

Nie edytuj wygenerowanego klienta ani OpenAPI, jeżeli nie zmieniasz HTTP. Wspólna dokumentacja migracji zostanie scalona przez integratora.
