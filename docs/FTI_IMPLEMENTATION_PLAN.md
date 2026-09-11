# FTI — rekomendacja zakresu i plan rozwoju

Data: 2026-09-11. Status: A1–A4 i AR1 zaimplementowane; B1 zweryfikowane, trwały wycinek B2 dostarczony; B2/AR2 pozostają otwarte dla adapterów natywnych i orphan GC (sekcje 9–10). Wyniki odbioru A, ograniczenia i incydent seeda w sekcji 8.

Podstawa: [katalog funkcji](FTI_FEATURE_CATALOG.md), [analiza braków](FTI_FEATURE_GAPS.md), [plan UX](FTI_UX_WANDB_PLAN.md), [przegląd dokumentacji Langfuse i MLflow](FTI_LANGFUSE_MLFLOW_ANALYSIS.md), [ocena architektury](FTI_ARCHITECTURE_REVIEW.md) oraz aktualny kod. Ocena dotyczy obecności i kontraktów implementacji; nie potwierdza działania konkretnego wdrożenia. Katalog opisuje zakres docelowy, więc liczba jego pozycji nie jest miarą ukończenia produktu.

## 1. Rekomendacja

Największy zwrot da domknięcie procesu: **wybierz dane → porównaj kandydatów → uruchom ocenę → podejmij decyzję → sprawdź użycie wersji**. Platforma ma już większość mechanizmów wykonania, rejestracji i obserwacji. Brakuje przede wszystkim powiązań, wiarygodnych porównań i wygodnego dostępu do istniejących możliwości.

Pierwsze wydanie powinno dać porównanie treningów, jawny baseline ewaluacji, lokalnie zapisany widok i podstawowe linki pochodzenia. Następne: wersjonowany kontekst eksperymentu, trwałe dowody i ocena zapisanych odpowiedzi, potem generowanie i ocena przez istniejący workflow. Porównanie jakości/czasu/zużycia, bramka regresji w CI oraz feedback → review → przypadek testowy mają pierwszeństwo przed raportami zespołowymi. Alerty i współdzielenie analiz korzystają z tych fundamentów.

Założenie robocze: instancja self-hosted dla jednej grupy mającej wspólny dostęp do danych. Izolacja wielu zespołów nie jest potwierdzonym wymaganiem. Jeśli jest potrzebna przed udostępnieniem produktu, zakres danych i autoryzację trzeba przesunąć przed współdzielenie oraz nowe zasoby serwerowe. Sam filtr `project` nie zapewnia izolacji.

## 2. Co faktycznie mamy

| Obszar | Potwierdzony fundament | Co warto dołożyć |
| --- | --- | --- |
| Feature | Datasets, receptury, pipeline'y, adnotacje i kontrolowany korpus rozmów | Przejścia po konkretnych wersjach i wygodny powrót do źródła wyniku |
| Training | Trwały rekord runu, parametry, epoki, próbki, checkpointy, profile; modele i pakiety | Wielokrotny wybór runów, różnice parametrów i wspólne krzywe |
| Prompts | Wersje, optymalizacje, obliczany verdict i powiązania z raportami | Wpięcie w jeden proces eksperymentu i oceny |
| Evaluation | Zdarzenia `eval.*`, raport, przypadki, agregaty, automatyczny baseline; referencje wykonania i kroku | Jawny wybór baseline, wersje kryteriów, trwały wynik, konfigurator uruchomienia |
| Inference | Trace, waterfall, live, Query, metryki opóźnień i zużycia tokenów | Powiązanie obserwacji z niezmienną wersją wariantu i porównywalną próbą |
| Execution | Managed workflows, próby, retry, anulowanie, input człowieka, harmonogramy, artefakty i cache | Szablon ewaluacji oraz reguły reagujące na zdarzenia |
| Dostęp | Tożsamość, role, OIDC/proxy i mechanizmy dla producentów/workerów | Diagnostyka możliwości, preferencje; projektowe uprawnienia dopiero przy wymaganiu izolacji |

Najważniejsze miejsca potwierdzenia: [run treningowy](../crates/aiwatcher-training/src/run.rs), [model](../crates/aiwatcher-training/src/model.rs), [prompty](../crates/aiwatcher-core/src/prompts.rs), [API ewaluacji](../crates/aiwatcher-api/src/evaluations.rs), [projekcja ewaluacji](../crates/aiwatcher-projector/src/evaluations.rs), [SDK kroku](../sdk/python/aiwatcher_sdk/worker/context.py), [artefakty wykonania](../crates/aiwatcher-execution/src/artifact/mod.rs), [role](../crates/aiwatcher-auth/src/identity.rs).

### Uściślenia zmieniające plan

1. **Training compare nie wymaga nowej domeny Experiments.** Przed A2 [ekran treningów](../apps/panel/src/features/training/screens/runs/page.tsx) pobierał jeden szczegół; obecnie pobiera wybrane 1–5 runów i porównuje 2–5. Istniejące API dostarcza parametry i krzywe. [Experiments](../apps/panel/src/features/experiments/screens/overview/page.tsx) pozostaje placeholderem i wymaga osobnej relacji wariant → obserwacje.
2. **A3 uszczelniło baseline obecnej projekcji.** Wcześniej dobór dopuszczał `Failed` i nie sprawdzał wersji scorera ani splitu. Obecnie działa jawny `baseline_id`, automatyka wybiera sukces, a serwer rozróżnia zgodność, niezgodność i brak dowodu. Nie jest to jeszcze trwały dowód ani nowa polityka promocji; szczegóły w sekcji 8 i ADR 0010.
3. **Trwałość wyniku oceny wymaga osobnej pracy.** Projekcja ma ograniczenia liczby ewaluacji, przypadków i rozmiaru dokumentu. Może oddać pamięć przez usunięcie szczegółów. Zapisany identyfikator lub dashboard nie zamraża dowodu; zapis logu również nie jest kontraktem wieczystego dostępu. Raporty zespołowe i decyzje potrzebują wersjonowanych artefaktów wynikowych.
4. **Bramki modelu i promptu są różne.** `ModelVersion::check_promotable` wymaga odtwarzalnego datasetu i niepustych metryk testowych; nie wymaga przewagi nad baseline. Verdict optymalizacji promptu sprawdza poprawę wskazanej metryki held-out. W obu przypadkach samo pole `test` nie dowodzi niezależności zbioru — pochodzenie oceny trzeba zapisać.
5. **Potwierdzona baza kosztowa to przede wszystkim tokeny.** [Metryki](../crates/aiwatcher-projector/src/metrics.rs) i [ich ekran](../apps/panel/src/features/observability/screens/metrics/page.tsx) pokazują zużycie, nie kompletny rachunek pieniężny. Porównanie w walucie wymaga źródła i wersji ceny, obsługi cache oraz pokrycia wyceny. Czas treningu również nie jest automatycznie jego kosztem.
6. **Nie zaczynamy od nowego wykonawcy.** SDK ma już `TaskContext.record_evaluation`, wypełniający wykonanie/krok i stabilne ID raportu przy retry. W katalogu roboczym trwają zmiany uruchamiania w podach; nie należy uznawać ich za zweryfikowane wdrożenie. Wariant ewaluacji w Kubernetes zależy od zakończenia anulowania, obsługi śmierci poda i próby na lokalnym klastrze. Wariant z istniejącym workerem nie musi czekać na Kubernetes.

