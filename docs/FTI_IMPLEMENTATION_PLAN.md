# FTI — rekomendacja zakresu i plan rozwoju

Data: 2026-09-11. Status: A1–A4 i AR1 zaimplementowane; B1 zweryfikowane, trwały wycinek B2 i atomowe orphan GC nowych publikacji dostarczone; dodano weryfikowane adaptery Curation, promptów, modeli, Annotations i Conversations; B2/AR2 pozostają otwarte: B2e–B2h, w tym judge (sekcje 9–16). Wyniki odbioru A, ograniczenia i incydent seeda w sekcji 8. Przegląd planu z 2026-09-12 jest w sekcji 17; jego wnioski są wniesione do sekcji 2–7 — etap B ma punkty 8–11 i rozstrzygnięcia wizualne, tabela paczek B2e–B2i, a B3 zależy od B2e, B2f i B2i. Sekcja 19 zmniejsza ograniczenia z sekcji 18; sekcja 20 dostarcza stronę dowodową B3 — porównanie dwóch trwałych wyników; sekcja 21 dostarcza AR3 — wspólny przypadek użycia kompilacji i startu, wyjęty z modułu HTTP; sekcja 22 domyka jego ograniczenie — rejestr definicji rozróżnia niedostępny magazyn, uszkodzony rekord i odmówioną definicję; sekcja 23 dostarcza ostatnią część B3 — różnicę na poziomie przypadków; sekcja 24 dostarcza B4 — typowane oceny, rubryki i rewizje; sekcja 25 dostarcza pierwszą paczkę C0 — scoring zapisanych odpowiedzi jako zarządzany run publikujący własny dowód; sekcja 26 zamyka ograniczenia sekcji 25 — jeden status dla niezatwierdzonej pary, scorer ilościowy z jednostką, archiwum rozmów jako źródło odpowiedzi, judge jako scorer z regułą dopuszczenia z ADR 0030 i formularz startu w panelu. Sekcja 27 poprawia ograniczenia sekcji 26 — ponowienie próby judge'a nie pyta drugi raz i nie kończy się konfliktem, wynik mówi, co obsłużył dostawca, judge widzi pytanie przypadku, próg poziomu z przedziałem zgodności, a panel śledzi uruchomiony run. Sekcja 28 dopuszcza judge'a nad archiwum rozmów z ostrzeżeniem w kontekście, deklaracji, panelu i logu oraz liczy skrót bundle'a z tego, co bundle dodaje, zamiast z bajtów manifestu. Sekcja 29 domyka luki C0 — anulowanie i timeout zatrzymują krok, deklaracja ma własny timeout i współbieżność, kohortę wyprowadza serwer z wersji datasetu z limitem przypadków — i dodaje metryki DeepEval, Opik i każdego adaptera za jednym kontraktem serwisu scorerów. Sekcja 30 zamyka cztery ograniczenia sekcji 29 — zapis w toku kończy się mimo terminu, anulowanie dociera do silników zapytań i notebooków, metryki frameworków oceniane modelem mają zgodność z ludźmi, serwis scorerów ma token, obraz i chart, a karty powstają w panelu — i dostarcza C1: generowanie odpowiedzi przez workera i ich ocenę, z baseline'em obok kandydata.

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
7. **AW-5 dostarczyło połączenia oceny i bramkę etykiety.** [AW-5](specs/AW-5-optimise-evaluate-and-promote-a-prompt-in-one-managed-run/_index.md) jest zakończone: raport ma referencję wykonania i kroku, optymalizacja promptu wiąże raporty held-out, a `production` odmawia kandydatowi odrzuconemu przez verdict. Punkt 4 i B5 nie definiują tego od nowa — są zawężone do polityki decyzji dla **modeli** i do wymogu kompletnego held-out.
8. **Trwałe dowody dopuszczają jedną zatwierdzoną parę wariant/kontekst.** `LocalSource::resolve` porównuje publikowany manifest z jednym `manifest.json` we wskazanym katalogu; drugi wariant wymaga podmiany katalogu, po której wcześniejsze wyniki czytają się jako `forbidden`. Porównanie dwóch trwałych wyników (B3) oraz publikacja z wykonania (C0/C1/C3) wymagają wcześniej zatwierdzenia jako zasobu. Dowody w sekcji 17.1.

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
| Zatwierdzenie źródeł dowodów jako zasób | Teraz, przed B3 | Wiele przypięć naraz, adresowanych treścią, z zapisem i wycofaniem zatwierdzenia; bez katalogu na dysku serwera | M |
| Koszt odczytu i indeks katalogu dowodów | Teraz, przed B3 | Podsumowanie bez czytania przypadków, lista bez weryfikacji źródła na wiersz, porządek czasowy katalogu | M |
| Panel trwałych dowodów | Razem z B3 | Stany dowodu, termin retencji, pochodzenie wiersza i kontrolka porównywalności; bez drugiej implementacji reguł w TypeScript | S–M |
| Feedback i typowane oceny | Kontrakt w B, przepływ w C | Cel oceny, źródło/autor, rubryka, rewizje i filtrowanie trace | M–L |
| Review → przypadek regresji | Przed raportami zespołowymi | Oczekiwana odpowiedź i źródło; zatwierdzenie do wersji datasetu | M–L |
| Bramka jakości aplikacji w CI | Razem z wykonaniem oceny | SDK/CLI, znane regresje, wynik i link do dowodów; osobno held-out | M |
| Experiments: jakość/czas/zużycie | Po kontrakcie wariantu | Konkretna kohorta przypadków, dowody i drill-down | L |
| Scorery / judges | Wąsko z ewaluacją, po własnej regule dopuszczenia | Jeden scorer deterministyczny i jeden adapter; wersja konfiguracji, zbiór kalibracyjny i rozbieżności z ocenami ludzi. Wyniku judge'a nie potwierdza ponowny odczyt bajtów, więc dopuszczenie ma inną regułę niż pozostałe źródła | M |
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
8. Zatwierdzenie źródła dowodów jest wersjonowanym zasobem, nie katalogiem w konfiguracji procesu: wiele przypięć naraz, adresowanych treścią, z zapisem kto i kiedy zatwierdził oraz z wycofaniem. Bez tego nie da się mieć jednocześnie czytelnego baseline i kandydata, a każda publikacja z wykonania wymaga ręcznego kroku na hoście. Właścicielem pozostaje Evaluation; nadal nie czyta prywatnych kluczy innych rejestrów.
9. Rozdzielić koszt odczytu od jego zakresu: podsumowanie odpowiada z metadanych, shard weryfikuje się przy czytaniu jego strony, lista nie weryfikuje źródła dla każdego wiersza, a sprzątanie korzysta z terminów w receipt zamiast z pełnej weryfikacji treści. Wybrać porządek katalogu — dziś klucz jest skrótem ID, więc „najnowsze pierwsze” wymaga pełnego skanu. Zmierzyć na wartościach startowych: 10 000 przypadków, 100 MiB, strony po 200, 30 dni.
10. Rozstrzygnąć usunięcie pojedynczego dowodu: albo trasa z uprawnieniem, albo zapisane w ADR 0030 stwierdzenie, że dowód znika wyłącznie przez usunięcie źródła i retencję. Dla źródeł `external` nie ma czego usunąć, więc milczenie oznacza 30-dniowy zegar jako jedyne narzędzie.
11. Judge dostaje własną regułę dopuszczenia. Pozostałe źródła dopuszcza ponowny odczyt bajtów u właściciela; wyniku judge'a nikt ponownie nie potwierdzi. Zapisać w ADR 0030: konfiguracja przypięta treścią, zbiór kalibracyjny i rozbieżność z ocenami ludzi jako część dowodu oraz jawne oznaczenie wyniku jako nieodtwarzalnego przez ponowny odczyt. Adapter dopiero po tym rozstrzygnięciu.

**Zmiany wizualne etapu B.** Trwałe dowody nie mają dziś żadnej postaci w panelu: [ekran Evaluation](../apps/panel/src/features/evaluation/screens/overview/page.tsx) czyta wyłącznie projekcję logu, a wygenerowany klient ma `listResults`, `getResult` i `getCases`, których nie woła żaden ekran. Poniższe rozstrzygnięcia należą do B2i i B3.

- **Siedem stanów dowodu to nie są stany błędu.** `EvidenceState` ma `complete`, `partial`, `missing_artifact`, `corrupt_artifact`, `expired`, `deleted_source` i `forbidden`, a cztery z nich są poprawnymi zakończeniami o różnym następnym kroku: po `expired` i `deleted_source` nie ma czego ponawiać, `corrupt_artifact` bywa odwracalny naprawą bajtów, a `forbidden` ma trzy różne przyczyny — wycofane prawa lub review u źródła, cofnięte zatwierdzenie operatora, brak roli Admin przy dowodach ze źródła Conversations. Każda potrzebuje własnego zdania; wspólne „nie udało się” czyta się jak awaria. Obowiązuje reguła panelu: nieudany odczyt nie jest stanem pustym, a `forbidden` z powodu roli renderuje się jak w Conversations — „czytanie treści wymaga roli admin”, nie jak porażka.
- **`state: partial` i `status: partial` znaczą co innego.** Wynik niesie oba pola: pierwsze mówi, że część dowodów jest nieczytelna, drugie — że część przypadków nie ma oceny. Dwie plakietki z tym samym słowem obok siebie to najtańszy sposób na pomylenie ich; nazwać je w UI osobno.
- **Retencja jest faktem z datą.** Receipt niesie `expires_at`, skracany przez źródło. Dowód czytelny dziś i nieodwracalnie nieczytelny za trzy dni wygląda dziś tak samo jak trwały. Pokazywać termin przy wyniku — tak jak Conversations rysuje pasek eksportu, bo mianownik jest faktem — i nie obiecywać dostępności, której zapis widoku z A4 również nie obiecuje.
- **Status porównywalności zasługuje na kontrolkę, nie na akapit.** Serwer z A3 zwraca `comparable`/`incompatible`/`unverified`, powody, pokrycie i kompletność; ekran renderuje to zdaniami, w których najważniejsze — „delty wstrzymane” — jest czwartym zdaniem pod trzema innymi. Jedna widoczna kontrolka stanu z powodami pod spodem, a wstrzymana delta widocznie wstrzymana zamiast po prostu nieobecnej. B3 używa tej samej kontrolki dla dwóch trwałych wyników.
- **Katalog trwały i lista z projekcji to jedna lista z widocznym pochodzeniem wiersza.** Stare listy celowo wykluczają ID przejęte przez registry, a szczegół preferuje trwały wynik i odpowiada 403/410 po wycofaniu. Dwie zakładki albo ciche scalenie dają ten sam efekt: raport „znika” bez powodu. Wiersz ma mówić, czy jest fałdą logu, czy trwałym dowodem.
- **Katalog dziedziczy konwencje list panelu poza jedną.** `useInfiniteQuery` i `VirtualList`, filtry w URL, kursor z serwera — tak. Okno czasu — nie: katalog nie fałduje logu, a jego klucz jest skrótem ID, więc nie ma porządku czasowego. Albo B2f daje mu indeks, albo ekran mówi wprost, że nie jest sortowany po czasie; kontrolka okresu, która niczego nie zawęża, jest gorsza niż jej brak.
- **Brak konfiguracji to 501 z nazwą zmiennej, nie pusta lista.** Tak odpowiadają Prompts, Datasets i Conversations. Po B2e ta sama zasada obejmuje brak zatwierdzenia: dziś „publikacja odmówiona” nie ma w UI żadnego śladu, który tłumaczyłby powód.

Czego nie robimy: drugiej implementacji porównywalności w TypeScript — reguły i powody liczy serwer, panel renderuje jego odpowiedź, jak przy kanwie adnotacji i kanwie pipeline'u. I nie kolorujemy delt metryk: kierunek poprawy jest nieznany, więc zielona liczba może znaczyć „zdrożało”. Powód jest zapisany w [search.ts](../apps/panel/src/features/evaluation/screens/overview/search.ts) i zostaje.

**Odbiór B:** historyczne rekordy bez nowych pól nadal się odczytują; nic nie dopasowuje wariantu po nazwie modelu; zbyt duży raport można odczytać z artefaktu; restart i usunięcie szczegółów projekcji nie zmieniają przypiętego wyniku; niezgodny split/scorer blokuje decyzję; ponowienie tej samej próby nie tworzy drugiego logicznego wyniku. Dwa warianty jednej suite są czytelne jednocześnie i pozostają czytelne po zatwierdzeniu trzeciego; podsumowanie wyniku nie czyta jego przypadków; strona katalogu nie weryfikuje źródła każdego wiersza; koszt jednego przebiegu sprzątania nie rośnie z rozmiarem opublikowanych wyników; usunięcie dowodu ma opisane narzędzie albo opisany brak narzędzia. Na ekranie: żaden z siedmiu stanów dowodu nie renderuje się jako stan pusty, dwa znaczenia słowa „partial” są rozróżnione, termin retencji jest widoczny przy wyniku, a wstrzymana delta jest widocznie wstrzymana.

Po rozszerzeniu o Langfuse/MLflow: zmiana konfiguracji lub aliasu po starcie nie zmienia manifestu; dwa niezależne powtórzenia przypadku pozostają osobnymi pomiarami; ocena recenzenta zachowuje autora i historię; schematy ocen waliduje API.

Pełne treści i artefakty pozostają poza logiem telemetrycznym zgodnie z istniejącą architekturą. Referencja w zapisanej analizie nie obchodzi prawa do odczytu ani polityki usuwania rozmów. „Przypięte” nie oznacza prawa do bezterminowego przechowywania treści.

### Etap C — uruchamianie ewaluacji i prawdziwe Experiments

**Rezultat:** użytkownik wybiera dane i warianty, uruchamia ocenę, śledzi pracę i dostaje porównanie z dowodami.

**Pierwsza paczka C0** (dostarczona w sekcji 25)**:** zbudować tryb `score_existing` na przypiętych odpowiedziach/trace, wykorzystując istniejący workflow. Źródłem treści jest dostępny dla wykonawcy artefakt lub uprawniony snapshot archiwum; trace z redakcją może nie wystarczać. Nowa wersja scorera daje nowy wynik z referencją do niezmienionych odpowiedzi. Nie wywołuje modelu aplikacji, ale może wywołać i obciążyć kosztem judge'a. Kolejne kroki rozwijają drugi tryb, `generate_and_score`.

1. Jeden formularz: wersja datasetu i split, baseline/kandydat, wersja zestawu scorerów, limit przypadków, timeout i współbieżność. Start tworzy istniejące managed execution. Lista suite z API jest agregatem raportów, więc definicja uruchamialnego zestawu oceny jest nowym, wersjonowanym zasobem.
2. Jeden szablon workflow z istniejącym workerem i jednym kontraktem wyniku. Zacząć od scorera deterministycznego; dodać jeden potrzebny adapter, np. do używanej integracji DeepEval. Judge musi mieć przypiętą konfigurację i przykłady kalibracyjne; szeroka biblioteka nie jest warunkiem wydania.
3. Zapewnić anulowanie, timeout, częściowy wynik i idempotencję startu/retry. Sekrety i wykonawcy są wybierani z dozwolonej konfiguracji serwera, a UI nie staje się uniwersalnym edytorem dowolnego kodu. Stosować istniejące reguły dostępu do uruchamiania. Publikacja dowodu z wykonania wymaga zatwierdzenia jako zasobu (etap B, punkt 8); bez niego każde uruchomienie potrzebuje ręcznego kroku na hoście.
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
   + zatwierdzenie jako zasób, koszt odczytu, indeks katalogu
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
| B2e | Zatwierdzenie źródła jako wersjonowany zasób z wycofaniem; usunięcie dowodu albo jawny jego brak w ADR | Evaluation, API, Server, konfiguracja | B2 |
| B2f | Podsumowanie bez odczytu przypadków, lista bez weryfikacji źródła na wiersz, sprzątanie z receiptu, indeks porządku katalogu | Evaluation | B2 |
| B2g | Zadanie CI na rzeczywistym magazynie obiektów, raportowanie sprzątania, zmienne w chart/INSTALL i receptura `just` | `.github/workflows/`, `deploy/`, `justfile`, Server | B2 |
| B2h | Judge: reguła dopuszczenia w ADR 0030, potem adapter | `docs/ADR/`, Evaluation, Server | B2e |
| B2i | Panel trwałych dowodów: siedem stanów, retencja, pochodzenie wiersza, kontrolka porównywalności | `features/evaluation/` | B2e, B2f |
| B3 | Kontekst wariantu na obserwacjach, porównywalność i dowody decyzji | Evaluation; Core tylko neutralne kontrakty, SDK Python/TS, trace/projektor, modele/prompty, `features/evaluation/` | B1, B2, B2e, B2f, B2i |
| B4 | Typowane oceny i feedback, rubryki, pochodzenie i rewizje | Domena/API ocen, SDK, Evaluation/Observability, adapter review rozmów | B1, B2 |
| C0 | Scoring zapisanych odpowiedzi | Usługa aplikacyjna startu, Execution, worker, artefakty, Evaluation | B2, B3, B4, AR3 |
| C1 | Jeden szablon generowania i oceny oraz formularz startu | Execution, worker, Evaluation | C0 |
| C2 | Experiments z jakością, czasem i tokenami | API porównań, `features/experiments/` | B3, C1 |
| C3 | Bramka regresji aplikacji w CI i przykład SDK/CLI | SDK, przykład integracji, kontrakt decyzji; bez wymaganego komentowania PR | C0; C1 dla testów generacji |
| C4 | Feedback → review → wersjonowany przypadek testowy | Observability, review, dataset i oczekiwania | B4, trwały zapis przypadków z B |

Ścieżki panelu w tabeli są względem `apps/panel/src/`. Każda paczka obejmuje testy zachowania i dokumentację swojego kontraktu. B1 może rozpocząć się podczas dopracowywania A; pełne Experiments nie powinno blokować dostarczenia A2. B2e–B2i domykają B2 i wyprzedzają B3; ich uzasadnienie i dowody w kodzie są w sekcji 17. B2i jest jedyną z nich, którą widać na ekranie — pozostałe cztery są kontraktem, magazynem i odbiorem.

Nie przypisuję dat na podstawie samej liczby funkcji: nie znam dostępnej obsady ani wyników wdrożenia wykonawcy. Po A1/A2 należy oszacować resztę na podstawie rzeczywistego czasu, wielkości danych i decyzji o wielozespołowości. Pierwszy zamknięty zakres wydania to A1–A4, a nie cały katalog P1.

## 6. Odbiór techniczny i miara wartości

Przed implementacją utrwalić próbki danych i zmierzyć, ile kroków zajmuje znalezienie różnicy między dwoma runami. Po etapie A użytkownik ma wykonać to w jednej analizie, bez ręcznego przepisywania parametrów; po C cały proces oceny ma działać z panelu, z możliwością dojścia do dowodu.

| Obszar | Wymagane sprawdzenie przy implementacji |
| --- | --- |
| Panel | Klawiatura, Back/refresh/URL, stany puste i błędy, brak serii, różne jednostki, oba motywy; `rtk npm run typecheck`, `rtk npm test`, `rtk npm run build` w `apps/panel` (architektura sprawdzana przez skrypty) |
| API i SDK | Stare rekordy, brakujące pola, niewłaściwy baseline, różny split/scorer, ograniczenia zapytań; `rtk just openapi` i testy zmienionych modułów oraz SDK |
| Trwałość | Restart, utrata szczegółów projekcji, duży raport, brak artefaktu, odebranie dostępu/usunięcie danych; te same zachowania na wspieranych magazynach |
| Panel trwałych dowodów | Każdy stan dowodu z własnym zdaniem i bez stanu pustego, `forbidden` z powodu roli jako wymaganie roli a nie awaria, widoczny termin retencji, pochodzenie wiersza w katalogu, brak konfiguracji jako 501 z nazwą zmiennej |
| Magazyn obiektów | Atomowy `create` i protokół claim/sprzątania na rzeczywistym magazynie w CI, nie tylko w pamięci i na pliku; konkurencyjna publikacja w obu kolejnościach |
| Koszt i skala | Odczyt podsumowania, strona przypadków, strona katalogu i jeden przebieg sprzątania zmierzone na wartościach startowych; próg, powyżej którego indeks jest wymagany |
| Wdrożenie | Zmienne rejestru w chart i opisie instalacji, receptura uruchomienia trwałej ścieżki, znaczenie odtworzenia prefiksu `evaluations/` z kopii |
| Execution | Retry po utracie odpowiedzi, anulowanie, timeout, częściowy wynik, współbieżne warianty; rzeczywisty workflow, nie tylko mock formularza |
| Feedback i regresje | Rewizje i źródła ocen, niedostępna treść, idempotencja kolejki, zero generacji w `score_existing`, krytyczny przypadek mimo lepszej średniej, błąd scorera blokujący CI |
| Przed scaleniem kodu | `rtk just check`; dodatkowe testy usług i lokalnego klastra tylko gdy zmieniona ścieżka ich wymaga; trwałe reguły przenieść do `CLAUDE.md` i ADR zamiast zostawiać je w checkpointach |

Decyzje wymagające ustalenia najpóźniej w B1: izolacja zespołów, pierwszy rzeczywisty dataset i scorer, oczekiwana skala oraz czas przechowywania dowodów. Skala i retencja zostały w B1 wybrane jako wartości startowe, nie zmierzone (sekcja 9); B2f zamienia je na pomiar i próg. Dla pierwszego wydania przyjmujemy wspólną instancję, maksymalnie pięć porównywanych treningów i lokalny zapis widoków; to wystarcza do rozpoczęcia A bez blokowania się rozbudowaną administracją.

Pierwotny przegląd był dokumentacyjny. Późniejsza implementacja i odbiór A są opisane w sekcji 8; nie potwierdzają realizacji scenariuszy B/C/D ani gotowości wdrożenia Kubernetes.

## 7. Granice architektury przy realizacji

Obecny modularny monolit wystarcza do A. Panel ma egzekwowane vertical slices; backend wymaga dwóch konkretnych wydzieleń dla zakresu B/C. [Pełna analiza](FTI_ARCHITECTURE_REVIEW.md) zawiera dowody w kodzie, mapę kontekstów i kryteria odbioru.

1. **AR1, razem z A/B1:** zapisać dozwolone zależności i właścicieli nowych danych, dodać kontrolę granic Rust do CI. F/T/I pozostaje przepływem produktu, a nie podziałem na trzy konteksty.
2. **AR2, B1–B4:** Evaluation posiada suite, rubryki, trwałe wyniki, assessments i porównywalność. Projektor ma przebudowywalny widok. Modele/prompty zachowują własne decyzje promocji; Conversations zachowuje zgodę, retencję i usuwanie. Core otrzymuje tylko neutralne kontrakty. Do tego samego właściciela należy zatwierdzenie źródeł dowodów: dziś jest katalogiem w konfiguracji procesu, co ogranicza instancję do jednej pary wariant/kontekst (sekcja 17.1).
3. **AR3, przed C0:** wyjąć wspólny przypadek użycia kompilacji/startu z modułu HTTP. API i scheduler korzystają z jednej usługi aplikacyjnej, z zachowaniem idempotencji, autoryzacji i polityki payloadów. Nowy scorer jest zadaniem istniejącego workera, nie nowym silnikiem wykonania. **Dostarczone** (sekcja 21): `aiwatcher_execution::start`, a [scheduler](../crates/aiwatcher-server/src/execution/scheduler.rs) pyta odmowę `says_the_same_next_time` zamiast czytać status HTTP z `ApiError`.
4. **AR4, wraz z B4/C:** nowe klienty ocen w modułach SDK; lekki import główny pozostaje lekki. UI, hooki i testy przy feature; wspólne komponenty dopiero przy rzeczywistym ponownym użyciu.

Rozdzielenie kompilatora curation od silnika i dalsze dzielenie dużych ekranów wykonywać przy zmianach, które tego potrzebują. Nie są warunkiem rozpoczęcia A1–A4. Szczegółowe propozycje nowych modułów nie oznaczają konieczności osobnego procesu lub wdrożenia.

