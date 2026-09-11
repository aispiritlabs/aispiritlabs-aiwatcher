# FTI — lista funkcji platformy

Katalog docelowy: obejmuje funkcje istniejące i proponowane. To lista nazw funkcji bez opisu UX; aktualne statusy i priorytety są w [analizie braków](FTI_FEATURE_GAPS.md). Podział na istniejące/proponowane nie jest tu celowo powielany. Rozszerzony 2026-09-11 na podstawie [analizy Langfuse i MLflow](FTI_LANGFUSE_MLFLOW_ANALYSIS.md).

## Feature — dane

- Katalog datasetów.
- Wersjonowanie datasetów.
- Podgląd i filtrowanie rekordów.
- Wyszukiwanie datasetów w Hugging Face i Kaggle.
- Import danych i śledzenie odrzuceń.
- Receptury przygotowania danych.
- Wizualne pipeline'y transformacji.
- Transformacje danych w silnikach zapytań.
- Kroki notebookowe.
- Kontrola jakości i kompletności danych.
- Katalog źródeł danych.
- Adnotacje obrazów.
- Wersjonowanie adnotacji.
- Review i zatwierdzanie adnotacji.
- Eksport zbiorów adnotacji.
- Podział danych według grup.
- Archiwum rozmów.
- Zgody, redakcja treści i retencja rozmów.
- Review rozmów do uczenia.
- Eksport wersjonowanych korpusów.
- Przenoszenie przykładów z obserwacji do review.
- Zarządzanie przypadkami ewaluacyjnymi i oczekiwanymi odpowiedziami.
- Tworzenie przypadków regresji z zatwierdzonych obserwacji.
- Wersjonowanie schematu wejść i oczekiwań ewaluacji.

## Training — modele, prompty i eksperymenty

- Rejestr runów treningowych.
- Rejestrowanie parametrów treningu.
- Metryki epok i kroków.
- Krzywe uczenia.
- Rejestrowanie checkpointów.
- Rejestrowanie profili treningu.
- Porównywanie runów treningowych.
- Porównywanie wariantów eksperymentu.
- Porównywanie jakości, czasu i kosztu.
- Wersjonowanie całej aplikacji/agenta i konfiguracji wariantu.
- Powiązanie przypadku eksperymentu z generacją i trace.
- Rozróżnianie powtórzeń pomiaru i technicznych ponowień.
- Wyszukiwanie hiperparametrów — sweeps.
- Limity i zatrzymywanie prób eksperymentu.
- Rejestr modeli.
- Wersjonowanie modeli.
- Pakiety modelu i referencje artefaktów.
- Etykiety wersji modeli.
- Promocja modelu na podstawie wyników held-out.
- Rejestr promptów.
- Wersjonowanie i porównywanie promptów.
- Historia optymalizacji promptów.
- Integracja wyników optymalizacji DeepEval.
- Promocja promptu na podstawie wyników held-out.
- Playground promptów i modeli.
- Strukturalne prompty chat i schemat odpowiedzi.
- Przypinanie parametrów generacji i rozwiązywanie aliasów do wersji.

## Evaluation — ocena jakości

- Katalog zestawów ewaluacyjnych.
- Rejestrowanie raportów ewaluacji.
- Wyniki poszczególnych przypadków.
- Porównanie z baseline.
- Konfigurowanie i uruchamianie ewaluacji.
- Katalog scorerów.
- Ocena przez LLM judges.
- Wersjonowanie kryteriów oceny.
- Kalibracja ocen automatycznych.
- Typowane oceny trace, spanów, sesji i eksperymentów.
- Wersjonowane rubryki ocen i historia ich rewizji.
- Feedback użytkowników i ekspertów z jawnym pochodzeniem.
- Kolejki review jakości i poprawione odpowiedzi.
- Ponowna ocena zapisanych odpowiedzi bez nowej generacji.
- Ocena całych rozmów i trajektorii wywołań narzędzi.
- Porównanie wyników na poziomie przypadku.
- Trwałe artefakty pełnych wyników ewaluacji.
- Weryfikacja porównywalności danych, splitów i scorerów.
- Testy znanych regresji aplikacji i bramki jakości w CI.
- Ewaluacja na wydzielonym zbiorze testowym.
- Ciągłe próbkowanie i ocena jakości online.