## 3. Priorytety względem katalogu

Rozmiar oznacza względny wysiłek obejmujący kontrakty, implementację i odbiór: S — lokalna zmiana; M — jeden przepływ; L — kilka warstw lub trwałość; XL — osobna inicjatywa. To nie estymaty kalendarzowe.

| Funkcja / grupa | Decyzja | Wartość i granica pierwszej wersji | Rozmiar |
| --- | --- | --- | --- |
| Porównywanie treningów | Teraz | 2–5 runów, epoki, parametry, metryki i URL | M |
| Baseline i rzetelność ewaluacji | Teraz | Jawna para, status porównywalności, brak fałszywych delt | M |
| Zapisane widoki | Teraz, lokalnie | Nazwane filtry, wybór runów i wykresów; bez backendu współpracy | S |
| Podstawowy lineage | Teraz, przy obiektach | Linki do znanych wersji i wykonania; bez globalnego grafu | S–M |
| Preferencje i dostępność | Przy pierwszym wydaniu | Motyw, klawiatura, stabilne kolory, czytelne stany błędów | S–M |
| Uruchamianie ewaluacji | Następne wydanie | Jeden szablon workflow, przypięte wejścia, ograniczenia, wynik | L |
| Scoring zapisanych odpowiedzi | Pierwszy wariant wykonania oceny | `score_existing` z wersją scorera i artefaktem wejścia, bez ponownej generacji | M |
| Trwały wynik i dowód promocji | Następne wydanie, fundament | Wersjonowany artefakt oceny i powtarzalna decyzja serwera | L |
| Feedback i typowane oceny | Kontrakt w B, przepływ w C | Cel oceny, źródło/autor, rubryka, rewizje i filtrowanie trace | M–L |
| Review → przypadek regresji | Przed raportami zespołowymi | Oczekiwana odpowiedź i źródło; zatwierdzenie do wersji datasetu | M–L |
| Bramka jakości aplikacji w CI | Razem z wykonaniem oceny | SDK/CLI, znane regresje, wynik i link do dowodów; osobno held-out | M |
| Experiments: jakość/czas/zużycie | Po kontrakcie wariantu | Konkretna kohorta przypadków, dowody i drill-down | L |
| Scorery / judges | Wąsko z ewaluacją | Jeden scorer deterministyczny i jeden adapter; wersja konfiguracji, zbiór kalibracyjny i rozbieżności z ocenami ludzi | M |
| Alerty | Po uruchamialnej ewaluacji | Błąd wykonania / regresja oceny, jeden kanał, deduplikacja | L |
| Diagnostyka i pierwsza integracja | Przed szerszym użyciem | Faktyczne możliwości instancji i potwierdzenie pierwszego sygnału | M |
| Workspace zespołowe i raporty | Po trwałych wynikach | Zapisane widoki serwerowe, opis + przypięte dane | L |
| Globalny lineage i katalog artefaktów | Później | Rozszerzenie sprawdzonych relacji, wyszukiwanie i paginacja | L |
| Playground | Później | Klient istniejącego procesu oceny, wspólne wejścia | M–L |
| Prompty chat i format odpowiedzi | Później | Natychmiast przypinać konfigurację wariantu; rozszerzenie tekstowego registry wdrożyć z migracją | M–L |
| Ocena sesji i trajektorii | Później | Najpierw zapisane sesje i kroki; przypięty zakres oraz kompletność | M–L |
| Sweeps / HPO | Później, na żądanie | Ograniczona liczba prób istniejącego workflow; najpierw prosty grid/random | L |
| Monitoring jakości online i budżety | Później | Wymagają scorerów, próbkowania, a budżety również wyceny | L |
| Organizacje, zespoły, projekty, klucze, audyt | Warunkowo | Wcześniej tylko przy wymaganiu wielu odizolowanych zespołów | XL |
| Global search, komentarze, wzmianki | Później | Gdy będzie wystarczająco dużo zapisanych analiz i użytkowników | M–L |
| Szeroki katalog integracji | Według popytu | Najpierw jedna rzeczywiście używana integracja z testem pierwszego sygnału | M na adapter |
| Asystent, HiveMind, zarządzane endpointy | Poza obecną roadmapą | Odrębne produkty; za mało wartości przed domknięciem podstawowego procesu | XL |
| Gateway dostawców LLM | Poza obecną roadmapą | Adapter używanego dostawcy/gatewaya wystarczy do ocen; własny routing wymaga osobnej decyzji | XL |

Rejestry, serving SDK, review danych, Query i harmonogramy należy utrzymywać i integrować. Ich ponowna implementacja nie jest potrzebna do powyższego zakresu. Rozbudowanej administracji i redesignu całego panelu nie stawiamy jako warunku rozpoczęcia porównań.

## 4. Kolejność realizacji

### Etap A — użyteczne porównania na obecnych danych

**Rezultat:** użytkownik porównuje kilka treningów, analizuje ewaluację względem wybranego baseline i wraca do tej analizy przez URL lub lokalny zapis.