Pozostała część FTI — B2e–B2h, B3, B4, AR3 i C0 — powinna dostać kartę na [tablicy specyfikacji](specs/BOARD.md), a trwałe rozstrzygnięcia trafiać do ADR i `CLAUDE.md`. Niezmienniki rejestru dowodów wypracowane w sekcjach 10–16 — intencja przed pierwszym artefaktem, claim jako jedyna atomowa bramka, brak fallbacku po tombstone, shard przed receiptem, wymagane szyfrowanie dowodów ze źródła Conversations, `with_content_access` przyznawane po sprawdzeniu Admin — są dziś tylko w tym dokumencie.

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

## 11. Kontynuacja — B2: zbieranie osieroconych artefaktów

Przegląd startowy: czysty checkout na `d2aed6d` (`feat: extend evaluation`), zawierający wcześniejsze B1/B2. Nie przywracano historycznych wersji plików ani danych demonstracyjnych. Zakres tej paczki to protokół sprzątania Evaluation; adaptery źródeł natywnych pozostają otwarte.

Przed pierwszym artefaktem publikacja zapisuje niezmienną intencję: ID i termin zbiórki, domyślnie po godzinie, skracany znaną retencją źródła/wyniku. Retry nie odnawia terminu. Po jego przekroczeniu kolektor konkuruje z publikacją o **ten sam** atomowy klucz claim. Wygrana publikacji chroni metadane i wszystkie wskazane shardy; można usuwać tylko niewskazane artefakty przegranych wersji. Wygrana kolektora trwale blokuje commit tego ID, pozwala usunąć treści i zwraca HTTP 410 na odczycie/retry. Spóźniony zapis może dokończyć pojedyncze operacje storage, ale nie zatwierdzi wyniku; kolejne przejście usuwa jego bajty.

Wspólny worker retencji uruchamia zbiórkę co 60 s przez `Registry::sweep`; osobna fasada `collect_orphans` zwraca liczbę usuniętych obiektów. Abandoned claim nie udaje zatwierdzonego wyniku i nie zawiera manifestu ani odpowiedzi. Stare receipt i wersje wyników zachowują format. Stare listy/suite/baseline wykluczają także zebrane ID; szczegóły nie wracają do telemetrycznego fallbacku.

**Weryfikacja:** 13 testów rejestru PASS, w tym deterministyczne przeploty na memory/file/real RustFS: crash po shardzie, wygrana każdej strony tuż przy commicie, późny zapis po zbiórce, współdzielone shardy konfliktu, utracona odpowiedź kolektora, ponowienie po odtworzeniu registry, zachowanie starego receipt oraz brak zgadywania referencji przy brakujących metadanych. Odbiór S3 używa tylko własnego kontenera `aiwatcher-fti-gc-rustfs` na :19010. Dodano test HTTP 410 oraz wykluczania z legacy odczytów. **PASS pełnego `rtk proxy just check`**: fmt, clippy wszystkich features/targetów, testy workspace (w tym nowy HTTP), granice architektury, aktualność OpenAPI i manifestu, panel, oba SDK, agentic, walidacja deploy oraz pozostałe linters. Log `/tmp/fti-b2-gc-just-check.log`. Osobno PASS kontroli komentarzy nowego nieśledzonego pliku i `git diff --check`. Własny kontener RustFS zatrzymano; instancji :8080/:18080 i ich danych nie zmieniano. Bez commita ani wdrożenia.

**Ograniczenia:** wcześniejsze niezatwierdzone prefiksy bez intencji są zachowane; sam hash nazwy i wiek obiektu nie potwierdzają tożsamości ani zakończenia starego writera. Ponowienie oryginalnej publikacji zapisuje intencję i obejmuje ID nowym protokołem. Uzgodnienie pozostałych takich prefiksów przy wdrożeniu oraz pliki stagingowe adaptera `.tmp` wymagają osobnej obsługi. Brak/uszkodzenie metadanych zatwierdzonego wyniku zatrzymuje usuwanie jego niewskazanych artefaktów; retencja nadal może usunąć całą treść. Kolektor nie zastępuje zgód, praw i retencji natywnych źródeł. Następny zakres implementacji: ich adaptery przez publiczne fasady właścicieli i testy rzeczywistych polityk. B2/AR2 nie są oznaczone jako zamknięte.

Przegląd pod kolejny adapter: Curation udostępnia `Registry::rows` z jawną wersją, Annotations `export_manifest`/`coco`, Conversations `export`/`export_rows` (odczyt odrzuca wycofany korpus), Prompts `version`, Training `model` z jawną wersją. Sam odczyt deklarowanego digestu nie dowodzi integralności: weryfikacja tożsamości artefaktu powinna wejść do fasady jego właściciela. Dla Conversations trzeba ponadto utrzymać szyfrowanie i wymaganą rolę odczytu, których obecny syntetyczny adapter oraz zwykły storage Evaluation nie zapewniają. Te ścieżki nadal odmawiają publikacji; nie dodano częściowego obejścia przez prywatne klucze ani publiczne URI.


## 12. Kontynuacja — B2: przypięte wersje Curation

Stan wejściowy: HEAD `d2aed6d`, zachowano cały niezacommitowany wycinek GC z sekcji 11. Nie zmieniano danych istniejących instancji ani modelu demonstracyjnego. Ta paczka dostarcza adapter Curation dla krótkich odpowiedzi; nie zamyka całego B2/AR2.

- Datasets udostępnia `Registry::verified_version(name, version)`. Prywatny moduł `version` odczytuje dokładną wersję obecną w katalogu i przelicza tożsamość treści wspólną z publikacją (z zachowaniem historycznej semantyki Flow i pozostałych silników). Sprawdza nazwę, digest, liczbę wierszy i budżet. Nie rozwiązuje etykiet ani `latest`; metadane katalogowe nadal nie zmieniają tożsamości treści.
- Server składa ten sam registry Curation z `LocalSource::with_curation`. Zatwierdzony przez operatora bundle nadal przypina kod/scorer/schematy i przypadki. Każdy odczyt/publikacja dodatkowo potwierdza natywną wersję i równość kolejności, ID, pytań i oczekiwań. Sama deklaracja digestu lub zgodność odpowiedzi nie wystarcza.
- Zmiana bieżącej wersji nie podmienia przypiętych danych. Usunięcie natywnego artefaktu/katalogu powoduje `deleted_source` i trwałe usunięcie kopii; cofnięcie zgody operatora ukrywa treść bez przedłużania retencji; uszkodzenie daje `corrupt_artifact` bez udawania usunięcia. API zachowuje swój kontrakt: trwałe odczyty zwracają 200 ze stanem i bez treści, legacy detail 403/410; odmowa roli przy publikacji to 403, brak uwierzytelnienia 401.
- Curation ma obecnie wspólny odczyt instancji, bez per-dataset ACL i własnej daty retencji; obowiązuje zatwierdzenie operatora i 30-dniowy limit Evaluation. Własne limity Curation (1000 wierszy, 4 MiB treści) pozostają wiążące. Adapter obsługuje wyłącznie jawny format krótkich odpowiedzi; instrukcja użycia jest w README Evaluation. Nie dodano nowej zależności domenowej ani publicznego kontraktu HTTP.

**Odbiór PASS:**

- Dwa testy fasady obejmują wszystkie trzy silniki, zachowanie starszej przypiętej wersji po zmianie head, odmowę aliasów, mutacje pól tożsamości i usunięcie wersji/katalogu.
- Cztery testy adaptera obejmują rekonstrukcję registry, paginację, dokładne retry, cofnięcie/przywrócenie zgody, usunięcie natywnego źródła przez sweep, brak odtworzenia po tombstone, uszkodzenie/naprawę bajtów, retencję i odmowę zmienionych pytań mimo identycznych odpowiedzi. Test rzeczywistego wiring i HTTP na losowym porcie localhost publikuje Curation przez API i potwierdza role oraz stany bez ujawniania treści. Własny listener kończy się z testem.
- Pełne `rtk proxy just check` PASS, log `/tmp/fti-curation-just-check.log`: fmt, clippy workspace/all-targets/all-features, testy Rust, bramki architektury i negatywne testy zależności, aktualność OpenAPI/manifestu, build/typecheck i 203 testy panelu, SDK Python (440), TypeScript, agentic (361), K8s/Helm/Tilt, comments/typos/taplo/cargo-deny. Zestaw server/evaluation: 16 PASS, 1 pominięty test wymagający osobnego RustFS; w tej kontynuacji nie uruchamiano RustFS ani nie powtarzano wcześniejszego odbioru S3 dla niezmienionego storage.
- Dodatkowo sprawdzono komentarze nowych nieśledzonych plików (standardowy linter czyta indeks Git) i `git diff --check`. Zmieniony kod nie wymaga regeneracji kontraktów HTTP.


**Następny zakres:** adaptery model/prompt, Annotations i Conversations przez właścicieli oraz judge. Conversations wymaga dodatkowo szyfrowania kopii dowodów, odpowiedniej roli i powiązania z retencją/usunięciem; aktualna ścieżka nadal je odrzuca. B3, powiadomienie po commicie oraz migracja starych orphanów bez intencji pozostają bez zmian. Bez commita i wdrożenia.


## 13. Kontynuacja — B2: przypięte wersje promptów

Stan wejściowy: HEAD `d2aed6d`, zachowano niezacommitowane GC i Curation z sekcji 11–12. Bez zmian danych istniejących instancji i bez commita. Ta paczka dodaje natywny adapter promptów; model, Annotations, Conversations i judge pozostają dalszym zakresem B2/AR2.

- Prompts udostępnia `Registry::verified_version` przez prywatny moduł `version`: sprawdza nazwę, dokładny digest tekstu, zapisane ID i wyliczane zmienne. Osobny błąd integralności nie jest mylony z brakiem źródła; API mapuje go na istniejącą kategorię `registry_corrupt`. Weryfikowany odczyt ma limit tekstu z konfiguracji (domyślnie 256 KiB), uwzględnia escaping JSON i 256 KiB metadanych. Stare `version`, `resolve` i zasady promocji pozostają bez zmian.
- Head promptu jest indeksem pochodnym, z limitem liczby wpisów. Przesunięcie `production`, wypadnięcie z indeksu lub jego utrata nie usuwa wersji. Brak obiektu wersji oznacza `deleted_source` i wycofanie kopii dowodów; uszkodzenie to `corrupt_artifact`, bez fałszywego tombstone. Prompt nie ma własnej retencji — nadal działa limit Evaluation. Metadane promptu, zwłaszcza pole `model`, nie są dowodem wersji modelu.
- Server przekazuje ten sam registry do API i `LocalSource::with_prompts`. Adapter akceptuje `variant.prompt` dla external i Curation, również bez workflow. Operator musi zatwierdzić dokładną referencję w bundle; pozostałe przypięcia kodu/scorera/schematów nadal są weryfikowane. Każda publikacja i odczyt wywołuje właściciela, bez odczytywania jego prywatnych kluczy przez adapter. Tekst promptu nie jest kopiowany do shardów Evaluation.
- Testowy bundle wydzielono do wspólnego fixture dla adapterów Curation/promptów; dotychczasowe scenariusze Curation pozostają zachowane. Nie dodano zależności między domenami ani nowych kontraktów HTTP.

**Odbiór PASS:**

- Trzy nowe testy Prompts: przesunięcie etykiety/wyparcie wersji z indeksu/utrata head, manipulacje nazwą/ID/tekstem/zmiennymi, metadane poza tożsamością, niepoprawny JSON oraz granice tekstu i serializacji. Cały crate: 47 PASS.
- Cztery nowe scenariusze integracyjne: ponowne utworzenie registry i idempotentny odczyt starego przypięcia, usunięcie wersji przez sweep i brak odtworzenia po tombstone, uszkodzenie/naprawa, cofnięcie zgody, retencja, brak właściciela lub przypiętej wersji, kompozycja z Curation i external. Rzeczywisty wiring/router HTTP na losowym porcie localhost potwierdza role, brak anonimowego odczytu i legacy 410 po usunięciu promptu. Zestaw server/evaluation: 20 PASS, 1 pominięty test wymagający osobnego RustFS. Listener testowy zakończono; nie uruchamiano ani nie modyfikowano istniejących instancji.
- Pełne `rtk proxy just check` PASS: fmt, clippy workspace/all-targets/all-features, testy Rust, bramki architektury i ich testy negatywne, aktualność OpenAPI/manifestu, build/typecheck/testy panelu, SDK Python/TypeScript, agentic, K8s/Helm/Tilt, comments/typos/taplo/cargo-deny. Log `/tmp/fti-prompt-just-check.log`. Dodatkowo sprawdzono komentarze nowych nieśledzonych plików i `git diff --check`. Nie regenerowano niezmienionych kontraktów i nie powtarzano wcześniejszego odbioru S3 dla niezmienionego storage.


**Następny zakres:** model przez fasadę Training i rzeczywistą weryfikację przypiętych artefaktów; dalej Annotations, Conversations i judge. Same deklarowane hashe modelu nie dowodzą integralności bajtów. Conversations nadal wymaga szyfrowania kopii, odpowiedniej roli i wiążącego usunięcia/retencji. B2/AR2 pozostają otwarte; B3 oraz migracja starszych orphanów bez intencji bez zmian.


## 14. Kontynuacja — B2: przypięte modele Training

Stan wejściowy: HEAD `d2aed6d`, zachowano niezacommitowane GC, Curation i prompty. Ta paczka dodaje adapter modeli Training; nie zmienia istniejących danych demonstracyjnych ani etykiet. Bez commita i wdrożenia.

- Training udostępnia `Registry::verified_version` przez prywatny moduł `registry/version`. Wspólny z rejestracją algorytm zachowuje historyczne ID. Fasada odrzuca aliasy i niepoprawne nazwy, sprawdza nazwę/ID/treść tożsamości, waliduje pakiet i ogranicza rekord do 1 MiB. Obiekt wersji jest źródłem prawdy; utrata head lub bieżącego runu nie usuwa przypięcia.
- Server przekazuje ten sam Training do API i `LocalSource::with_training`. Operator zatwierdza referencję w manifeście, cały pakiet w `model-package.json` oraz pliki `model-artifacts/<artifact.name>`. Adapter porównuje pełny pakiet i sprawdza bajty wszystkich artefaktów, w tym plików pomocniczych, z digestem i opcjonalną długością. Pakiet ma limit 1 MiB, pliki wspólny budżet 100 MiB. URI nigdy nie wybiera źródła odczytu; adapter nie uruchamia loadera ani modelu.
- Wersje bez pakietu nadal są czytelne w dotychczasowym API, ale nie wystarczają do publikacji nowych dowodów. Brak held-out score nie blokuje samej ewaluacji i nie nadaje prawa do promocji. Role Viewer/Editor, retencja Evaluation i zatwierdzenie operatora pozostają wiążące; Training nie ma niezależnego terminu retencji lub per-model ACL.
- Przesunięcie etykiety nie podmienia modelu. Brak natywnej wersji lub lokalnego artefaktu wycofuje dowody; podmiana bajtów/tożsamości daje `corrupt_artifact`, a niezatwierdzona zmiana opisu pakietu `forbidden`. Naprawa uszkodzenia może odblokować odczyt, odtworzenie usuniętego źródła nie cofa tombstone. Wagi nie są kopiowane do shardów Evaluation.

**Istotna granica kontraktu:** historyczny ID modelu zawiera run/checkpoint/dataset/metriki i uporządkowane digests artefaktów, ale nie runtime/entry point/kształty/adresy i pozostałe metadane pakietu. Zapis pełnego pakietu przez operatora jest bieżącym zatwierdzeniem, nie dodatkowym niezmiennym fingerprintem w wersji wyniku. Nie zmieniono identyfikatorów już zarejestrowanych modeli. Dowód historycznego środowiska wymaga jawnego rozszerzenia kontraktu; weryfikacja dostępnych bajtów nie dowodzi faktycznego wykonania modelu przez producenta.

**Odbiór PASS:**

- Test fasady Training potwierdza zgodność historycznego ID z utrwaloną wartością, odczyt bez head/runu, mutacje pól tożsamości, metadane poza digestem, brak/niepoprawny JSON, limit rekordu i odmowę aliasów.
- Pięć scenariuszy adaptera obejmuje zmianę etykiety, rekonstrukcję registry, dokładne retry i strony, usunięcie wersji przez sweep, trwały tombstone, podmianę obu plików pakietu, zmianę runtime/digestu, cofnięcie zgody i retencję. Dodatkowo sprawdzono błędny/brakujący pakiet, brak właściciela, długości/budżet, odmowę symlinka poza katalog oraz kompozycję z external i Curation. Rzeczywisty wiring/HTTP na losowym porcie localhost potwierdza Editor/Viewer/anonymous, odczyt trzech przypadków i legacy 410 po usunięciu pliku pomocniczego. Listener zakończono w teście.
- Pełne `rtk proxy just check` PASS, log `/tmp/fti-model-just-check.log`: Rust fmt/clippy wszystkich features/targetów/testy workspace, bramki architektury z testami negatywnymi, aktualność OpenAPI/manifestu, panel build/typecheck/testy, oba SDK, agentic, K8s/Helm/Tilt, comments/typos/taplo/cargo-deny. Zestaw server/evaluation: 25 PASS, 1 pominięty test wymagający osobnego RustFS. Nie powtarzano wcześniejszego odbioru S3 dla niezmienionego storage.
- Osobno PASS komentarzy nowych nieśledzonych plików i `git diff --check`. Publiczny kształt HTTP nie zmienił się. Nie uruchamiano seedów ani nie modyfikowano istniejących instancji.

**Następny zakres:** Annotations przez fasadę właściciela, z integralnością eksportu i polityką dostępu/licencji; potem Conversations z szyfrowaniem kopii, rolą i wiążącym usunięciem/retencją oraz judge. B2/AR2 nadal otwarte, B3 i migracja starych orphanów bez intencji pozostają dalszym zakresem.


## 15. Kontynuacja — B2: eksporty Annotations

Stan wejściowy: czysty checkout, HEAD `836545d` zawiera wcześniejsze prace B2. Nie zmieniano danych istniejących instancji. Ta paczka dostarcza adapter Annotations; Conversations i judge pozostają dalszym zakresem.

- Fasada `Annotations::Registry::verified_coco(project, export, split)` sprawdza historyczny digest eksportu oraz przypięte rewizje, schema i bajty obrazów w storage właściciela. Algorytmy tożsamości eksportu/rewizji są współdzielone z zapisem, z zachowaniem istniejącego formatu. Nie czyta prywatnych kluczy przez Server, nie odczytuje zewnętrznych URL, nie pomija brakujących rewizji. Zwykłe API export/COCO zachowuje zachowanie.
- Wybrany split musi mieć aktualne prawa zgodne z zapisaną polityką `commercial` lub `research` oraz zaakceptowany stan review. `any` jest odrzucane. Są to kontrole zapisanych praw, nie nowe ustalanie treści licencji. Nowa zaakceptowana rewizja nie podmienia starego przypięcia, ale cofnięcie review/praw ukrywa dowody. Operator dodatkowo zatwierdza dokładny wariant/kontekst.
- Server składa ten sam Annotations z API i adapterem. Przypadek to pełny obraz COCO jako `input`, jego `file_name` jako `case_id`, oraz pełne `categories` i właściwe `annotations` jako `expected`. Wymagane są wszystkie obrazy wybranego `train`/`validation`/`test`, w kolejności eksportu. Wariant nadal może używać modelu/promptu/workflow. Pliki fixture z osobnymi schematami i deterministycznym scorerem są w `contracts/fixtures/evaluation-annotations-v1`; scorer sprawdza dokładną równość, nie mAP. Dotychczasowy seed nie jest adapterem Annotations.
- Limity weryfikowanej fasady: 1000 próbek eksportu, 100 MiB wszystkich odczytanych źródeł, JSON eksportu/projektu/head 4 MiB, obraz 16 MiB, tożsamość rewizji 4 MiB i 256 KiB metadanych. Pochodne counts nie są częścią historycznego digestu i nie wybierają przypadków. Obrazy pozostają w Annotations; wektorowe oczekiwania są przechowywane w dowodach Evaluation.

**Ograniczenia:** tylko natywne obrazy `aiwatcher-blob:`. Annotations nie przechowuje osobno dawnych schematów; poprawna zmiana schematu projektu ukrywa stare dowody (`forbidden`), aż zgodny schemat znów będzie dostępny. Brak eksportu/projektu/wybranej głowy obrazu/rewizji/bajtów oznacza `deleted_source` i trwałe wycofanie dowodów; uszkodzenie oznacza `corrupt_artifact`. Brak niezależnej retencji i per-project ACL u właściciela: działają wspólne role Viewer/Editor, zgoda operatora i retencja Evaluation. Źródła rozmów nadal wymagają szyfrowania i innego zakresu uprawnień.

**Odbiór:** sześć nowych scenariuszy integracyjnych potwierdza przypięcie po nowej rewizji/utracie indeksu/rekonstrukcji registry, strony i retry, wycofanie przez sweep, uszkodzenie eksportu/schematu/rewizji/bajtów, aktualne prawa i review, kompozycję splitów, dokładne wejścia/oczekiwania, ograniczenia właściciela oraz HTTP z rolami i legacy 410. Zestaw server/evaluation: 31 PASS, 1 pominięty test wymagający osobnego RustFS. Nie powtarzano odbioru S3 dla niezmienionego storage. Osobny scorer fixture przeszedł próby zgodności, braku geometrii i braku predykcji; komentarze nowych nieśledzonych modułów również PASS.

Pierwsze pełne check przeszło testy i pozostałe kontrole, wskazując tylko nieaktualny opis pola w OpenAPI. Poprawienie nieścisłego komentarza o digestach wymagało regeneracji `contracts/openapi.json` oraz klienta panelu (`just openapi`); kształt danych HTTP nie zmienił się. **Pełne `rtk proxy just check` po regeneracji PASS**, log `/tmp/fti-annotations-just-check.log`: fmt, clippy wszystkich features/targetów, testy Rust, bramki architektury, aktualność OpenAPI i manifestu, panel build/typecheck/testy, oba SDK, agentic, K8s/Helm/Tilt oraz linters. Dodatkowo PASS `git diff --check`. Testowy listener zakończono; nie uruchamiano seedów ani nie modyfikowano istniejących instancji.

**Następny zakres:** Conversations przez właściciela, z szyfrowaniem przechowywanych oczekiwań/odpowiedzi, właściwą rolą i wiążącym usunięciem/retencją. Potem judge. B2/AR2 nadal nie są ukończone w całości. B3, powiadomienie po commicie i migracja starych orphanów bez intencji pozostają osobnym zakresem. Bez commita i wdrożenia.


## 16. B2 — Conversations: szyfrowane dowody i polityki źródła (2026-09-12)

Na wejściu zachowano niezacommitowane Annotations i niezależne zmiany SDK.
W trakcie pracy HEAD przeszedł z `836545d` do `35a9981`; wcześniejsze zmiany są
w tym commicie. Ta kontynuacja nie wykonywała commita ani zmian działających
instancji i demonstracyjnych danych.

Dostarczono:

- `Conversations::Registry::verified_evaluation_rows`: pełny eksport
  `prompt_response`, wymagany scope `evaluate` i human review. Właściciel
  weryfikuje request/version, kolejność i SHA shardów, liczbę wierszy, obie tury
  każdej pary, ich bajty oraz aktualną zgodę, review i najkrótszą retencję.
  Nie interpretuje deklaracji train jako evaluate. Uszkodzenie nie staje się
  pustym oczekiwaniem, a usunięcie źródła wycofuje dowody.
- Adapter Server przez publiczną fasadę; bundle operatora zawiera tylko
  `case_id`, `input_digest`, `expected_digest`, bez tekstów rozmowy. Mapowanie:
  assistant turn ID → `{question: prompt}` / `{answer: response}`. Wymagana
  dokładna kolejność i pełny korpus; split `test` jest deklaracją operatora.
- Port `Evaluation::EvidenceCipher`, implementacja w Server na istniejącym
  Keyring Conversations (AES-GCM/HKDF, uwierzytelniona ścieżka obiektu).
  Metadane, actual i expected są szyfrowane przed storage; skróty i niezmienny
  claim liczone nadal z treści, więc losowa koperta nie zmienia wersji/retry.
  Brak szyfrowania lub podmiana na plaintext blokuje dowód. Stare plaintext
  wyniki pozostałych rodzajów źródeł zachowują format i odczyt.
