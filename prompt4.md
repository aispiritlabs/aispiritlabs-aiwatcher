# Strumień D — manifest i bezpieczny wykonawca migracji IAM-01

Jesteś agentem implementującym strumień D migracji IAM w AIWatcher. Zbuduj narzędzia i testy migracji istniejących zasobów do jawnie wskazanego projektu. Nie migruj danych użytkownika i nie wykonuj produkcyjnego cutover.

## Przygotowanie i współpraca

- Przeczytaj instrukcje repozytorium, `docs/ux-migration-plan-2026-09-14.md`, rozdziały migracji i granic rejestrów w `crates/aiwatcher-iam/README.md`, a następnie ADR-y właścicieli zasobów objętych daną iteracją.
- Zbadaj istniejące `crates/aiwatcher-datasets/src/scope.rs` oraz `crates/aiwatcher-datasets/examples/migration_manifest.rs`. To częściowy, read-only dry-run, nie kompletny wykonawca.
- Pracuj w osobnej gałęzi/worktree na wspólnym snapshocie zawierającym bieżące niezacommitowane zmiany i nowe pliki. Sam stary HEAD nie jest takim snapshotem. Zgłoś brak, nie odtwarzaj pracy innych. Nie resetuj ani nie stashuj cudzych zmian.
- A implementuje wykonania, B artefakty/cache, C nowe ewaluacje. Ich layouty mogą się zmienić. Nie zgaduj ich finalnych formatów ani nie migruj nieobsługiwanej rodziny jako zwykłego katalogu plików.

## Cel i kolejność

1. Zaprojektuj wersjonowany manifest obejmujący jawny scope docelowy, pochodzenie snapshotu, obsługiwane rodziny, liczniki obiektów/bajtów, mapowanie kluczy, SHA-256 oryginalnych bajtów, referencje, stan celu i blokady. Manifest powinien być deterministyczny dla tego samego snapshotu i konfiguracji oraz sam posiadać stabilną tożsamość.
2. Rozszerz istniejącą inwentaryzację o kolejne STABILNE rejestry, począwszy od promptów oraz treningu/modeli. Potem anotacje i dalsze rodziny, o ile mają kompletny kontrakt. Raport musi jawnie rozróżniać rodzinę pustą, nieobsługiwaną i uszkodzoną. Brak adaptera nie jest liczbą zero ani sukcesem całej migracji.
3. Layout i semantykę referencji dostarcza właściciel rejestru. Nie twórz drugiego, swobodnego zestawu reguł kluczy w uniwersalnym kopierze. Preferuj małe API inventory/migration u właściciela i kompozycję na poziomie narzędzia. Zmiany publicznych seamów w modułach B/C uzgodnij lub pozostaw jako jawnie nieobsługiwane.
4. Waliduj istnienie wskazanej organizacji/projektu w autorytatywnym IAM w ścieżce wykonania. Dry-run offline może tylko jasno zaznaczyć brak tej walidacji. Grupy IdP i ich ewentualne mapowanie opisuj jawnie; nie twórz członkostw ani grantów na podstawie nazw grup/emaili. Mapowanie danych nie jest nadaniem dostępu.
5. Zaimplementuj wznawialne, idempotentne wykonanie manifestu dla ukończonych rodzin: weryfikacja źródłowych bajtów, brak nadpisywania konfliktów, weryfikacja celu, trwały checkpoint dopiero po poprawnym zapisie i odczycie celu. Po awarii już skopiowany, identyczny obiekt jest bezpiecznym powtórzeniem. Stan postępu wiąż z dokładnym manifestem i celem; nie pozwól wznowić go z innym zakresem.
6. Nie przepisuj treści, hashy wersji, ID ani historycznych referencji. Jeśli zachowanie referencji wymaga adaptera odczytu lub dodatkowej migracji właściciela, zgłoś blokadę zamiast podmieniać URI wewnątrz JSON. Waliduj referencje w zakresie, który adapter rozumie; nie deklaruj analizy zależności z arbitralnego tekstu query.
7. Opisz i przygotuj operatorowy protokół cutover: snapshot, zatrzymanie zapisów/ingestu, wykonanie, liczniki i referencje, wznowienie oraz rollback. Narzędzie nie może samo zatrzymywać produkcji, zmieniać deploymentu, usuwać źródła ani przełączać tras. Rollback danych nie polega na włączeniu globalnego API.