1. Przygotować zestaw danych do odbioru: trzy treningi, dwie zgodne oceny, ocenę nieudaną, inny dataset, brak metryki oraz brak wersji danych. Sprawdzić ekran na działającej lokalnej instancji przed ustaleniem szczegółów interakcji.
2. Rozszerzyć wybór w Training runs do 2–5 ID w URL. Pobierać wyłącznie wybrane szczegóły. Pokazać różnice parametrów oraz krzywe tej samej metryki względem epoki; wartości `best` i ostatniego pomiaru oznaczać oddzielnie. Nie wprowadzać ogólnego rankingu metryk o nieznanym kierunku.
3. Ustabilizować kolor po tożsamości runu/serii, dodać dostępny odczyt wartości i rozdzielić jednostki. Przed A1 [LearningCurve](../apps/panel/src/shared/components/charts/learning-curve.tsx) dobierał kolor po indeksie przefiltrowanej listy. Brak pomiaru ma być widoczny, a interpolacja nie może udawać zarejestrowanego wyniku.
4. W Evaluation dodać jawny wybór baseline w URL i kontrakcie API. Domyślny wybór ograniczyć do sukcesu. Pokazać różnice parametrów, identyfikatory danych, pokrycie wspólnych przypadków i kompletność szczegółów. Nieznane pochodzenie oznaczać jako nieweryfikowalne; przy znanej niezgodności nie wyliczać delty jakości ani rekomendacji.
5. Zapis lokalny: nazwa, typ ekranu, `schema_version`, filtry, ID runów/baseline i wybór metryk. Oznaczyć „na tym urządzeniu”; klucz danych rozdzielić według instancji i tożsamości. Nie zapisywać treści rozmów, sekretów ani kopii raportów. Zapis widoku nie gwarantuje dostępności historycznych danych.
6. Dodać linki z dostępnych referencji: model → trening, ocena → wykonanie/krok, obiekt → rozpoznana wersja danych. Rozróżniać eksport adnotacji od datasetu curation; nazwa `project@version` nie wystarcza do wyboru rejestru. Brak pewnego celu pokazywać bez zgadywanego linku.

**Odbiór A:** zaznaczenie, odświeżenie i Back zachowują analizę; usunięcie runu nie zmienia pozostałych kolorów; brak metryki nie jest zerem; failed nie zostaje domyślnym baseline; niepełna lista przypadków nie jest opisana jako pełna analiza regresji. Dane bez wersji można oglądać, ale nie otrzymują statusu zweryfikowanego porównania.

**Granica:** limit listy treningów w obecnym API nie jest pełną paginacją. Pierwszy UI ma informować o ograniczonym zakresie wyników, a nie sugerować kompletność rejestru. Rozbudować stronicowanie dopiero wraz z potwierdzonym wymaganiem skali. Oś czasu/kroku można dodać później po ustaleniu semantyki timestampów i próbek.

### Etap B — kontrakty i trwałe dowody

**Rezultat:** nowe oceny mają odtwarzalny kontekst, a dowód decyzji pozostaje dostępny po utracie szczegółów projekcji.

1. Zaprojektować mały, wersjonowany manifest eksperymentu: `experiment_id`, niezmienny `variant_id`, referencje modelu/promptu/kodu oraz jawnie przypięte dane. Ponownie wykorzystać obecne `evaluation_id`, `execution_id`, `step_id`, `workflow_run_id`; nowe nazwy nie mogą dublować istniejących tożsamości.
2. Dodać kontekst oceny: typ i wersja datasetu, manifest wybranych przypadków, split, wersja suite/scorera oraz konfiguracja judge'a, jeśli jest użyty. Opisać jednostkę, kierunek poprawy i agregację metryki. Porównywalność oblicza serwer: zgodne / niezgodne / niezweryfikowane, z przyczynami.
3. Przenieść pełny wynik nowych ocen do wersjonowanego artefaktu z digestem; projekcja trzyma podsumowanie i referencję. Określić trwałość, pobieranie, paginację przypadków i zachowanie po usunięciu danych źródłowych. Stan „wynik częściowy” i „utracony szczegół” nie może wyglądać jak pełny raport.
4. Utrwalić relację wariant → ocena → konkretne runy/spany. Przenosić kontekst jawnie przez SDK, także przy współbieżności. [DimensionKind](../crates/aiwatcher-projector/src/dimensions.rs) nie ma dziś wariantu. Indeks do wyszukiwania i agregacji projektować dla spodziewanej liczby wariantów; nie dodawać automatycznie każdego ID do etykiet eksportowanych metryk.
5. Zdefiniować politykę decyzji osobno od etykiety wersji. Nowa rekomendacja promocji ma wymagać kompletnego held-out, zgodnego baseline i poprawy metryki według zadanej polityki. Obsłużyć pierwszy model bez baseline jako jawny przypadek inicjalizacji, bez komunikatu „lepszy od poprzednika”. Zmiana istniejących zasad etykietowania modeli wymaga opisanej migracji, nie ukrytej zmiany w UI.
6. Manifest wariantu obejmuje również rzeczywistą konfigurację generacji, schemat odpowiedzi, wersję kodu i definicji narzędzi/workflow. Schemat wejść i oczekiwań ma własną przypiętą wersję. Aliasy rozwiązywać w momencie startu. Dodać relację `case_id` → artefakt odpowiedzi → trace/span oraz tożsamość niezależnego powtórzenia pomiaru; techniczne retry pozostaje idempotentne.
7. Wprowadzić kontrakt typowanej oceny/feedbacku: jednoznaczny cel (trace/span/snapshot sesji/przypadek), wersja rubryki, typ wartości, źródło i autor, uzasadnienie, czas i rewizja. Ocena człowieka nie nadpisuje wyniku judge'a. Oczekiwana odpowiedź jest osobnym polem/artefaktem od oceny otrzymanej odpowiedzi. Wykorzystać istniejące review rozmów, zachowując rozdzielenie jakości i zgody na użycie treści.

**Odbiór B:** historyczne rekordy bez nowych pól nadal się odczytują; nic nie dopasowuje wariantu po nazwie modelu; zbyt duży raport można odczytać z artefaktu; restart i usunięcie szczegółów projekcji nie zmieniają przypiętego wyniku; niezgodny split/scorer blokuje decyzję; ponowienie tej samej próby nie tworzy drugiego logicznego wyniku.

Po rozszerzeniu o Langfuse/MLflow: zmiana konfiguracji lub aliasu po starcie nie zmienia manifestu; dwa niezależne powtórzenia przypadku pozostają osobnymi pomiarami; ocena recenzenta zachowuje autora i historię; schematy ocen waliduje API.

Pełne treści i artefakty pozostają poza logiem telemetrycznym zgodnie z istniejącą architekturą. Referencja w zapisanej analizie nie obchodzi prawa do odczytu ani polityki usuwania rozmów. „Przypięte” nie oznacza prawa do bezterminowego przechowywania treści.

