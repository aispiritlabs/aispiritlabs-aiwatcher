# FTI — braki funkcjonalne względem W&B, Langfuse i MLflow

Stan przeglądu: 2026-09-11. Uzupełnienie [planu UX](FTI_UX_WANDB_PLAN.md). Szczegółowe rozszerzenie: [Langfuse i MLflow](FTI_LANGFUSE_MLFLOW_ANALYSIS.md); kolejność realizacji: [plan implementacji](FTI_IMPLEMENTATION_PLAN.md).

Ocena opiera się na kodzie panelu, trasach API, modułach Rust i SDK Python oraz oficjalnej dokumentacji W&B, Langfuse i MLflow. W&B jest wcześniejszym punktem odniesienia; aktualizacja obejmuje dokumentację Langfuse i MLflow. „Jest” oznacza znalezioną implementację, nie potwierdzenie konfiguracji ani test działania konkretnego wdrożenia. „Brak” oznacza brak odpowiednika w przejrzanych obszarach repozytorium. Funkcje możliwe przez własny skrypt nie są automatycznie gotową funkcją platformy.

## 1. Najważniejsze luki

| Funkcja | Stan u nas | Czego konkretnie brakuje | Priorytet |
| --- | --- | --- | --- |
| Porównanie eksperymentów | Częściowo; ekran Experiments jest placeholderem | Warianty powiązane z trace, porównanie jakości/czasu/kosztu na tych samych danych, wybór baseline | P1 |
| Workspace analityczny | Częściowo: istnieją listy, filtry i wykresy | Zapis zestawu paneli i wyboru runów, wiele nazwanych widoków, osobisty i zespołowy zakres | P1 lokalnie; P2 współdzielenie |
| Porównanie wielu treningów | Częściowo: lista i szczegóły pojedynczego runu | Nakładanie krzywych wybranych runów, różnice parametrów, wspólne osie, ranking | P1 |
| Raporty zespołowe | Brak edytora raportów analitycznych | Dokument z tekstem i wykresami, przypięte wersje danych, udostępnianie w ramach uprawnień | P2, po trwałych ocenach i regresjach |
| Alerty i automatyzacje zdarzeniowe | Częściowo: harmonogramy i workflow istnieją | Reguły na błąd, metrykę lub zmianę wersji; kanały dostarczania, deduplikacja, historia powiadomień | P1 |
| Uniwersalne uruchamianie ewaluacji | Częściowo: raporty, przypadki, baseline, bridge DeepEval | Wybór datasetu/modelu/scorerów w panelu, uruchomienie i śledzenie oceny jako jednego procesu | P1 |
| Ponowna ocena zapisanych odpowiedzi | Częściowo: odczyt trace i zapis raportów | Tryb scoringu istniejących wyników, bez ponownej generacji, z wersją scorera i referencją treści | P1 |
| Feedback i typowane oceny trace | Częściowo: wyniki przypadków i preferencje review rozmów | Wspólny kontrakt ocen trace/spana/sesji, rubryki, autor/źródło, rewizje i filtry | P1 |
| Review jakości i zestaw regresji | Częściowo: review obrazów/rozmów, pochodzenie i eksport | Kolejka przypadków z trace, oczekiwana/poprawiona odpowiedź, zatwierdzenie do wersjonowanego datasetu ewaluacyjnego | P1 |
| Bramka jakości aplikacji w CI | Częściowo: bramki rejestrów i CI samej platformy | Przepis SDK/CLI, znane przypadki regresji, wynik pass/fail/error, link do trwałych dowodów | P1 |
| Wersja całej aplikacji/agenta | Częściowo: wersje promptów, modeli i planów wykonania | Manifest kodu, konfiguracji, narzędzi i zależności; powiązanie wariantu z konkretnymi przypadkami i trace | P1 |
| Trwały wynik i ścisła porównywalność | Częściowo: ograniczona projekcja raportów, automatyczny baseline | Artefakt pełnej oceny, jawny baseline, split, wersje schematu/suite/scorera, pokrycie i dowód decyzji | P1 |
| Biblioteka scorerów i LLM judges | Integracja z zewnętrznym narzędziem | Katalog, konfiguracja, wersjonowanie i kalibracja scorerów; gotowe przepisy oceny | P1 kontrakt i kalibracja; P2 szeroki katalog |
| Ocena multi-turn i kroków agenta | Częściowo: sesje, trace, narzędzia i archiwum rozmów | Snapshot kompletnej sesji, oczekiwania dla trajektorii, scorery poziomu sesji/kroku | P2 |
| Prompty chat i structured output | Częściowo: wersjonowany tekst promptu | Struktura wiadomości, format odpowiedzi i jawna konfiguracja generacji; zgodna migracja | P2; konfiguracja wariantu P1 |
| Playground promptów i modeli | Częściowo: rejestr i historia optymalizacji | Interaktywne uruchomienie kilku wariantów na tych samych wejściach i zapis wyniku | P2 |
| Sweeps / HPO | Brak gotowego produktu | Przestrzeń parametrów, strategie poszukiwania, limity prób/kosztu, zatrzymywanie słabych prób | P2 |
| Przekrojowy widok lineage | Częściowo: wersje, ArtifactRef i katalog artefaktów wykonania | Jeden widok dataset → wykonanie → model/prompt → ewaluacja → użycie, z działającymi przejściami | P1 |
| Uniwersalny katalog artefaktów | Częściowo: artefakty modeli i wykonania | Przeglądanie, wyszukiwanie, kolekcje i polityki dla różnych rodzajów artefaktów w jednym miejscu | P2 |
| Organizacje, zespoły, projekty | Częściowo: tożsamość, role, projekty adnotacji | Wspólny model zakresu całej platformy, członkostwa, zaproszenia, uprawnienia per projekt | P2; P1 dla wdrożenia wielozespołowego |
| Panel administracyjny | Częściowo: konfiguracja i autoryzacja backendu | UI integracji, dostępów, retencji, kont technicznych i diagnostyki możliwości instancji | P1/P2 zależnie od sposobu wdrażania |
| Cykl życia kluczy i kont technicznych | Częściowo: istnieją mechanizmy uwierzytelniania | Samoobsługowe wydawanie, zakresy, wygaszanie, odwoływanie i historia użycia w panelu | P2 |
| Audyt administracyjny | Częściowo: historia wykonań i zmian domenowych | Wspólny rejestr zmian dostępu, konfiguracji i operacji administracyjnych; wyszukiwanie i eksport | P2 |
| Profil i preferencje | Częściowo: tożsamość i wylogowanie | Ekrany preferencji, motyw, strefa czasu, gęstość, synchronizacja ustawień użytkownika | P1 |
| Wyszukiwanie globalne | Częściowo: wyszukiwanie lokalne ekranów | Wyszukiwarka runów, modeli, datasetów, promptów i stron z respektowaniem uprawnień | P2 |
| Współpraca na wynikach | Częściowo: review danych istnieje | Komentarze do analiz, wzmianki, przypisanie wniosku/zadania, subskrypcje zmian | P2 |
| Asystent typu ARIA | Brak | Analiza z dowodami, generowanie widoków/raportów, pamięć zakresu projektu, zadania w tle | P3 |
| Analityka typu HiveMind | Brak gotowego produktu | Zbieranie sesji narzędzi programistycznych, koszty zespołów/repozytoriów, związki z PR i wynikami pracy | P3, osobny moduł |
| Szeroki katalog integracji agentowych | Częściowo: SDK i integracja agentic | Gotowe integracje popularnych harnessów i SDK, sprawdzanie pierwszego sygnału, dokumentacja per integracja | P2 |
| Monitoring jakości online | Częściowo: telemetryka i wyniki ewaluacji | Konfiguracja ciągłego próbkowania, scorerów i sygnałów jakości; alarmowanie i ścieżka review | P2 |
| Zarządzanie kosztem | Częściowo: potwierdzone agregaty tokenów; brak pełnego rachunku pieniężnego | Koszt raportowany/estymowany, waluta, wersja ceny, pokrycie, budżety i alokacja | P2 |
| Zarządzane endpointy inferencyjne | Częściowo: SDK serving, zmiana wersji, rollback, shadow | Katalog endpointów, konfiguracja pojemności i autoskalowania, obsługa wdrożeń z panelu | P3 |
| Gateway dostawców LLM | Brak gotowego produktu; serving SDK ma inny zakres | Wspólny dostęp do dostawców, routing, fallbacki i egzekwowanie limitów w ścieżce żądania | P3; adapter istniejącego gatewaya według potrzeby |