Pierwszy odbieralny etap: kompletny mechanizm manifest/checkpoint/resume dla kilku nazwanych stabilnych rodzin, z jawną listą nieobsługiwanych. Nie nazywaj go pełną migracją aplikacji, dopóki wszystkie rodziny i zależności nie zostały objęte.

## Warunki bezpieczeństwa

- Domyślnie dry-run; zapis wymaga osobnej, jawnej operacji i dokładnego manifestu. Przyjęty zakres i konflikt mają być widoczne przed wykonaniem.
- Pracuj na nieruchomym snapshocie. Zmiana/zniknięcie źródła lub obcy schemat przerywa migrację bez nadpisania celu. Wykrywanie nie może opierać się wyłącznie na count/mtime.
- Nie ufaj ścieżkom z edytowalnego manifestu. Ponownie sprawdzaj dozwolone prefiksy, scope, traversal, duplikaty i kolizje mapowania. Nie zapisuj do dowolnego klucza tylko dlatego, że manifest go podał.
- Sprawdź gwarancje create-only/conditional write adaptera. Na backendzie bez wymaganych gwarancji odmów wykonania lub jawnie ogranicz narzędzie do offline, wyłącznego dostępu; nie obiecuj odporności na równoległych writerów, której nie ma.
- Head/index publikuj dopiero po zależnościach zgodnie z regułami właściciela. Samo posortowanie nazw nie dowodzi poprawnej kolejności. Jeśli częściowy cel musi pozostać niewidoczny, zapewnij bramkę operacyjną i opisz ją w runbooku.
- Cache/derived indeksy nie są automatycznie danymi do kopiowania. Autoratywne etykiety, approvals i withdrawal markers nie mogą zginąć jako rzekomo odtwarzalne indeksy.
- Zaszyfrowanego archiwum rozmów nie kopiuj pod inną ścieżkę: ścieżka uczestniczy w derivation/AAD. Bez dedykowanego protokołu właściciela ta rodzina jest BLOKADĄ, nie wspieranym copy. Nie obchodź encryption/retention/erasure.
- Nie wykonuj migracji na aktualnym katalogu danych użytkownika, istniejącej bazie projektu, produkcyjnym S3 ani klastrze. Używaj jednorazowych fixtures.

## Własność plików

- Nowy moduł/narzędzie migracji, jego testy, runbook oraz istniejący seam inwentaryzacji datasetów.
- Niewielkie adaptery inventory stabilnych rejestrów, które nie należą do A/B/C.
- Bez edycji ich store/dispatchera, katalogu artefaktów i scope ewaluacji. Brakujący publiczny kontrakt zapisz dla integratora zamiast duplikować prywatny layout.
- Bez zmian panelu, aktywacji selektorów, routingu produkcyjnego czy automatycznego cutover. Nie zmieniaj wspólnego planu i README IAM w tej gałęzi.

## Testy i odbiór

- Deterministyczny dry-run bez zapisów; oryginalne bajty, hashe/ID i referencje zachowane po kopii i reopen.
- Pusty, częściowy i konfliktowy cel; brak nadpisania różniących się danych; identyczne dane jako no-op.
- Przerwanie przed zapisem, po zapisie przed checkpointem i po checkpointcie; wznowienie bez utraty obiektów i podwójnego liczenia.
- Błąd zapisu i read-back, zmiana/zniknięcie źródła, uszkodzony/obcy schemat, podmieniony manifest/checkpoint/scope, traversal i kolizje kluczy.
- Referencje brakujące, zależności spoza wspieranej rodziny, nieistniejący cel IAM; każde jako nazwany stan lub blokada, nigdy pozorny sukces.
- Test CLI na tymczasowym magazynie plikowym oraz testy użytych adapterów. Jeśli deklarujesz obsługę PostgreSQL/S3, zweryfikuj ją na osobnym testowym backendzie albo jednoznacznie nazwij jako niezweryfikowaną.
- Uruchom testy dotkniętych crate'ów, Clippy `-D warnings`, format i `git diff --check`. Nie raportuj nieuruchomionych testów jako zaliczonych.

## Raport

Zapisz `docs/iam-parallel-D-report.md` oraz osobny runbook narzędzia: obsługiwane rodziny i ograniczenia, polecenia dry-run/apply/resume, dowody walidacji, lista blokad cutover, wymagania wobec A/B/C. Zaznacz wprost, że nie przeprowadzono migracji danych użytkownika ani wdrożenia. Integrator zaktualizuje wspólny plan na podstawie raportów wszystkich strumieni.