### Etap C — uruchamianie ewaluacji i prawdziwe Experiments

**Rezultat:** użytkownik wybiera dane i warianty, uruchamia ocenę, śledzi pracę i dostaje porównanie z dowodami.

**Pierwsza paczka C0:** zbudować tryb `score_existing` na przypiętych odpowiedziach/trace, wykorzystując istniejący workflow. Źródłem treści jest dostępny dla wykonawcy artefakt lub uprawniony snapshot archiwum; trace z redakcją może nie wystarczać. Nowa wersja scorera daje nowy wynik z referencją do niezmienionych odpowiedzi. Nie wywołuje modelu aplikacji, ale może wywołać i obciążyć kosztem judge'a. Kolejne kroki rozwijają drugi tryb, `generate_and_score`.

1. Jeden formularz: wersja datasetu i split, baseline/kandydat, wersja zestawu scorerów, limit przypadków, timeout i współbieżność. Start tworzy istniejące managed execution. Lista suite z API jest agregatem raportów, więc definicja uruchamialnego zestawu oceny jest nowym, wersjonowanym zasobem.
2. Jeden szablon workflow z istniejącym workerem i jednym kontraktem wyniku. Zacząć od scorera deterministycznego; dodać jeden potrzebny adapter, np. do używanej integracji DeepEval. Judge musi mieć przypiętą konfigurację i przykłady kalibracyjne; szeroka biblioteka nie jest warunkiem wydania.
3. Zapewnić anulowanie, timeout, częściowy wynik i idempotencję startu/retry. Sekrety i wykonawcy są wybierani z dozwolonej konfiguracji serwera, a UI nie staje się uniwersalnym edytorem dowolnego kodu. Stosować istniejące reguły dostępu do uruchamiania.
4. Wypełnić Experiments: jeden wiersz na przypięty wariant, jakość, liczebność próby, błędy, czas i tokeny, linki do oceny i trace. Dla jakości i wydajności używać jawnego zbioru przypadków/wykonań. Oddzielać pomiary benchmarku od obserwacji produkcyjnych, a czas całego workflow od opóźnienia pojedynczej inferencji. Nie uśredniać percentyli podgrup.
5. Kwoty pieniężne pokazywać dopiero po dodaniu jawnego źródła/wersji ceny i pokrycia. Bez tego wydanie dostarcza porównanie tokenów. Nie mieszać kosztu budowy wariantu z kosztem jego obsługi ruchu.
6. Dodać przepis SDK/CLI do uruchomienia oceny w CI i sprawdzenia wyniku: pass, regresja, błąd lub niekompletna ocena. Zapisać commit, wersję suite i link do dowodów. Krytyczne przypadki mają własne warunki, niezależne od średniej. Zacząć od deterministycznego przykładu; bramka korzysta z tej samej polityki co UI. Widoczny zestaw regresji nie zastępuje niezależnego held-out do oceny poprawy.
7. Dodać ścieżkę feedback/trace → kolejka review → oczekiwana/poprawiona odpowiedź → zatwierdzony przypadek regresji. Wpiąć przypadki w istniejące wersjonowanie datasetów, z zachowaniem pochodzenia i decyzji o użyciu treści. Nie zapisywać automatycznie każdej krytycznej oceny użytkownika jako ground truth. Przepływ review może powstać po B4 i trwałym zapisie przypadków, bez czekania na pełny launcher generowania.

**Odbiór C:** na niewielkim, przypiętym zbiorze baseline i kandydat przechodzą cały proces; z raportu można dojść do właściwego wykonania oraz trace; retry nie podwaja metryk; brakujące obserwacje obniżają widoczne pokrycie; anulowanie zatrzymuje uruchomioną pracę. Wariant podowy wymaga dodatkowo testu śmierci workera i zakończenia Job na lokalnym klastrze.

Po rozszerzeniu: `score_existing` wykonuje zero nowych generacji aplikacji; niedostępna treść daje jawny brak wyniku. CI odrzuca krytyczną regresję mimo poprawy średniej i nie przepuszcza awarii scorera. Ponowne dodanie przykładu do review nie duplikuje pracy; eksport zachowuje źródło i uprawnienia. Odbiór judge'a obejmuje porównanie z ocenami ludzi na wydzielonym zbiorze; automatyczne dostrajanie judge'a pozostaje późniejszym rozszerzeniem.

### Etap D — operacje i współpraca

**Rezultat:** platforma informuje o wyniku lub problemie, a zespół może wrócić do uzasadnienia decyzji.

1. Alerty v1: terminalny błąd wykonania oraz regresja zakończonej, porównywalnej ewaluacji. Jeden kanał, najlepiej konfigurowany webhook. Trwała kolejka dostarczeń/outbox, retry z ograniczeniem, deduplikacja po zdarzeniu i wersji reguły, historia oraz test kanału. Odbiorca może otrzymać powtórzenie po niejednoznacznym błędzie sieci; przekazywać klucz deduplikacji zamiast obiecywać exactly-once.
2. Alerty okienkowe metryk dodać później: okres, minimalna próba, cooldown, odzyskanie poprawnego stanu i jawne zachowanie przy braku danych. Harmonogram uruchamia sprawdzenie, ale sam nie definiuje reguły alertu.
3. Diagnostyka możliwości instancji i integracja pierwszego źródła: dostępność registry/workera/oceny oraz link do pierwszego odebranego sygnału. Nie udostępniać sekretów w odpowiedzi diagnostycznej. UI konfiguracji tylko dla rzeczywiście edytowalnych ustawień.
4. Dopiero teraz serwerowe zapisane widoki i prosty raport: opis decyzji, przypięte warianty/oceny, kilka wykresów i referencje artefaktów. Osobno określić właściciela i uprawnienia zapisu/odczytu. Link do raportu nie nadaje dostępu do obiektów źródłowych.

**Odbiór D:** powtórzone zdarzenie nie tworzy nowego logicznego alertu; awaria kanału jest widoczna i ponawiana; raport zachowuje przypięte wyniki po zmianie etykiety produkcyjnej; osoba bez prawa do źródła nie dostaje jego treści przez raport.

## 5. Zależności i pierwsze paczki prac

