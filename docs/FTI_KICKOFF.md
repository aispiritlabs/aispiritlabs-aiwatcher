# FTI — kickoff implementacji w nowej sesji

Przygotowano: 2026-09-11. Repozytorium: `/Users/mkubaszek/Projects/ai_spirit/aiwatcher`.

**Checkpoint po implementacji (2026-09-11):** A1–A4 i AR1 dostarczone; aktualny odbiór i ograniczenia opisuje [sekcja 8 planu](FTI_IMPLEMENTATION_PLAN.md#8-postęp-implementacji-2026-09-11). Pełne `just check` przeszło. Zachowano wcześniejsze zmiany AW-4. Istotny incydent dodatkowego seeda: przed naprawą obsługi `--base-url` jego część treningowa dodała demonstracyjny model/run i przestawiła etykietę `demo.segmenter/production` na istniejącej instancji :8080; szczegóły i ID w planie. Użytkownik zdecydował o pozostawieniu nowej wersji demonstracyjnej. Dalszy zakres rozwoju to B1/AR2, a nie powtarzanie A.

## Cel sesji

Zrealizuj pierwsze wydanie FTI: **A1–A4 z planu implementacji**, z zachowaniem vertical slices i granic kontekstów. Dodaj małą bramkę zależności **AR1**, zgodnie z audytem architektury. Zacznij od przeglądu aktualnego stanu i przejdź do implementacji; nie kończ na kolejnym planie.

Rezultat dla użytkownika: wybór 2–5 treningów i porównanie ich parametrów/krzywych, jawny baseline ewaluacji, powrót do analizy przez URL oraz lokalny zapis widoku, podstawowe linki pochodzenia.

To pierwszy zamknięty zakres. Etapy B/C/D są dalszą roadmapą. Wydzielenie trwałej domeny Evaluation i usługi startu workflow wykonujemy w terminach opisanych w planie; nie stanowią warunku rozpoczęcia porównania treningów.

## Przeczytaj na początku

1. Obowiązujące `AGENTS.md` oraz `/Users/mkubaszek/.codex/RTK.md`. Polecenia shell uruchamiaj przez `rtk`, w razie potrzeby `rtk proxy`.
2. [CLAUDE.md](../CLAUDE.md): komendy, architektura, konwencje modułów, kontrakty, panel i guardrails. [README panelu](../apps/panel/README.md).
3. [FTI_IMPLEMENTATION_PLAN.md](FTI_IMPLEMENTATION_PLAN.md) — zakres, kolejność i kryteria odbioru; szczególnie etap A, tabela paczek oraz sekcje 6–7.
4. [FTI_ARCHITECTURE_REVIEW.md](FTI_ARCHITECTURE_REVIEW.md) — dowody z kodu, właściciele domen, AR1–AR4.
5. [FTI_FEATURE_GAPS.md](FTI_FEATURE_GAPS.md), [FTI_LANGFUSE_MLFLOW_ANALYSIS.md](FTI_LANGFUSE_MLFLOW_ANALYSIS.md) — uzasadnienie priorytetów i semantyka ocen.
6. [FTI_FEATURE_CATALOG.md](FTI_FEATURE_CATALOG.md) i [FTI_UX_WANDB_PLAN.md](FTI_UX_WANDB_PLAN.md) — materiał odniesienia dla zmienianych ekranów. Katalog opisuje też funkcje docelowe; nie jest listą ukończonych implementacji.

Kod w aktualnym checkoutcie rozstrzyga, co już działa. Jeżeli od audytu zaszły zmiany, zaktualizuj lokalny plan na podstawie różnicy, bez powtarzania całego badania konkurencji. Stare `docs/mlflow-comparison.md` jest historyczne; nowsza analiza ma zweryfikowane źródła Langfuse/MLflow.

## Stan przekazania i katalog roboczy

- W sesji analitycznej powstała dokumentacja. Nie zaimplementowano w niej paczek A1–A4 ani AR1–AR4.
- Przeszła kontrola granic panelu, statyczna kontrola acykliczności zależności 16 crate'ów oraz sprawdzenie lekkiego importu SDK. Nie uruchamiano pełnych testów aplikacji ani odbioru runtime.
- W momencie przekazania dokumenty `FTI_*.md` są **nieśledzone przez Git**. Nowy worktree utworzony tylko z HEAD może ich nie zawierać. Najprościej rozpocząć w podanym katalogu projektu; jeśli wybierzesz izolację, najpierw zapewnij w niej dostęp do tych dokumentów i potrzebnego stanu kodu.
- W katalogu są liczne wcześniejsze zmiany wykonawcy podowego AW-4: Execution, Server, API worker, SDK worker, Cargo, Helm i deploy. Sprawdź bieżący `git status` oraz diff przed edycją; oddziel je od własnej pracy i zachowaj. Nie wykonuj reset/clean/stash całego katalogu ani nie nadpisuj cudzych zmian.
- Nie uznawaj obecności kodu podów za potwierdzenie gotowości Kubernetes. Etap A nie wymaga zakończenia tej ścieżki.

## Kolejność realizacji

### A1 — dane odbiorowe i wykresy

Przygotuj powtarzalny lokalny zestaw: minimum trzy treningi, dwie zgodne ewaluacje, failed, inny dataset, brak metryki i brak wersji danych. Wykorzystaj istniejące fixture/seedy i sposób uruchamiania projektu, zamiast tworzyć równoległą infrastrukturę. Zobacz aktualny ekran na działającej lokalnej instancji.

Ustabilizuj tożsamość i kolory serii w wykresach, zapewnij dostępny odczyt wartości. Nie zamieniaj braków na zera ani nie mieszaj jednostek. Główne miejsce odniesienia: `apps/panel/src/shared/components/charts/learning-curve.tsx`.

### A2 — porównanie treningów

Pracuj w `apps/panel/src/features/training/screens/runs/`. Dodaj wybór 2–5 runów w URL, pobieranie szczegółów wybranych runów, różnice parametrów i krzywe metryk względem epoki. Rozróżnij wynik najlepszy od ostatniego. Korzystaj z istniejącego API; nowa domena Experiments nie jest potrzebna do tego ekranu.

Obsłuż Back/refresh, brakujące i usunięte runy oraz limit listy API. Nie sugeruj, że ograniczona lista zawiera pełną historię.

### A3 — jawny baseline

Pracuj w `crates/aiwatcher-api/src/evaluations.rs`, `crates/aiwatcher-projector/src/evaluations.rs` i `apps/panel/src/features/evaluation/`.

Dodaj jawny baseline do kontraktu API i URL. Automatyczny wybór ma dopuszczać tylko zakończony sukcesem wynik. Znana niezgodność danych/oceny blokuje deltę jakości i rekomendację; brak dowodu zgodności oznacza stan niezweryfikowany. Pokazuj pokrycie przypadków i kompletność szczegółów. Nie dodawaj fikcyjnych wersji do starszych zdarzeń.

Na tym etapie poprawiamy odczyt istniejących ocen. Trwałe wyniki, rubryki i nowy registry Evaluation należą do B. Po zmianie publicznego kontraktu regeneruj OpenAPI i klienta, zamiast edytować wygenerowane pliki ręcznie.

### A4 — zapis widoków i linki

Dodaj nazwane lokalne widoki: `schema_version`, rodzaj ekranu, filtry, ID runów/baseline i wybrane metryki. Rozdziel klucz danych według instancji i tożsamości; oznacz zapis jako „na tym urządzeniu”. Nie kopiuj raportów ani treści rozmów do localStorage. Uwzględnij preferencje i dostępność opisane w planie.

Linkuj tylko potwierdzone referencje: model → trening, ocena → wykonanie/krok, źródło → właściwy rodzaj wersji danych. Brak rozpoznanego celu nie może prowadzić do zgadywanego rejestru. Zapis widoku nie gwarantuje zachowania danych historycznych.

### AR1 — mała ochrona granic

Utrwal właścicieli danych i dozwolone zależności na podstawie audytu. Dodaj kontrolę zależności Rust z `cargo metadata`, odróżniając produkcję od zależności testowych i uwzględniając aliasy/targety. Test kontrolowanego naruszenia ma potwierdzić, że bramka wykrywa niedozwoloną krawędź. Włącz ją do właściwego istniejącego check/CI.

Nie zastępuj kontroli architektury samym wykrywaniem cykli ani nie twórz reguły automatycznie akceptującej wszystkie aktualne krawędzie przy każdym uruchomieniu. Obecne wyjątki mają być jawne. Zachowaj działającą kontrolę panelu.

## Zasady implementacji

- Feature panelu nie importuje innego feature. Ekran, hooki, URL i testy pozostają przy funkcji; wspólne elementy wydzielaj przy rzeczywistym ponownym użyciu.
- Reguły i dane mają jednego właściciela. Nie dopisuj nowych polityk jakości do Core, prywatnego storage innej domeny ani stanu silnika Execution.
- Utrzymuj kompatybilność odczytu starszych danych. Nie zmieniaj niejawnie polityki promocji modelu lub promptu.
- Dalsze rozszerzenia: AR2 z B1/B2/B4, AR3 przed C0, AR4 przy nowych klientach ocen. Nowy scorer powinien być zadaniem istniejącego workera.
- Przyjmij wspólną instancję zespołu i lokalne widoki zgodnie z planem. Izolacja wielu zespołów i backend współpracy nie należą do pierwszego wydania.
- Realizuj małe, sprawdzalne paczki. Rutynowe decyzje podejmuj na podstawie kodu i planu. Zapisuj istotne odstępstwa wraz z powodem.

## Odbiór i zakończenie

Testuj zachowanie użytkownika: wybór i odtworzenie porównania, stabilność kolorów po usunięciu serii, brak metryki, failed baseline, niezgodność i niepełne dane, zapis widoku oraz poprawne linki. Sprawdź zmienione ekrany w przeglądarce, w obu motywach i z klawiatury.

W panelu uruchom odpowiednie testy oraz `rtk npm run typecheck`, `rtk npm test`, `rtk npm run build`. Przy zmianach API uruchom `rtk just openapi` oraz właściwe testy Rust. Dodaj weryfikację AR1 do istniejącego procesu i wykonaj wymagane repozytoryjne sprawdzenia zgodnie z `CLAUDE.md`/planem, w tym `rtk just check` przed scaleniem. Nie powtarzaj kosztownych sprawdzeń bez zmiany lub nowej przesłanki.

Istniejący błąd niezwiązany z zakresem odróżnij od regresji własnej zmiany i poprzyj wynikiem. Nie oznaczaj niesprawdzonego scenariusza jako PASS. Przeszkoda w jednej paczce nie blokuje niezależnych prac w pozostałych.

Na koniec zaktualizuj plan o stan A1–A4/AR1, wykonane sprawdzenia i pozostałe problemy. Jeśli praca przechodzi do kolejnej sesji, dopisz tutaj krótki checkpoint: ukończone paczki, zmienione pliki, testy i dokładny następny krok. Odpowiedz po polsku: co użytkownik już może zrobić, co zmieniono, jak sprawdzono i jakie ograniczenia pozostały.
