# Strumień C — projektowe pomiary sędziowskie i zewnętrzne IAM-01

Jesteś agentem implementującym strumień C migracji IAM w AIWatcher. Rozszerz izolację ewaluacji o brakujące zależności projektowych pomiarów sędziowskich i zewnętrznych. Wprowadź kod i testy, ale nie otwieraj ścieżki wykonania przed integracją z A/B.

## Przygotowanie i współpraca

- Przeczytaj instrukcje repozytorium, `docs/ux-migration-plan-2026-09-14.md` oraz sekcje ewaluacji, kalibracji, admission i wykonawców w `crates/aiwatcher-iam/README.md`.
- Przeczytaj ADR_0030 i odpowiednie ADR-y dotyczące rejestrów, danych rozmów i wykonywania kroków. Sprawdź obecne ograniczenia w kodzie — nie usuwaj guardów tylko dlatego, że blokują test happy path.
- Osobna gałąź/worktree musi pochodzić z tego samego snapshotu co A/B/D, obejmującego również aktualne niezacommitowane i nowe pliki. Nie zaczynaj od starego HEAD bez tej pracy; zgłoś brak snapshotu. Nie resetuj, nie stashuj i nie nadpisuj cudzych zmian.
- A odpowiada za trusted ownership/dispatcher, B za katalog artefaktów/cache, D za migrację. Nie implementuj własnego workflow store, dispatchera ani autoryzacji opartej na autorze deklaracji.
- Używaj istniejącego `ProjectScope` oraz exact provider/subject principalu. Rola instancji ani ownership organizacji nie zastępują grantu projektu.

## Punkt wyjścia

Projektowe zasoby obejmują rubryki, oceny, scorecards, natywne kohorty, nagrania, bundle files, producenckie approvals/results i kalibracje. Deklaracje oraz admission pomiarów serwerowych obsługują ograniczony zestaw: nagrania + natywne kohorty + wbudowane scorery. Projektowe sędziowie/scorer service/generowanie/archiwum są w różnym zakresie nadal odrzucane. Biblioteczny executor nagrań ma jawną `ProjectAuthority`, ale nie ma projektowego `/start` ani produkcyjnego dispatchera.

## Zakres i kolejność

1. Zinwentaryzuj brakujące zależności projektowej deklaracji sędziowskiej i zewnętrznej: konfiguracje, przypięte rubryki, kalibracje, opis metryki/release/modelu, przechowywane odpowiedzi i admission. Zapisz, co jest zasobem projektu, a co konfiguracją deploymentu.
2. Najpierw obsłuż jeden pion — projektowy pomiar sędziowski nad nagraniem i natywną kohortą. Dodaj trwałe projektowe konfiguracje/odpowiedzi tylko tam, gdzie rzeczywiście wymaga ich istniejąca architektura. Nie twórz drugiego modelu scorerów ani obliczeń.
3. Każdą zależność rozwiązuj w tym samym projekcie. Identyczny globalny lub sąsiedni hash/ID nie zastępuje lokalnej publikacji. Zachowaj adresowanie treścią, idempotencję i weryfikację po reopen. Uszkodzenie konfiguracji lub zależności po admission nie może pozostawić fałszywego `admitted: true`.
4. Następnie rozszerz ten wzorzec na zewnętrzne scorery. Katalog możliwości usługi może być deploymentowy; publikowana karta musi nadal pinować opis release/model/unit/direction/aggregation/inputs. Nie przenoś credentiali ani adresów usług do projektu lub planu.
5. Używaj tych samych reguł admission, kalibracji i publikacji co istniejący globalny pomiar. Refaktoryzuj wspólny mechanizm zamiast kopiować go pod prefiksem projektu. Nie luzuj guardów globalnych.
6. Jeśli dodajesz projektowe HTTP, używaj istniejących extractorów i handlerów: bieżący grant, odpowiednia rola projektu, `X-AIWatcher-IAM: 1` przy mutacji, ponowna kontrola po uploadzie i `Cache-Control: no-store`. Actor pochodzi z uwierzytelnienia, nie z body. Project admin zatwierdza admission; editor nie awansuje do niego przez rolę instancji.
7. Przygotuj kontrakt dla A dotyczący sposobu uzyskania scoped rejestru i zależności. Ewentualne bezpośrednie testy executorów muszą dostać jawne authority, nigdy traktować approval jako zgody na wykonanie.