```text
A: porównania + lokalny zapis + linki
                  │
                  ▼
B: kontekst + trwały wynik + wariant + typowane oceny
                  │
                  ▼
C: scoring istniejących odpowiedzi → generacja i ocena
   → porównanie → decyzja / CI
   feedback → review → przypadek regresji → wersja danych
                  │
                  ▼
D: alerty + zapis zespołowy + raport

Izolacja zespołów, jeśli wymagana → przed nowymi współdzielonymi zasobami
Wykonawca podowy → tylko ścieżka uruchomień w Kubernetes
```

Pierwsze paczki do implementacji, w tej kolejności:

| Paczka | Konkretny zakres | Główne miejsca zmian | Zależność |
| --- | --- | --- | --- |
| A1 | Zestaw odbiorowy, stabilne serie i odczyt wartości | `scripts/`, `shared/components/charts/` | Brak |
| A2 | Wybór 2–5 treningów, krzywe, parametry, URL | `features/training/screens/runs/` | A1 |
| A3 | Jawny baseline i uczciwe stany porównania | API/projektor evaluations, `features/evaluation/`, wygenerowany klient | Kontrakt obecnych danych |
| A4 | Nazwane lokalne widoki, preferencje, podstawowe linki | Training, Evaluation, ustawienia i wspólne prymitywy według użycia | A2, A3 |
| B1 | ADR: właściciele danych, referencje, kontekst oceny, trwałość i kompatybilność | `docs/ADR/`, fasada Evaluation, typy domeny i kontrakty SDK; AR1/AR2 | Wnioski z A |
| B2 | Zapis i odczyt trwałego wyniku | Nowy moduł Evaluation z własnym storage metadanych, artefakty, worker SDK, projekcja i API | B1 |
| B3 | Kontekst wariantu na obserwacjach, porównywalność i dowody decyzji | Evaluation; Core tylko neutralne kontrakty, SDK Python/TS, trace/projektor, modele/prompty | B1, B2 |
| B4 | Typowane oceny i feedback, rubryki, pochodzenie i rewizje | Domena/API ocen, SDK, Evaluation/Observability, adapter review rozmów | B1, B2 |
| C0 | Scoring zapisanych odpowiedzi | Usługa aplikacyjna startu, Execution, worker, artefakty, Evaluation | B2, B3, B4, AR3 |
| C1 | Jeden szablon generowania i oceny oraz formularz startu | Execution, worker, Evaluation | C0 |
| C2 | Experiments z jakością, czasem i tokenami | API porównań, `features/experiments/` | B3, C1 |
| C3 | Bramka regresji aplikacji w CI i przykład SDK/CLI | SDK, przykład integracji, kontrakt decyzji; bez wymaganego komentowania PR | C0; C1 dla testów generacji |
| C4 | Feedback → review → wersjonowany przypadek testowy | Observability, review, dataset i oczekiwania | B4, trwały zapis przypadków z B |

Ścieżki panelu w tabeli są względem `apps/panel/src/`. Każda paczka obejmuje testy zachowania i dokumentację swojego kontraktu. B1 może rozpocząć się podczas dopracowywania A; pełne Experiments nie powinno blokować dostarczenia A2.

Nie przypisuję dat na podstawie samej liczby funkcji: nie znam dostępnej obsady ani wyników wdrożenia wykonawcy. Po A1/A2 należy oszacować resztę na podstawie rzeczywistego czasu, wielkości danych i decyzji o wielozespołowości. Pierwszy zamknięty zakres wydania to A1–A4, a nie cały katalog P1.

## 6. Odbiór techniczny i miara wartości

Przed implementacją utrwalić próbki danych i zmierzyć, ile kroków zajmuje znalezienie różnicy między dwoma runami. Po etapie A użytkownik ma wykonać to w jednej analizie, bez ręcznego przepisywania parametrów; po C cały proces oceny ma działać z panelu, z możliwością dojścia do dowodu.

| Obszar | Wymagane sprawdzenie przy implementacji |
| --- | --- |
| Panel | Klawiatura, Back/refresh/URL, stany puste i błędy, brak serii, różne jednostki, oba motywy; `rtk npm run typecheck`, `rtk npm test`, `rtk npm run build` w `apps/panel` (architektura sprawdzana przez skrypty) |
| API i SDK | Stare rekordy, brakujące pola, niewłaściwy baseline, różny split/scorer, ograniczenia zapytań; `rtk just openapi` i testy zmienionych modułów oraz SDK |
| Trwałość | Restart, utrata szczegółów projekcji, duży raport, brak artefaktu, odebranie dostępu/usunięcie danych; te same zachowania na wspieranych magazynach |
| Execution | Retry po utracie odpowiedzi, anulowanie, timeout, częściowy wynik, współbieżne warianty; rzeczywisty workflow, nie tylko mock formularza |
| Feedback i regresje | Rewizje i źródła ocen, niedostępna treść, idempotencja kolejki, zero generacji w `score_existing`, krytyczny przypadek mimo lepszej średniej, błąd scorera blokujący CI |
| Przed scaleniem kodu | `rtk just check`; dodatkowe testy usług i lokalnego klastra tylko gdy zmieniona ścieżka ich wymaga |

Decyzje wymagające ustalenia najpóźniej w B1: izolacja zespołów, pierwszy rzeczywisty dataset i scorer, oczekiwana skala oraz czas przechowywania dowodów. Dla pierwszego wydania przyjmujemy wspólną instancję, maksymalnie pięć porównywanych treningów i lokalny zapis widoków; to wystarcza do rozpoczęcia A bez blokowania się rozbudowaną administracją.

Pierwotny przegląd był dokumentacyjny. Późniejsza implementacja i odbiór A są opisane w sekcji 8; nie potwierdzają realizacji scenariuszy B/C/D ani gotowości wdrożenia Kubernetes.

## 7. Granice architektury przy realizacji

Obecny modularny monolit wystarcza do A. Panel ma egzekwowane vertical slices; backend wymaga dwóch konkretnych wydzieleń dla zakresu B/C. [Pełna analiza](FTI_ARCHITECTURE_REVIEW.md) zawiera dowody w kodzie, mapę kontekstów i kryteria odbioru.