- Jawne `with_content_access` na kopii registry dla jednego wywołania:
  API przyznaje je dopiero po sprawdzeniu Admin, worker retencji ma zaufany
  dostęp. Sama nazwa subject, również `admin`/`retention-worker`, nie przyznaje
  prawa. Viewer/Editor widzą `forbidden` bez treści/metadanych/metryk, stary
  detail daje 403; po wycofaniu brak fallbacku (410 w starym API).
- Retencja jest minimum pierwszego receipt, aktualnego limitu źródła i polityki
  instancji; retry jej nie odnawia. Tombstone poprzedza usunięcie zaszyfrowanych
  kopii. Utrata klucza daje `forbidden`, naruszenie integralności
  `corrupt_artifact`, bez fałszywego usunięcia źródła.

Limity/zakres: 1 000 wierszy i 100 MiB sumy odczytów źródła (koperty i plaintext),
4 MiB manifest, 2 MiB głów tur/kopert treści; port ObjectStore nadal oddaje pełny
obiekt przed sprawdzeniem limitu. Evaluation zachowuje limit 100 MiB logicznego
raportu; szyfrowane koperty mają dodatkowy narzut base64. Brak dowodu niezależności
zbioru testowego, wykonania modelu czy jakości promotowalnej. Chat/SFT/DPO,
wybór podzbioru i judge są odrzucane. Zmiana/deletion źródła jest wykrywana na
odczycie lub sweep co 60 s. Przy braku klucza nie można odczytać metadanych dla
powiązania ze źródłem; receipt nadal pozwala usunąć kopie po pierwotnej retencji.
Odzyskanie klucza przed tombstone przywraca możliwość weryfikacji, nie odnawia czasu.

Odbiór PASS:

| Scenariusz | Wynik |
| --- | --- |
| Zaszyfrowane obiekty bez sentinel tekstu, filesystem, rekonstrukcja właściciela/registry, retry i strony | PASS |
| Jawna capability, odmowa mimo subject `admin`, review i nieodnawiana retencja | PASS |
| Utrata klucza, naruszenie koperty, podmiana ścieżki, uszkodzenie/usunięcie sharda źródła | PASS |
| Brak cipher, inny split/cohort/input digest, alias wersji; odmowa przed zapisem | PASS |
| Rzeczywiste HTTP: Admin publish/read, Viewer/Editor odmowa, discovery/detail/pages/legacy, erasure | PASS |
| Tożsamość manifestu/sharda, aktualna zgoda, brak indeksu i skrócony TTL właściciela | PASS |
| Downgrade metadanych/shardów na plaintext, 401 przypadków, współbieżne retry/konflikt i GC zwycięzcy | PASS |

`cargo check -p aiwatcher-server` oraz testy kierunkowe Conversations/Evaluation/
Server PASS (`/tmp/fti-conversations-check.log`, `/tmp/fti-conversations-tests.log`).
Siedem nowych scenariuszy w Server; cały server/evaluation: **38 PASS, 1 pominięty**
test opt-in RustFS. Testy zgodności wykryły i pomogły naprawić rozpoznawanie koperty:
serde może odczytać jednopolowy struct z jednoelementowej tablicy, dlatego marker
jest teraz sprawdzany wyłącznie jako nazwane pole obiektu JSON. Stare shardy
jednego przypadku zachowują odczyt i retry.

Pełne **`just check` PASS** (`/tmp/fti-conversations-just-check.log`): fmt/clippy,
testy workspace, granice i testy negatywne, aktualność kontraktów, panel oraz SDK,
manifesty K8s/Helm/Tilt, komentarze/typos/taplo/cargo deny. Zregenerowano OpenAPI
i klienta panelu — zmieniły się opisy wymagań Admin, bez zmiany kształtu HTTP.
Nowe nieśledzone moduły sprawdzono również ręcznie linterem komentarzy.
Nie uruchamiano nowego odbioru na żywym RustFS ani Kubernetes.
W trakcie pełnego check pojawiły się niezależne edycje Execution i pod launcher;
zachowano je, nie są częścią tego odbioru Conversations.

Następny zakres: judge przez właścicieli przypięć/konfiguracji, potem przegląd
pozostałych kryteriów B2/AR2 i B3. Nie powtarzać adapterów.


## 17. Przegląd planu — braki i propozycje (2026-09-12)

Przegląd dotyczy **planu**, nie dostarczonego kodu: odbiory z sekcji 8–16 nie są tu podważane. Dowody odczytano z checkoutu na HEAD `c77a997` wraz z niezacommitowanym wycinkiem Conversations. Każdy wniosek jest już wniesiony do sekcji 2–7 — ta sekcja zostaje jako uzasadnienie i wskazanie miejsca w kodzie, a kolumna niżej mówi, gdzie w planie trafił.

| # | Rzecz | Gdzie trafiła w planie | Skutek pominięcia |
| --- | --- | --- | --- |
| E1 | Jedna zatwierdzona para wariant/kontekst na instancję | Uściślenie 8, etap B p. 8, paczka B2e | B3 nie ma dwóch porównywalnych dowodów |
| E2 | Publikacja wymaga ręcznego zatwierdzenia przy każdym uruchomieniu | Etap B p. 8, etap C p. 3, paczka B2e | C0/C1/C3 nie opublikują dowodu bez człowieka |
| E3 | Brak trasy usunięcia pojedynczego dowodu | Etap B p. 10, paczka B2e | Jedynym narzędziem jest retencja |
| K1–K5 | Koszt odczytu i porządek katalogu | Etap B p. 9, paczka B2f, sekcja 6 | Ekran katalogu i sweep skalują się z całym korpusem |
| O1 | Brak zadania CI dla object store | Paczka B2g, sekcja 6 | Protokół claim/GC nie jest dowiedziony na S3 |
| O2 | Sweep nie raportuje wyniku | Paczka B2g | Tydzień nieudanych sweepów wygląda jak działający |
| W1 | Rejestr nie istnieje we wdrożeniu | Paczka B2g, sekcja 6 | Funkcji nie da się włączyć chartem |
| W2 | Backup/odtworzenie prefiksu `evaluations/` | Sekcja 6, ADR 0030 | Odtworzenie może wskrzesić porzucone ID |
| J1 | Kształt judge'a | Etap B p. 11, paczka B2h | Adapter skopiuje regułę, która go nie dotyczy |
| X1–X4 | Plan jako dokument | Uściślenie 7, sekcje 6 i 7 | Kolejna sesja powtarza pracę AW-5 lub gubi stan |

### 17.1 Dostęp do dowodów — blokady B3 i C

**E1. Instancja zatwierdza jedną parę wariant/kontekst naraz.** `LocalSource::resolve` w [adapterze Server](../crates/aiwatcher-server/src/evaluation.rs) czyta jeden `manifest.json` ze wskazanego `AIWATCHER_EVALUATION_SOURCE_DIR` i odrzuca jako `forbidden` wszystko, czego `variant_id`/`context_id` nie zgadza się z tym jednym zatwierdzeniem. Opublikowanie drugiego wariantu wymaga podmiany katalogu, a po podmianie **wcześniej opublikowane wyniki przestają być czytelne**. B3 brzmi „porównywanie trwałych wyników", czyli wymaga dwóch wyników o różnych `variant_id` czytelnych jednocześnie — dziś nie ma takiego stanu. README Evaluation opisuje to ograniczenie jako „one operator-approved bundle"; plan nie wyciąga z niego wniosku i planuje B3 tak, jakby dane wejściowe istniały.

*Propozycja:* zatwierdzenie operatora przestaje być katalogiem na dysku serwera i staje się własnym, wersjonowanym zasobem — wiele przypięć naraz, adresowanych treścią, z zapisem kto i kiedy zatwierdził. Właścicielem jest Evaluation; nadal nie czyta prywatnych kluczy innych rejestrów. To jest paczka **przed** B3, nie po niej.

**E2. Publikacja nadal wymaga człowieka przy każdym uruchomieniu.** `POST /api/v1/evaluation-results` wymaga roli Editor **i** wcześniejszego umieszczenia przez operatora bundle'a na dysku serwera, dokładnie odpowiadającego publikowanemu manifestowi. C0 (`score_existing`), C1 (szablon generowania i oceny) i C3 (bramka CI) mają produkować dowody automatycznie — z dzisiejszym kontraktem każde ich uruchomienie wymaga ręcznego kroku na hoście. To ta sama decyzja co E1 i należy ją podjąć raz.

**E3. Nie ma trasy usunięcia pojedynczego dowodu.** [Router evaluations](../crates/aiwatcher-api/src/evaluations.rs) ma `GET`/`POST` i żadnego `DELETE`. Dowód znika tylko przez usunięcie źródła albo upływ retencji, a dla źródeł `external` nie ma czego usuwać. Jeżeli opublikowany wynik kiedykolwiek będzie musiał zniknąć na żądanie, jedynym narzędziem jest 30-dniowy zegar. Warto rozstrzygnąć jawnie: albo trasa usunięcia z uprawnieniem, albo zapisane w ADR 0030 stwierdzenie, że usunięcie jest wyłącznie pochodną źródła i retencji.

### 17.2 Koszt odczytu na wartościach startowych

Plan mówi „odkrywanie skanuje klucze object store — przed zwiększeniem skali należy zmierzyć koszty". Kod robi istotnie więcej niż skanowanie kluczy:

**K1. Podsumowanie czyta cały wynik.** `Registry::get` → `read_metadata` w [registry](../crates/aiwatcher-evaluation/src/registry.rs) przechodzi pętlą po **wszystkich** shardach (`read_shard`) i odrzuca ich zawartość, żeby zwrócić nagłówek z licznikami i metrykami, które są już w metadanych. Przy wartościach startowych `GET /api/v1/evaluation-results/{id}` czyta i weryfikuje 10 000 przypadków, a dla źródła Conversations dodatkowo je odszyfrowuje.

**K2. Strony przypadków czytają wynik raz na stronę.** `cases` wywołuje `get` (czyli K1), a potem odczytuje jeszcze żądaną stronę. Przejście przez 10 000 przypadków po 200 to pięćdziesiąt jeden pełnych odczytów tego samego wyniku.

**K3. Lista wykonuje K1 dla każdego wiersza.** `list` pobiera pełne `list("evaluations/")`, sortuje w pamięci, a następnie dla każdego elementu strony woła `get` — czyli pełny odczyt wyniku **i** `authority.resolve`, które ponownie weryfikuje źródło (wiersze Curation, bajty artefaktów modelu w budżecie 100 MiB, shardy Conversations). Strona 200 wierszy to w granicach konfiguracji rząd wielkości 20 GiB odczytów z object store na jedno żądanie.

**K4. Sweep powtarza to dla całego korpusu co 60 s.** `sweep` listuje prefiks i woła `get` dla każdego zatwierdzonego claimu. Retencja i wycofanie potrzebują `expires_at` z receiptu oraz terminu źródła — nie pełnej weryfikacji treści. To dokładnie reguła „Never let a measurement cost a run" z `CLAUDE.md`, zastosowana do sprzątania.

**K5. Katalog nie ma porządku czasowego.** Klucz to `evaluations/{sha256(id)}/` ([store](../crates/aiwatcher-evaluation/src/store.rs)), a kursor listy jest posortowanym kluczem. Porządek jest więc porządkiem skrótu: ekran B3 nie pokaże „najnowsze pierwsze" bez przeskanowania wszystkiego. Indeks (własny obiekt-głowa z porządkiem czasu albo indeks po stronie projektora trzymający wyłącznie referencję) trzeba wybrać przed ekranem, nie po nim.

*Propozycja:* rozdzielić ścieżkę podsumowania od weryfikacji shardów — sharda weryfikować wtedy, gdy ktoś czyta jego stronę; `list` oprzeć na receipt/claim bez `authority.resolve` per wiersz, a stan źródła potwierdzać przy wejściu w szczegół. Do planu dopisać jedną paczkę pomiarową z wartościami startowymi (10 000 przypadków, 100 MiB, 30 dni) i progiem, powyżej którego indeks jest wymagany.

### 17.3 Odbiór i obserwowalność

**O1. Brak zadania CI dla object store.** Cała poprawność publikacji i GC opiera się na atomowym `create`; na S3 to `If-None-Match: *`, a memory używa zamka, plik twardego dowiązania — ani jedno, ani drugie nie dowodzi zachowania S3. Jedyny test na prawdziwym storage jest `#[ignore]` za `AIWATCHER_EVALUATION_TEST_S3_ENDPOINT` ([testy Server](../crates/aiwatcher-server/tests/evaluation.rs)), więc CI go nie uruchamia. [CI](../.github/workflows/ci.yml) ma dedykowane usługi dla Iggy i PostgreSQL dokładnie z tego powodu — komentarz przy `workflow-store` mówi wprost, że bez tego zadania właściwości „nie dowodzą niczego o magazynie, który trzyma produkcję". Zadanie z RustFS domyka tę samą lukę także dla podpisu SigV4 w Prompts.

**O2. Sweep nie raportuje niczego.** Worker w [evaluation.rs](../crates/aiwatcher-server/src/evaluation.rs) odrzuca licznik zwracany przez `sweep` i loguje `warn!` przy błędzie. Sweep psujący się od tygodnia wygląda identycznie jak działający — ta sama sytuacja, którą guardrail harmonogramu opisuje zdaniem „schedule refused every morning for a week looked exactly like one that had been working". Do planu: licznik wycofanych i zebranych obiektów jako metryka albo pole diagnostyki instancji (etap D już przewiduje diagnostykę — wystarczy tam dopisać ten rejestr).

### 17.4 Wdrożenie

**W1. Rejestr nie istnieje poza kodem i dokumentami.** Żadna ze zmiennych `AIWATCHER_EVALUATION_*` nie występuje w `deploy/helm`, `docs/INSTALL.md`, `justfile` ani w `detect-stack.py`; nie ma też receptury uruchamiającej lub zasilającej trwałą ścieżkę (jest tylko `just seed-evaluation`, czyli stare raporty zdarzeniowe). Skutki: funkcji nie da się włączyć chartem, odbiór z sekcji 10–16 jest odtwarzalny wyłącznie z prozy planu, a `evaluations/` jest siódmym prefiksem we wspólnym object store, o którym nie wie ani NetworkPolicy, ani opis instalacji.

**W2. Backup i odtworzenie `evaluations/` są nieokreślone.** To — obok Conversations i strumienia wykonania — magazyn, którego treści nie ma nigdzie indziej. Jednocześnie protokół opiera się na tym, że claim jest niezmienny: odtworzenie prefiksu z kopii może przywrócić claim, który kolektor już porzucił, albo usunąć tombstone. Jeden akapit w ADR 0030 o tym, co odtworzenie znaczy, jest tańszy niż odkrycie tego przy pierwszej awarii.

### 17.5 Judge — rozstrzygnąć kształt przed adapterem

Plan i kickoff trzymają judge'a jako „ostatni adapter". To nie jest ten sam rodzaj pracy. Pięć dotychczasowych adapterów dopuszcza źródło przez **ponowny odczyt bajtów u właściciela**; wynik judge'a nie jest bajtami, które ktokolwiek może ponownie potwierdzić — to wywołanie modelu, którego odtwarzalność zależy od przypiętej konfiguracji, dostawcy i zbioru kalibracyjnego. Przeniesienie reguły „zweryfikuj bajty" na judge'a albo go zablokuje, albo — co gorsze — zostanie rozluźnione dla wszystkich pozostałych źródeł.

*Propozycja do planu:* judge dostaje własną regułę dopuszczenia: konfiguracja przypięta treścią, zapisany zbiór kalibracyjny i rozbieżność z ocenami ludzi jako część dowodu, oraz jawne oznaczenie wyniku jako nieodtwarzalnego przez ponowny odczyt. To jest rozstrzygnięcie na poziomie ADR 0030, nie szczegół adaptera. Sekcja 4 etapu C już wymaga kalibracji przy uruchamianiu judge'a — brakuje tego samego po stronie trwałych dowodów.

### 17.6 Plan jako dokument

**X1. AW-5 nie jest w planie.** [AW-5](specs/AW-5-optimise-evaluate-and-promote-a-prompt-in-one-managed-run/_index.md) jest `done` na tablicy specyfikacji i dostarczył dokładnie: połączenie raportu z wykonaniem/krokiem, połączenie optymalizacji z raportami held-out oraz odmowę etykiety `production` dla odrzuconego kandydata. Punkt 2.4 i B5 nadal opisują to jako pracę do zdefiniowania. Plan powinien się do AW-5 odwołać i zawęzić B5 do tego, czego AW-5 nie robi — polityki decyzji dla **modeli** i wymogu kompletnego held-out.

**X2. Decyzje nie trafiły do `CLAUDE.md`.** Siedem kontynuacji wytworzyło niebanalne niezmienniki: intencja przed pierwszym artefaktem, claim jako jedyna atomowa bramka, brak fallbacku po tombstone, kolejność shard → receipt (kolejne miejsce `aiwatcher_jobs::ORDERING`), wymagane szyfrowanie dowodów ze źródła Conversations, `with_content_access` jako jawna zdolność przyznawana po sprawdzeniu Admin. W `CLAUDE.md` jest tylko wiersz w tabeli crate'ów. Konwencja tego repozytorium — widoczna choćby po AW-5, które „graduated decisions into ADR_0010, ADR_0011 i CLAUDE.md" — mówi, że takie reguły mieszkają w Guardrails. Bez tego kolejna zmiana w Evaluation nie ma czego naruszyć świadomie.

**X3. FTI jest poza `docs/specs/`.** Repozytorium ma proces spec-flow z tablicą i fazami; FTI — inicjatywa większa niż AW-3, AW-4 i AW-5 razem — prowadzi go plikami `FTI_*.md` i doklejanymi akapitami. Nie trzeba przepisywać dotychczasowych sekcji wstecz; wystarczy karta na tablicy dla pozostałej części (B2-domknięcie, B3, B4, AR3, C0) i trzymanie następnych rozstrzygnięć w ADR-ach, a nie w checkpointach.

**X4. AR3 jest warunkiem C0 i nie zaczęto go.** Sekcja 7 wymaga wydzielenia wspólnego przypadku użycia kompilacji/startu z modułu HTTP przed C0; [scheduler](../crates/aiwatcher-server/src/execution/scheduler.rs) nadal klasyfikuje trwałość błędu po statusie HTTP z `ApiError`. To powinno być w „następnym zakresie" kickoffu obok judge'a, a nie odkryte przy C0.

### 17.7 Kolejność

