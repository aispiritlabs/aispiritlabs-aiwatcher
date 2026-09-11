# FTI — uzupełnienie porównania o Langfuse i MLflow

Przegląd dokumentacji: 2026-09-11. Uzupełnia [analizę braków](FTI_FEATURE_GAPS.md), [katalog](FTI_FEATURE_CATALOG.md) i [plan implementacji](FTI_IMPLEMENTATION_PLAN.md). Źródłem są oficjalne strony produktów i aktualny kod aiwatchera. Opis funkcji w dokumentacji nie jest testem jej działania ani potwierdzeniem dostępności w każdym wariancie wdrożenia. Adresy MLflow `/latest/` i dokumentacja Langfuse są ruchome; przed implementacją adaptera trzeba przypiąć wersję API/SDK.

## 1. Co zmienia to porównanie

Rekomendacja dla FTI: utrzymać pierwszy zakres porównania treningów i baseline, ale w kolejnych etapach podnieść priorytet **oceny istniejących odpowiedzi, feedbacku przy trace, budowania przypadków regresji i uruchamiania ich w CI**. To domyka proces poprawiania jakości wcześniej niż edytor raportów zespołowych.

Langfuse jest szczególnie użytecznym punktem odniesienia dla przejścia od obserwacji przez ocenę i review do datasetu oraz kolejnego eksperymentu. MLflow obejmuje dziś zarówno tradycyjne ML, jak i aplikacje LLM/agentów: samo określenie go jako historycznego trackera eksperymentów jest zbyt wąskie. Dokumentacja opisuje m.in. ewaluację i monitoring, prompt registry oraz wersje aplikacji. [Langfuse: Evaluation](https://langfuse.com/docs/evaluation/overview), [MLflow: dokumentacja](https://mlflow.org/docs/latest/), [MLflow: wersje aplikacji](https://mlflow.org/docs/latest/genai/version-tracking/).

W&B pozostaje wcześniejszym odniesieniem dla workspace, wykresów i raportów. Nie powtarzano w tej aktualizacji jego audytu. Nie rekomenduję instalacji trzech backendów ani migracji aiwatchera do któregoś z nich: porównujemy wzorce funkcjonalne, a integrację wybieramy dopiero dla konkretnej potrzeby.

## 2. Langfuse — co warto przejąć

| Funkcja potwierdzona w dokumentacji | Stan FTI | Rekomendacja i priorytet |
| --- | --- | --- |
| Wynik oceny przypisany do trace, obserwacji, sesji lub eksperymentu; typy numeric/boolean/category/text i konfiguracja schematu. [Scores](https://langfuse.com/docs/evaluation/scores/data-model) | Metryki i wynik przypadku w Evaluation; osobne review rozmów. Brak znalezionego wspólnego zasobu ocen tych obiektów | P1: wersjonowana definicja oceny, jednoznaczny cel, autor/źródło, uzasadnienie i rewizje. Nie sprowadzać każdej oceny do jednej liczby |
| Feedback użytkownika jako ocena powiązana z trace. [User Feedback](https://langfuse.com/docs/observability/features/user-feedback) | Jest ocena preferencji w review rozmów; nie znaleziono ogólnego przepływu feedbacku aplikacji do trace | P1: API/SDK do oceny odpowiedzi i filtr trace po feedbacku. Oddzielić ocenę użytkownika, recenzenta i judge'a |
| Kolejki oceny trace/obserwacji/sesji według określonych kryteriów, z przypisaniem użytkowników i poprawionymi odpowiedziami. [Annotation Queues](https://langfuse.com/docs/evaluation/evaluation-methods/annotation-queues) | Review obrazów i rozmów istnieje, ale nie jest uniwersalną kolejką oceny zachowania agenta | P1: jeden przepływ „do review → oceń/popraw → zatwierdź przypadek regresji”; wykorzystać istniejące reguły dostępu do treści |
| Dataset z wejściem, oczekiwanym wyjściem i odniesieniem do produkcyjnego trace; wersje i walidacja schematu. [Datasets](https://langfuse.com/docs/evaluation/experiments/datasets) | Wersjonowanie danych i pochodzenie rozmów już istnieją; brakuje wygodnego produktu do zarządzania przypadkami oceny | P1: widok przypadku testowego na istniejącym magazynie danych, stabilne `case_id`, oczekiwanie oraz źródło. Nie tworzyć drugiego ogólnego rejestru datasetów |
| Jawne połączenie elementu datasetu, przebiegu eksperymentu i trace wyniku. [Experiments data model](https://langfuse.com/docs/evaluation/experiments/data-model) | Ocena ma wariant i wykonanie, lecz pojedynczy `EvaluationCase` nie ma jawnej referencji trace/spana | P1: mapowanie przypadku na wynik generacji i jego trace. W naszym kontrakcie rozróżnić powtórzenie pomiaru od technicznego retry |
| Start porównania promptów/modeli z datasetu w UI, z opcjonalnymi scorerami i mapowaniem zmiennych. [Experiments via UI](https://langfuse.com/docs/evaluation/experiments/experiments-via-ui) | Rejestr promptów i workflow są; launcher oceny i mapowanie wejść nie tworzą jeszcze jednego procesu | P1: walidowany formularz nad istniejącym workflow. P2: pełny playground |
| Koszt przekazany przez producenta albo wyliczony z usage i definicji modelu. [Token & Cost Tracking](https://langfuse.com/docs/observability/features/token-and-cost-tracking) | Potwierdzone metryki zużycia tokenów; brak kompletnego kontraktu wyceny | P2: osobne `reported`/`estimated`, waluta, wersja ceny i pokrycie; nie sumować nakładających się kategorii cache/reasoning |
| Eksperyment w CI ze sprawdzeniem progów i wynikiem zadania. [Experiments in CI/CD](https://langfuse.com/docs/evaluation/experiments/experiments-ci-cd) | Są bramki domenowe i CI kodu platformy; nie znaleziono gotowej bramki jakości aplikacji użytkownika | P1: przepis SDK/CLI zwracający wynik i link do oceny, wspólny kontrakt z UI |

Zakres „brak znalezionego” obejmuje API, typy domenowe, SDK i panel przejrzane dla tej analizy. Zapis dowolnego JSON przez producenta nie stanowi gotowej funkcji produktu.

## 3. MLflow — co warto przejąć

| Funkcja potwierdzona w dokumentacji | Stan FTI | Rekomendacja i priorytet |
| --- | --- | --- |
| Tracking runów, parametrów, metryk i artefaktów; autologging dla bibliotek ML. [Tracking](https://mlflow.org/docs/latest/ml/), [Autologging](https://mlflow.org/docs/latest/ml/tracking/autolog/) | Treningi, pakiety modeli oraz integracje torch/vision już są | P1: porównywanie istniejących treningów. P2: jeden dodatkowy adapter według użycia, bez budowy kolejnego trackera |
| Registry modeli z wersjami, aliasami, tagami i lineage. [Model Registry](https://mlflow.org/docs/latest/ml/model-registry/) | Modele, wersje, etykiety i referencje treningu są | Rozwijać nawigację i dowody promocji. Alias wskazujący wersję sam nie oznacza, że przeszła ona walidację jakości |
| Wersja całej aplikacji/agenta łączy kod, konfigurację, oceny i trace przez `LoggedModel`. [Version Tracking](https://mlflow.org/docs/latest/genai/version-tracking/) | Model version, prompt version i execution plan są rozdzielone; brak wspólnego manifestu wariantu aplikacji | P1: rozszerzyć planowany manifest wariantu o kod, konfigurację generacji, prompty, model i wersję definicji workflow/narzędzi |
| Uruchamianie oceny na danych i funkcji predykcji, z zestawem scorerów. [Evaluation](https://mlflow.org/docs/latest/genai/eval-monitor/) | FTI zapisuje raporty; bridge DeepEval nie jest uniwersalnym launcherem | P1: jeden adapter wykonawczy i jeden kontrakt wyniku, używane zarówno z panelu, jak i skryptu |
| Ocena istniejących trace bez `predict_fn`; scorer może analizować także kroki pośrednie. [Evaluate Traces](https://mlflow.org/docs/latest/genai/eval-monitor/running-evaluation/traces/) | Odczyt trace i zapis wyników istnieją; brak gotowego procesu ponownej oceny zapisanych odpowiedzi | P1: tryb `score_existing` przed kosztowniejszym `generate_and_score`. Nowy scorer tworzy nowy wynik, zachowując referencję do tych samych odpowiedzi |
| Feedback i oczekiwania przy trace. [Feedback Collection](https://mlflow.org/docs/latest/genai/assessments/feedback/) | Review korpusu ma inny cel i zakres niż ocena pojedynczego trace | P1: rozdzielić oczekiwaną odpowiedź od oceny otrzymanej; zachować autora, czas i historię zmian |
| Testy zachowania w pytest, oceny i asercje połączone z zapisanym runem. [Regression Testing and CI/CD](https://mlflow.org/docs/latest/genai/eval-monitor/regression-testing/) | Brak gotowego produktu do bramkowania jakości aplikacji użytkownika | P1: test znanych regresji jako niezależne kryterium obok średniej jakości. Awaria ewaluatora nie może przechodzić jako pozytywna ocena |
| Automatyczne oceny z próbkowaniem, filtrem i aktywacją scorera. [Automatic Evaluation](https://mlflow.org/docs/latest/genai/eval-monitor/automatic-evaluations/) | Scheduler i telemetryka są; brak konfiguracji cyklicznego scoringu ruchu | P2 po uruchamialnej ocenie: ograniczona próbka, jawny mianownik, wersja scorera i stan wykonania |
| Ocena całych sesji i symulowanie rozmów. [Evaluate Conversations](https://mlflow.org/docs/latest/genai/eval-monitor/running-evaluation/multi-turn/) | Sesje, rozmowy i pochodzenie istnieją; brak gotowego evaluatora multi-turn | P2: najpierw ocena zapisanej sesji; symulacje później. Przypiąć listę tur, kolejność i stan kompletności |
| Dopasowanie judge'a do ocen ludzi przez `align()`. [Judge Alignment](https://mlflow.org/docs/latest/genai/eval-monitor/scorers/llm-judge/alignment/) | Podział danych i wyniki optymalizacji są; nie ma procesu kalibracji judge'a | P1: zbiór kalibracyjny i przegląd rozbieżności. P2: automatyczne dostrajanie dopiero z niezależną walidacją |
| Prompt registry obsługuje szablony tekstowe i chat, format odpowiedzi, konfigurację modelu oraz aliasy. [Prompt Registry](https://mlflow.org/docs/latest/genai/prompt-registry/) | `PromptVersion` wersjonuje tekst; konfiguracja wariantu nie jest pełnym pakietem promptu chat | P1: zamrozić konfigurację w manifeście eksperymentu. P2: natywny prompt chat/structured output z migracją obecnych wersji tekstowych |
| AI Gateway skupia dostęp do dostawców, routing i fallbacki. [AI Gateway](https://mlflow.org/docs/latest/genai/governance/ai-gateway/) | Serving SDK własnych modeli nie jest gatewayem do dostawców LLM | P3: osobna decyzja. Do uruchomienia ocen wystarczy adapter skonfigurowanego dostawcy lub istniejącego gatewaya |

## 4. Czego nie kopiować bez sprawdzenia kontraktu

- **Wersja danych ma obejmować także schemat.** Langfuse wersjonuje elementy datasetu, ale dokumentacja wyłącza zmiany schematu z tego mechanizmu. W FTI przypinamy schemat wejścia i oczekiwań razem z manifestem przypadków. To rekomendacja dla odtwarzalności, nie opis dodatkowej funkcji Langfuse. [Datasets: Versioning](https://langfuse.com/docs/evaluation/experiments/datasets#versioning).
- **Wersja promptu nie wystarcza do odtworzenia konfiguracji generacji.** MLflow opisuje `model_config` jako edytowalny przy danej wersji. W FTI manifest oceny zamraża jego rzeczywistą wartość oraz rozwiązuje aliasy do konkretnych wersji w momencie startu. [Prompt Registry](https://mlflow.org/docs/latest/genai/prompt-registry/).
- **Powtórzenia eksperymentu i retry muszą mieć różne tożsamości.** Dokumentacja Langfuse opisuje obecnie ograniczenie do jednego elementu eksperymentu na element datasetu. U nas trzeba móc później zapisać kilka niezależnych prób tego samego przypadku, bez zniesienia idempotencji technicznego ponowienia. [Experiments data model](https://langfuse.com/docs/evaluation/experiments/data-model).
- **Cisza w sesji nie dowodzi jej zakończenia.** Automatyczna ocena sesji w MLflow może bazować na okresie bez nowych trace i zastępować wcześniejsze wyniki po nadejściu kolejnych. W FTI ocena wskazuje snapshot sesji; późniejsze tury tworzą nową ocenę, zachowując poprzednią jako historyczną. [Automatic Evaluation](https://mlflow.org/docs/latest/genai/eval-monitor/automatic-evaluations/).
- **Nie przenosić ocenianych treści do telemetryki tylko dla wygody scorerów.** W FTI `score_existing` pobiera uprawniony snapshot z artefaktu lub archiwum rozmów. Sam trace z redakcją może nie zawierać danych potrzebnych do oceny. Brak treści oznacza brak możliwości tej oceny, nie pustą odpowiedź modelu.
- **Znane testy regresji nie są ukrytym zbiorem testowym.** Przypadki widoczne i wielokrotnie sprawdzane w CI służą wykrywaniu powrotu znanych błędów. Nie zastępują niezależnego held-out używanego do decyzji o poprawie jakości; nie należy dostrajać judge'a na tym samym held-out.

## 5. Korekta roadmapy FTI

Pierwszy zakres A1–A4 pozostaje bez zmian. Nowe elementy wprowadzamy przy kontraktach i wykonaniu oceny; nie blokują one porównania treningów.

| Etap | Dopisany rezultat | Dlaczego wtedy |
| --- | --- | --- |
| B1–B3 | Manifest wersji aplikacji, schemat oczekiwań i relacja case → generacja → trace; osobna tożsamość powtórzenia | To dane, od których zależy uczciwe porównanie |
| B4 | Typowane oceny/feedback z pochodzeniem, rewizjami i jasno określonym celem | Wspólny fundament ręcznej oceny, kalibracji i online scoringu |
| C0 | Ponowna ocena zapisanych odpowiedzi, bez generowania ich od nowa | Najprostszy pierwszy proces wykonania oceny i tańsza weryfikacja nowych scorerów |
| C1–C2 | Generowanie i ocena wariantów na tych samych przypadkach; porównanie także per case | Rozwija sprawdzony scorer i magazyn wyników |
| C3 | Uruchomienie regresji z SDK/CLI w CI, wynik i link do dowodów | Korzysta z tego samego kontraktu co UI; nie wymaga dashboardu zespołowego |
| C4 | Feedback/trace → kolejka review → zatwierdzony przypadek → wersja datasetu | Domyka powrót z produkcji do testów przed inwestycją w raporty narracyjne |
| D | Alerty i raporty; następnie ograniczony scoring online | Mają już stabilne źródło ocen i wiadomo, co oznacza regresja |

C4 po B4 i trwałym zapisie przypadków może być realizowane niezależnie od pełnego formularza generowania C1. C3 powinno mieć mały przykład deterministyczny, żeby instalacja testów jakości nie wymagała od razu płatnego judge'a.

### Minimalne kryteria odbioru nowych elementów

1. Dwie oceny tego samego trace od człowieka i judge'a pozostają rozróżnialne; edycja zachowuje historię, a zmiana rubryki tworzy nową wersję.
2. Oczekiwana odpowiedź i feedback nie nadpisują oryginalnej odpowiedzi. Zatwierdzenie użycia treści pozostaje niezależne od pozytywnej oceny jakości.
3. `score_existing` nie wywołuje ponownie modelu aplikacji; brakujące lub niedostępne treści są raportowane, a nie zastępowane fikcyjnym wynikiem. Koszt judge'a jest nadal kosztem nowej oceny.
4. Zmiana aliasu promptu, parametrów generacji lub schematu danych po starcie nie zmienia kontekstu zapisanej oceny.
5. Krytyczny przypadek regresji blokuje bramkę mimo poprawy średniej. Błąd uruchomienia, niekompletna ocena i merytoryczna regresja są różnymi wynikami; CI nie uznaje pierwszych dwóch za sukces.
6. Dodanie tego samego przykładu do review jest idempotentne. Eksport wymaga zatwierdzenia i zachowuje źródło; członkostwo w kolejce nie nadaje uprawnień do treści.

## 6. Uściślenia względem starego porównania MLflow

[Historyczny dokument](mlflow-comparison.md) zawiera pomiary konkretnego adaptera i nieaktualne stwierdzenia o braku modeli oraz artefaktów w aiwatcherze. Nie stosujemy ich do obecnej roadmapy. Nie powtarzano benchmarku, więc wcześniejsze liczby nie dowodzą przewagi nad bieżącym MLflow. Aktualna dokumentacja opisuje zgodność tracingu MLflow z OpenTelemetry, co wyklucza dalsze traktowanie go jako formatu dostępnego wyłącznie dla MLflow. [MLflow: Tracing](https://mlflow.org/docs/latest/genai/).

Po stronie FTI sprawdzono dodatkowo: [review rozmowy](../crates/aiwatcher-conversations/src/review.rs), [pochodzenie treści](../crates/aiwatcher-conversations/src/turn.rs), [prompt tekstowy](../crates/aiwatcher-core/src/prompts.rs), [przypadek ewaluacji](../crates/aiwatcher-projector/src/evaluations.rs), [pakiet modelu](../crates/aiwatcher-training/src/package.rs) i [integracje SDK](../sdk/python/aiwatcher_sdk/integrations/). W szczególności FTI ma już etykiety preferencji i autora review rozmowy; brak dotyczy uniwersalnego scoringu trace oraz całego przepływu do regresji, nie wszelkiej ludzkiej oceny.

Przegląd dotyczy dokumentacji open-source MLflow i dokumentacji Langfuse. Funkcji opisanych dla Databricks/Unity Catalog lub planów enterprise nie traktujemy jako automatycznie dostępnych w podstawowym self-hostingu. Nie badano tu licencji, cen ani kompletności parytetu edycji. Nie uruchamiano usług ani testów implementacji.