1. **AR1, razem z A/B1:** zapisać dozwolone zależności i właścicieli nowych danych, dodać kontrolę granic Rust do CI. F/T/I pozostaje przepływem produktu, a nie podziałem na trzy konteksty.
2. **AR2, B1–B4:** Evaluation posiada suite, rubryki, trwałe wyniki, assessments i porównywalność. Projektor ma przebudowywalny widok. Modele/prompty zachowują własne decyzje promocji; Conversations zachowuje zgodę, retencję i usuwanie. Core otrzymuje tylko neutralne kontrakty.
3. **AR3, przed C0:** wyjąć wspólny przypadek użycia kompilacji/startu z modułu HTTP. API i scheduler korzystają z jednej usługi aplikacyjnej, z zachowaniem idempotencji, autoryzacji i polityki payloadów. Nowy scorer jest zadaniem istniejącego workera, nie nowym silnikiem wykonania.
4. **AR4, wraz z B4/C:** nowe klienty ocen w modułach SDK; lekki import główny pozostaje lekki. UI, hooki i testy przy feature; wspólne komponenty dopiero przy rzeczywistym ponownym użyciu.

Rozdzielenie kompilatora curation od silnika i dalsze dzielenie dużych ekranów wykonywać przy zmianach, które tego potrzebują. Nie są warunkiem rozpoczęcia A1–A4. Szczegółowe propozycje nowych modułów nie oznaczają konieczności osobnego procesu lub wdrożenia.

## 8. Postęp implementacji (2026-09-11)

- Przegląd checkoutu: zachowane 32 wcześniejsze zmienione pliki AW-4 i nieśledzone dokumenty FTI. Nie wykonywano reset/clean/stash.
- Wszystkie paczki A1–A4/AR1 zaimplementowane. Nie utworzono commita ani nie wdrażano aplikacji.

| Paczka | Dostarczone zachowanie |
| --- | --- |
| A1 | `scripts/seed-dev.py --fti`: cztery treningi i sześć ocen, stałe ID, zachowanie istniejących rekordów. Kolor i kreskowanie po ID serii, przerwy w brakujących pomiarach, pełne wartości w dostępnej tabeli. |
| A2 | 2–5 runów w `runs` URL; obsługa starego `run`. Szczegóły pobierane niezależnie od ograniczonej do 100 listy; jawne błędy i usuwanie brakujących ID z porównania. Osobne osie metryk, różnice parametrów, zgłoszony best z epoką osobno od ostatniego pomiaru. |
| A3 | `GET /api/v1/evaluations/{id}?baseline_id=...`; jawny baseline w URL i lokalnym widoku. Brak wskazanego ID daje 404, bez podstawiania innego. Automatyka dopuszcza tylko sukces. Serwer zwraca status porównywalności, powody, kontekst baseline, pokrycie i kompletność. OpenAPI i klient zregenerowane. |
| A4 | Nazwane widoki z `schema_version=1`, listą dozwolonych metadanych, kluczem instancja + tożsamość; obsługa braku tożsamości, błędnego formatu i odmowy storage. Motyw dark/light, gęstość tabel, widoczny fokus, dostępny disclosure profilu. Linki do potwierdzonych runów, wykonań/kroków oraz właściwego registry danych. |
| AR1 | Jawna polityka `scripts/rust-boundaries.json`, kontrola `cargo metadata`, osobne normal/build/dev, aliasy i nieaktywne targety. Trzy testy negatywne; bramka w `scripts/check.sh` i CI. Właściciele i wyjątki opisani w przeglądzie architektury. |

### Kontrakt porównywalności

Kontekst starszych zdarzeń odczytuje opcjonalne `dataset_kind`, `dataset_version`, `suite_version`, `scorer_version`, `split` z danych zdarzenia lub jego `params`. Nie dopisuje fikcyjnych wartości. Sukces obu wyników, ta sama suite i dane oraz komplet zgodnych pól pozwalają na `comparable`; znana różnica albo niewłaściwy status daje `incompatible`, a brak dowodu `unverified`. Dla dwóch ostatnich stanów serwer wstrzymuje delty oraz wnioski o zmienionych przypadkach. Tak samo zabezpieczono agregaty suite.

`common_cases` obejmuje wyłącznie wspólne zachowane identyfikatory. Niepełne szczegóły nie unieważniają dostępnych agregatów zgodnej pary, ale UI wyraźnie ogranicza wnioski o regresji do zachowanych przypadków. Polityka promocji modeli i promptów nie została zmieniona w kodzie.

### Powtórzenie odbioru

Standardowo uruchom projekt zgodnie z `just dev`, a seed skieruj jawnie do właściwej instancji:

```sh
rtk proxy python3 scripts/seed-dev.py --fti --api http://127.0.0.1:18080
```

W tej sesji API działało na `127.0.0.1:18080`, panel na `localhost:15173`, dane odbiorowe w `.data/fti-acceptance`, z memory bus i memory workflow store. Istniejącego procesu `:8080` nie zatrzymywano. Bind i localhost wymagały wyjścia z sandboxa. Seed FTI uruchomiono ponownie po restarcie API; trwałe treningi zostały zachowane, a projekcja ocen odtworzona przez ponowne wysłanie brakujących ID.

W Training wybierz `fti-training-1`, `-2`, `-3`. Porównanie pokazuje brak accuracy w trzecim runie, przerwę w epoce 2 i różnicę best/last. W Evaluation ustaw `window=0`, raport `fti-eval-candidate`, baseline `fti-eval-baseline`; następnie sprawdź `fti-eval-failed`, `fti-eval-other-data`, `fti-eval-legacy` i nieistniejące ID. Raport `fti-eval-partial` zachowuje 1 z 3 zgłoszonych przypadków.

### Wyniki weryfikacji

- **PASS:** pełne `rtk just check` po naprawieniu oczekiwań starych testów wobec nieweryfikowanych danych. Rust fmt/clippy/test, aktualność OpenAPI, granice panelu/Rust, panel build/test, Python i TypeScript SDK, agentic engine, manifesty Kubernetes, Helm, Tiltfile, komentarze, typos, taplo, cargo deny. Log sesji: `/tmp/fti-just-check-unsandboxed.log`.
- **PASS:** właściwe testy projekcji ewaluacji (19), HTTP, testy negatywne AR1; odczyt starych rekordów, failed/self/missing baseline, różny lub brakujący split/scorer/dataset, utrata szczegółów i agregaty bez przypadków.
- **PASS:** po ostatnich poprawkach UI ponownie wykonano `npm run typecheck`, `npm test` (203 testy / 23 pliki) i `npm run build`; także `git diff --check`, typos i kompilację składni skryptów Python. Testy panelu obejmują szczegóły spoza listy, usunięty run, limit 5, Back i odtworzenie URL, stabilność koloru/kreskowania, brak pomiaru, wysłanie baseline do API, niezgodne porównanie, zapis i rozdzielenie tożsamości, odmowę storage i potwierdzone cele lineage.
- **PASS runtime:** trzy treningi → usunięcie → Back/refresh → zapis/otwarcie; best/last, osobne jednostki i braki; zgodna para, failed, różna wersja danych, brak wersji, brak ID i pokrycie 1/3; zapis ewaluacji z baseline i metrykami → odświeżenie; model → run → właściwy eksport adnotacji. Oględziny Training/Evaluation w obu motywach, widoczny fokus, zmiana motywu i nawigacja klawiaturą.
- **Zakres odbioru dostępności:** podstawowe zmienione przepływy, bez deklaracji pełnego audytu WCAG. Natywny fullscreen zależy od przeglądarki; osadzona przeglądarka nie potwierdziła wejścia w ten tryb, więc nie oznaczono go jako PASS. Link wykonania/kroku i curation sprawdzono testem komponentu; nie uruchamiano dodatkowego rzeczywistego workflow.

