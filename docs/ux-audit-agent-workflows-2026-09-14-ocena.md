# Ocena audytu UX — kompletność i pokrycie obszarów UX

Data: 14.09.2026. Dotyczy: [`ux-audit-agent-workflows-2026-09-14.md`](ux-audit-agent-workflows-2026-09-14.md). Stan kodu: commit `595844a`, bez lokalnych zmian w `apps/panel`. Status pierwotnego przeglądu: ocena dokumentu przed wdrożeniem. Aktualny stan zmian opisuje [plan migracji](ux-migration-plan-2026-09-14.md).

## 1. Wniosek

Jako kierunek produktu dokument jest dobry: nawigacja, strony obiektów, pętla ewaluacji i goldensetu oraz inspiracje z W&B są przemyślane, a sekcja 2 uczciwie opisuje granice ustaleń. Nie jest jednak listą wszystkiego, co da się dziś poprawić w UX.

- Pomija mniej więcej połowę ekranów panelu.
- Nie wymienia kilku błędów, przez które użytkownik traci dane albo wyciąga złe wnioski.
- Dobrze projektuje, *gdzie co jest*, ale prawie nie ocenia, *jak się z tym pracuje*. Na 16 obszarów UX (sekcja 7) 4 opisuje dobrze, 7 częściowo, a 5 słabo albo wcale.

Przegląd dał ok. 80 konkretnych ustaleń. Około 45 dokument w ogóle nie wspomina, około 30 pojawia się w nim tylko jako ogólna zasada. To szacunek z przeglądu, nie dokładne liczenie.

## 2. Metoda i ograniczenia

- **Kod.** Przeczytano całe `apps/panel/src`, ok. 34 tys. linii, w pięciu częściach:
  - annotations, conversations, datasets;
  - observability, workflows;
  - evaluation;
  - data-curation, training, experiments, prompts;
  - shell, `shared/` i wzorce wspólne, sprawdzone wyszukiwaniem w kodzie.

  Każde ustalenie porównano z treścią audytu. Ustalenia o najwyższym priorytecie sprawdzono w kodzie ponownie.
- **Działająca instancja.** Uruchomiono lokalny backend (`target/debug/aiwatcher`, magazyny w pamięci, dane z `scripts/seed-dev.py`) i frontend Vite. Skrypt w przeglądarce sprawdził 22 trasy przy 1280 i 375 px:
  - tytuł strony i nagłówek `h1`;
  - poziome przepełnienie;
  - pola bez etykiet;
  - rozmiar tekstu;
  - obcięcie list.

  Sprawdzono też zachowanie panelu przy niedostępnym backendzie.
- **Czego nie zrobiono.** Nie wykonano pełnego audytu WCAG, badania z użytkownikami, pomiaru kontrastu ani przebiegów wymagających workera (trening, nowa ewaluacja).
- **Numery linii.** Odnoszą się do commitu powyżej.

# Część I — kompletność wobec obecnego produktu

## 3. Ekrany, których audyt nie obejrzał

Sekcja 2 audytu obejmuje nawigację, shell, Explore, Metrics, szczegóły runu, modele, ewaluacje, review, workflow i Data Curation. Poza zakresem zostały:

- Annotations: Label, Sources, Imports, Exports;
- Conversations: Review, Corpora;
- Datasets;
- Training → Runs, Experiments, Prompts (lista i szczegóły);
- Data Curation → Recipe;
- Observability: Live, Query, Runs.

Mapa kodu w sekcji 14 też ich nie wymienia. Wnioski o ścieżkach anotacji, danych i treningu (sekcje 5 i 13) powstały więc bez przejrzenia ekranów, które te ścieżki już obsługują.

## 4. Błędy prowadzące do utraty danych lub złych wniosków

Żadnego z nich audyt nie wymienia.