## Inference — obserwowalność i serwowanie

- Przegląd sesji, runów i spanów.
- Eksploracja według agenta, modelu, runtime'u i narzędzia.
- Drzewo wykonania.
- Waterfall spanów.
- Podgląd zdarzeń na żywo.
- Wznawianie strumienia zdarzeń.
- Wyszukiwanie i filtrowanie zdarzeń.
- Kreator i edytor zapytań.
- Agregaty metryk.
- Percentyle opóźnień.
- Analiza błędów i niezawodności narzędzi.
- Analiza tokenów i kosztu.
- Rozróżnianie kosztu raportowanego i estymowanego.
- Wersjonowanie źródeł cen i prezentacja pokrycia wyceny.
- Porównywanie wersji w użyciu.
- Sygnały jakości i regresji.
- Serwowanie wersjonowanych modeli.
- Weryfikacja integralności artefaktów.
- Warmup i gotowość modelu.
- Zmiana obsługiwanej wersji modelu.
- Rollback modelu.
- Shadow inference.
- Katalog endpointów inferencyjnych.
- Zarządzanie pojemnością i skalowaniem endpointów.
- Opcjonalny gateway dostawców LLM, routing i fallbacki.

## Wykonania i automatyzacje

- Rejestr definicji workflow.
- Uruchamianie workflow.
- Historia wykonań i prób kroków.
- Graf workflow.
- Pauzowanie i wznawianie wykonania.
- Anulowanie wykonania.
- Ponawianie kroku.
- Wprowadzanie danych i akceptacja przez człowieka.
- Cache wyników kroków.
- Katalog artefaktów wykonania.
- Harmonogramy pipeline'ów i workflow.
- Historia uruchomień harmonogramu.
- Automatyzacje wyzwalane zdarzeniem.
- Alerty stanu runu i metryk.
- Kanały powiadomień.
- Historia i deduplikacja powiadomień.

## Analiza i współpraca

- Workspace analityczne.
- Zapisane widoki.
- Osobiste i zespołowe dashboardy.
- Konfigurowalne panele wykresów.
- Raporty łączące opis i dane.
- Udostępnianie analiz z kontrolą dostępu.
- Komentarze i wzmianki.
- Przypisywanie elementów do review.
- Wyszukiwanie globalne.
- Przekrojowy lineage danych, modeli i wykonań.
- Uniwersalny katalog artefaktów.
- Budżety i alokacja kosztów.

## Konto i administracja

- Logowanie SSO/OIDC.
- Sesje użytkownika i wylogowanie.
- Role i mapowanie grup.
- Profil użytkownika.
- Preferencje wyglądu i pracy.
- Organizacje i zespoły.
- Projekty i środowiska.
- Członkostwa i zaproszenia.
- Uprawnienia per projekt.
- Konta techniczne.
- Zarządzanie kluczami dostępu.
- Konfiguracja integracji.
- Polityki retencji i dostępu do treści.
- Audyt administracyjny.
- Diagnostyka możliwości instancji.

## Integracje i dostarczanie platformy

- REST API i OpenAPI.
- SDK Python i TypeScript.
- Integracje agentowe.
- Integracje treningowe i vision.
- Integracja DeepEval.
- Eksport OpenTelemetry.
- Integracje magazynów obiektowych.
- Worker SDK i runtime zadań.
- Wdrożenie self-hosted.
- Wdrożenie kontenerowe i Kubernetes.
- Instrukcje integracji i weryfikacja pierwszego sygnału.

## Opcjonalny rozwój AI

- Asystent analityczny z dostępem do kontekstu.
- Odpowiedzi z odnośnikami do danych.
- Generowanie filtrów, wykresów i raportów.
- Pamięć projektu.
- Zadania asystenta wykonywane w tle.
- Zbieranie sesji coding agents.
- Analityka użycia agentów według zespołu i repozytorium.
- Powiązanie sesji z pull requestami.
- Wyszukiwanie historii pracy z agentami.