### Ograniczenia i incydent dodatkowego seeda

Widoki przechowują maksymalnie 30 nazwanych konfiguracji lokalnych; nie zachowują raportów ani nie obchodzą retencji/uprawnień. Lista treningów nadal ma limit 100, bez pełnej paginacji. Preferencje A obejmują motyw i gęstość; strefa i format dat pozostają ustawieniami przeglądarki. B/C/D i AR2–AR4 pozostają roadmapą.

Podczas dodatkowego sprawdzania lineage istniejący `seed-demo-annotations.py --base-url http://127.0.0.1:18080` poprawnie zasilił adnotacje na :18080, ale część treningowa użyła stałego `BASE` (:8080). Poprawiono ją na `TrainingClient(args.base_url)` i potwierdzono ponownym uruchomieniem na :18080. Pierwsze uruchomienie **dodało na istniejącej instancji :8080** run `demo-train-1789154960` oraz wersję `demo.segmenter` `530453878ffcc48165ad8008fb359298d4ba15b0f5110d21f4729f54b1be8963`, ustawiając jej etykietę `production`. Starsza wersja `a2fcc5f04de5f85a39f82069c08b601eb908bb55ea30f661fc4a5ca9bc71a45e` i jej run pozostały zachowane. Nie ma zapisu poprzedniej wartości etykiety; nie cofano jej przez zgadywanie. Użytkownik po otrzymaniu opisu skutku zdecydował: „Pozostaw nową wersję demonstracyjną”. Etykieta pozostaje zgodnie z tą decyzją; nie ma oczekującej operacji przywracania.

## 9. Kontynuacja — B1 / początek AR2 (2026-09-11)

Przegląd startowy: czysty checkout, HEAD `26be55e` zawiera poprzednią implementację FTI i zachowane zmiany AW-4. Zakres tej kontynuacji to B1; trwały zapis B2 i pozostałe paczki nie są oznaczane jako dostarczone.

- Dodano [ADR 0030](ADR/ADR_0030_EVALUATION_EVIDENCE.md): właściciele, tożsamości, przypięty kontekst, protokół trwałego zapisu, retencja, dostęp i migracja starszych raportów.
- `aiwatcher-evaluation`: publiczna fasada `Evaluation::prepare`, prywatne moduły manifest/context/reference, walidacja wersji, referencji, danych, metryk i tożsamości; niezmienny wynik z `variant_id` i `context_id`.
- Neutralne `core::storage::{ObjectStore, ObjectEntry}` z zachowanymi reexportami root i `core::prompts`. Bramka AR1 dopuszcza Evaluation → Core; żadnych nowych zależności od Projector/API/Execution.
- Generowany JSON Schema i typy producenta Python/TypeScript. Nowy moduł SDK nie importuje workerów ani transportów, nie zastępuje starego `record_evaluation`.
- Wspólny syntetyczny fixture w `contracts/fixtures/evaluation-v1/`; lokalny przykład `prepare` waliduje i wylicza identyfikatory, nie zapisuje registry i nie kontaktuje się z działającą instancją.

Użytkownik nie ma jeszcze docelowego datasetu/scorera/skali/retencji i polecił wybrać opcje. Wybrano krótki syntetyczny zbiór odpowiedzi i dokładne dopasowanie Unicode, bez normalizacji ani płatnego judge'a. Dla B2: konfigurowalne 10 000 przypadków, 100 MiB raportu, strony po 200 i 30 dni retencji treści, skracane polityką/usunięciem źródła. To wartości startowe do implementacji B2, nie potwierdzone obciążenie produkcyjne. Aktualna fasada ogranicza manifest do 256 KiB i 128 metryk.

**Weryfikacja zakończona:**

- **PASS:** pełne `rtk just check`; log `/tmp/fti-b1-just-check.log`. Rust fmt/clippy ze wszystkimi features/testy workspace, bramki architektury, Dockerfile, aktualność OpenAPI i nowego schematu Evaluation, panel build/typecheck i 203 testy, oba SDK, agentic, manifesty Kubernetes, Helm, Tiltfile, komentarze, typos, taplo i cargo deny.
- **PASS:** 11 testów nowej fasady Rust — stabilne ID retry, odrębne niezależne powtórzenie, zmiana generacji/kodu/datasetu/workflow, zmiana scorera/kohorty/splitu/jednostki/kierunku/agregacji, konfiguracja judge'a, błędne/metadane ponad limit, normalizacja pól opcjonalnych, utrwalone ID fixture i oba stare importy storage.
- **PASS:** 13 testów kontraktu Python — zgodność fixture z generowanym schematem, pola/wymagalność/enumy SDK, digest i długość rzeczywistych plików, dokładna semantyka scorera, lekki import. TypeScript kompiluje wygenerowany fixture konsumenta, w tym judge i negatywne przypadki typowania.
- **PASS:** cztery testy bramki Rust, w tym niedozwolone zależności nowego właściciela. Osobno sprawdzono komentarze nowych, jeszcze nieśledzonych plików, których istniejący linter oparty na `git ls-files` nie obejmuje.
- **PASS lokalnego przykładu:** `rtk cargo run -p aiwatcher-evaluation --example prepare -- contracts/fixtures/evaluation-v1/manifest.json` zwraca przygotowany manifest i identyfikatory zgodne z `identities.json`. Nie uruchamiano seedów ani nie modyfikowano danych działających instancji.