P1 = najbliższy rozwój produktu; P2 = kolejne rozszerzenie; P3 = odrębna większa inwestycja. To rekomendacja, nie estymacja ani zatwierdzony zakres; P1 obejmuje kilka wydań według planu. Nie wszystkie funkcje porównywanych produktów są potrzebne lub dostępne w podstawowej instancji self-hosted.

## 2. Czego nie należy oznaczać jako brak

| Istniejąca funkcja | Dowód w repozytorium | Ograniczenie porównania |
| --- | --- | --- |
| Runy treningowe, metryki epok, próbki, checkpointy, profile | `crates/aiwatcher-training/src/{lib,run}.rs`, `crates/aiwatcher-api/src/training.rs` | Nie oznacza kompletnego workspace do porównywania eksperymentów |
| Rejestr modeli, wersje i etykieta produkcyjna | `crates/aiwatcher-training/src/model.rs` i `registry/` | Nie oznacza organizacyjnego registry ze wszystkimi funkcjami governance W&B |
| Pakiety modeli i artefakty z digestem | `crates/aiwatcher-training/src/package.rs` | Nie oznacza uniwersalnego eksploratora wszystkich artefaktów |
| Serwowanie modeli, weryfikacja, warmup, zmiana wersji, rollback i shadow | `sdk/python/aiwatcher_sdk/serving/server.py` | Nie oznacza usługi serverless ani zarządzania flotą endpointów |
| Rejestr promptów, wersje, optymalizacje i bramka promocji | `crates/aiwatcher-prompts/`, `sdk/python/aiwatcher_sdk/prompts.py` | Nie oznacza playgroundu ani własnego silnika optymalizacji |
| Zapis wyników DeepEval i podział dev/test po grupach | `sdk/python/aiwatcher_sdk/integrations/deepeval.py`, `optimization.py` | Sam bridge nie uruchamia DeepEval i nie jest biblioteką judge'ów |
| Raporty ewaluacji, wyniki przypadków i porównanie z baseline | `crates/aiwatcher-api/src/evaluations.rs`, `features/evaluation/` | Raport ewaluacji nie jest współdzielonym raportem narracyjnym z wykresami |
| Dataset versions, receptury i pipeline blokowy | `crates/aiwatcher-datasets/`, `features/data-curation/` | Globalny lineage UX jest szerszym zakresem |
| Adnotacje obrazów, review i eksport | `crates/aiwatcher-annotations/` | To review danych, nie komentarze do analiz i raportów |
| Chronione rozmowy, consent, retencja, review i eksport korpusu | `crates/aiwatcher-conversations/` | Nie powinno się kopiować do logów pełnej treści rozmów w celu parytetu z Weave |
| Managed workflows, pauza, wznowienie, anulowanie, retry i input człowieka | `crates/aiwatcher-api/src/executions.rs`, `aiwatcher-execution/` | Nie oznacza automatyzacji zdarzeniowych typu „metryka przekroczyła próg” |
| Harmonogramy workflow i pipeline | `crates/aiwatcher-api/src/schedules.rs` | Harmonogram czasowy i alert to różne funkcje |
| Trace, waterfall, live, agregaty i Query | `features/observability/`, API `runs`, `metrics`, `live` | Większość potrzeb obserwowalności ma już bazę |
| OIDC, sesje, role i integracja z proxy | `crates/aiwatcher-auth/`, API `auth.rs` | Nie oznacza pełnej samoobsługowej administracji organizacją |