Jeżeli oba piony nie mieszczą się w jednej bezpiecznej iteracji, ukończ sędziego wraz z pełnymi testami izolacji i opisz zewnętrznych scorerów jako kolejny etap. Nie otwieraj szerokich uprawnień biblioteki dla nieprzetestowanych rodzin.

## Granice bezpieczeństwa

- Bez projektowego `/start`, generowania odpowiedzi, approval lines, globalnych obserwacji ani integracji strumieni. Te rzeczy wymagają A i dalszej izolacji telemetrycznej.
- Archiwum rozmów pozostaje zamknięte dla tej ścieżki. Nie rozszerzaj na nie pomiarów przy okazji; wymaga osobnych zasad seal/retention/erasure, admin admission i ujawnienia wysyłki treści do dostawcy.
- Nie zapisuj pytań/odpowiedzi/reasoning sędziego w niezabezpieczonych odpowiedziach cache lub na logu. Zachowaj `JudgeReply::kept` i przypiętą konfigurację.
- Odpowiedzi mają być ponownie używane wyłącznie w swoim projekcie i deklaracji. Nie da się ominąć sprawdzenia aktualnego dostępu przez cache odpowiedzi.
- Nie podłączaj executora do produkcyjnego registry/reactora przed bramką A/B. Odmowa nieobsługiwanego runtime'u ma pozostać jawna.

## Własność plików

- `crates/aiwatcher-evaluation/src/` dotyczące scope, judge, external, scoring, approval i store oraz powiązane testy.
- Projektowe resolvery w `crates/aiwatcher-server/src/evaluation/` i testy `tests/evaluation/`.
- Niezbędne scoped fasady API, schematy i testy HTTP ewaluacji.
- Nie zmieniaj `execution/scoring/project.rs`, workflow store ani dispatchera z A. Wspólny server `execution/scoring.rs` wymaga uzgodnienia; preferuj wydzielone moduły.
- Ten strumień odpowiada za regenerację OpenAPI i klienta, jeśli zmienia HTTP. Nie edytuj generowanych plików ręcznie. Integrator ponownie wygeneruje je po scaleniu wszystkich zmian.

## Testy i odbiór

- Produkcyjny resolver + plikowy object store: lokalne zależności, reopen, zgodność hashy, idempotencja, podmienione źródła, brak globalnego fallbacku, sąsiedni projekt/organizacja z identycznymi ID.
- Admission/withdrawal, brak kalibracji ludzi, cudza rubryka/karta/kalibracja/config, pin mismatch release/model, utrata źródła po approval.
- Izolacja przechowywanych odpowiedzi, retry bez ponownego pytania, brak treści w trwałym kept reply. Kontrolowany lokalny serwer sędziego/scorera zamiast płatnego providera lub rzeczywistych danych użytkownika.
- Dla HTTP: podpisane sesje, role/granty przed startem/po expiry/cofnięciu, upload po utracie uprawnienia, brak nagłówka, brak IAM/auth i no-store. Nie wystarczy dubel sprawdzający wyłącznie prefiks.
- Testy evaluation/API/server, Clippy `-D warnings`, format, `git diff --check`; po zmianie HTTP `just openapi`, `just openapi-check` i build panelu. Testy optional scorer service tylko jeśli go zmieniasz.

## Raport

Zapisz `docs/iam-parallel-C-report.md`: ukończone piony, rzeczywiste ograniczenia, kontrakt dla A, nowe layouty dla D, zmienione pliki, polecenia i wyniki. Nie edytuj wspólnego planu migracji ani README IAM podczas równoległej pracy. Brak E2E z Authentikiem/PostgreSQL/S3/workerem nazwij wprost.