B1 nie dodaje nowego ekranu ani endpointu. `Evaluation::prepare` weryfikuje deklarację i przypina metadane; nie pobiera artefaktów, nie sprawdza ich uprawnień ani nie przechowuje raportów. Typy SDK nie są walidatorem runtime. Wersje/referencje wymagają rozstrzygnięcia przez właściciela przed publikacją; poprawny kształt digestu nie potwierdza jego zgodności z bajtami. Starsze zdarzenia i ich odczyt pozostają kompatybilne. Nie wykonano commita ani wdrożenia.

**Następny zakres B2:** prywatny zapis i odczyt Evaluation, atomowy konflikt/idempotencja po `evaluation_id`, artefakty z weryfikacją digestów, strony przypadków, retencja i usunięcie źródła, odczyt API preferujący trwały wynik; brak fallbacku po tombstone/odmowie dostępu. AR2 jest rozpoczęte, a nie ukończone: test restartu i utraty projekcji wymaga implementacji B2.


## 10. Kontynuacja — B2: trwały wycinek syntetyczny

Stan wejściowy: zachowane niezacommitowane B1. Dodano neutralne atomowe `ObjectStore::create` (memory, plik z hard-linkiem kompletnego stagingu i fsync, S3 `If-None-Match: *`). Evaluation ma prywatny storage, niezmienne wyniki, agregaty z metryk przypadków, osobne artefakty odpowiedzi/oczekiwań, paginację, retencję i minimalne tombstones. Klienci registry są oddzieleni od starego best-effort `record_evaluation`.

Bieżąca ścieżka odbiorowa: operator wskazuje lokalny syntetyczny bundle przez `AIWATCHER_EVALUATION_SOURCE_DIR`; adapter sprawdza zatwierdzony wariant/kontekst oraz rzeczywiste bajty wszystkich referencji. Native curation/annotations/conversations i zewnętrzne modele/judge nie mają jeszcze adapterów potwierdzających prawa i są odrzucane. Samo zadeklarowanie digestu lub URI nie nadaje dostępu. To jawne ograniczenie pierwszego adaptera, nie migracja uprawnień innych rejestrów.

Nowe `/api/v1/evaluation-results` zapewnia trwałe odkrywanie niezależne od projekcji. Stare szczegóły preferują trwały wynik; stare listy/suite/baseline wykluczają ID zajęte przez registry. Pierwsza strona pozostaje dostępna w starym kształcie, pełne strony i stany dowodów przez nowe API. Porównywanie trwałych wyników należy do B3.

**Odbiór wykonany:**

- **PASS pełnego `rtk just check`**, log `/tmp/fti-b2-just-check.log`: Rust fmt/clippy all-features/testy workspace, architektura, kontrakty, panel build/typecheck/203 testy, Python SDK 440 testów, TypeScript typecheck i 3 nowe testy runtime, agentic 361 testów, deploy/Helm/K8s, komentarze, typos, taplo i cargo deny. Po końcowych korektach skracania retencji źródła i kontroli całego budżetu przed pierwszym zapisem ponowiono odpowiednie testy Rust/clippy/fmt.
- **PASS wspólnego protokołu memory/file/real S3**: jedyny zwycięzca różnych publikacji, odzyskanie utraconej odpowiedzi na każdym adapterze, 1201 przypadków stronicowanych po 137, ponowny odczyt, odmowa dostępu i trwałe usunięcie. Dodatkowe testy obejmują częściowe wyniki, brak/uszkodzenie shardu, skrócenie retencji tuż przed commitem, rzeczywiste pliki źródła i odrzucenie pełnego budżetu bajtów przed pierwszym zapisem. S3 używał wyłącznie własnego kontenera `aiwatcher-fti-b2-rustfs` na :19010.
- **PASS mostka HTTP**: trwały wynik zastępuje stary przy tym samym ID, retry zwraca ten sam receipt, zmiana treści daje 409, usunięcie źródła daje 410 na starym szczególe; listy/suite/automatyczny baseline nie wskrzeszają starego ID, odkrywanie trwałe działa po utracie całej projekcji.
- **PASS odbioru procesu i obu SDK** na osobnej instancji :19080: opublikowano `fti-b2-live-1`, accuracy=1.0, strony 2+1, zachowana pusta oczekiwana odpowiedź. Po zatrzymaniu i ponownym uruchomieniu z tym samym katalogiem plikowym wynik pozostał dostępny, a retry zwrócił identyczne ID i pierwotny czas commita/retencji. Logi `/tmp/fti-b2-server*.log`, dane `/tmp/aiwatcher-fti-b2-runtime`. Wersja `f8d185d20d34faac2f94d4cc7ba4b48292df45222f66765afa08dc9865a3e71f`.
- **PASS ręcznej kontroli komentarzy nowych, nieśledzonych plików**; zwykły linter opiera się na `git ls-files`.

**Dostarczony zakres i ograniczenia:**

Pierwszy adapter jest wyłącznie syntetyczny: nie zastępuje zgód ani polityk natywnych rejestrów. Native dataset/model/prompt/judge są odrzucane do czasu wdrożenia ich adapterów. Artefakty bez zatwierdzonego ID po przerwaniu publikacji pozostają niewidoczne, ale wymagają przyszłej bezpiecznej zbiórki; nie obiecujemy dla nich wdrożonego orphan GC. Odkrywanie aktualnie skanuje klucze object store, a odczyt sprawdza shardy — przed zwiększeniem skali należy zmierzyć koszty. HTTP ma stały dodatkowy limit ciała 100 MiB.

Registry nie emituje jeszcze powiadomienia na log po commicie. Nowy trwały katalog jest osobnym endpointem; pełny ekran i porównania pozostają B3. Stary SDK telemetryczny nie został zmieniony. Nie wykonano commita ani wdrożenia; istniejące dane :8080/:18080 nie były modyfikowane. Własną instancję testową :19080 i kontener RustFS :19010 zatrzymano po odbiorze. Końcowo PASS: 10 testów rejestru (w tym real S3), 4 wybrane testy HTTP `durable_` oraz clippy zmienionych crate’ów.

**B2/AR2 nie są zamknięte w pełnym zakresie:** następne są adaptery źródeł przez publiczne fasady właścicieli i współbieżnie bezpieczne orphan GC. Przed zamknięciem rozszerz ten sam zestaw testów o ich rzeczywiste polityki usunięcia/retencji/uprawnień. Nie powtarzaj implementacji B1 ani A.