Uwaga o dokumentacji: `docs/mlflow-comparison.md` zawiera starsze stwierdzenia o braku rejestru modeli i artefaktów, sprzeczne z aktualnym kodem. Dokument oznaczono jako historyczny; nie użyto go jako podstawy klasyfikacji ani nie powtarzano jego benchmarku. Aktualne porównanie jest w [uzupełnieniu Langfuse/MLflow](FTI_LANGFUSE_MLFLOW_ANALYSIS.md).

## 3. Najlepsza kolejność produktowa

1. Porównanie treningów, jawny baseline, lokalnie zapisane widoki i podstawowe linki pochodzenia.
2. Manifest wariantu, trwałe wyniki, kontekst porównywalności i typowane oceny z pochodzeniem.
3. Najpierw scoring zapisanych odpowiedzi, następnie generowanie i ocena przez istniejący workflow oraz porównanie wariantów.
4. Bramka regresji aplikacji w CI oraz feedback → review → zatwierdzony przypadek testowy; podstawy review można rozwijać po kontrakcie ocen.
5. Alerty, diagnostyka integracji, serwerowe workspace i raporty zespołowe. Ograniczony scoring online po sprawdzeniu procesu oceny.
6. Wielozespołowość warunkowo od modelu uprawnień i danych; asystent, analityka coding agents i gateway jako późniejsze inicjatywy.

