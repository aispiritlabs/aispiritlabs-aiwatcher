# FTI — rekomendacja zakresu i plan rozwoju

Data: 2026-09-11. Status: A1–A4 i AR1 zaimplementowane; B1 zweryfikowane, trwały wycinek B2 i atomowe orphan GC nowych publikacji dostarczone; dodano weryfikowane adaptery Curation, promptów, modeli, Annotations i Conversations; B2/AR2 pozostają otwarte: B2e–B2h, w tym judge (sekcje 9–16). Wyniki odbioru A, ograniczenia i incydent seeda w sekcji 8. Przegląd planu z 2026-09-12 jest w sekcji 17; jego wnioski są wniesione do sekcji 2–7 — etap B ma punkty 8–11 i rozstrzygnięcia wizualne, tabela paczek B2e–B2i, a B3 zależy od B2e, B2f i B2i.

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

**Pierwsza paczka C0:** zbudować tryb `score_existing` na przypiętych odpowiedziach/trace, wykorzystując istniejący workflow. Źródłem treści jest dostępny dla wykonawcy artefakt lub uprawniony snapshot archiwum; trace z redakcją może nie wystarczać. Nowa wersja scorera daje nowy wynik z referencją do niezmienionych odpowiedzi. Nie wywołuje modelu aplikacji, ale może wywołać i obciążyć kosztem judge'a. Kolejne kroki rozwijają drugi tryb, `generate_and_score`.

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
3. **AR3, przed C0:** wyjąć wspólny przypadek użycia kompilacji/startu z modułu HTTP. API i scheduler korzystają z jednej usługi aplikacyjnej, z zachowaniem idempotencji, autoryzacji i polityki payloadów. Nowy scorer jest zadaniem istniejącego workera, nie nowym silnikiem wykonania. Nie zaczęte: [scheduler](../crates/aiwatcher-server/src/execution/scheduler.rs) nadal klasyfikuje trwałość błędu po statusie HTTP z `ApiError`.
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
Usunięcie źródła egzekwuje worker w ciągu godziny, a odczyt natychmiast.

**Porządek katalogu i próg indeksu.** Klucz to `evaluations/{sha256(id)}/`, więc
porządek jest porządkiem skrótu — i dlatego ekran B2i **nie ma kontrolki
okresu**. Wiersz kosztuje trzy żądania, czyli strona 200 wierszy ~600. Próg, od
którego indeks jest wymagany: **około tysiąca opublikowanych wyników**, albo
pierwsze żądanie porządku innego niż skrót. Indeks to jeden obiekt na commit pod
kluczem z czasem, dopisywany po wygranej claimu i uzupełniany przez przebieg
zbierania; celowo jeszcze nie zbudowany, bo to ta sama zmiana co porządek czasowy.

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