| # | Problem | Dowód | Skutek |
|---|---|---|---|
| 1 | Publish w Data Curation jest aktywny po Preview i wysyła wiersze z próbki. | `features/data-curation/screens/pipeline/page.tsx:353` ustawia wynik także w trybie `preview`. `:373` wysyła `result.rows`, a `:893` sprawdza tylko `!result`. | Próbka 25 wierszy staje się niezmienną wersją datasetu. |
| 2 | Metrics liczy procent sukcesu od wszystkich runów, razem z trwającymi, a wykres rysuje trwające runy jako udane. Przy braku porażek kafelek jest zielony, także przy „0%”. | `features/observability/screens/metrics/page.tsx:76, 118, 190` | Błędny obraz stanu systemu. |
| 3 | Lista Runs pokazuje 50 wierszy, a nagłówek mówi o wszystkich pasujących. Brak paginacji i informacji o obcięciu. | `features/observability/screens/runs/page.tsx:116-145`. W przeglądarce: „595 matching”, 50 wierszy. | Szukany run może nie być na liście, a nic o tym nie mówi. |
| 4 | Cancel, pause i retry w Workflows działają bez potwierdzenia. Gdy nic nie wybrano, dotyczą najnowszego wykonania z listy odświeżanej co 5 s. | `features/workflows/screens/overview/page.tsx:149`, `features/workflows/components/managed-workflow.tsx:216` | Anulowanie niewłaściwego wykonania. |
| 5 | Przejście z Build do Write w Query nadpisuje ręcznie napisane zapytanie. | `features/observability/screens/query/page.tsx:265-269` | Utrata pracy. |
| 6 | „Save draft” na zaakceptowanym obrazie: po odświeżeniu edytor wraca do zaakceptowanej rewizji. | `features/annotations/screens/label/page.tsx:157` pobiera obraz bez `revision`, `:171` resetuje szkic, `:209-214` odświeża dane. Backend zwraca rewizję zaakceptowaną przed najnowszą (`crates/aiwatcher-annotations/src/images/store.rs:238-253`). | Poprawka wygląda na utraconą. |
| 7 | Approve w Conversations działa bez odsłonięcia treści. | `features/conversations/screens/review/page.tsx:482-488` | Dane osobowe mogą trafić do korpusu z etykietą „zatwierdzone przez człowieka”. |
| 8 | Publikacja przypadków z review tworzy wersję datasetu jednym kliknięciem, a API nie ma cofnięcia. | `features/evaluation/screens/overview/reviews.tsx:319-325` | Nieodwracalna zmiana bez podglądu. |
| 9 | Przy niedostępnym backendzie panel pokazuje spinner bez końca i w pętli ponawia żądania `/api/v1/auth/config`. | `shared/components/auth-gate.tsx` pokazuje spinner, gdy konfiguracja jest w stanie pending. `shared/components/user-menu.tsx` przy każdym zamontowaniu ponownie wywołuje ten sam błędny odczyt. Potwierdzone w przeglądarce. | Brak komunikatu i ruch na serwerze bez końca. |

## 5. Kategorie problemów nieobecne w audycie

- **Potwierdzenia nieodwracalnych akcji.** W całej aplikacji jest jeden `window.confirm` (`features/conversations/screens/review/page.tsx:211`). Bez potwierdzenia działają m.in.:
  - promocja modelu;
  - „Make production” promptu;
  - „Execute & save dataset” w Recipe;
  - anulowanie importu;
  - „Forget” harmonogramu;
  - akceptacja anotacji klawiszem `a`.