Największa wartość leży w spięciu istniejących funkcji w pełne procesy użytkownika. Same nazwy zakładek nie wystarczą; istniejący scheduler, registry czy SDK mogą obsłużyć nowe procesy bez budowy ich od nowa.

## 4. Źródła W&B

- Workspace, porównania, raporty, sweeps i organizacja projektu: [Projects](https://docs.wandb.ai/models/track/project-page).
- Wersjonowanie i powiązania artefaktów: [Artifacts](https://docs.wandb.ai/models/artifacts).
- Wyzwalacze zmian i progów metryk: [Automation events and scopes](https://docs.wandb.ai/models/automations/automation-events).
- Uruchamianie porównań bez kodu: [Evaluation Playground](https://docs.wandb.ai/weave/guides/tools/evaluation_playground).
- Scorery: [Scoring overview](https://docs.wandb.ai/weave/guides/evaluation/scorers).
- Integracje agentowe i OTel: [Choose an agent integration](https://docs.wandb.ai/weave/agent-integration-quickstart).
- Sesje, spany, sygnały jakości i przejście do danych: [View agent activity](https://docs.wandb.ai/weave/guides/tracking/view-agent-activity).
- Asystent: [ARIA](https://docs.wandb.ai/aria/overview).
- Analityka sesji programistycznych: [HiveMind](https://docs.wandb.ai/hivemind).
- Zakresy ustawień konta: oglądane wcześniej zalogowane [Account settings](https://wandb.ai/account-settings/mkubasz-ood-org/) i [User settings](https://wandb.ai/settings).

Powyższe jest porównaniem funkcjonalnym, nie potwierdzeniem dostępności każdej funkcji W&B w każdym planie lub wariancie wdrożenia.

## 5. Rozszerzenie: Langfuse i MLflow

Nowe pozycje, dowody w kodzie, ograniczenia dokumentacji i wpływ na etapy B–D zawiera [szczegółowa analiza](FTI_LANGFUSE_MLFLOW_ANALYSIS.md). Najważniejsza zmiana rekomendacji: przepływ od produkcyjnego błędu do zweryfikowanego testu regresji ma pierwszeństwo przed raportami narracyjnymi.

Langfuse dostarcza wzorca dla ocen przy trace, kolejek review i datasetów tworzonych z obserwacji. [Scores](https://langfuse.com/docs/evaluation/scores/data-model), [Annotation Queues](https://langfuse.com/docs/evaluation/evaluation-methods/annotation-queues), [Datasets](https://langfuse.com/docs/evaluation/experiments/datasets).

MLflow pokazuje wersję całej aplikacji, ocenę istniejących trace i testy zachowania w CI. [Version Tracking](https://mlflow.org/docs/latest/genai/version-tracking/), [Evaluate Traces](https://mlflow.org/docs/latest/genai/eval-monitor/running-evaluation/traces/), [Regression Testing](https://mlflow.org/docs/latest/genai/eval-monitor/regression-testing/).