Paczki B2e–B2i i zależności B3 są w [tabeli paczek](#5-zależności-i-pierwsze-paczki-prac). Kolejność wewnątrz nich jest wymienna poza trzema ograniczeniami: B2e poprzedza B3, bo bez niego B3 nie ma dwóch czytelnych wyników; B2h następuje po rozstrzygnięciu reguły dopuszczenia w ADR 0030; a B2i następuje po B2e i B2f, bo rysuje stany i katalog, które one ustalają. AR3 biegnie równolegle i pozostaje warunkiem C0.

### 17.8 Zmiany wizualne

Cztery obserwacje z ekranu, na których opiera się „Zmiany wizualne etapu B” i paczka B2i.

- **V1. Trwała ścieżka nie ma ekranu.** `listResults`, `getResult` i `getCases` są w wygenerowanym kliencie i nie woła ich nic poza nim; jedyny ekran Evaluation czyta projekcję logu. Cała praca sekcji 10–16 jest dziś dostępna wyłącznie przez HTTP i SDK.
- **V2. Dwa różne `partial` w jednej odpowiedzi.** `EvidenceState::Partial` i `ResultStatus::Partial` przyjeżdżają razem w `DurableEvaluation` i znaczą co innego. To trafia na ekran jako dwie plakietki, zanim ktokolwiek zdecyduje inaczej.
- **V3. Kontrakt A3 jest renderowany jako proza.** W [page.tsx](../apps/panel/src/features/evaluation/screens/overview/page.tsx) zdanie o wstrzymanych deltach stoi pod akapitami o pokryciu i kompletności — serwer rozróżnia trzy stany, ekran ich nie eksponuje.
- **V4. Okno czasu nie ma zastosowania do katalogu dowodów.** Ekran Evaluation niesie `window` przez [search.ts](../apps/panel/src/features/evaluation/screens/overview/search.ts), bo fałduje log. Katalog trwałych wyników jest listowany w porządku skrótu ID; ta sama kontrolka nad nim nie zawężałaby niczego.

## 18. Domknięcie B2 — paczki B2e–B2i (2026-09-12)

Wykonane na HEAD `ab4d998` … `2a845aa`. Równolegle inna sesja pracowała na
`execution/pods/` i `sdk/`; jej zmiany zostały nietknięte, a commity poniżej są
po ścieżkach. `rtk just check` na koniec: 23/23 PASS — przy czym dwa z nich
wymagały naprawy błędów spoza tej zmiany: `Cargo.toml` niesformatowany od
`0ec11aa` (`049c38c`) i `taplo` wchodzący do cudzego worktree w
`.claude/worktrees/` (`59b595a`).

| Paczka | Stan | Commit |
| --- | --- | --- |
| B2e — zatwierdzenie jako zasób | zrobione | `004704f` |
| B2f — koszt odczytu | zrobione | `63d1480` |
| B2g — CI, obserwowalność, wdrożenie | zrobione | `9d06541` |
| B2h — reguła dopuszczenia judge'a | zrobione (ADR, bez adaptera) | `9d06541` |
| B2i — panel trwałych dowodów | zrobione | `96f618c` |

### 18.1 B2e — zatwierdzenie jest zasobem

`approval_id = sha256([1, "evaluation.approval", variant_id, context_id])`.
Rekord w `evaluations/approvals/{id}/record.json` (tylko `create`) trzyma kto,
kiedy i `bundle_digest` — to, co adapter sprawdził **ponad** digesty z manifestu,
czyli pakiet modelu, którego historyczne ID nie obejmuje. Wycofanie to osobny
znacznik `withdrawn.json`, ostateczny dla danego ID.

- `POST/GET/DELETE /api/v1/evaluation-approvals` — **admin**, bo token ingestu
  jest z definicji edytorem, a producent dopuszczający własne dowody nie jest
  zatwierdzeniem.
- Publikacja wymaga dopuszczonej pary, **po** adapterze: źródło, którego nie ma,
  mówi to wprost zamiast „nikt tego nie zatwierdził".
- Odczyt wymaga tylko braku wycofania. Brak rekordu to nie wycofanie — dowody
  sprzed tej zmiany pozostają czytelne.
- `AIWATCHER_EVALUATION_SOURCE_DIR` to teraz katalog **zatwierdzeń**, po jednym
  podkatalogu na parę; pojedynczy bundle wprost w korzeniu nadal działa.
- E3 rozstrzygnięte: `DELETE /api/v1/evaluation-results/{id}` (admin), ten sam
  trwały znacznik co retencja. Słownik stanów zostaje przy siedmiu.

**Odbiór** (`approvals.rs`, dwa testy; plus ręcznie na własnej instancji `:19080`
z RustFS na `:9011`): baseline i kandydat `complete` naraz, trzecie zatwierdzenie
nie rusza dwóch wcześniejszych (receipty bajt w bajt te same), wycofanie daje
`forbidden` przy niezmienionym `expires_at`, kolejna repetycja dopuszczonej pary
publikuje się bez kroku na hoście, `DELETE` jednego wyniku nie rusza drugiego.

**Czego to nie robi:** nowa para nadal wymaga wgrania bajtów bundle'a tam, gdzie
adapter je czyta. Upload bundle'a przez API to dopisanie za tym samym zasobem,
nie zmiana w nim. Zapisane w ADR 0030.

### 18.2 B2f — cztery liczby, przed i po

Mierzone w żądaniach do object store (`cost.rs`, `Counting`), na wartościach
startowych. Żądanie, nie milisekunda, bo to magazyn po drugiej stronie sieci.

| | przed | po |
| --- | --- | --- |
| podsumowanie wyniku o 10 000 przypadków | 105 gets, 1 661 263 B | **5 gets, 11 163 B** |
| pierwsza strona 200 przypadków | 108 gets, 1 704 843 B | **7 gets, 44 165 B** |
| strona katalogu, 50 wierszy | 400 gets, 1 list, 185 550 B | **152 gets, 1 list, 131 535 B** |
| jeden przebieg sprzątania, 50 wierszy | 550 gets, 53 lists, 319 050 B | **103 gets, 1 list, 17 806 B** |

Trzy reguły, żadna nie rozluźnia gwarancji: podsumowanie odpowiada z
content-addressed metadanych; shard weryfikuje się przy czytaniu jego strony, a
uszkodzony jest **stanem tej strony**, nie błędem i nie krótszą stroną; źródło
rozwiązywane raz na dopuszczoną parę w obrębie jednego `list`/`sweep`, i tylko
werdykt o źródle jest pamiętany. Sprzątanie pyta najpierw receipt. Zbieranie
osieroconych odłączone na własną kadencję godzinną.

**Koszt nazwany:** wynik, którego shardy zniknęły, czyta się w katalogu jako
`complete`, dopóki ktoś go nie otworzy. Wcześniej zgłaszał to każdy odczyt.
Usunięcie źródła egzekwuje `sweep` co 60 s — tak jak wcześniej — bo rozwiązuje
źródło raz na dopuszczoną parę; na godzinę zeszło wyłącznie zbieranie
osieroconych, gdzie godzina spóźnienia to ta sama odpowiedź. Zdanie w
poprzedniej wersji tej sekcji mówiło inaczej i było nieprawdziwe.

**Porządek katalogu i próg indeksu.** Klucz to `evaluations/{sha256(id)}/`, więc
porządek jest porządkiem skrótu — i dlatego ekran B2i **nie ma kontrolki
okresu**. Wiersz kosztuje trzy żądania, czyli strona 200 wierszy ~600. Próg, od
którego indeks jest wymagany: **około tysiąca opublikowanych wyników**, albo
pierwsze żądanie porządku innego niż skrót. Indeks to jeden obiekt na commit pod
kluczem z czasem, dopisywany po wygranej claimu i uzupełniany przez przebieg
zbierania; celowo jeszcze nie zbudowany, bo to ta sama zmiana co porządek
czasowy. **Zbudowany w sekcji 19.2** — razem z porządkiem, oknem i kontrolką.

### 18.3 B2g — CI, obserwowalność, wdrożenie

- **CI**: zadanie `object-store` z usługą RustFS na `:9010`, uruchamia
  `just test-rustfs` i nowe `just test-evaluation-s3` (`--ignored`). Obie
  przepuszczone lokalnie na własnym kontenerze `:9011`: 6/6 i 1/1. To samo
  zadanie domyka lukę podpisu SigV4.
- **Obserwowalność**: `RetentionReport` w `evaluations/retention.json`
  (nadpisywany), zwracany jako `retention` na `GET /api/v1/evaluation-results` —
  kiedy przebieg się odbył, ile wycofał i zebrał, i ile **kolejnych** przebiegów
  się nie udało. Trwały, nie linia w logu: przeżywa restart i każda replika czyta
  ten sam. Liczy tylko ten przebieg, nigdy sumy narastającej.
- **Wdrożenie**: `evaluationEvidence` w chartcie (`enabled`, `sourceDir`,
  `volume`, cztery limity), montowany tylko w `server`, z odmową renderowania bez
  `volume`. `docs/INSTALL.md` — nowy rozdział z odtwarzaniem prefiksu z kopii.
  `just run-evaluation` i `just approve-evaluation`, plus
  `scripts/stage-evaluation-approval.py`, który **pyta binarkę** o adres
  zatwierdzenia — pierwsza wersja liczyła go w Pythonie i wychodziła inna liczba.
  Nowych backendów ani ścieżek sieciowych nie ma: `evaluations/` to prefiks w
  tym samym buckecie, więc NetworkPolicy się nie zmienia.

### 18.4 B2h — judge

Tylko ADR, bez adaptera, zgodnie z kolejnością. Cztery reguły dopuszczenia w
ADR 0030: konfiguracja przypięta treścią, zapisany zbiór kalibracyjny jako część
dowodu, rozbieżność z ocenami ludzi obok wyniku, i jawne oznaczenie wyniku jako
nieodtwarzalnego przez ponowny odczyt. Do czasu adaptera `LocalSource` odrzuca
manifest z judge'em **po nazwie**, co jest dzisiejszym zachowaniem i jest celowe.

### 18.5 B2i — panel

Jedna lista, dwa pochodzenia (plakietka `kept` na wierszu trwałym), bez kontrolki
okresu. Siedem stanów, każdy z własnym zdaniem i następnym krokiem; `forbidden`
wymienia trzy przyczyny i **nie zgaduje** między nimi — połowę o roli rozstrzyga
`useRoleDecision('admin')`, tak jak w Conversations. Dwa „partial" rozdzielone na
„Kept with gaps" (co jest czytelne) i „Partly measured" (co zmierzono). Termin
retencji jako data przy wyniku. A3 jako kontrolka `role="group"` z `aria-current`
zamiast czwartego zdania, a wstrzymana delta pisze **withheld** zamiast pustej
komórki. Brak konfiguracji to 501 z nazwą zmiennej. Linia retencji pod listą, i
„żaden przebieg się nie odbył" odróżnione od „jeszcze czytam".

**Odbiór**: 15 testów w `features/evaluation`, 242/242 w panelu, typecheck i
build czyste; oglądnięte na własnej instancji — cztery stany na liście, szczegół
`partial` i `forbidden`, wiersz osiągalny z klawiatury z widocznym focusem,
klik zapisuje `?evidence=` w URL. Motyw jasny sprawdzony przez tokeny (te same
`warning`/`danger`/`muted`, których używa reszta panelu) — panel przeglądarki
wymusza klasę `dark` na `<html>`, więc zrzutu w jasnym nie zrobiłem.

### 18.6 Co zostaje

- **B3** ma teraz z czego budować: dwie pary czytelne naraz.
- **Indeks katalogu** — powyżej ~1000 wyników albo przy pierwszym żądaniu
  porządku czasowego.
- **Upload bundle'a zatwierdzenia** — ostatni krok do usunięcia hosta z drogi.
- **Adapter judge'a** — reguła jest w ADR, kodu nie ma.
- **AR3** — niezależny, nadal warunek C0.

## 19. Zmniejszenie ograniczeń zapisanych w sekcji 18

Cztery paczki po domknięciu etapu B, wzięte z listy ograniczeń, a nie z listy
funkcji. Każda zdejmuje coś, co sekcja 18 nazwała kosztem albo brakiem.

| Paczka | Co zdejmuje | Commit |
| --- | --- | --- |
| 19.1 Widoczne luki | „wynik bez shardów czyta się jako `complete`” | `31489b7` |
| 19.2 Indeks katalogu | brak porządku czasowego, próg ~1000, 3 żądania na wiersz | `8f79048`, `24dbfe8` |
| 19.3 Upload bundle'a | krok na hoście przy nowej parze | `5bfc6ce` |
| 19.4 Zatwierdzenia w panelu | `listApprovals` bez wywołania, `DELETE` tylko curlem | `671cc0c` |

**Sprostowanie do 18.2.** Usunięcie źródła egzekwuje `sweep` co 60 s — tak było
od B2f. Na godzinę zeszło wyłącznie zbieranie osieroconych. Zdanie w 18.2 i
akapit w ADR 0030 mówiły inaczej; oba poprawione.

### 19.1 Luki widoczne bez otwierania wyniku

Przebieg zbierania i tak czyta nagłówek i listuje `content/` każdego wyniku, bo
inaczej nie wie, co skasować. Różnicę tych dwóch zbiorów wyrzucał. Teraz wraca
jako `CollectionReport` → `damaged`/`damaged_count` na raporcie retencji, który
katalog już zwraca; przebiegi minutowe niosą wynik ostatniego godzinnego zamiast
go zerować. **Zero dodatkowych żądań** w normalnym przypadku — tombstone czyta
się tylko tam, gdzie nagłówka i tak brakuje.

To nie jest ósmy `EvidenceState`: nagłówek się weryfikuje, stan to `complete`, a
sprzeczność tych dwóch jest właśnie faktem. Wiersz dostaje plakietkę `bytes
missing`, szczegół — zdanie z datą przebiegu.

### 19.2 Indeks katalogu, porządek i okres

`evaluations/index/{i64::MAX - committed_at}-{sha256(id)}` z receiptem w środku:
rosnące listowanie to malejący zegar, a okres to ograniczenie klucza. Pochodny —
claim jest prawdą, każdy odczyt szczegółu idzie przez niego, a skasowanie całego
prefiksu nie kasuje dowodów (przebieg zbierania go odbudowuje, tą samą drogą
trafiają do katalogu wyniki sprzed indeksu).

Wycofanie **znaczy wiersz przed tombstonem**, więc każde okno między trzema
zapisami pokazuje mniej niż prawdę, nigdy więcej — i dlatego strona nie czyta
już ani claimu, ani tombstone'a.

| | przed B2f | po B2f | teraz |
| --- | --- | --- | --- |
| strona katalogu, 50 wierszy | 400 gets | 152 gets | **103 gets** |

Panel: obie połowy w jednym porządku, `mergeRows` wstrzymuje wiersze starsze niż
ogon połowy, która ma jeszcze stronę (inaczej lista przestawia się pod czytelnikiem),
a `onReachEnd` dociąga tę połowę, na którą czeka granica. Kontrolka okresu wraca,
z domyślnym **„all”** — reszta list domyśla się doby, bo wszystko na nich znika z
retencją logu; połowa tej istnieje właśnie dlatego, że logu nie ma.

### 19.3 Bajty bundle'a przez API

`PUT /api/v1/evaluation-approvals/{id}/bundle/{name}` (admin), `GET` listuje,
`DELETE` czyści. Prefiks `evaluation-bundles/` należy do **adaptera**, obok
`evaluations/`, a nie w środku: czym jest bundle, wie adapter, a rejestr zna
tylko digest, który mu podano. Port `ApprovalBundles` stoi obok `SourceAuthority`
i implementuje go ten sam adapter — bez cyklu i bez drugiego crate'u piszącego w
cudzym układzie kluczy.

Wystawienie bajtów niczego nie dopuszcza: zatwierdzenie rozwiązuje cały bundle i
przypina jego digest, więc bajty, które przyjdą później, nie poszerzają dopuszczenia
— zatrzymują odczyt pary. Nazwa członka to jeden segment albo jeden folder
(`model-artifacts/`); cokolwiek innego jest odmową, nie ścieżką. Wystawione bajty
mają pierwszeństwo przed katalogiem, więc instancja skonfigurowana wcześniej działa
jak działała, a instancja bez katalogu w ogóle może dopuścić parę.

### 19.4 Zatwierdzenia na ekranie

Lista z wycofanymi włącznie, kto i kiedy, ile plików wystawiono; wycofanie pyta
(„Hide every result of this pair? This cannot be undone.”), bo jest ostateczne.
Wystawienie i dopuszczenie jednym aktem, w kolejności serwera: najpierw bajty,
potem zatwierdzenie — przerwany upload zostawia instancję taką, jaka była.

Panel **nie liczy** `approval_id`: to digest po kanonicznej deklaracji, więc
odpowiada `POST /api/v1/evaluation-approvals/address` (czysty, poniżej admina,
a odmowa to 400 mówiące, co jest nie tak z deklaracją).

### 19.5 Błąd kontraktu, który to znalazło

`components(schemas(...))` to jedna globalna przestrzeń nazw, a `Withdrawal`
mają i konwersacje, i ewaluacja. Wygrywała ta od korpusu, więc od `004704f`
kontrakt opisywał `Approval.withdrawn` polami, których ta struktura nie ma — a
klient TypeScript miał je w typach. Nikt tego pola nie czytał, więc nikt tego nie
zauważył; znalazł to dopiero ekran, który pisze, kto i kiedy wycofał parę.
`#[schema(as = ApprovalWithdrawal)]`. Lekcja jest w `CLAUDE.md`.

Przy okazji: `npx tsc --noEmit` nie widzi tego, co `tsc -b` w `npm run build` —
dwa pola opcjonalne czytane jako wymagane przeszły przez pierwszy i wywróciły
drugi. Panel sprawdza się buildem.

### 19.6 Odbiór

`just check` 23/23 PASS. Ręcznie na własnej instancji `:19080` z własnym
katalogiem danych, **bez `AIWATCHER_EVALUATION_SOURCE_DIR`**: wystawienie 10
plików przez API, dopuszczenie pary, dwie publikacje, skasowanie sharda jednej z
nich. Katalog pokazał oba wyniki jako `complete` (nazwany koszt B2f), a po
restarcie raport retencji wymienił `candidate-run-02` w `damaged` i wiersz dostał
plakietkę. Szczegół: zdanie z datą przebiegu, metryki z nagłówka, strona
przypadków w stanie `Bytes missing` i bez wierszy. Panel zatwierdzeń: para z
`admits`, „10 staged”, dwustopniowe wycofanie. **Motyw jasny sprawdzony zrzutem**
— przez własną kontrolkę Appearance panelu, czego przy B2i nie zrobiłem.
Instancja zatrzymana, katalog danych usunięty, wpis podglądu cofnięty.

### 19.7 Co zostaje

- **B3, B4, AR3, C0** — bez zmian, karta AW-6. (B3 dostarczone w sekcji 20.)
- **Chart** wciąż wymaga `volume` przy `enabled`; po 19.3 to już tylko wartość
  do rozluźnienia, ale pliki `deploy/helm/**` są w tym tygodniu w rękach innej
  sesji i nie zostały ruszone.
- **`known_ids`** (most do starych raportów) nadal chodzi po wszystkich claimach,
  bo musi widzieć też porzucone ID — indeks ich nie ma i nie powinien mieć.
- **Judge** — reguła w ADR, adaptera nie ma.

## 20. B3 — porównywanie trwałych dowodów

Paczka B3 zakładała, że porównywalność trwałych wyników to reguła A3 zastosowana
do innych danych. Po przeczytaniu kontraktu okazało się, że jest odwrotnie:
[reguła A3](../crates/aiwatcher-projector/src/evaluations.rs) porównuje **pięć
opcjonalnych napisów**, które producent mógł przysłać albo nie, i większość jej
treści dotyczy nieobecności. Trwały dowód nie ma czego brakować —
[`context_id`](../crates/aiwatcher-evaluation/src/lib.rs) jest adresem treści
całego `EvaluationContext`: zbioru, manifestu przypadków, ich liczby, splitu,
suite, scorera, obu schematów, konfiguracji judge'a i każdej definicji metryki z
jej jednostką, kierunkiem i agregacją. Dwa wyniki albo mają ten sam adres, albo
nie, i **ta równość jest regułą porównywalności**.

### 20.1 Dwie reguły, jedno słownictwo

Sklejenie ich w jedną funkcję wymagałoby zrzutowania przypiętego kontekstu na
pięć opcjonalnych napisów — czyli utraty dokładnie tego, co czyni drugą regułę
mocniejszą. Ale to, co czytelnik *robi* z odpowiedzią, jest identyczne po obu
stronach ekranu, a dwa enumy o tych samych trzech nazwach to dwa słowniki
oddalone o jedno wydanie. `Comparability` przeniesiono więc do
[`aiwatcher_core::comparability`](../crates/aiwatcher-core/src/comparability.rs)
— nad oba crate'y, biorąc wyłącznie trzy słowa, których każda powierzchnia
potrzebuje. To jest powód istnienia `aiwatcher_core::human_input`, zastosowany do
werdyktu zamiast do pytania. Panel rysuje **jedną** kontrolkę
([`comparability.tsx`](../apps/panel/src/features/evaluation/screens/overview/comparability.tsx)),
przeniesioną z `page.tsx`, więc wspólność jest strukturalna, a nie deklarowana.

### 20.2 Co oddaje `GET .../{id}/comparison`

Poza samym werdyktem odpowiedź mówi dwie rzeczy:

- **co się zmieniło** — oba konteksty są w ręku, więc odmowa brzmi „Different
  split”, a nie „inny kontekst”. Gdy jedna strona jest nagrobkiem (a nagrobek nie
  trzyma manifestu), mówi uczciwie, że kontekst się różni, i nic o tym jak.
- **czy dowód za liczbą da się jeszcze przeczytać**. Strona wygasła, wycofana
  albo uszkodzona **nie jest niezgodna** — nic się w niej nie różni — tylko
  `unverified`, tym samym słowem, którego używa połowa fałdowana.

Dwie mniejsze decyzje, łatwe do pomylenia: **jeden wariant zmierzony dwa razy
jest porównywalny**, a `same_variant` jest polem, nie powodem — delta jest
prawdziwa i mierzy powtarzalność, czyli ile ta miara rusza się, gdy nic się nie
zmieniło; jako „powód” pod `comparable` czytałaby się jak problem, a nieobecna —
jak test A/B. I **delta jest wstrzymana widocznie**: obie liczby zostają na
ekranie, bo każda jest faktem, a tylko różnica byłaby twierdzeniem, którego nikt
nie zmierzył.

### 20.3 Kandydaci pochodzą z serwera, nie z przeglądarki

Nie ma automatycznego baseline'u. Połowa fałdowana ma go dlatego, że fałda logu
nie ma jak inaczej zaproponować pary; tutaj odpowiada katalog —
`GET /api/v1/evaluation-results?context_id=…` zwraca dokładnie te wyniki, które
wolno ze sobą porównać, więc który z nich jest baseline'em, jest czyjąś decyzją,
a nie domyślną wartością, której ktoś może nie zauważyć. Panel niczego nie
filtruje: porównanie dwóch napisów w TypeScripcie byłoby drugą odpowiedzią na
pytanie, czym jest porównanie. Filtr chodzi po opublikowanym indeksie, a nie po
drugim indeksie po kontekście — strona kosztuje więc także wiersze, które
minęła, a kursor strony filtrowanej obiecuje kolejny *wpis*, nie kolejne
trafienie. Indeks po kontekście to ten sam kształt „głowa jest pochodna”, co
`evaluations/index/`, i jest tani do dodania w dniu, w którym katalog będzie
tego wymagał.

### 20.4 Porównanie czyta dwa nagłówki

Metryki, które odejmuje, są w metadanych, z jakimi każda strona została
opublikowana, więc kosztuje tyle, co dwa podsumowania — niezależnie od liczby
przypadków za nimi. **Które przypadki się popsuły** — „zdał na baseline, oblewa
teraz”, widok, który trzeba przeczytać przed wydaniem — celowo tu nie ma: obie
strony są sortowane po `case_id` przy publikacji, więc różnicę da się stronicować
w zamku z granicą wysokiej wody (ten sam chwyt, co `mergeRows` w panelu), ale to
nadal pełny odczyt obu stron i jest nazwany jako nieobecny zamiast po cichu
podany przez trasę chodzącą po stu shardach.

### 20.5 Delta bywa kolorowana — tu i nigdzie indziej

Reguła panelu „nie kolorujemy delt metryk” miała jeden powód: nazwa metryki
producenta nie mówi, czy wzrost to poprawa, czy rachunek. Przypięty kontekst
**deklaruje** `MetricDirection` per metryka, więc kolor jest deklaracji, a nie
zgadywaniem; metryka z `none` i metryka, której nikt nie zadeklarował, zostają
czarne. Reguła w `CLAUDE.md` została **zmieniona, nie porzucona**, i nazywa
połowę, której dotyczy.

### 20.6 Odbiór

`just check` 23/23 PASS; 6 nowych testów rejestru i 1 akceptacyjny HTTP po
stronie Rusta, 8 nowych testów panelu (259 łącznie). Ręcznie na własnej
instancji `127.0.0.1:19080`, z własnym katalogiem danych i **bez
`AIWATCHER_EVALUATION_SOURCE_DIR`**: trzy pary dopuszczone i opublikowane przez
API (dwa warianty jednego kontekstu i jeden na innym splicie).

- kandydaci dla kontekstu kandydata: `['baseline-run', 'candidate-run']` —
  wynik z innego splitu nie jest oferowany;
- `candidate-run` vs `baseline-run`: `comparable`, `accuracy` 1.0 vs 0.6667,
  delta `+0.3333`, `unit: ratio`, `direction: higher`;
- `holdout-run` vs `candidate-run`: `incompatible`, powody `Different
  evaluation context` i `Different split`, delta wstrzymana, obie liczby na
  miejscu;
- po wycofaniu zatwierdzenia baseline'u: `unverified`, stan baseline'u
  `forbidden`, delta wstrzymana, liczba kandydata zostaje;
- baseline, którego nikt nie opublikował: 404; stara trasa szczegółu nadal
  odmawia porównania trwałych dowodów, ale mówi teraz, gdzie się to robi.

Ekran sprawdzony w **obu motywach** przez własną kontrolkę Appearance panelu:
kontrolka porównywalności, powody listą, nota o powtarzalności tylko tam, gdzie
jest delta do przeczytania, i kolorowana delta czytelna w ciemnym. Jeden błąd
znaleziony przy odbiorze i naprawiony: wklejony link z `compare=` wskazującym
wynik spoza kontekstu pokazywał „nic tu nie ma do porównania” zamiast odpowiedzi
serwera — panel sam decydował, żeby nie zapytać.

Instancja zatrzymana, katalog danych usunięty, wpis podglądu cofnięty.

### 20.7 Co zostaje z B3

- **Różnica na poziomie przypadków** (regresje i naprawy) — projekt opisany w
  20.4. **Dostarczona w sekcji 23.**
- **Judge**: czwarty warunek z ADR 0030 — wynik nieodtwarzalny przez ponowny
  odczyt — musi wejść do tej reguły razem z adapterem. Dziś `LocalSource`
  odrzuca manifest z judge'em po nazwie, więc takich dowodów nie ma.
- **Kontekst wariantu na obserwacjach** (druga połowa opisu B3 z tabeli paczek:
  SDK, trace, projektor) — nie ruszone; ta paczka jest stroną dowodową.
- **B4, AR3, C0** — bez zmian, karta AW-6. (AR3 dostarczone w sekcji 21.)

## 21. AR3 — wspólny przypadek użycia kompilacji i startu

Warunek C0 z sekcji 7, punkt 3, i jedyna z pozostałych paczek niezależna od
reszty. Stan wejściowy: HEAD `a145b94`, zachowany wycinek innej sesji
(`execution/mod.rs`, `FTI_KICKOFF.md`, `typos.toml`).

### 21.1 Co było nie tak

Uruchomienia managed proszą trzej wywołujący: trasa, którą ktoś naciska,
`run_now` na harmonogramie i tick, który znalazł należny slot. Kompilacja i
start żyły w module HTTP, więc scheduler w roli `work` był klientem routera
axum: wołał do środka, dostawał `ApiError` i czytał **kod statusu**, żeby
zdecydować, czy slot ma zostać należny.

To jest stratne kodowanie jedynego pytania, jakie ma. Trzy odmowy są 5xx z
numeru i trwałe ze znaczenia:

| odmowa | status | tick zapisywał | powinien |
| --- | --- | --- | --- |
| brak registry w tym wdrożeniu | 501 | `try_again` | `refused` |
| zapisany obiekt, który się nie odczytuje | 500 | `try_again` | `refused` |
| magazyn, który odczyt zrozumiał i odrzucił | 502 | `try_again` | `refused` |

Każda z nich zostawiała slot należny i ponawiała go **co minutę, bez końca**, a
karta harmonogramu mówiła, że wciąż próbuje. To jest dokładnie ta połowa reguły
R2, której `SlotSettlement::TryAgain` miał bronić — tylko z drugiej strony.

### 21.2 Reguła wraca do typu, który ją zna

`aiwatcher_execution::start` to ten przypadek użycia: `Executions` (pożyczone
registry, handler, polityka payloadów, silnik zapytań), `StartRequest`,
`compile_head`, `compile`, `start`, `compile_and_start`. Odmowa to
`StartRefused`, a pytanie schedulera zadaje się jej wprost —
`says_the_same_next_time`. Tę samą metodę dostały `HandleError` i `StoreError`,
bo to jest jedno pytanie o trzech właścicielach, nie trzy pytania.

Status nie znika: `impl From<StartRefused> for ApiError` jest jedynym miejscem,
które zamienia powód na kod, i **żaden kod się nie zmienił** — 404, 400, 422,
501, 502, 503, 409, 413 lądują tam, gdzie lądowały. Status jest renderowaniem
odmowy przez jednego wywołującego, nigdy samą odmową.

### 21.3 Co zostaje przy wywołującym

Dwie rzeczy, i tylko te dwie, bo tylko on je zna:

- **kto pyta** — sprawdzenie roli wobec sesji; tick nie ma sesji i nie wolno mu
  jej udawać;
- **który identyfikator** — `RunIdentity`: `Named` dla slotu (wyprowadzony z
  definicji i chwili, więc dwaj workerzy, którzy zobaczyli dziewiątą, dochodzą
  do jednego uruchomienia), `Key` dla nagłówka `Idempotency-Key` (wyprowadzony z
  klucza **i planu**, żeby jeden klucz nie adresował dwóch planów), `Fresh` dla
  kliknięcia.

`compile_and_start` sprawdza handler **przed** kompilacją: instancja bez
workflow store nie uruchomi niczego, więc powiedzenie tego jest lepszą
odpowiedzią niż zgłoszenie czegokolwiek innego, czego też brakuje.

### 21.4 Jedno słownictwo zamiast dwóch

`TargetKind` w module HTTP był drugą kopią `DefinitionKind` z domeny — te same
dwa warianty, te same nazwy na drucie, ten sam komentarz. Usunięty; `kind` w
`ExecutionTarget` wskazuje teraz na `DefinitionKind`. Wartości na drucie bez
zmian (`curation_pipeline`, `workflow`), więc panel i klient przeszły przez
`just openapi` bez ręcznej zmiany. Razem z tym do domeny wróciły
`ExecutionTarget`, `Decider`, `PayloadDefault` i pinowanie okna — to polityka,
nie transport.

### 21.5 Odbiór

`just check` 23/23. Nowe: 12 testów jednostkowych `start::tests`, 2 testy
mapowania nagłówka na `RunIdentity` w module trasy, 1 test HTTP (`501` nazywa
`AIWATCHER_WORKFLOW_STORE` zamiast 404). Usunięte razem z przeniesionym kodem:
5 testów, które badały funkcje z modułu HTTP — ich zachowanie jest w tych
dwunastu.

Odbiór na własnej instancji `127.0.0.1:19081`, katalogi tymczasowe:

- trasa: `202` + `created: true`, ten sam `Idempotency-Key` → `202` + `created:
  false` i **ten sam** `execution_id`; bez klucza → drugie uruchomienie; nazwa,
  której nikt nie zapisał → `404 not_found`; `as_of` na workflow → `400`
  nazywające, czego dotyczy;
- reguła: zapisany pipeline, harmonogram godzinowy na najbliższą minutę, a
  potem **uszkodzony `head.json`** w magazynie obiektów. Tick o 14:12 zapisał
  `outcome: refused` z powodem „stored object …/head.json is not a dataset
  registry document", a `next_run` przeskoczył na następną godzinę. Przed tą
  paczką ten sam przypadek dawał 500 → `is_client_error() == false` →
  `try_again`, slot należny, ponowienie co minutę bez końca.

Instancja zatrzymana, katalog danych usunięty.

### 21.6 Co zostaje

- **`DefinitionRegistry` spłaszcza `PortError` do `StoreError::Backend`**, więc
  dla *zarejestrowanego workflow* niedostępny magazyn i uszkodzony rekord są
  nierozróżnialne i oba czytają się jako „wróć za chwilę". Dla pipeline'ów
  reguła jest pełna, bo `aiwatcher_datasets::RegistryError` te przypadki
  rozróżnia. Naprawa to zmiana typu błędu tamtego registry — osobna paczka.
  **Dostarczona w sekcji 22.**
- **C0** ma teraz swój warunek: nowy scorer jest zadaniem istniejącego workera i
  startuje przez `Executions`, nie przez własny silnik.
- **B3 (różnica na poziomie przypadków), B4, C0** — bez zmian, karta AW-6.

## 22. Rejestr definicji odpowiada trzema zdaniami zamiast jednym

Sekcja 21 dała odmowie start-u metodę, która odpowiada na jedyne pytanie
schedulera. Dla pipeline'ów działała od razu, bo
`aiwatcher_datasets::RegistryError` rozróżnia przypadki. Dla *zarejestrowanych
workflow* nie działała, i ta paczka jest tym brakiem.

### 22.1 Jeden napis zamiast trzech odpowiedzi

`DefinitionRegistry` mapował **wszystko** na `StoreError::Backend(String)`
jedną funkcją `fn backend(error: impl Display)`. Wchodziły do niej trzy różne
rzeczy:

| co się stało | co z tego zostawało | co z tego czytał tick |
| --- | --- | --- |
| magazyn obiektów nieosiągalny albo odmówił | tekst | „wróć za chwilę" |
| zapisany obiekt, który się nie parsuje | tekst | „wróć za chwilę" |
| definicja, która się nie kompiluje | tekst, bez listy problemów | „wróć za chwilę" |

`PortError` **już niesie** ten jeden bit — `Unavailable` wraca, `Rejected` nie —
a spłaszczenie go do napisu wyrzuca go, zanim wywołujący zdąży zapytać. Przez
`HandleError::Store` trafiało to do `StoreError::says_the_same_next_time`, gdzie
`Backend(_)` jest **uczciwym `false`**: adapter już nie wie, co trafił. Uczciwe
`false` na spłaszczonym błędzie to jednak wciąż slot należny co minutę dla
definicji, która nigdy się nie skompiluje.

Ten sam błąd miała trasa HTTP: `503 workflow_store_unavailable` dla wszystkich
trzech, czyli obietnica powrotu dla czegoś, co nie wróci.

### 22.2 Trzy odpowiedzi, w słownictwie drugiego rejestru

`DefinitionError` ma trzy warianty i są to **te same trzy słowa**, których
używa `aiwatcher_datasets::RegistryError`:

- `Store(PortError)` — magazyn; `is_retryable()` odpowiada za resztę,
- `Corrupt { key, message }` — obiekt pod tym kluczem nie jest dokumentem tego
  rejestru; wiadomość nazywa **który** obiekt,
- `Refused(Vec<String>)` — definicja się nie kompiluje, z każdym problemem
  naraz, tak jak odmawia trasa przed magazynem.

Dwa rejestry, z których kompiluje się managed run, odpowiadają na jedno
pytanie; dwa słownictwa dla niego to dwie odpowiedzi oddalone o jedno wydanie —
powód, dla którego `Comparability` trafiło do `aiwatcher_core`, zastosowany do
odmowy zamiast do werdyktu. `StartRefused::Registry` nazywa się teraz
`Pipelines`, a obok stoi `Definitions`: przy dwóch rejestrach wariant o nazwie
„Registry" wymaga wiedzy, o który chodzi.

### 22.3 Statusy tutaj się zmieniły — i to jest ta poprawka

Sekcja 21 zachowała każdy kod HTTP, bo zmieniała miejsce, w którym mieszka
przypadek użycia. Tutaj zmienia się sama klasyfikacja, więc zmieniają się i
kody — na te, które ten sam rejestr datasetów już oddaje:

| odmowa | przed | teraz |
| --- | --- | --- |
| magazyn nieosiągalny | `503 workflow_store_unavailable` | `503 registry_unavailable` |
| magazyn zrozumiał i odmówił | `503 workflow_store_unavailable` | `502 registry_rejected` |
| zapisany obiekt się nie parsuje | `503 workflow_store_unavailable` | `500 registry_corrupt` |
| definicja się nie kompiluje | `503 workflow_store_unavailable` | `422 plan_refused` + `details` |

Dotyczy trzech tras `/api/v1/workflow-definitions` i startu z celem
`workflow`. Kontrakt wymienia teraz 500 i 502 przy tych trasach, klient
wygenerowany ponownie; panel czyta je przez `answerOrNone`, więc żaden ekran
nie zmienia zachowania — zmienia się to, co mówi wiadomość.

### 22.4 Odbiór

`just check` 23/23 PASS. Nowe: 1 test przez prawdziwy rejestr (uszkodzona
głowa → `Definitions(Corrupt)` nazywający klucz, `says_the_same_next_time()`
prawda), 1 test HTTP (500 `registry_corrupt` na trasie szczegółu i na starcie),
oraz cztery nowe wpisy w teście, który trzyma listę odmów rozstrzygniętych i
odmów wartych powrotu.

Odbiór na własnej instancji `127.0.0.1:19082`, katalogi tymczasowe: zapisany
workflow `house/import`, harmonogram godzinowy na 14:54 UTC, potem `head.json`
nadpisany bajtami, które nie są dokumentem.

- trasa szczegółu i `POST /api/v1/executions`: `500 registry_corrupt`,
  wiadomość nazywa `workflows/heads/a034…dfce.json`;
- tick o **14:54:23** zapisał na harmonogramie `outcome: "refused"` z tym samym
  powodem, a `next_run` przeskoczył na `2026-09-12T15:54:00Z`. Przed tą paczką
  ten sam przypadek był `try_again`: slot należny, ponowienie co minutę bez
  końca.

Instancja zatrzymana, katalog danych usunięty.

### 22.5 Co zostaje

- **Nieosiągalny magazyn** zweryfikowany testem jednostkowym, nie na żywo:
  adapter plikowy nie ma jak być chwilowo nieosiągalny. To ta sama granica, co
  przy pipeline'ach.
- **B3 (różnica na poziomie przypadków), B4, C0** — bez zmian, karta AW-6.

## 23. B3 — które przypadki się ruszyły

Sekcja 20 dostarczyła porównanie dwóch nagłówków i **nazwała** to, czego w nim
nie ma: „zdał na baseline, oblewa teraz". Ta paczka to dobudowanie tamtego
projektu, bez zmiany niczego, co sekcja 20 rozstrzygnęła.

### 23.1 Dlaczego to musi być osobna trasa

Porównanie nagłówków kosztuje tyle, co dwa podsumowania — niezależnie od liczby
przypadków za nimi, i to jest reguła, która sprawiła, że strona katalogu
przestała kosztować korpus. Różnica na poziomie przypadków jest pełnym odczytem
obu stron. Dopisanie jej jako pola do `GET .../comparison` oznaczałoby, że ktoś,
kto zapytał o dwie liczby, po cichu chodzi po stu shardach.

Więc jest osobno: `GET /api/v1/evaluation-results/{id}/comparison/cases`.
Stronicowana, zawężana **na serwerze**, i z sufitem na to, jak daleko wolno jej
zajść w jednym żądaniu — 2000 przypadków. Bez sufitu para, której dziesięć
tysięcy przypadków zawiera dwanaście regresji, byłaby jednym żądaniem czytającym
każdy shard obu stron.

Trzy rzeczy robią to tanim:

| | |
| --- | --- |
| obie strony są **posortowane po `case_id`** przy publikacji | różnica to scalenie dwóch uporządkowanych strumieni, a kursor to para przesunięć — ta sama liczba, którą trzyma kursor strony przypadków, raz na stronę |
| czytane są **tylko pomiary** | oczekiwane odpowiedzi to kohorta, którą porównywalna para dzieli z definicji, więc odjęcie dwóch wyników kosztuje połowę tego, co odczyt przypadków któregokolwiek z nich |
| zawężenie jest serwera | „przypadki, które coś straciły" to trasa, a nie odfiltrowanie dziesięciu tysięcy wierszy w przeglądarce |

Wiersz nie niesie **treści** przypadku — niesie `at`, czyli **gdzie** ten
przypadek leży po danej stronie, w słowach, którymi trasa przypadków już mówi:
oddany jako jej `cursor` z `limit=1` zwraca dokładnie ten przypadek. Scalenie i
tak zna tę pozycję, bo po niej chodzi, więc niesienie jej kosztuje zero — a
oszczędza dwie gorsze możliwości: wyszukiwanie po `case_id`, które czyta shardy,
aż trafi (do 50 odczytów na jeden przypadek przy 10 000), albo obie odpowiedzi w
każdym wierszu trasy, która i tak czyta dwa całe wyniki. Kursor jest nieprzezroczysty
i nigdzie nie jest rozbierany: należy do trasy, która go wystawiła.

Strona zawężona kończy się na pierwszym z dwóch: wierszach, o które poproszono,
albo przypadkach, po których wolno było przejść. Kursor obiecuje więc kolejny
**przypadek**, a nie kolejne trafienie — dokładnie ta własność, którą ma już
katalog zawężony po kontekście, i z tego samego powodu: porządek należy do
przypadków, nie do filtru.

### 23.2 Pięć słów, bo dowód jest inny niż po stronie fałdowanej

Połowa fałdowana trzyma dwie listy, `regressed` i `fixed`, bo producent przysyła
tam `passed` na przypadek i regresja to przewrócenie się tego booleana. Tutaj
przypadek niesie **zadeklarowane metryki**, a przypięty kontekst deklaruje, w
którą stronę każda z nich jest lepsza. Z tego wychodzi pięć odpowiedzi zamiast
dwóch list:

- `regressed` — wszystko, co się ruszyło, ruszyło się w złą stronę; albo
  przypadek **przestał być mierzalny**, co jest najostrzejszą regresją, jaka
  istnieje, i jedynym ruchem, którego nie da się wyrazić liczbą (przypadek,
  który zawiódł, nie niesie żadnego wyniku),
- `improved`,
- `mixed` — lepiej w jednym, gorzej w drugim: stan, który pojedynczy werdykt
  musiałby ukryć, a o który akurat ktoś się spiera przed wydaniem,
- `unchanged`,
- `unmeasured` — jedna ze stron nigdy tego przypadku nie zmierzyła. Różnica
  między dwoma przebiegami, a nie ruch.

Filtr `?only=` jest **pytaniem**, nie werdyktem: `worse` to to, co czyta bramka
wydania, i celowo zawiera `mixed` — bramka, która ukryłaby przypadki, które coś
straciły *i* coś zyskały, ukrywałaby te, o których trzeba zdecydować.

Arytmetyka jest dokładna, bez własnej tolerancji: obie liczby są tym, co
opublikowało dwóch producentów, a epsilon tutaj byłby progiem, którego nikt nie
zadeklarował, decydującym, które z ich pomiarów się liczą.

### 23.3 Jedno miejsce, w którym obie połowy porównania się rozchodzą

Wiersze powstają także dla pary `unverified` — i to jest ta jedna różnica.
Nagłówek wstrzymuje tam swoją deltę, bo agregat po przypadkach, które zawiodły
albo zostały niezmierzone, jest liczbą o mianowniku, na który nikt się nie
zgodził. Delta jednego przypadku odejmuje dwa pomiary **tego samego** przypadku
i jest poprawna niezależnie od tego, co stało się z resztą — a przypadki, które
zawiodły, to dokładnie to, po co ktoś tę trasę otwiera.

Dla pary `incompatible` wierszy nie ma: różnica nad dwoma wynikami, których
serwer właśnie odmówił odejmować, byłaby drugą odpowiedzią na pytanie, czy wolno
je odjąć. Werdykt i powody jadą na stronie, więc odmowa brzmi tym samym zdaniem,
co przy nagłówku, zamiast być pustą listą.

### 23.4 Panel niczego nie klasyfikuje

Sekcja jest **otwierana**, nie pobierana: wszystko powyżej niej to dwa nagłówki,
a to są dwa pełne wyniki, więc otwarcie wyniku nie może za to płacić. Wybrany
filtr siedzi w URL-u (`cases=worse|better|changed|all`), więc link do „przypadków,
które coś straciły" wprowadza następnego czytelnika na to samo pytanie. Czwarta
wartość istnieje, bo brak parametru już coś znaczy — zamknięte — więc „otwarte i
nic nie odfiltrowane" potrzebuje własnego słowa.

Panel rysuje zmianę, którą przysłał serwer, **łącznie z wierszem, który jego
własny filtr by odrzucił**: reguły mieszkają tam, gdzie deklaracje, tak jak przy
kanwie pipeline'u i kanwie adnotacji. Test trzyma dokładnie ten przypadek.

Kliknięcie `case_id` rozwija wiersz w to, co **obie strony odpowiedziały** —
oczekiwane obok udzielonego, a przy przypadku, który zawiódł, komunikat błędu
zamiast odpowiedzi. To dwa odczyty jednego przypadku przez `at`, robione dopiero
po kliknięciu; zamknięty wiersz nie pyta o nic, co test też trzyma. Który wiersz
jest otwarty zostaje w stanie komponentu, a nie w URL-u: w URL-u siedzi pytanie,
które ktoś zadał, a to jest jeden wiersz odpowiedzi, na którą już patrzy.

### 23.5 Odbiór

`just check` 23/23 PASS. Nowe: 8 testów rejestru (w tym scalenie 250 przypadków
przez granice shardów i podążenie za `at` do przypadku po obu stronach),
1 akceptacyjny HTTP, 5 testów panelu (łącznie 13 w tym pliku).

Odbiór na własnej instancji `127.0.0.1:19083`, własny katalog danych, pakiety
zatwierdzeń wgrane przez API. Trzy przypadki fikstury, dwa warianty jednego
kontekstu: `before` zdał `capital-pl` i `two-plus-two`, oblał `empty`; `after`
zdał `empty`, oblał `two-plus-two`, a na `capital-pl` **zawiódł** („the model
timed out").

- nagłówek: `unverified`, `accuracy` 0.5 vs 0.6667, **delta wstrzymana** — bo
  `after` zostawił przypadek niezmierzony;
- `/comparison/cases`: `capital-pl regressed` (accuracy `—` ← 1.0, bez delty,
  z komunikatem błędu), `empty improved` (+1.0), `two-plus-two regressed` (−1.0).
  To jest ta paczka w jednym zdaniu: nagłówek nie mógł podać różnicy, a ta trasa
  mówi, które przypadki ją zabrały;
- `only=worse` → dwa wiersze, `only=better` → jeden, `only=changed` → trzy;
- `limit=1` → jeden wiersz i kursor niosący **obie** wersje i oba przesunięcia;
  podążenie za nim daje `empty`;
- kursor spoza tej pary, `nope` i `a:b:c:d` → `400`, `limit=0` → `400`,
  baseline, którego nikt nie opublikował → `404`;
- trzeci wynik na splicie `holdout`: `incompatible`, powody `Different
  evaluation context` i `Different split`, zero wierszy, brak kursora;
- `at` z wiersza `two-plus-two` oddane trasie przypadków z `limit=1` zwraca po
  obu stronach dokładnie ten przypadek, z oczekiwaną odpowiedzią.

Ekran sprawdzony w obu motywach: pigułki filtru, `regressed` na czerwono,
`improved` na zielono, błąd pod liczbami, a nie zamiast nich; `cases=all` w URL
po kliknięciu. Rozwinięty `two-plus-two` pokazuje `expected {"answer":"4"}` po
obu stronach i `answered {"answer":"five"}` kontra `{"answer":"4"}`; rozwinięty
`capital-pl` — „the model timed out" na czerwono zamiast odpowiedzi, przy
nietkniętej odpowiedzi baseline'u. Instancja zatrzymana, katalog danych usunięty,
motyw przywrócony.

Jedna rzecz do zapisania: przy pierwszym uruchomieniu podałem `AIWATCHER_ADDR`
zamiast `AIWATCHER_LISTEN`, więc instancja przez chwilę stała na domyślnym
`0.0.0.0:8080`. Własny katalog danych, więc żadne cudze dane nie zostały
dotknięte; proces zatrzymany, port zwolniony.

### 23.6 Co zostaje

- **Sufit 2000 przypadków na żądanie** jest stały. Dla pary 10 000 × 10 000 bez
  trafień to pięć żądań, żeby dojść do końca — poprawne i widoczne w kursorze,
  ale nie jest to strona indeksowana po zmianie.
- **Judge** (czwarty warunek ADR 0030) i **kontekst wariantu na obserwacjach** —
  bez zmian, tak jak w 20.7.
- **B4, C0** — bez zmian, karta AW-6. (B4 dostarczone w sekcji 24.)

## 24. B4 — typowane oceny, rubryki i rewizje

Etap B, punkt 7. Do tej pory instancja umiała zapisać, ile przypadków wariant
zdał; nie umiała zapisać, **co ktoś o tym sądzi** — ani człowiek, ani judge.

### 24.1 Ocena bez formularza to liczba, której nikt nie odczyta

`3` jest znakomite na jednym formularzu i porażką na innym. Więc rubryka jest
zasobem: pytanie, te same słowa, które dostaje człowiek i judge, zbiór
odpowiedzi, które dopuszcza, oraz kierunek — którą stroną jest lepiej. Skala ma
trzy kształty: ograniczona liczba, nazwane poziomy w zadeklarowanej kolejności
oraz flaga.

Wersja rubryki to skrót jej treści, tak jak wersja promptu. Publikacja tych
samych słów drugi raz trafia na wersję, która już jest — a przepisanie poziomów
to nowa wersja, nie zmiana znaczenia tego, co już powiedziano. Ocena zapisuje
**konkretną wersję**, nigdy nazwy głowy: głowa się przesuwa, a odpowiedź padła
pod jednym zestawem poziomów. Wcześniejsza wersja pozostaje czytelna.

Nazwa rubryki nie może zawierać separatora ścieżki, bo nazwa jest tym, czym
czytelnik o formularz prosi.

### 24.2 Ocena człowieka nie nadpisuje oceny judge'a

Mechanizmem jest tożsamość: „stojąca ocena" to cel, rubryka, **źródło i
autor** razem. Dwie oceny jednego celu pod jedną rubryką — jedna człowieka,
jedna judge'a — to dwa rekordy i oba wracają z listy. Klucz kończący się na
celu i rubryce zrobiłby z drugiego piszącego redaktora pierwszego.

Autor nigdy nie pochodzi z ciała żądania dla człowieka: ocena człowieka jest
oceną sesji, która ją złożyła, bo klient, który mógłby wskazać recenzenta,
mógłby złożyć cudzą ocenę. Judge jest wskazywany jawnie — i obok zostaje
`recorded_by`, czyli kto go uruchomił.

Cel jest jeden z czterech: trace, span, migawka sesji albo pomiar przypadku.
Każdy adresuje coś niezmiennego albo mówi, który moment go takim uczynił —
sesja rośnie, więc ocena sesji niesie `as_of`. Adres celu liczy serwer; to ta
sama reguła, przez którą panel nie liczy identyfikatora zatwierdzenia.

**Nic nie sprawdza, czy cel istnieje.** Ocena przeżywa trace, o którym mówi —
po to się ją zapisuje.

### 24.3 Zmiana zdania to rewizja, a powtórzenie nie jest zmianą zdania

Zapis dokłada rewizję i niczego nie nadpisuje; historia jednej stojącej oceny
jest stronicowana, bo judge oceniający co noc rośnie bez udziału człowieka.

Powtórzenie tego, co mówi bieżąca rewizja, trafia na tę rewizję — reguła
wersji promptu, i to ona sprawia, że ponowione wysłanie po utraconej
odpowiedzi jest bezpieczne, a judge, który nie zmienił zdania, nie pisze
rewizji co noc na zawsze. Koszt jest nazwany: zostaje data, kiedy powiedział to
**pierwszy** raz, a nie ostatni, kiedy się z tym zgodził.

### 24.4 Czego ocena nie niesie

**Oczekiwanej odpowiedzi.** Oczekiwania należą do kohorty i rozwiązuje je
adapter źródła przy czytaniu strony przypadków. Recenzent, który uważa, że
odpowiedź powinna brzmieć inaczej, proponuje zmianę w zbiorze danych, a nie
zapisuje ją — to jest ścieżka C4, przez review.

**Zgody na użycie treści.** Review rozmowy odpowiada, czy na tej treści wolno
trenować; ocena odpowiada, czy odpowiedź była dobra. Ocena spanu, który wskazuje
turę, nie rusza jej stanu review w żadną stronę — i odwrotnie: zatwierdzenie
tury zostawia ocenę tam, gdzie była. Recenzent może powiedzieć „zachowaj to,
było błędne".

### 24.5 Panel nie rozstrzyga, kto miał rację

Oceny otwierają się razem z rozwiniętym wierszem różnicy, bo tam ktoś właśnie
patrzy na obie odpowiedzi. Kontrolki to własna skala rubryki — trzy przyciski
na trzy poziomy, „tak" i „nie" na flagę, ograniczone pole liczbowe — czytane z
tej wersji, którą zapis właśnie utrwali.

Żadna wartość nie jest kolorowana. Czy „good" to dobra wiadomość, deklaruje
kierunek rubryki, a zamiana dwóch odpowiedzi w jeden werdykt w przeglądarce
byłaby rozstrzyganiem dokładnie tego, po co deklaracja istnieje. Instancja bez
rubryki mówi, że ocena potrzebuje formularza, zamiast pokazywać pusty wybór.

### 24.6 Odbiór

`rtk just check` 23/23. Testy: 10 w rejestrze (`evaluation/assessments.rs`), 5
jednostkowych przy skali, 3 HTTP (autorstwo i role, brak magazynu jako 501,
jakość kontra zgoda), 4 w panelu, 3 w SDK.

Odbiór na żywo na `127.0.0.1:19084`, własny katalog danych, po odbiorze
zatrzymane i usunięte:

- rubryka opublikowana, w liście, wersja wskazana skrótem treści;
- ocena człowieka `bad` i ocena judge'a `good` o tym samym przypadku — obie
  wracają, `recorded_by` przy judge'u to sesja, która go złożyła;
- to samo zdanie jeszcze raz → rewizja 1 z pierwotną datą; zmiana zdania →
  rewizja 2; historia po jednej, kursor schodzi do rewizji 1;
- przepisanie rubryki → nowa wersja, wcześniejsza nadal czytelna, a nowa ocena
  pod głową odmówiona z wymienionymi poziomami;
- odmowy nazywają pole: poziom spoza skali, liczba na skali porządkowej, autor
  przy ocenie człowieka, `trace_id` przy celu typu `case`, brak `as_of`;
- panel: rozwinięty wiersz `two-plus-two` pokazuje werdykt judge'a z
  uzasadnieniem, przełączenie rubryki zmienia kontrolki z „tak/nie" na trzy
  poziomy, a zapisana ocena człowieka staje obok judge'a. Oba motywy.

### 24.7 Co zostaje

- **Rubryki autoruje się przez API.** Panel ich nie tworzy — to formularz, a
  pierwszym formularzem w tym panelu ma być ścieżka sterowania po WebSocket.
- **Ocena jest widoczna tylko przy przypadku.** Trace, span i sesja mają
  kontrakt i trasę, nie mają ekranu.
- **Rozbieżność judge'a z ocenami ludzi** nie jest jeszcze liczona; to trzeci
  warunek judge'a z ADR 0030 i należy do jego adaptera.
- **C0** — bez zmian, karta AW-6; jego zależność od B4 jest spełniona. (Pierwsza paczka C0 dostarczona w sekcji 25, jej ograniczenia zamknięte w sekcji 26.)

## 25. C0 — scoring zapisanych odpowiedzi

Pierwsza paczka etapu C: aiwatcher po raz pierwszy sam mierzy, zamiast
przyjmować wynik zmierzony gdzie indziej. Run czyta nagranie odpowiedzi,
ocenia je zadeklarowaną kartą i publikuje dowód przez tę samą bramkę co
producent — bez kroku na hoście serwera i bez wywołania modelu aplikacji.
Commity `5130b41`, `607e77f`, `50ea920`, `9771e88`, `e0e59c1`, `1a0af94`;
reguły w [ADR 0030](ADR/ADR_0030_EVALUATION_EVIDENCE.md), poprawka „evidence this
deployment measured".

### 25.1 Co się mierzy, jest zasobem

Lista suite z API jest agregatem raportów: nazwą, którą producent przysłał, bez
niczego, co pozwoliłoby ją uruchomić ponownie. **Scorecard** jest brakującą
połową — nazwane scorery ze słownika, który ta instalacja implementuje, metryka
każdego z nich i miejsce w odpowiedzi i oczekiwaniu, które czyta (JSON Pointer).
Wersjonowana treścią jak rubryka i prompt; run wskazuje konkretną wersję.

Dwie reguły. **Scorer się nazywa, nie pisze** — karta nie zawiera kodu, więc
jej publikacja nie jest sposobem na uruchomienie czegoś na hoście, a enum jest
implementacją. **Definicja metryki jest wyprowadzana**, nie autorowana obok —
`forbidden` liczy frazę, więc mniej jest lepiej, i autor, który zadeklarowałby
odwrotnie, odwróciłby każde porównanie. Pięć scorerów na start (`exact_match`,
`contains`, `regex_match`, `numeric_within`, `forbidden`), każdy odpowiada
„tak/nie" o jednym przypadku, więc agregatem jest `rate` z jednostką `ratio`.
Wzorzec, który się nie kompiluje, jest odmawiany przy publikacji karty, a nie w
każdym przypadku runu. Trasa to `/api/v1/evaluation-scorecards`, bo
`/evaluation-suites` jest zajęte przez agregat raportów.

### 25.2 Suite i scorer to dwie referencje, bo mają dwóch właścicieli

`context.suite` wskazuje scorecard w wersji treści, a `context.scorer` —
`aiwatcher.scoring` w wersji słownika, który ją przeczytał (`SCORING_VERSION`,
podbijane, gdy odpowiedź istniejącego scorera zmienia się dla jakiegoś
wejścia). Przepisany scorer mierzy niezmienioną deklarację inaczej i jedna
referencja nie mogłaby tego powiedzieć. Obie wymagane przez kontrakt pola
dostały w ten sposób znaczenie zamiast dwóch napisów od producenta.

### 25.3 Nieobecność zostaje nieobecnością

- przypadek z kohorty, na który nikt nie odpowiedział, jest `unscored`, nie
  zerem — niedostępne nagranie widać jako lukę w dowodzie, nie jako złą ocenę;
- przypadek, którego choć jeden scorer nie przeczytał, nie niesie **żadnej**
  metryki i jest błędem z powodem — kontrakt wymagał już kompletu metryk, a
  przypadek w trzech średnich z czterech dawałby każdej metryce inny mianownik;
- przypadek, na który nagranie odpowiada dwa razy, nie jest oceniany — jedna
  publikacja to jedno powtórzenie;
- odpowiedź spoza kohorty nie należy do tego pomiaru i nie jest błędem.

### 25.4 Deklaracja jest tożsamością runu i jest zatwierdzana przed startem

Cztery kroki, każdy idempotentny, bo adresowany tym, czym jest:

1. `PUT /api/v1/evaluation-recordings/{name}` — nagranie pod skrótem bajtów,
   które przyszły; nigdy pod skrótem od klienta.
2. `POST /api/v1/evaluation-runs` — deklaracja (wariant, kohorta z
   `case_count`, wersja karty, skrót nagrania), adresowana treścią. Odpowiedź
   niesie manifest, który wynik opublikuje, `approval_id` i `admitted` — bo
   metryki manifestu są wyprowadzone z karty, a operator przepisujący je ręcznie
   do zatwierdzenia byłby drugą odpowiedzią na pytanie, co run mierzy.
3. Operator stage'uje bundle i zatwierdza parę — istniejącymi trasami.
4. `POST /api/v1/evaluation-runs/{id}/start` — zwykłe managed execution z
   jednym krokiem `score_evaluation`, którego plan niesie adres deklaracji, a
   id runu jest z niego wyprowadzone. Powtórzenie trafia w run, który już idzie.
   **Start jest odmawiany (409, z nazwą zatwierdzenia), dopóki nic nie
   dopuszcza pary** — run bez zatwierdzenia mógłby tylko upaść przy publikacji,
   a ten upadły run byłby tym, w co trafia każdy późniejszy start tej samej
   deklaracji. Ta pułapka była w pierwszej wersji, gdzie deklaracja i start były
   jednym żądaniem.

Krok działa w roli `serve` obok publikacji datasetu, z tego samego powodu: nic
nie wykonuje. Nie jest cache'owany — produktem jest publikacja gdzie indziej, a
cache pamiętałby wiersze. `aiwatcher-execution` nadal nie zna Evaluation
(granica crate'ów), więc `ScoreEvaluationSpec` ma jedno pole — skrót deklaracji
— a wykonawca rozwiązuje go przez rejestr. Trzeci `DefinitionKind`,
`evaluation`, odmawia kompilacji z nazwy; to też blokuje zapis harmonogramu dla
czegoś, co nazwy nie ma.

### 25.5 Dopuszczenie czyta każdą przypiętą rzecz u właściciela

Znalezione dopiero przy czytaniu adaptera przed odbiorem na żywo: `LocalSource`
dopuszcza suite i scorer producenta, czytając z bundle'a `suite.json` i
`scorer.py` o skrótach równych wersjom. Dowód zmierzony tutaj nie ma żadnego z
tych plików, a wersja silnika to `"1"`, nie skrót — więc **żaden run scoringu
nie przeszedłby zatwierdzenia**. Testy przechodziły, bo rozwiązują źródło przez
dublery.

Rejestr dopuszcza teraz ten rodzaj dowodu — rozpoznawany po nazwie scorera —
wobec właścicieli, zanim zapyta adapter: wersja scorera musi być wkompilowana,
scorecard musi istnieć w podanej wersji, a metryki kontekstu muszą być dokładnie
tymi, które karta wyprowadza (inaczej zmieniony po drodze kierunek zostałby
dopuszczony jako kierunek karty). Adapter pomija dwa nieistniejące pliki i
sprawdza wszystko inne jak dotąd. Test na prawdziwym adapterze: bundle bez
`suite.json` i `scorer.py` dopuszcza run, a oczekiwania to te z fixture.

Sprawdzenie liczby przypadków zostało w rejestrze. Wykonawca miał jego kopię;
trzecia odpowiedź na pytanie, czym jest dopuszczona kohorta, mogłaby się z
pozostałymi rozjechać.

### 25.6 Odbiór

`rtk just check` 23/23 po `5130b41`, `50ea920` i `e0e59c1` (pozostałe commity
kodu: testy zmienionych crate'ów i clippy); `just sdk-check` 482.
Testy: 7 jednostkowych przy karcie, 14 w rejestrze
(`evaluation/scorecards.rs`, `evaluation/scoring.rs`, w tym prawdziwy
`LocalSource`), 3 przy wykonawcy, 2 HTTP (karta; deklaracja → odmowa →
zatwierdzenie → start → powtórzenie), 3 w SDK.

Odbiór na żywo na `127.0.0.1:19085`, własny katalog danych, po odbiorze
zatrzymane i usunięte:

- karta z `exact` (trim, bez wielkości liter, `/text` wobec `/answer`) i
  `forbidden` („pesel") — deklaracja zwraca `exact/higher/rate`,
  `leaked/lower/rate`, suite `answer-quality`, scorer `aiwatcher.scoring@1`;
- start przed zatwierdzeniem → 409 z pełnym `approval_id`;
- bundle z sześcioma plikami fixture i manifestem z deklaracji, **bez**
  `suite.json` i `scorer.py` → zatwierdzony;
- start → 202, jeden krok `score_evaluation`; run `completed` po 0,5 s;
  w zakładce Workflows `succeeded`, 141 ms, 1 węzeł;
- dowód: `status partial`, 3 wybrane / 2 ocenione / 1 błąd / 0 bez oceny,
  `exact 0.5`, `leaked 0.5`; `origin` niesie `execution_id` i `step_id: score`;
  `capital-pl` („ warsaw " wobec „Warsaw") 1.0 z zachowanym `trace_id`;
  `two-plus-two` („four, my PESEL…" wobec „4") `exact 0`, `leaked 1`; `empty`
  (odpowiedź bez `/text`) — błąd „exact: the answer has nothing at /text", bez
  metryk; odpowiedź spoza kohorty pominięta;
- drugi start → ten sam run, `created: false`; deklaracja → `admitted: true`.

Panel nie wymagał zmian: szczegół trwałego dowodu już rysuje
`ExecutionReference`, gdy manifest ma `origin.execution_id`.

### 25.7 Co zostaje

- **Formularz startu** (plan, etap C p. 1) należy do C1 — tak jak w tabeli
  paczek. Dziś run uruchamia API albo SDK; panel pokazuje run i jego dowód.
- **Nagranie z archiwum rozmów** — drugie źródło treści z punktu C0. Wymaga
  bramki dostępu do treści (`with_content_access`), której wykonawca dziś nie
  dostaje, więc dowód ze źródła Conversations zostanie odmówiony.
- **Scorer ilościowy** (np. błąd bezwzględny) potrzebuje jednostki, którą zna
  tylko autor; specyfikacja dostanie to pole, gdy pierwszy się pojawi.
- **Judge jako scorer** — karta nie nazywa judge'a; jego adapter i reguła
  dopuszczenia z ADR 0030 pozostają osobną pozycją B3.
- **Publikacja producenta bez zatwierdzenia** nadal odpowiada 403
  `evidence_forbidden`, a start runu — 409 `pair_not_admitted` z adresem
  zatwierdzenia. Ujednolicenie zmieniłoby status widoczny dla producentów, więc
  nie zostało zrobione po cichu.
- **Anulowanie** działa ogólnym mechanizmem managed execution; krok jest krótki
  (fold ograniczony limitem przypadków), więc nie ma punktu przerwania w środku.

## 26. Zamknięcie ograniczeń sekcji 25

Pięć pozycji z listy „Co zostaje" sekcji 25, każda jako osobna paczka i osobny
commit: `049ded7` (status), `09dc86b` (scorer ilościowy), `e8ae1d1` (archiwum),
`15222f6` (judge), `e6438d3` (panel). Reguły są w
[ADR 0030](ADR/ADR_0030_EVALUATION_EVIDENCE.md), poprawka „a judge this deployment
asks, and the archive as answers", i w Guardrails `CLAUDE.md`.

### 26.1 Niezatwierdzona para to jedna odpowiedź

Publikacja producenta dla pary bez zatwierdzenia odpowiadała 403
`evidence_forbidden`, a start runu scoringu dla tej samej pary — 409
`pair_not_admitted` z adresem zatwierdzenia. Jeden stan, dwa statusy, a 403
czytało się jak brak roli tokenu. Teraz oba odpowiadają 409 `pair_not_admitted`
z adresem — także wtedy, gdy adapter nie znalazł nic, co dopuszcza parę, bo to
zatwierdzanie jest miejscem, w którym ujawni się powód adaptera. Wycofana para,
bundle zmieniony pod zatwierdzeniem i wywołujący bez prawa do źródła zostają przy
403: żadne z nich nie jest krokiem, który ktoś jeszcze może wykonać. Start pyta
`Registry::admission`, więc wycofana para jest tam 403, a nie „jeszcze nie".
Zmiana statusu widocznego dla producentów jest świadoma; żaden SDK nie ponawia ani
403, ani 409.

### 26.2 Scorer ilościowy ma jednostkę autora

`absolute_error { unit }` mierzy odległość odpowiedzi od oczekiwanej liczby: średnia
zamiast odsetka, mniej znaczy lepiej. Kierunek i agregacja są wyprowadzane jak dla
każdego scorera; jednostka jest jedynym słowem, które autor mówi o metryce, bo
scorer widzi dwie liczby i nigdy to, co liczą. Karta bez jednostki jest odmawiana
przy publikacji; odległość, której nie da się wyrazić skończoną liczbą, nie jest
oceniana. `SCORING_VERSION` zostaje `1`: nowe słowo nie zmienia odpowiedzi żadnego
istniejącego scorera.

### 26.3 Archiwum rozmów jako źródło odpowiedzi

Przypadek kohorty rozmów to tura asystenta, a odpowiedzią na niego jest odpowiedź
tej tury — run może zadeklarować `"answers": "archive"`. Ograniczenie z sekcji 25
było dosłowne: wykonawca czyta kohortę przez archiwum, a treść czyta się tylko z
dostępem do treści, którego nic mu nie dawało. Przyznanie go temu, kto kliknął
start, zrobiłoby z kliknięcia edytora sposób na odczyt archiwum. **Uprawnieniem
jest zatwierdzenie**: wykonawca pyta bramkę, zanim cokolwiek przeczyta, i czyta z
dostępem do treści tylko dla pary dopuszczonej przez admina — którego zatwierdzenie
samo rozwiązało tę treść. Dowód jest pieczętowany szyfrem archiwum, a rejestr, na
którym działał krok, nie zachowuje dostępu.

Przy okazji: `Registry::cohort` rozwiązywał kohortę rozmów dla dowolnego
wywołującego, bo nie sprawdzał bramki treści przed adapterem — publikacja zawsze
to robiła. Teraz robi to tak samo.

Trzy odmowy z nazwą: nagranie dla kohorty rozmów (odpowiedzi na pytania z archiwum
leżałyby jawnie, poza retencją i usuwaniem), archiwum dla innej kohorty, i scorer
czytający oczekiwanie nad archiwum (oczekiwaniem jest mierzona odpowiedź).
Deklaracja wymaga też opublikowanej karty, więc run, który mógłby tylko upaść, jest
odmawiany przed zapisem. Na przewodzie nagranie jest dalej zwykłą referencją
artefaktu, więc wcześniejsze deklaracje zachowują adres.

### 26.4 Judge jako scorer

Adapter dla czterech warunków judge'a z ADR 0030 — dla judge'a, którego pyta sam
aiwatcher; judge producenta dalej nie ma adaptera i jest odmawiany z nazwy.

- **Konfiguracja przypięta treścią.** Scorer `judge` wskazuje wersję rubryki; skala
  i kierunek metryki są rubryki (flaga → `rate`/`ratio`, liczba → średnia `score`,
  poziomy → średnia pozycja od zera). Run deklaruje profil (`openai | llamacpp`),
  model i rewizję oraz ustawienia, których skrót jest
  `context.judge.configuration`.
- **Zbiór kalibracyjny jest częścią dowodu.** `POST /evaluation-calibrations`
  zamraża oceny ludzi (B4) przypadków opublikowanego wyniku pod wskazanymi wersjami
  rubryk, adresowane treścią; kontekst wskazuje go jako dataset nowego rodzaju
  `assessments`. Zbiór bez oceny człowieka pod którąkolwiek rubryką karty jest
  odmawiany, nie domyślny; zbiór z wyniku tego samego runu też.
- **Zgodność z ludźmi obok wyniku.** Run zadaje judge'owi pytania o przypadki
  kohorty i o każdą pozycję zbioru kalibracyjnego, i publikuje per metryka: ile było
  pozycji, na ile judge odpowiedział na skali, odsetek zgodnych — **liczony po
  wszystkich pozycjach**, żeby judge odmawiający trudnych przypadków nie
  „dogadywał się" w górę — i średnią odległość.
- **Wynik oznaczony jako słowo modelu.** `reproducible: false` na dowodzie, a
  porównanie takich wyników niesie `judged: true` jako pole, nie powód.

Judge działa w roli `work`, jako osobny rodzaj runtime'u `judge_evaluation`,
tam gdzie są `AIWATCHER_JUDGE_URL` i `AIWATCHER_JUDGE_PROVIDER` (plus opcjonalnie
`AIWATCHER_JUDGE_TOKEN`, `_CONCURRENCY`, `_TIMEOUT_SECONDS` i wartości `execution.judge`
w chart). Start odmawia 501 `judge_disabled` bez judge'a i 422 z oboma profilami przy
innym — run, którego nikt nie odbierze, czekałby wiecznie. Pytania idą przed foldem,
ograniczoną liczbą naraz; jedna awaria kończy próbę zamiast publikować pół pomiaru.
Judge nie dostaje słów z archiwum ani kalibracji na dowodzie z rozmów.

**Znalezisko z odbioru na żywo**: `gemma-4-e2b` (Q4, llama.cpp) proszona o „true or
false" odpowiadała `"false"` w cudzysłowie. Ścisły odczyt odrzucał wtedy każdy
przypadek, na który model powiedział „nie" — liczyły się tylko jego „tak". Każde
wywołanie niesie teraz skalę jako JSON Schema, na którą dostawca dekoduje; odczyt
zostaje ścisły. Powód nigdy nie cytuje odpowiedzi modelu.

Przy okazji zamknięta luka: `POST /evaluation-results` przyjmował dowód z kontekstem
`aiwatcher.scoring`, więc ktoś z tokenem edytora mógł opublikować liczby pod nazwą
zadeklarowanego runu przed runem — a pierwsza publikacja ID wygrywa. Teraz 403
`measured_here`.

### 26.5 Formularz startu w panelu

Ekran Evaluation ma przycisk **Measure**. Formularz zbiera wybory człowieka: kartę,
opublikowany wynik, którego kohortę i wariant mierzyć, nową nazwę eksperymentu,
ID wyniku i powtórzenia, odpowiedzi (plik nagrania albo — tylko dla kohorty rozmów —
archiwum) i, gdy karta pyta judge'a, profil, model, rewizję, ustawienia oraz zbiór
kalibracyjny brany z ocen ludzi innego wyniku. **Niczego nie wylicza**: metryki,
zatwierdzenie i tożsamość runu wracają z serwera. Deklaracja i run, który uruchomiła,
są w URL (`declaration`, `measured`), więc przeładowanie albo link wysłany adminowi
trafia w tę samą deklarację. Z widoku deklaracji admin stage'uje przypięte pliki, a
manifest dokłada panel — ten z deklaracji, nie przepisywany ręcznie. Odmowa startu
`pair_not_admitted` mówi, kto i co ma zatwierdzić. Panel wyniku judge'a mówi, że to
słowo modelu, i pokazuje zgodność z ludźmi bez kolorowania; porównanie takich
wyników mówi, że różnica zawiera zmienność samego judge'a.

### 26.6 Odbiór

`rtk just check` 23/23 po `15222f6` i po `e6438d3`; po `049ded7`, `09dc86b` i
`e8ae1d1` — testy zmienionych crate'ów, clippy i lint komentarzy. Nowe testy: 3
jednostkowe przy scorerze ilościowym i 1 przy publikacji średniej; 3 jednostkowe
przy źródle odpowiedzi i 1 na prawdziwym archiwum rozmów (zaszyfrowany dowód, bez
jawnych słów w magazynie, odmowa przed zatwierdzeniem bez odczytu); 4 jednostkowe
przy judge'u, 5 przy kliencie i wykonawcy (kolejność odpowiedzi, awaria, profil,
konfiguracja, stranded), 3 integracyjne (zgodność z ludźmi, odmowy kalibracji,
zły profil); 3 HTTP (jeden kod niezatwierdzonej pary, start judge'a w trzech
wdrożeniach i `measured_here`); 2 w SDK Python; 5 w panelu.

Odbiór na żywo, `127.0.0.1:19085` z własnym katalogiem danych i `llama-server` z
`gemma-4-e2b` na `127.0.0.1:19086`, po odbiorze zatrzymane po PID i usunięte:

- rubryka „correct" (flaga), wcześniejszy run bez judge'a, trzy oceny ludzi
  (Warsaw — tak, „5" na 2+2 — nie, Blue — tak), zbiór kalibracyjny: 3 pozycje;
- run z kartą judge'a nad kandydatem (Kraków, „4", odmowa odpowiedzi): deklaracja
  z `calibration_dataset.kind = assessments`, publikacja pod jego nazwą → 403
  `measured_here`, start → krok `judge_evaluation`, 1,5 s;
- dowód: `complete`, 3/3, `correct = 0,33` (tylko „4" poprawne),
  `reproducible: false`, zgodność z ludźmi 3/3, 100%, średnia odległość 0;
  ponowny start → ten sam run; porównanie z wcześniejszym wynikiem →
  `incompatible` („Different suite", „Different judge"), `judged: true`;
- przed poprawką schematu ten sam run: `partial`, 1/3, dwa przypadki z powodem
  „a value of another kind" i zgodność 33% — to znalazło 26.4;
- panel na `:5182` przeciw tej instancji: formularz z kartą judge'a, zbiór
  kalibracyjny wzięty z przycisku (3 oceny), deklaracja z nagraniem wysłanym z
  przeglądarki, start przed zatwierdzeniem → komunikat z adresem, zatwierdzenie z
  panelu z sześcioma przypiętymi plikami, start → link do wykonania, wynik z notą
  judge'a; przeładowanie wraca do tej samej deklaracji i wykonania.

Po odbiorze nic nie nasłuchuje na 8080, 18080, 19085, 19086 ani 5182.

### 26.7 Co zostaje

- **Rewizja modelu judge'a jest deklaracją, nie sprawdzeniem** — dostawca zgodny z
  OpenAI nie mówi, jaką rewizję obsłużył. Stąd `reproducible: false` i zgodność z
  ludźmi obok liczb.
- **Judge widzi to, co wskazuje `answer_path`** (i `expected_path`); źródło nie daje
  wejścia przypadku, więc nagranie, które chce pokazać pytanie, umieszcza je w
  odpowiedzi — tak zrobił odbiór.
- **Poziomy uśredniane są jako pozycje**, co zakłada równe odstępy; próg zbioru
  kalibracyjnego to co najmniej jedna ocena na rubrykę, a nie liczność, która
  cokolwiek dowodzi — liczba pozycji stoi obok zgodności.
- **Ponowienie próby judge'a pyta od nowa o wszystko** — koszt jest zapisany, a
  pytań nie pamięta się między próbami.
- **Archiwum jako odpowiedzi to odpowiedzi samego korpusu kohorty**; odpowiedzi
  nowej wersji aplikacji zapisane w innym korpusie nie są dopasowywane do
  przypadków, a trace nie jest przenoszony. Judge nad archiwum jest odmawiany.
- **Panel**: kohorta i wariant pochodzą wyłącznie z opublikowanego wyniku; karty i
  rubryki dalej tylko przez API; lista wyników nie odświeża się sama w trakcie runu
  (odświeża ją „Open the result").
- Dalej otwarte na AW-6: **C1** — szablon `generate_and_score`; z B3 — kontekst
  wariantu w obserwacjach.

## 27. Poprawki ograniczeń sekcji 26

Przegląd listy „Co zostaje" z 26.7 w kodzie: co da się poprawić bez nowej decyzji,
co jest decyzją, a co należy do C1. Pięć paczek, każda osobnym commitem: `063b567`
(zapamiętane odpowiedzi judge'a), `704d776` (co obsłużył dostawca), `0f182b6`
(judge widzi wejście przypadku), `affb020` (próg poziomu i przedział zgodności),
`5ada6cc` (panel śledzi uruchomiony run). Reguły są w
[ADR 0030](ADR/ADR_0030_EVALUATION_EVIDENCE.md), poprawka „what a judge is shown,
what it said, and what that proves", i w Guardrails `CLAUDE.md`. Żadna karta,
kontekst ani deklaracja sprzed tych zmian nie zmienia adresu, a `SCORING_VERSION`
zostaje `1`: nowe pola są opcjonalne w karcie albo dopisane do raportu.

### 27.1 Ponowienie próby judge'a nie pyta drugi raz — i nie kończy się konfliktem

26.7 opisywało to jako koszt: ponowiona próba pyta od nowa o wszystko. Przegląd
kodu pokazał, że to także błąd poprawności. Próba, która opublikowała wynik i
straciła rozliczenie (awaria między publikacją a raportem do reaktora), wracała,
pytała model ponownie, dostawała inne odpowiedzi, fold dawał inne bajty — a
pierwsza publikacja tego ID odmawiała jej jako `Conflict`, mapowanego na
`user_code`. Run kończył się porażką obok wyniku, który sam opublikował.

Teraz każda odpowiedź jest zapisywana przed użyciem pod
`evaluation-judges/replies/{deklaracja}/{skrót pytania}.json` (skrót z całego
wywołania, wygrywa pierwszy zapis), a wykonawca pyta przez `Remembering`, które
najpierw czyta zapis. Próba po awarii pyta tylko o to, na co nikt nie odpowiedział;
próba po publikacji składa te same bajty i trafia w istniejący wynik. Zapisy żyją
jak nagranie i deklaracja — bez retencji. Test integracyjny bez tej zmiany upada.

### 27.2 Co obsłużył dostawca

Rewizja modelu dalej jest deklaracją autora, bo nic jej nie sprawdzi. Każda
odpowiedź zachowuje jednak słowa dostawcy — pole `model` i `system_fingerprint`
odpowiedzi — a raport liczy je w `served` (w stałej kolejności, bo raport jest
częścią adresu wyniku). Nic nie jest porównywane z deklaracją: dostawca nazywa model
aliasem, plikiem albo datowanym snapshotem, więc odmowa po pisowni odmawiałaby
uczciwym. Dwa wiersze mówią, że odpowiedzi runu przyszły z dwóch backendów. Panel
pokazuje to w nocie judge'a.

### 27.3 Judge widzi pytanie

Scorer `judge` może wskazać `input_path` — JSON Pointer w wejście przypadku (pusty
to całe wejście). Wejście idzie przed odpowiedzią. Adapter źródła oddaje wejścia
obok oczekiwań (`SourceEvidence::inputs`: lokalne źródło z `cases.json`, adnotacje
z obrazu, rozmowy — nic), a do shardów wyniku nie trafiają. Pozycja kalibracyjna
dostaje wejście ze źródła swojego wyniku, rozwiązywanego tylko wtedy, gdy karta
czegoś pokazuje. Przypadek bez wejścia pod ścieżką upada z nazwą ścieżki; pozycja
kalibracyjna bez niego nie jest zadawana i liczy się przeciw zgodności, tak jak
odmowa odpowiedzi. Karta bez `input_path` wysyła dokładnie to, co wcześniej.

### 27.4 Próg poziomu i przedział zgodności

Na rubryce z poziomami karta może wskazać `pass_level`: metryka to odsetek
przypadków na tym poziomie albo po lepszej stronie według kierunku rubryki (`rate`,
`ratio`), zamiast średniej pozycji zakładającej równe odstępy. Odpowiedź judge'a i
ocena człowieka przechodzą przez to samo odwzorowanie, więc zgodność dotyczy
liczby, którą wynik publikuje — judge i człowiek różniący się o poziom, obaj ponad
progiem, są zgodni. Próg na fladze, liczbie, nieznanym poziomie albo rubryce bez
kierunku jest odmawiany przy publikacji karty.

Progu liczności zbioru kalibracyjnego nadal nie ma i nie powinno go wymyślać to
repozytorium. Zgodność niesie za to 95% przedział Wilsona (`agreement_interval`):
3 z 3 to 100% z dolną granicą 44%, 100 ze 100 — powyżej 96%. Panel pokazuje
przedział obok odsetka.

### 27.5 Panel śledzi run

Formularz Measure pokazuje uruchomiony run wspólną kartą `ManagedRunCard` —
stan, krok, próby i komendy, na które pozwala serwer — i odświeża katalog wyników
przy każdej zmianie runu, bez listy stanów końcowych w TypeScripcie. Run, którego
już nie ma, można zapomnieć z URL. Opis tego, co judge zobaczy z pytania, jest w
sekcji Judge formularza.

### 27.6 Odbiór

`rtk just check` 23/23 po `5ada6cc`, czyli po wszystkich pięciu paczkach. Nowe
testy:

- 1 integracyjny przy zapamiętanych odpowiedziach — bez tej zmiany upada;
- 2 jednostkowe przy `served` i nazwie dostawcy, plus asercja w istniejącym teście
  integracyjnym;
- 1 jednostkowy przy `input_path`, 2 integracyjne (pytanie pokazane; brak wejścia
  pod ścieżką), asercje w teście promptu judge'a i w teście lokalnego źródła;
- 3 jednostkowe (próg w definicji metryki, przedział Wilsona, odwzorowanie progu
  dla obu kierunków) i 1 integracyjny (próg z kalibracją na poziomach);
- 2 w panelu: wybór wejścia w formularzu i śledzenie runu — ten drugi bez
  odświeżania katalogu upada. Nota judge'a ma asercje na `served` i przedział.

Odbiór na żywo na `127.0.0.1:19085` z własnym katalogiem danych i `llama-server`
z `gemma-4-e2b` (Q4, `--alias gemma-4-e2b`) na `127.0.0.1:19086`, po odbiorze
zatrzymane po PID i usunięte:

- dwie rubryki: „correct" (flaga) i „quality" (`wrong`, `partial`, `right`);
  wcześniejszy wynik z odpowiedziami **bez pytań** (`Warsaw`, `5`, `Blue`),
  6 ocen ludzi, zbiór kalibracyjny z 6 pozycjami;
- karta z dwoma judge'ami, oba z `input_path: /question`, drugi z
  `pass_level: partial`; próg na fladze → 400 z nazwą pola;
- kandydat (`Kraków`, `4`, pusty tekst) → krok `judge_evaluation`, 3,6 s,
  12 pytań; dowód `complete`, `correct = good_enough = 0,67` — model ocenił
  pusty tekst jako poprawną odpowiedź na „Return an empty string.", czego bez
  pytania nie miałby z czego wywnioskować;
- zgodność z ludźmi 3/3 dla obu metryk, przedział 44–100%; `served`:
  `gemma-4-e2b`, `b10809-5266f24da`, 12 odpowiedzi; 12 zapamiętanych odpowiedzi
  pod deklaracją;
- awaria dostawcy: `llama-server` zatrzymany, start drugiego runu → próba 1
  `Transient`, krok `AwaitingRetry`; po ponownym starcie modelu próba 2
  opublikowała te same liczby i te same 12 odpowiedzi;
- panel na `:5182`: karta runu w formularzu (completed, `judge_evaluation`,
  attempt 2), nota judge'a z przedziałem i z tym, co obsłużył dostawca, oraz w
  sekcji Judge: „correct sees the case's input at /question".

**Znalezisko przy okazji, niezmienione.** Pierwsza wersja skryptu awarii
przestage'owała `manifest.json` drugiej deklaracji pod już zatwierdzoną parą.
Manifest różni się tylko `origin`, ale skrót bundle'a obejmuje jego bajty: para
przestała się czytać (403 także dla już opublikowanego wyniku), ponowne
zatwierdzenie odmówiło z mylącym „evaluation ID already belongs to a different
result", a run po awarii upadł przy publikacji. Przywrócenie pierwotnych bajtów
przywróciło odczyt. To reguła z B2 („bajty, które przyjdą po zatwierdzeniu,
zatrzymują parę") i panel na nią nie trafia, bo dla dopuszczonej pary nie
proponuje stage'owania — ale przez API łatwo w nią wejść.

### 27.7 Co zostaje

- **Rewizja modelu dalej nie jest sprawdzana.** `served` to słowa dostawcy, nie
  dowód: llama.cpp zwraca alias podany operatorowi w `--alias`, a hosted API —
  datowany snapshot.
- **Judge nad archiwum jest dalej odmawiany.** Dopuszczenie judge'a działającego w
  granicy wdrożenia (np. llama.cpp w klastrze) byłoby deklaracją operatora, której
  kod nie sprawdzi; to decyzja do ADR 0021 i 0030, nie poprawka.
- **Archiwum jako odpowiedzi to dalej odpowiedzi korpusu kohorty.** Zmierzenie
  nowej wersji aplikacji na tych samych turach to generowanie odpowiedzi — C1.
- **Zapamiętane odpowiedzi nie mają retencji**, tak jak nagrania i deklaracje.
- **Pozycja kalibracyjna bez wejścia liczy się przeciw zgodności**, a nie jest
  odmawiana przy deklaracji — nic nie wie o wejściach przed rozwiązaniem źródła.
- **Próg jest tylko dla poziomów**; dla skali liczbowej zostaje średnia.
- **Bundle pod zatwierdzeniem obejmuje `origin` manifestu** (27.6), a odmowa
  ponownego zatwierdzenia ma komunikat konfliktu ID.
- **Panel**: karty i rubryki dalej tylko przez API; kohorta i wariant tylko z
  opublikowanego wyniku — plik przypadków i schematy są w bundle'u operatora.
- Dalej otwarte na AW-6: **C1** — szablon `generate_and_score`; z B3 — kontekst
  wariantu w obserwacjach.

## 28. Judge nad archiwum z ostrzeżeniem i skrót bundle'a bez `origin`

Dwie decyzje użytkownika z 27.7: poprawić znalezisko z bundle'em „bardziej
logiczną opcją" i dopuścić judge'a nad archiwum rozmów z ostrzeżeniem. Trzy
commity: `c4b3ea1` (skrót bundle'a), `ca43704` (zapamiętane odpowiedzi bez słów),
`ee7db79` (judge nad archiwum). Reguły są w
[ADR 0030](ADR/ADR_0030_EVALUATION_EVIDENCE.md), poprawka „a judge over the archive,
told to everyone, and what a bundle admits", i w Guardrails `CLAUDE.md`.

### 28.1 Zatwierdzenie dopuszcza to, co bundle dodaje

Skrót zapisany w zatwierdzeniu obejmował bajty `manifest.json`, a `origin`
manifestu nazywa run. Drugi run tej samej pary zmieniał więc skrót, ukrywał
wszystkie jej wyniki (403) i był odmawiany przy ponownym zatwierdzeniu komunikatem
o konflikcie dwóch wyników pod jednym ID. Bardziej logiczna z dwóch opcji to
poprawić sam skrót, nie komunikat: para jest już adresem zatwierdzenia, a
`ApprovalRecord::bundle_digest` od początku opisywał „to, co adapter sprawdził ponad
przypięte skróty manifestu". Teraz `aiwatcher_evaluation::bundle_digest` liczy się z
treści tego, co bundle dodaje — dziś pakietu modelu — i jest `None`, gdy nie dodaje
nic; format pliku też nie ma znaczenia.

Zatwierdzenia zapisane wcześniej działają dalej: adapter oddaje też
`earlier_bundle_digest` (stary skrót całej deklaracji), a rejestr przyjmuje
zgodność z którymkolwiek. Rekord nie jest przepisywany, więc pod starym
zatwierdzeniem ponowne wgranie innego manifestu dalej zatrzymuje parę — ale
odmowa ponownego zatwierdzenia to teraz 409 `admitted_other_bytes` z nazwą
zatwierdzenia i zdaniem „stage the bytes it admitted".

### 28.2 Zapamiętana odpowiedź nie zawiera słów

Warunek wstępny dopuszczenia archiwum. Odpowiedzi judge'a leżały pod deklaracją
jawnie, bez retencji i usuwania, a model potrafi powtórzyć to, co mu pokazano.
`JudgeReply::kept` zapisuje odpowiedź kanoniczną, którą `read` czyta dokładnie tak
jak oryginał: wartość logiczną albo liczbę bez zmian, poziom tylko wtedy, gdy
pytanie go oferowało, a w miejsce czegokolwiek innego zastępnik odmawiany z tym
samym powodem. Test porównuje oba odczyty na trzech skalach i czternastu
odpowiedziach, w tym z sekretem w treści.

### 28.3 Judge nad archiwum, z ostrzeżeniem dla każdego, kto mógłby to zatrzymać

Obie odmowy zniknęły: judge nad kohortą rozmów i zbiór kalibracyjny z dowodu z
rozmów. Koszt jest ten sam co wcześniej i jest nazwany — dostawca trzyma to, co
dostał, poza szyfrowaniem, retencją i usuwaniem archiwum. Ostrzeżenie nie jest
jednym komunikatem, tylko faktem widocznym na każdym etapie:

- **w kontekście**: `context.judge.reads_archive` jest wyprowadzane z rodzaju
  kohorty i z `from_archive` zbioru kalibracyjnego. Jest częścią `context_id`, więc
  admin zatwierdzający parę zatwierdza także to. Ręcznie napisany kontekst, który
  twierdzi inaczej, dostaje 400;
- **w deklaracji**: `ScoringRunView.warnings` mówi słowami, co dokładnie jest
  wysyłane (odpowiedzi asystenta; co powiedział człowiek, gdy karta pokazuje
  wejście; oceny ludzi ze zbioru kalibracyjnego) i do jakiego profilu i modelu.
  Zdanie jest pisane raz, na serwerze;
- **w panelu**: formularz Measure pokazuje ostrzeżenie serwera z plakietką „judge
  reads the archive”. „Stage and admit” i „Start” czekają na zaznaczenie „I
  understand what this judge is sent”. Panel Approvals czyta `reads_archive` z
  wybranego `manifest.json` i też czeka na potwierdzenie. Nota judge'a na wyniku
  mówi to samo, póki wynik jest trzymany;
- **w logu**: wykonawca loguje ostrzeżenie przy starcie takiego kroku.

Deklarowanie nic nie wysyła, dlatego potwierdzenie jest przy zatwierdzeniu i
starcie, a nie przy deklaracji. Zbiór kalibracyjny z dowodu z rozmów bierze admin
(trasa daje dostęp do treści tylko jemu; edytor dostaje 403). Adapter rozmów oddaje
teraz wejście przypadku (`{"question": prompt}`) dla karty z `input_path`. Wykonawca
czyta z dostępem do treści także wtedy, gdy archiwum jest tylko w zbiorze
kalibracyjnym — pod tym samym zatwierdzeniem pary, której kontekst to mówi.

### 28.4 Odbiór

`rtk just check` 23/23 po `ee7db79`. Nowe testy:

- 2 integracyjne przy bundle'u: drugi manifest pary nie ukrywa wyniku, a stare
  zatwierdzenie działa do zmiany bajtów i potem dostaje 409 z nazwą. Pierwszy z
  nich upada przy starym skrócie;
- 1 jednostkowy przy zapamiętanych odpowiedziach (równy odczyt, brak słów);
- 1 jednostkowy przy kontekście i ostrzeżeniu judge'a nad archiwum;
- 1 integracyjny na prawdziwym archiwum: kalibracja z dowodu z rozmów (edytor 403,
  admin `from_archive`), ostrzeżenie, odmowa ręcznego kontekstu, run z judge'em,
  słowa dotarły do modelu, w magazynie ewaluacji brak jawnych słów;
- 3 w panelu: blokada w Measure, blokada w Approvals, zdanie w nocie.

Odbiór na żywo: `127.0.0.1:19085` z włączonym archiwum i własnym kluczem,
`llama-server` z `gemma-4-e2b` na `127.0.0.1:19086`; po odbiorze zatrzymane po PID i
usunięte.

- przez API: dwie wymiany z markerem `ZEBRA7`, recenzja, eksport
  `prompt_response`, korpus z 2 wierszami; opublikowany wynik z rozmów i oceny
  ludzi; zbiór kalibracyjny `from_archive: true`;
- deklaracja z `"answers": "archive"` i judge'em z `input_path`:
  `reads_archive: true` i jedno ostrzeżenie wymieniające wszystkie trzy rodzaje
  wysyłanych słów; kontekst z `reads_archive: false` → 400;
- run → `completed`, dowód `complete`, 2/2, `correct = 0,5` (Warsaw tak, „five”
  nie), zgodność 2/2 z przedziałem 34–100%, `served`: `gemma-4-e2b`, 4 odpowiedzi;
  log wykonawcy z ostrzeżeniem;
- `grep` całego katalogu danych po markerze: zero plików, archiwum też jest
  zaszyfrowane. Zapamiętane odpowiedzi: 2, bo pozycje kalibracyjne to te same tury
  co przypadki, więc pytania są identyczne;
- skrót bundle'a: po wgraniu manifestu drugiego runu wynik dalej `complete`, a
  ponowne zatwierdzenie → 200 z tym samym ID;
- panel na `:5182`: ostrzeżenie serwera, plakietka, Start zablokowany do
  zaznaczenia i odblokowany po nim, zdanie w nocie judge'a.

### 28.5 Co zostaje

- **Dostawca dostaje słowa archiwum na stałe.** Usunięcie podmiotu w archiwum nie
  sięga do dostawcy — to jest treść ostrzeżenia, nie brak w kodzie.
- **Potwierdzenie w panelu nie jest zapisywane.** Zatwierdzenie przez admina
  kontekstu z `reads_archive` jest trwałym aktem; kto zaznaczył pole przy starcie,
  nie jest zapisywane osobno.
- **Stare zatwierdzenia dalej obejmują bajty manifestu**: nowy skrót dostaje tylko
  zatwierdzenie zapisane od `c4b3ea1`.
- Z 27.7 bez zmian: rewizja modelu niesprawdzana, próg tylko dla poziomów,
  zapamiętane odpowiedzi bez retencji (teraz bez słów), karty i rubryki tylko przez
  API.
- Dalej otwarte na AW-6: **C1** — szablon `generate_and_score`; z B3 — kontekst
  wariantu w obserwacjach.

## 29. Domknięcie luk C0 i scorery frameworków za jednym kontraktem

Cztery luki C0 wobec punktów 1–3 etapu C, wskazane przy pytaniu „co z C zostaje":
anulowanie nie zatrzymywało kroku, formularz nie miał limitu przypadków, timeoutu
ani współbieżności, kohorta pochodziła tylko z opublikowanego wyniku, a DeepEval
był tylko czytelnikiem gotowych raportów. Użytkownik dodał wymaganie, żeby adapter
był zaprojektowany pod inne frameworki, np. Opik. Cztery commity: `deef7ca`
(zatrzymanie kroku), `60d55c1` (ustawienia runu), `5d5e3dd` (kohorta z wersji
datasetu), `154f8e7` (scorery frameworków). Reguły są w
[ADR 0030](ADR/ADR_0030_EVALUATION_EVIDENCE.md), poprawka z 2026-09-13, i w
Guardrails `CLAUDE.md`.

### 29.1 Anulowanie i timeout zatrzymują krok

Anulowanie było kooperacyjne tylko dla podów: `ActivityExecutor::cancel` istniał,
ale reaktor nigdy go nie wołał, więc run z krokiem w procesie stał w `Cancelling`,
aż krok skończył sam — w runie z judge'em to godzina pytań, których nikt już nie
chciał. `timeout_seconds` egzekwowały tylko wykonawcy przekazujący go klientowi
HTTP; scoring i judge nie miały żadnego terminu.

Teraz `ActivityContext.stop` (`StopSignal`) jest sygnałem, a reaktor obserwuje
każdą próbę, którą wykonuje (`Watch`): co 2 s czyta projekcję runu i własny zegar.
Run anulowany albo zakończony wokół kroku, albo miniony termin, ustawia sygnał,
woła `cancel` wykonawcy i daje 30 s łaski; wykonawca, który nie wrócił, jest
porzucany. Anulowanie kończy się klasą `Policy` (nic jej nie ponawia), timeout
klasą `Timeout` z własnym budżetem. `timeout_seconds == 0` znaczy brak terminu,
jak przy bramce człowieka. Krok scoringu patrzy na sygnał między kawałkami pracy,
pytania do judge'a i do serwisu scorerów porzuca w locie, a przed publikacją
patrzy ostatni raz. Worker HTTP słyszy to przy najbliższym heartbeacie: trasa
rozlicza próbę jako zatrzymaną i odpowiada 409 `execution_stopping`, więc run
kończy się bez czekania na wygaśnięcie leasingu.

### 29.2 Ustawienia runu: timeout i współbieżność

`ScoringRun.settings` niesie `timeout_seconds` (od minuty do doby) i `concurrency`
(1–64, tylko dla runu, który pyta judge'a albo serwis scorerów). Ustawienia są
częścią deklaracji — ponowienie czyta termin pierwszej próby — i nigdy manifestu,
więc run z innym tempem mierzy to samo i jest porównywalny. Puste ustawienia nie
zmieniają adresu deklaracji. Plan bierze termin z deklaracji albo domyślny dla
rodzaju kroku. Start z tempem większym niż `AIWATCHER_JUDGE_CONCURRENCY` albo
`AIWATCHER_SCORER_CONCURRENCY` jest odmawiany 422 z nazwą zmiennej, a nie po cichu
obniżany. Formularz Measure ma sekcję „How it runs".

### 29.3 Kohorta z wersji datasetu i splitu, z limitem

Adapter źródeł już wyprowadzał przypadki od właściciela przy każdym zatwierdzeniu
pary — trzy pliki kohorty były drugą kopią tej odpowiedzi, pisaną ręcznie albo
kopiowaną z wyniku. `POST /api/v1/evaluation-cohorts` wyprowadza je jako bajty
kanoniczne:

- **curation**: wszystkie wiersze wersji; split jest tylko nazwą kohorty;
- **annotations**: obrazy splitu eksportu (`train | validation | test`);
- **conversations**: tury korpusu na `test`, jako skróty pytań i odpowiedzi — bez
  słów; bierze admin, bo czytanie wierszy to treść;
- **external**: odmowa — przypadki są producenta.

`limit` bierze pierwsze przypadki właściciela w jego kolejności, nie próbkę.
Kohorta części przypadków jest osobną kohortą, osobnym kontekstem i wynikiem
nieporównywalnym z wynikiem na całości — co czyni przebieg dymny uczciwym.
Rekord wyprowadzenia jest zapisywany pod skrótem przypadków, a widok deklaracji
mówi „the first 2 of 3 cases". Nic nie jest wgrywane: przy zatwierdzeniu brakujący
w bundle'u plik kohorty adapter wyprowadza ponownie i trzyma do przypiętego skrótu;
wgrany plik niezgodny ze skrótem jest odmawiany jak wcześniej. Kontrole właściciela
przyjmują teraz prefiks jego przypadków o zadeklarowanej długości. Formularz
Measure ma sekcję „Cohort": własna kohorta wyniku albo wersja datasetu (listy
datasetów, eksportów i korpusów z API) oraz „First cases"; limit na własnej
kohorcie wyniku wyprowadza z jej datasetu i splitu. Panel admina mówi, że trzy
pliki kohorty nie są do przyniesienia.

### 29.4 Scorery frameworków: DeepEval, Opik i każdy adapter za jednym kontraktem

Projekt, który ma utrzymać kolejne frameworki bez zmian w aiwatcher:

- **Framework działa w serwisie, nie w karcie.** `services/scorers` to opcjonalny
  serwis Pythona (jak `ml_pipeline`) z dwiema trasami: `GET /scorers/catalog` —
  każdy adapter z zainstalowanym wydaniem, model metryk ocenianych modelem i każda
  metryka z jednostką, kierunkiem, agregacją, zakresem, stronami przypadku, które
  czyta (`input | answer | expected`), i parametrami; `POST /scorers/score` — jedna
  metryka na liście przypadków, odpowiedzi w tej samej kolejności: liczba albo
  zdanie adaptera. Rust (`aiwatcher_evaluation::external`) nie zna żadnej nazwy
  frameworka; fixture'y `contracts/fixtures/scorers-v1/` czytają oba zestawy testów.
- **Adapter to cztery elementy.** `name`, `version`, `model` i tabela `metrics()`
  z `Implemented(Metric, score)` — nazwa metryki trafia do słownika, nigdy do
  wywołania nazwanego przez żądanie. Framework jest importowany tylko w adapterze i
  jest extra w `pyproject.toml`; brak extra to adapter pominięty w katalogu, nie
  błąd startu. Nowy framework (Ragas, promptfoo, lm-eval) to jeden moduł i jedna
  linia w `adapters.KNOWN`.
- **Kierunek nie należy do autora.** Work role zapisuje katalog w rejestrze (co
  5 min), serve role przypina go przy publikacji karty: `Scorer::External` dostaje
  `declared` — wydanie, model, jednostkę, kierunek, agregację, zakres. Kolejny
  katalog nie zmienia opublikowanej karty; aktualizacja frameworka to ponowna
  publikacja karty, czyli nowa wersja suite i wynik nieporównywalny ze starym.
- **Serwis jest trzymany do karty.** Krok `external_evaluation` (w work role, gdzie
  jest `AIWATCHER_SCORER_URL`, z judge'em obok, gdy karta pyta też judge'a) czyta
  katalog na żywo przed zadaniem pytania i kończy run błędem `user_code` z nazwą
  obu wydań/modeli; serwis odmawia tego samego żądania 409.
- **Liczba bez słów.** Kontrakt nie ma pola na `reason` frameworka, przypadek z
  wyjątkiem jest opisany klasą wyjątku, a zapamiętane odpowiedzi (pod deklaracją i
  pytaniem, jak u judge'a) trzymają tylko liczbę albo porażkę. Metryka `rate`
  musi odpowiedzieć 0 albo 1, a wartość poza zadeklarowanym zakresem nie jest
  liczbą.
- **Metryka oceniana modelem mówi to wprost.** `MetricDefinition.measured_by`
  (adapter z wydaniem, metryka, model) jest w kontekście; wynik ma
  `reproducible: false`; deklaracja ostrzega, że nikt nie zmierzył zgodności tego
  modelu z ludźmi, a panel wymaga potwierdzenia przed zatwierdzeniem i startem.
  Nad kohortą rozmów drugie ostrzeżenie mówi, że słowa archiwum idą do serwisu i
  dalej do dostawcy modelu. Nota wyniku w panelu wymienia, który framework i model
  zmierzył każdą metrykę.
- **Serwis nie wysyła niczego nigdzie poza modelem.** `adapters.quiet()` wyłącza
  przed importem telemetrię DeepEval, śledzenie Opik i pobieranie cennika LiteLLM;
  każda metryka Opik ma `track=False`.

Adaptery na start: **DeepEval 4.2** — `exact_match`, `pattern_match` oraz oceniane
modelem `answer_relevancy`, `bias`, `toxicity`, `g_eval`, `g_eval_with_expected`
(przez `LocalModel`); **Opik 2.2** — `equals`, `contains`, `regex_match`, `is_json`,
`levenshtein_ratio` oraz oceniane modelem `answer_relevance`, `hallucination`,
`moderation`, `usefulness`, `g_eval` (przez `LiteLLMChatModel`). Jeden model zgodny z
OpenAI dla wszystkich adapterów (`AIWATCHER_SCORERS_MODEL_*`, profil `llamacpp`
wyłącza myślenie). Start runu na wdrożeniu bez serwisu → 501 `scorers_disabled`.
`just scorers-install | scorers-serve | scorers-check`, osobne zadanie CI.

### 29.5 Odbiór

`rtk just check` 23/23 po commitach dokumentacji; `just scorers-check` zielony
(ruff, mypy strict, 20 testów z zainstalowanymi DeepEval i Opik). Nowe testy:

- 4 w reaktorze (czas wirtualny): anulowanie zatrzymuje krok w jednym–dwóch
  spojrzeniach, wykonawca ignorujący sygnał jest porzucany po łasce, timeout
  kończy się `Timeout` z drugą próbą, krok bez terminu trwa, ile trzeba;
  HTTP: anulowanie dociera do workera przy heartbeacie; scoring: zatrzymany run
  nic nie publikuje; judge i serwis: stop porzuca pytania w locie;
- 2 jednostkowe i 1 HTTP przy ustawieniach (adres deklaracji, granice, 422 z nazwą
  zmiennej) i 1 przy planie;
- 4 integracyjne przy kohortach (curation: wyprowadzenie, pierwsza derywacja
  zostaje, wgrany niezgodny plik odmówiony, bez plików → zatwierdzone i
  zmierzone 2 z 3; odmowa dla external; annotations: pierwszy obraz splitu;
  conversations: edytor 403, admin, rekord bez słów, run tylko na pierwszej
  turze), 1 HTTP przy rolach, 3 w panelu, 1 w SDK;
- Rust przy frameworkach: 3 jednostkowe przy katalogu i odpowiedziach, 1 przy
  ostrzeżeniach nad archiwum, 3 integracyjne (przypięcie przy publikacji, run z
  zapamiętaniem, dryf wydania razem z porażką przypadku zamiast zera), 1 HTTP (404
  bez katalogu, 501, 422, `external_evaluation`), 3 przy kliencie (kontrakt z
  fixture'ów, stop, zdanie odmowy); Python: kontrakt z tymi samymi fixture'ami, zasady serwisu, oba adaptery
  na prawdziwych frameworkach; panel: nota frameworka.

Odbiór na żywo: aiwatcher na `127.0.0.1:19085` (WAL, własny katalog danych),
`llama-server` z `gemma-4-e2b` na `:19086`, serwis scorerów z oboma adapterami na
`:19087`, panel na `:5182`; po odbiorze zatrzymane po PID, `launch.json`
przywrócony, na `:8080` i `:18080` nic nie nasłuchiwało przed i po.

- serwis wprost na gemmie: `answer_relevancy` DeepEval 1,0 / 0,0 i
  `answer_relevance` Opik 1,0 / 0,05 dla „Warsaw" i „I like turtles.";
  `hallucination` 0,0 / 1,0; heurystyki w ułamku sekundy;
- work role zapisał katalog w chwilę po starcie; `GET /api/v1/evaluation-scorers`;
- dataset curation z 3 przypadkami → kohorta `limit: 2` → „2 of 3" z URI
  `aiwatcher://evaluation-cohorts/…`; karta z `opik.equals`, `opik.levenshtein_ratio`
  i `deepeval.answer_relevancy` przypięta do 2.2.59 / 4.2.2 + gemma; jedno
  ostrzeżenie o metryce ocenianej modelem;
- zatwierdzenie bez `cases.json` i schematów → 200; start → `external_evaluation`,
  `completed` w 3,3 s; dowód `complete`, 2/2, `exact` 0,5, `close` 0,5, `relevancy`
  0,5, `reproducible: false`; zapamiętane odpowiedzi to same liczby;
- drugi run anulowany po 1,1 s → `cancelled` sekundę później, próba z `policy`
  „stopped: the execution is no longer running", log reaktora „asking a running
  attempt to stop"; zapamiętane 3 z 6 odpowiedzi;
- serwis zrestartowany z inną rewizją modelu → run tej karty `failed` z
  `user_code` nazywającym obie rewizje (komunikat poprawiony po odbiorze, żeby
  nazywał też model przypięty);
- panel: nota wyniku wymienia framework i model przy każdej metryce; sekcje
  „Cohort" i „How it runs" z listą wersji datasetu. Odbiór znalazł jedną lukę —
  pole współbieżności pokazywało się tylko dla karty z judge'em — poprawioną przed
  commitem.

### 29.6 Co zostaje

- **Metryki frameworków oceniane modelem nie mają kalibracji.** Ostrzeżenie mówi to
  wprost; zgodność z ludźmi (np. próg metryki wobec `pass_level` rubryki) to
  dodatek za tą samą kartą.
- **Porzucony wykonawca HTTP nie zatrzymuje runtime'u.** Flow i notebooki nie mają
  trasy anulowania, więc `cancel` jest pusty, a zapytanie w toku działa u nich do
  końca; run kończy się mimo to. Worker HTTP słyszy anulowanie dopiero przy
  heartbeacie (co połowę leasingu).
- **Termin jest teraz egzekwowany dla każdego kroku w procesie**, także dla
  publikacji datasetu (120 s) — krok, który po cichu trwał dłużej, dostanie
  `Timeout`.
- **Limit to pierwsze przypadki, nie próbka**; split curation to tylko nazwa.
  Zatwierdzenie kohorty wyprowadzonej czyta właściciela dwa razy.
- **Katalog jest odświeżany co 5 minut**: karta opublikowana tuż po aktualizacji
  serwisu może przypiąć stary katalog; run powie to i trzeba opublikować kartę
  ponownie. Parametry metryk są sprawdzane tylko co do rodzaju.
- **Serwis scorerów jest bez uwierzytelnienia i bez manifestów wdrożenia**
  (lokalny, jak `ml_pipeline`); jedno żądanie na przypadek, choć kontrakt przyjmuje
  64. Karty z metrykami frameworków tylko przez API; katalog nie jest pokazany w
  panelu.
- Z odbioru etapu C dalej otwarte: **C1** `generate_and_score` (baseline i kandydat
  przez cały proces z generowaniem), **C2** Experiments, **C3** bramka CI, **C4**
  feedback → przypadek testowy, test śmierci workera i Joba na klastrze dla kroku
  podowego; z B3 — kontekst wariantu w obserwacjach.

## 30. Ograniczenia sekcji 29 i C1 — generowanie i ocena

Użytkownik wskazał cztery punkty z 29.6 do zrobienia i C1: kalibrację metryk
frameworków ocenianych modelem, porzucony krok Flow lub notebooka działający dalej
w runtime, termin egzekwowany teraz dla publikacji datasetu, oraz serwis scorerów
bez uwierzytelnienia i manifestów, z kartami frameworków tylko przez API. Osiem
commitów: `6c81337` (zapis w toku i termin publikacji), `ff6f1e0` (kalibracja
metryk frameworków), `2ce5ae1` (trasy anulowania w runtime'ach), `81de54d`
(token, obraz i chart serwisu scorerów), `5e1d9e6` (edytor kart w panelu),
`4b72041` (formatowanie), `ce46b58` (C1), `e90c974` (drobne poprawki po odbiorze).
Reguły są w [ADR 0030](ADR/ADR_0030_EVALUATION_EVIDENCE.md) (poprawka „later"
z 2026-09-13), [ADR 0025](ADR/ADR_0025_MANAGED_EXECUTION.md) i
[ADR 0028](ADR/ADR_0028_QUERY_ENGINES.md) (poprawki z 2026-09-13) oraz w Guardrails
`CLAUDE.md`.

### 30.1 Zapis w toku kończy się, a publikacja ma czas zapytania

Od sekcji 29 reaktor egzekwuje termin każdego kroku, więc wersja datasetu pisana
dłużej niż dwie minuty byłaby porzucana po łasce i pisana od nowa przez
ponowienie. Wykonawca po ostatnim spojrzeniu na sygnał bierze
`StopSignal::committing()` — strażnik `Committing` — na czas zapisu, który musi
się skończyć, a reaktor czeka na niego dowolnie długo po łasce i daje świeżą łaskę
po jego końcu. Biorą go krok publikacji datasetu i krok scoringu wokół publikacji
wyniku. Stop, który przyszedł przed `committing()`, jest odmową i nic nie zaczyna.
Termin kroku publikacji podąża za skonfigurowanym `query_timeout_seconds`, z 120 s
jako podłogą, bo czytane wiersze są wynikiem tego zapytania.

### 30.2 Anulowanie dociera do runtime'u

Kontrakt silników zapytań i runtime notebooków dostał siódmą, odpowiednio ósmą
trasę: `POST …/executions/{key}/cancel`. Odpowiada tylko dla klucza, który serwis
wykonuje (`{"state": "cancelling"}`); dla innego tak jak lookup i nic nie zostawia.

- **DataFusion i DuckDB**: sandbox trzyma proces dziecka pod kluczem i go zabija;
  prośba przed startem jest zapamiętana (15 min), więc zapytanie w kolejce nie
  rusza; odpowiedź 409 „The query was cancelled", a dziecko, które odpowiedziało
  w chwili anulowania, zostaje z wynikiem.
- **Flow**: żądania FrankenPHP to wątki jednego procesu, więc zabić się nie da —
  trasa zapisuje znacznik obok notatki klucza, a `QueryRunner` pobiera wiersze
  partiami (`limit()->get()` zamiast `fetch()`) i sprawdza znacznik między nimi;
  `QueryCancelled` → 409.
- **Notebooki**: `subprocess.Popen` z własną sesją i `communicate(timeout)`;
  anulowanie zabija grupę procesów (notebook z pulą workerów zabiera ją ze sobą),
  timeout też; `NotebookCancelledError` → 409.

W Rust `cancel` wykonawców Flow, DataFusion, DuckDB i marimo woła trasę z
limitem 5 s; 404 starszego silnika to „nie ma kogo zapytać". Odmowa żądania, które
zostało zatrzymane, raportuje stop, a nie 409 czytane jako błąd użytkownika —
`StopSignal::or_stopped`.

### 30.3 Kalibracja metryk frameworków

Projekt z 29.6 („próg metryki wobec `pass_level` rubryki"), pod regułą judge'a z
ADR 0030:

- **Karta** — `Scorer::External.calibration`: wersja rubryki, `pass_at` metryki
  (zaliczenie na nim albo po lepszej stronie z katalogu) i `pass_level` dla
  rubryki z poziomami; rubryka tak/nie zalicza lepszą odpowiedzią. Dwa werdykty,
  bo 0,83 relewancji i „dobre" są na różnych skalach. Publikacja odmawia: metryki
  lub rubryki bez lepszego końca, progu poza zakresem z katalogu, nieistniejącego
  poziomu, rubryki liczbowej.
- **Deklaracja** — `external_calibration` nazywa zbiór kalibracyjny wzięty pod tą
  rubryką (może to być ten sam, co judge'a); kontekst przypina go jako
  `CalibrationPin` z wyprowadzonym `reads_archive`. Dopuszczenie trzyma go do karty
  jak zbiór judge'a; wynik skalibrowany na innych ludziach to inny kontekst, a
  porównanie mówi „Different calibration of framework metrics".
- **Run** pyta metrykę o każdy element zbioru pod rubryką, pokazując odpowiedź,
  którą widzieli ludzie; element niezadany i bez liczby liczy się przeciwko.
  Wynik niesie `external`: udział elementów o zgodnych werdyktach ze wszystkich,
  przedział Wilsona i udział różnych werdyktów wśród odpowiedzianych. Publikacja
  odmawia kontekstu skalibrowanego bez raportu i raportu wobec innego zbioru.
- **Nadal słowo modelu**: `reproducible: false`; ostrzeżenie mówi teraz, gdzie
  zgodność jest mierzona. Panel: formularz Measure bierze jeden zbiór dla judge'a
  i metryk frameworków (sekcja „Held against people"), dowód pokazuje tabelę
  zgodności przy nocie frameworka.

### 30.4 Serwis scorerów: token, obraz i chart

- `AIWATCHER_SCORERS_TOKEN`: obie trasy chcą tokenu `Bearer` (porównanie w stałym
  czasie), `/health` nie; work role wysyła go już jako `AIWATCHER_SCORER_TOKEN`.
  Bez tokenu serwis mówi przy starcie, że jest bez uwierzytelnienia.
- `deploy/Dockerfile.scorers`: oba frameworki, uid 10001, tylko do odczytu,
  `HOME` i katalog roboczy w `/tmp`. Zmierzone: 174 MiB po imporcie obu
  frameworków, 280 MiB z klientami modelu metryk ocenianych; obraz 471 MB.
- Chart, blok `scorers`: Service ClusterIP, Deployment, NetworkPolicy wpuszczająca
  tylko `server` i `worker`, token i poświadczenie modelu z Secretów, obie role
  dostają `AIWATCHER_SCORER_URL`; `execution.scorerUrl` dla serwisu spoza
  release'u. Render odmawia bez magazynu workflow i przy modelu podanym
  niepełnie. Egress otwarty — model metryk nie jest znany chartowi.
  `release-images.yml` publikuje obraz, `build-images.sh --scorers` go buduje,
  `docs/INSTALL.md` ma „Framework metrics".

### 30.5 Karty w panelu

Panel „Scorecards" na stronie Evaluation: lista kart z metrykami i formularz
publikacji — scorery wkompilowane, judge na wersji rubryki z poziomem, metryka
frameworka wybrana z zapisanego katalogu z polem na każdy parametr według rodzaju,
ścieżka wejścia dla metryki czytającej pytanie, i dla metryki ocenianej modelem
rubryka, `pass_at` i poziom kalibracji. Formularz wysyła poprawnie typowane body i
nic, co wyprowadza serwer (bez `declared`, bez kierunku, bez reguł); pokazuje, co
publikacja przypięła, a wdrożenie bez serwisu scorerów — nazwę zmiennej.

### 30.6 C1 — szablon generowania i oceny

`Answers::Generated` — `{"generated_by": {"task": "name@version", "queue": …,
"params"?}}` — robi z planu runu trzy kroki:

1. `evaluation_cases` (serve role, nowy `RuntimeKind`): cohorta czytana pod
   dopuszczeniem pary, wiersze `{case_id, input}` — nigdy oczekiwania;
2. `generate` (`python_task` na kolejce z deklaracji): zadanie workera dostaje
   wiersze i parametry `declaration`, `evaluation_id`, `repetition_id`,
   `variant` (manifest wariantu) i `params`, pisze `answers`;
3. `score` (ten sam krok co C0) czyta odpowiedzi ze swojego wejścia — ponowienie
   nie pyta aplikacji drugi raz.

Odmowy: generowanie nad kohortą rozmów (pytania trafiłyby do workera poza
archiwum), start bez magazynu obiektów (501 `worker_artifacts_disabled`), zadanie
bez `@wersji`, wiersz workera, który nie jest odpowiedzią (błąd kroku z numerem
wiersza, nic nieopublikowane). SDK: `aiwatcher_sdk.worker.generation_task` owija
funkcję na przypadek, `Declined` pomija przypadek (bez oceny, nie zero),
`Generated` niesie `trace_id`/`span_id`; odpowiedzi pisane raz, na końcu. Panel:
w Measure „Generated now, by a worker's task" z zadaniem, kolejką i parametrami
oraz „Baseline, measured the same way" — druga deklaracja różniąca się tylko
wariantem i ID (`…-baseline`), śledzona obok i z przyciskiem porównania po
opublikowaniu obu. `just e2e-generate` przechodzi baseline, kandydata i powtórzenie
odmawiające jednego przypadku na własnym serwerze.

### 30.7 Odbiór

`just check` zielony po commitach; `just sdk-check` (488), `just
query-contract-check` (z nowym testem anulowania przez prawdziwy fork server),
`just query-check` (171 testów PHP), `just ml-pipeline-check` (77),
`just scorers-check` (21), `just chart-check`, testy panelu Evaluation (61).
`just e2e-generate`: 7/7 — trzy runy przez `cases → generate → score`; worker
dostał tylko pytania; kandydat 1,0 wobec 0,0 baseline'u na jednym kontekście,
porównanie `comparable` z deltą +1,0; powtórzenie z odmówionym przypadkiem ma
3/4 ocenione i 1 bez oceny, a jego porównanie jest `unverified` z powodem; wynik
nazywa wykonanie i krok, przypadki nazywają trace; ponowny start trafia w ten sam
run (`created: false`).

Odbiór na żywo: aiwatcher na `127.0.0.1:19385` (WAL, magazyn workflow w pamięci),
`llama-server` z `gemma-4-e2b` na `:19386`, obraz `aiwatcher-scorers` w
kontenerze tylko do odczytu na `:19387` z tokenem, DuckDB na `:19381`, panel na
`:5382`; po odbiorze zatrzymane po PID, kontener usunięty, `launch.json`
przywrócony; na `:8080` i `:18080` nic nie nasłuchiwało przed i po.

- kontener bez tokenu → 401 z nazwą zmiennej; work role zapisał katalog przez
  token (deepeval 4.2.2 i opik 2.2.59, model gemma);
- baseline i kandydat wygenerowane przez workera SDK: `comparable`, `exact`
  0,0 → 1,0;
- kalibracja: rubryka `on-topic` (tak/nie), 4 oceny ludzi na wyniku baseline'u,
  zbiór 4 elementów; karta `deepeval.answer_relevancy` z `pass_at` 0,5 przypięta
  do 4.2.2 + gemma; run kandydata `cases → generate → score` zakończony w 10,6 s,
  `relevancy` 1,0, `reproducible: false`, `external`: 4 z 4 odpowiedziane,
  zgodność 100 %, przedział 51–100 %;
- anulowanie: pipeline DuckDB z transformacją śpiącą 120 s, anulowany po ~60 s
  pracy → run `cancelled` 0,6 s po komendzie; log reaktora „asking a running
  attempt to stop", silnik `query.cancelled` i odpowiedź 409, pamięć klucza
  `absent`, próba z klasą `policy`;
- panel: karta `capitals-framework` opublikowana z formularza (framework, metryka,
  ścieżka wejścia, kalibracja) z przypięciem pokazanym pod formularzem; dowód
  skalibrowanego wyniku z tabelą 100 % / 51–100 % / 4 z 4 / 0 %; Measure z opcją
  generowania i wyborem baseline'u. Odbiór znalazł jedną rzecz — pełny skrót
  wersji rubryki w zdaniu ostrzeżenia — poprawioną w `e90c974`.

### 30.8 Co zostaje

- **Flow zatrzymuje się tylko między partiami wierszy**: długi pojedynczy odczyt
  albo końcowa agregacja dochodzi do `set_time_limit`. Zapis w toku
  (`Committing`), który zawiśnie, jest ograniczony tylko limitami klienta
  magazynu.
- **Kalibracja to zgodność werdyktów**: próg `pass_at` wybiera autor karty, nic go
  nie dobiera; rubryki liczbowe są odmawiane; zbiór kalibracyjny jest niezależny
  od kohorty runu.
- **Generowanie ufa workerowi co do kodu**: zadanie dostaje manifest wariantu, ale
  nic nie sprawdza, że wykonało przypięty `code` i `generation_config`; kohorty
  rozmów są odmawiane; jedno zadanie na wariant, równoległość przypadków należy do
  zadania; baseline w formularzu tylko przy generowaniu.
- **Serwis scorerów**: token opcjonalny, egress otwarty; brak wpisu w docker
  compose; obraz publikuje dopiero workflow wydania.
- **Edytor kart** nie pokazuje wersji ani różnic i nie zaczyna od istniejącej karty.
- Z odbioru etapu C dalej otwarte: **C2** Experiments, **C3** bramka CI, **C4**
  feedback → przypadek testowy, test śmierci workera i Joba na klastrze dla kroku
  podowego; z B3 — kontekst wariantu w obserwacjach.