- **Ochrona przed utratą pracy.** W `src` nie ma `useBlocker` ani `beforeunload`. Bez pytania znikają szkic canvasu (wczytanie zapisanego pipeline'u, import), szkic anotacji po zmianie obrazu, formularze Measure i Scorecard po przełączeniu panelu oraz Recipe po wczytaniu przykładu.
- **Błąd wyglądający jak brak danych.**
  - Annotations przy błędzie pokazuje „No annotation project yet” i zachęca do założenia duplikatu projektu.
  - Prompts pokazuje „No prompts yet”, Conversations „Nothing has been archived yet”.
  - Workflows pokazuje brak wykonań, Explore brak wiadomości.
  - Klasa `text-destructive` nie jest zdefiniowana w `styles.css`, więc błędy w `shared/components/ui/primitives.tsx:186` (`Refusal`) i `shared/components/schedule-card.tsx:353, 382` nie są czerwone.
- **Globalna informacja zwrotna.** Nie ma toastów ani potwierdzenia zapisu. Router ma tylko `notFoundComponent` (`routes/__root.tsx`), bez `errorComponent`.
- **Ciche obcinanie list.**
  - Explore: 100 runów (`features/observability/screens/explore/page.tsx:312`).
  - Obrazy anotacji: 200 (`features/annotations/screens/label/page.tsx:145`).
  - Tury rozmowy: 100 (`features/conversations/screens/review/page.tsx:259`).
  - Kandydaci do porównania: 50 (`features/evaluation/screens/overview/comparison.tsx:35`).
  - Regresje: 20 (`features/evaluation/screens/overview/page.tsx:961`).
- **Czas bez daty.** `formatTime` (`shared/lib/utils.ts:51`) pokazuje samą godzinę na listach Runs, Evaluation i Explore, także przy oknie 7 dni i „all”. Następne uruchomienie harmonogramu nie podaje strefy czasowej.
- **Wykresy.**
  - Podpowiedzi działają tylko myszą, oś Y nie ma podpisów, brak jednostek i tabeli alternatywnej. Wyjątek: krzywe treningu mają tabelę wartości.
  - Kolor serii treningu zależy od hasha (`shared/components/charts/learning-curve.tsx:46`), więc kolory się powtarzają, a legenda nie pokazuje wzoru linii.
- **Kierunek metryk.** Wzrost jest zawsze oznaczany jako poprawa (`shared/components/prompt-bits.tsx:98`). Tabela modeli nie podaje nazwy metryki.
- **Live.**
  - Pause gubi zdarzenia zamiast je buforować (`features/observability/screens/live/page.tsx:119`).
  - Lista przewija się sama podczas czytania.
  - Trwale zamknięte połączenie nie jest pokazane (`shared/lib/live.ts:207`).
  - Wpisy nie mówią, do którego runu i agenta należą.
- **Tytuł strony i skróty nawigacji.** Każda trasa ma tytuł „aiwatcher” (potwierdzone w przeglądarce). Brak linku „przejdź do treści”.
- **Historia przeglądarki.** Pola wyszukiwania w Annotations → Label i Sources dodają wpis na każdy znak. Datasets robi to poprawnie.
- **Motyw.** Graf workflow ma wymuszony ciemny motyw (`features/workflows/components/workflow-graph.tsx:391`), choć jasny motyw istnieje.
- **Etykiety pól (w przeglądarce).** Bez etykiet są 4 pola na Label, 3 na Sources, 4 na Conversations → Review (powód odrzucenia) i 2 na Data Curation → Pipeline.
- **Telefon.** Przepełnienie o 57 px przy 375 px występuje na każdej sprawdzonej trasie, więc wynika z shella, nie tylko z Evaluation.

## 6. Twierdzenia nieaktualne lub błędne

**Audyt opisuje jako przyszłe rzeczy, które już istnieją:**
- bezpośrednie wejścia do Datasets, Annotations i Conversations, a Annotations przenosi `project` między widokami (`app/navigation.ts`);
- w Experiments porównanie wariantów z podaną przez serwer przyczyną nieporównywalności oraz latencja, tokeny i koszt;
- porównanie przypadków A/B z filtrem gorsze, lepsze, zmienione dla zachowanych wyników ewaluacji (`features/evaluation/screens/overview/comparison.tsx`);
- porównanie 2–5 treningów z tabelą wartości, oznaczeniem najlepszego checkpointu i ostrzeżeniem o ciszy;
- kopiowanie pełnego ID przez `IdChip` (`shared/components/ui/primitives.tsx`);
- w Workflows pause, resume, cancel, retry kroku, bramki odpowiedzi człowieka, logi prób i „Rerun from node”;
- parametr `as_of` w API runów i spanów, więc zakres bezwzględny nie wymaga całkiem nowego API;
- eksport COCO z polityką praw i wykluczeniami jako gotowy punkt końcowy pracy nad obrazami.

Plan w obecnym kształcie może prowadzić do budowania tych funkcji drugi raz.

**Błędne lub niepełne rekomendacje:**
- **Sekcja 11.** Canvas Data Curation obsługuje tylko liniowy łańcuch (`features/data-curation/lib/pipeline.ts:43-72`), a nowe połączenie zastępuje istniejące. Proponowanej gałęzi do „Review człowieka” nie da się w nim zbudować bez rozbudowy edytora.
- **Sekcja 4, P1 (wiersz modelu).** Link w komórce nie wystarczy: `onClick` wiersza (`features/training/screens/models/page.tsx:196`) trzeba usunąć.
- **„Shell przenosi tylko `window`”.** Annotations przenosi `project`. Agent ma w adresie URL trzy różne nazwy: `by`/`key` w Explore, `agent[]` w Live i Query, `agent_id` w Runs i Metrics.
- **„Dataset → schemat etykiet → anotacja”.** Ta ścieżka jest dziś zablokowana:
  - projekt można założyć tylko z domyślnymi klasami i nie ma edytora schematu (`features/annotations/screens/label/page.tsx:840-915`);
  - stanu `in_review` UI nigdy nie ustawia, a komenda ⌘K filtruje po nim (`app/commands.ts:209`).
- **Sekcja 3.3.** `DatasetReference` nie rozwiązuje korpusów rozmów (`name@sha256`).
- **Sekcja 3.6.** Graf workflow ignoruje jasny motyw.

# Część II — ocena pod względem UX

Kryteria: heurystyki Nielsena oraz ścieżki osób wymienionych w sekcji 1 audytu.

## 7. Pokrycie obszarów UX

| Obszar UX | Audyt | Czego brakuje (przykłady z panelu) |
|---|---|---|
| Nawigacja i struktura | **Dobrze** (§5, §15A) | Każda karta ma tytuł „aiwatcher”. Label, Training → Runs i Models nie mają nagłówka strony. |
| Ciągłość kontekstu | **Dobrze** (§6–7) | Aktywne filtry bywają niewidoczne i nie da się ich usunąć: `agent_id` w Runs, `dataset` w Evaluation. |
| Język interfejsu | **Słabo** (P2, dwa przykłady) | Opis prawie każdego ekranu tłumaczy budowę systemu zamiast zadania (sekcja 8.1). |
| Nazwy obiektów | Częściowo (§15H) | Nagłówek runu to UUID, wersja modelu to pełny sha256, eksport to 12-znakowy hash bez daty, przy runie widać „cursor 0000…13131”. |
| Status systemu i informacja zwrotna | **Brak** | Zapis nie daje potwierdzenia. Declare, Approve i Start run są wyłączone bez podanego powodu. Zamknięte połączenie Live i obcięte listy nie są sygnalizowane. |
| Zapobieganie błędom | **Brak** | W całej aplikacji jest jedno potwierdzenie. Publish działa po podglądzie, a klawisz `a` akceptuje anotację. |
| Kontrola i cofanie | **Brak** | Anotacja i canvas nie mają undo ani powrotu do szkicu. Pause w Live gubi zdarzenia. Przejście Build → Write nadpisuje tekst. |
| Rozpoznanie błędu i wyjście z niego | Częściowo (§7.7, tylko dla czasu) | Błąd wygląda jak pusty stan. Użytkownik widzi surowy `SyntaxError` z parametrów JSON albo spinner bez końca. |
| Spójność wzorców | Częściowo | Trzy procesy review z różnym słownictwem (sekcja 8.4). Wersje danych mają trzy formaty. W Datasets „Build dataset” przełącza widok, a „Build” publikuje. Pozycja menu „Recipe” to na ekranie „One script”, a na przycisku „Save script”. Baseline wybiera się w dwóch kontrolkach z różnymi regułami. |
| Szybkość pracy seryjnej | Częściowo (§10) | Anotacja nie ma „następny/poprzedni” ani „Accept & next”. Review nie ma filtrów stanu ani operacji zbiorczych. Skróty klawiszowe są nieodkrywalne. |
| Pierwsze użycie i puste stany | Częściowo (§9) | `EmptyState` (`shared/components/ui/primitives.tsx:156`) nie ma miejsca na przycisk akcji. Puste stany mówią, co wywołać w API. Nie da się założyć projektu anotacji z własnymi klasami. |
| Zaufanie do liczb | Częściowo (§7.2, §9) | Godzina bez daty. Wzrost jest zawsze zielony, także dla metryk, gdzie lepsza jest niższa wartość. Delty nie mają liczebności ani niepewności. Przy braku porażek widać zielone „0%”. |
| Hierarchia i czytelność | **Dobrze** (§12) | Problem potwierdzony: na Training → Runs jest 128 elementów tekstu ≤10,5 px. |
| Dostępność | Częściowo (§12) | Pola bez etykiet, brak `aria-pressed` i `aria-expanded`, wykresy tylko myszą, przycisk usuwania kształtu widoczny dopiero po najechaniu. |
| Telefon | **Dobrze** (§4) | Przepełnienie pochodzi z shella i występuje na każdej trasie. |
| Praca zespołowa | **Słabo** (wzmianka w §15B) | Brak informacji, kto zatwierdził. Brak przypisań i komentarzy. Widoki zapisują się tylko w localStorage. |

## 8. Największe luki UX

### 8.1. Interfejs mówi językiem implementacji

Audyt ocenia to jako P2, drobną poprawkę czytelności. Przykłady z opisów ekranów:

- Live: „Nothing here is folded or retained — it is what a producer has just said.” (`features/observability/screens/live/page.tsx:145`)
- Imports: „…a job that survives the process that started it.” (`features/annotations/screens/imports/page.tsx:127`)
- Evaluation: „Reports arrive on the same log as the traces and are folded apart from them.” (`features/evaluation/screens/overview/page.tsx:193`)

Czasem jedyną drogą dalej jest instrukcja dla programisty:

- „Stage a batch with POST /api/v1/annotation-import-batches” (`features/annotations/screens/imports/page.tsx:141`)
- „Start it with `just query-serve`” (`features/datasets/screens/overview/page.tsx:342`, `features/observability/screens/query/page.tsx:662`)
- „TrainingClient.register_model” (`features/training/screens/models/page.tsx:81`)
- „Put this in `train.started.data.dataset`” (`features/annotations/screens/exports/page.tsx:240`)

Dla anotatora i reviewera bez znajomości SDK, a audyt wymienia ich jako główne grupy użytkowników, to blokada zadania, nie kosmetyka.

### 8.2. Brak ochrony przed pomyłką i brak cofania

Nieodwracalne akcje działają jednym kliknięciem, bez nazwy obiektu, wersji i skutku:

- publikacja wersji datasetu;
- promocja modelu;
- „Make production” promptu;
- anulowanie wykonania.

Szkice znikają bez pytania. Anotacja i canvas nie mają cofania.

### 8.3. System nie mówi, co się stało

Użytkownik nie wie:

- czy zapis się udał;
- dlaczego przycisk jest nieaktywny;
- czy lista jest pełna;
- czy połączenie żyje.

Błąd udaje brak danych, a pusta instancja nie proponuje pierwszego kroku.

### 8.4. Ta sama czynność wygląda inaczej w każdym module

- **Review** ma trzy postacie:
  - accept/reject obrazu bez powodu;
  - approve/reject tury rozmowy z wymaganym powodem;
  - review przypadków ewaluacji z publikacją.
- **Wersje danych** mają trzy formaty: `name@version`, `project@export`, `name@sha256`.
- **Baseline** wybiera się w dwóch kontrolkach.

Audyt proponuje wspólny komponent odnośnika (§15F), ale nie nazywa rozbieżności, które już istnieją i które trzeba ujednolicić.

### 8.5. Ścieżki anotatora i reviewera nie zostały przejrzane

Audyt opisuje je jako docelowe. Obecny przebieg (Label, Conversations → Review) zatrzymuje się na:

- braku edycji schematu;
- braku kolejki review;
- braku pracy seryjnej: następny/poprzedni, operacje zbiorcze, filtry stanu.

## 9. Uwagi do metody audytu

- Ścieżki per osoba są opisane tylko docelowo (§5). Brakuje przejścia krok po kroku przez obecny interfejs z problemem na każdym kroku.
- P0 w sekcji 4 miesza propozycje przebudowy (karta agenta, nowe menu) z defektami obecnego UI.
- Priorytety nie uwzględniają częstości: problem przy każdej anotacji waży więcej niż ten przy jednorazowej konfiguracji.
- Plan testu użyteczności w §13 jest dobry, ale obejmuje zadania, których ekranów audyt nie obejrzał.

# Rekomendacje

## 10. Proponowane zmiany w audycie

1. **Rozdzielić dwa rodzaje ustaleń.**
   - *Stan obecny — defekty UX:* sekcje 4, 5 i 8 tej oceny, w układzie obszar → dowód → skutek dla użytkownika → priorytet.
   - *Wizja architektury informacji:* obecne sekcje 5–11 i 15.
2. **Dodać listę szybkich poprawek bez nowych kontraktów API.** Sekcja 14 audytu mówi, że są możliwe, ale ich nie wymienia:
   - potwierdzenia z nazwą obiektu i skutkiem;
   - stany błędów różne od pustych;
   - data przy godzinie;
   - paginacja Runs;
   - `text-destructive` → `text-danger`;
   - tytuły stron;
   - etykiety pól;
   - opisy ekranów w języku zadań.
3. **Rozszerzyć kryteria odbioru w sekcji 13:**
   - każda nieodwracalna akcja ma potwierdzenie;
   - błąd pobrania nigdy nie wygląda jak pusty stan;
   - porzucenie szkicu wymaga decyzji użytkownika;
   - wyłączony przycisk podaje powód;
   - obcięta lista mówi, ile pokazuje z ilu.
4. **Uzupełnić zakres** o ekrany z sekcji 3 i poprawić twierdzenia z sekcji 6, zwłaszcza te o funkcjach opisanych jako przyszłe.
5. **Dopisać przejście krok po kroku obecnych ścieżek** anotatora, reviewera rozmów i osoby publikującej dataset, z miejscami, w których dziś trzeba znać API.


## Aktualizacja po weryfikacji i rozpoczęciu wdrożenia — 14.09.2026

Ocena trafnie wskazuje utratę kontekstu i błędy interpretacji danych. Jej listy są materiałem do priorytetyzacji, nie dowodem, że każda operacja wymaga dodatkowego potwierdzenia.

- Publikacja review pokazuje liczbę zatwierdzonych przypadków i tworzy nową wersję; brak cofnięcia wersji nie oznacza nadpisania poprzedniej. Potrzebne są czytelne pochodzenie i podgląd, a nie obowiązkowy modal każdego zapisu.
- Recipe i część edytorów mają lokalne potwierdzenia zapisu. Wniosek o ich całkowitym braku jest zbyt szeroki; standaryzujemy istniejącą informację zwrotną.
- Anotacje mają filtr stanu review. Problem dotyczy zachowania wybranej rewizji i płynności pracy.
- Lista regresji ujawnia liczbę dalszych przypadków. Nadal wymaga wygodnego dojścia do nich, ale nie jest całkowicie cichym obcięciem.
- Promocja modelu i etykieta produkcyjna promptu zmieniają odniesienie do wersji; treść historycznej wersji pozostaje. Potwierdzenie powinno wynikać z wpływu na działający system.
- Realizacja zaczęła się od publikacji pełnych wyników, metryk, paginacji Runs, wyboru workflow, ochrony szkicu anotacji, Query, Live, awarii konfiguracji logowania i nowej nawigacji. Dokładny stan oraz bramki kolejnych etapów są w planie migracji.
