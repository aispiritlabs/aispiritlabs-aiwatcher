# Migracja UX AIWatchera

Data: 14.09.2026. Status: wdrożenie etapowe, **cały plan nie jest zakończony**.

## Kontrakt produktu

Brak centralnego obiektu obowiązkowego dla wszystkich organizacji. Dane, anotacje, modele, trening, ewaluacje, aplikacje i workflow są równorzędnymi punktami wejścia. Strona startowa pomaga wybrać pracę; po przejściu do obiektu użytkownik widzi wersję, czas, pochodzenie i powiązane dowody.

Inspiracje W&B: oddzielenie kontekstu globalnego, projektu i obiektu; wspólne filtry tabel i wykresów; wersjonowane artefakty; porównania przypadków; pętla obserwacja → review → dataset → ewaluacja. Źródła i ograniczenia porównania: [audyt](ux-audit-agent-workflows-2026-09-14.md).

## Stan implementacji

| ID | Zmiana | Stan / kryterium |
|---|---|---|
| UX-01 | Publikacja wyników curation | Wdrożono blokadę zwykłej publikacji Preview oraz obcięcia w dowolnym etapie, także notebooku. Osobna akcja Publish sample zapisuje ograniczony wynik do `<dataset>/samples` z trwałym oznaczeniem i pochodzeniem. |
| UX-02 | Wiarygodność metryk | Wdrożono osobne running/succeeded/failed w API i wykresie, skuteczność od zakończonych oraz neutralny brak danych. Kafelki i wykres tokenów korzystają z tych samych spanów i filtrów; cache nie jest liczony podwójnie. Starsze API zachowuje jawnie opisaną serię łączoną. |
| UX-03 | Runs | Kursor `before`, kolejne strony, liczba załadowanych/pasujących, deduplikacja ID, przewijanie tabeli na małym ekranie. |
| UX-04 | Workflow | Pierwszy wybór wykonania przypięty w URL przed pokazaniem komend. Odświeżenie listy nie zmienia celu. |
| UX-05 | Query i ochrona szkiców | Build/Write zachowuje tekst. Query, One script, Pipeline, formularze promptów, harmonogram i oba widoki review chronią niezapisane zmiany przy opuszczaniu strony i reload; curation pyta przed zastąpieniem szkicu. Udany zapis nie usuwa późniejszych edycji; formularze publikacji promptów i decyzji o rozmowie blokują edycję na czas żądania. Wdrożono odtwarzanie kontekstu recipe/pipeline przy Wstecz/Dalej, odczyt historycznej rewizji pipeline z API oraz ochronę kodu i niepoprawnych ustawień w zagnieżdżonych edytorach notebooków. Lokalne, niezapisane flow nie są odzyskiwane po opuszczeniu strony lub reload. |
| UX-06 | Anotacje | Wdrożono lokalne undo/redo (100 kroków), poprzedni/następny obraz i paginację z zachowaniem filtrów. Zapis wskazuje rewizję w URL i cache; historia pozostaje po zapisie. Ochrona przejść obejmuje także niedokończony rysunek. |
| UX-07 | Live | Pauza buforuje ostatnie 400 nowych zdarzeń; licznik, ostrzeżenie o przepełnieniu i link do historii. |
| UX-08 | Awarie i dostępność | Awaria auth/config zamyka dostęp do panelu, pokazuje retry. Uprawnienia UI nie zakładają auth=none przy błędzie. Błąd routera ma komunikat. TimeRange zawija się i komunikuje aktywną opcję. Poprawiono kolor błędów, daty ze strefą czasową, tytuły stron i reakcję grafu workflow na motyw. |
| UX-09 | Nawigacja | Wdrożono neutralny start, wszystkie obszary, grupy mobilne i globalne wyszukiwanie/konto. Przełącznik nowy/klasyczny układ zachowuje edytor i URL; flaga wdrożenia wymusza rollout/rollback. Preferowany start, nazwane przypięcia pełnych linków, wersjonowany zapis per instancja/tożsamość i lokalna diagnostyka przejść. Centralna analityka i automatyczne kohorty pozostają poza tą implementacją. |
| UX-10 | Profil | `/account`: bieżąca tożsamość, role instancji, grupy SSO tylko do odczytu; jawny tryb lokalny. Brak deklaracji fikcyjnych zespołów i projektowych uprawnień. |
| IAM-01 | Organizacje, zespoły, projekty | W toku: model i magazyny IAM, OIDC issuer/sub, API z bootstrapem i atomowym audytem oraz rejestry zasobów z zakresem organizacja/projekt (datasety, curation, prompty, treningi, modele, anotacje z plikami obrazów oraz formularze, oceny, karty ewaluacji, definicje workflow oraz review przypadków z publikacją do datasetu i kohorty z natywnych datasetów/anotacji oraz nagrania odpowiedzi, pliki pakietów dowodów i wyniki producentów z approvals oraz retencją). Wykonanie ma trwałego właściciela (scope + principal + plan) zapisywanego razem z nim, a zakres wiąże magazyn, więc globalny reactor, worker, launcher, timer, outbox i retencja odmawiają projektowego wykonania (ADR_0033). Projektowy katalog artefaktów, lineage i cache oraz projektowe pomiary sędziowskie i zewnętrzne są izolowane. Jest manifest migracji i wznawialny wykonawca dla czterech rejestrów (`aiwatcher-migrate`), z rozmowami blokowanymi i jedenastoma prefiksami nazwanymi jako nieobsługiwane. Brak dispatchera i projektowego `/start`; query, strumienie, zadania, log zdarzeń i cutover nadal wymagają izolacji. Selektory UI są nieaktywne. |
| IAM-02 | Zaproszenia i dostęp warsztatowy | Niewdrożone; zależą od IAM-01. |
| FLOW-01 | Pełne przejścia i lineage | Wdrożono wszystkie cztery: jeden słownik filtra w URL na Runs, Metrics, Explore i stronach agentów, z osią `prompt` dołożoną w read modelu; strony agentów z ich własnymi liczbami, wymiarami i promptami; `ModelVersionReference` i prompt na fali, w liście spanów i w lineage; porównanie przedziału z poprzednim (`?compare=previous`) na Metrics i na stronie agenta. `/dimensions` i `/metrics` przyjmują dziś wszystkie dziesięć osi oraz `as_of`. |
| LEARN-01 | Learning | Wdrożono `/learning` nad kontrolą dostępu: **warsztat to projekt, uczestnik to grant, zapis to zrealizowane zaproszenie**. Lista warsztatów organizacji, strona warsztatu z uczestnikami i fazą okna każdego grantu czytaną osobno, zapis przez zaproszenie i dziewięć slotów laboratoriów. Bez pojęcia „warsztatu" w backendzie i bez nowej trasy. Treść instrukcji, testy, ewaluacje, wyniki i postęp nie mają kontraktu i są oznaczone jako niedostępne; silnik treści i ocen pozostaje. |

## Docelowa architektura informacji

- Praca: przegląd, ostatnie, przypięte, preferowany start.
- Dane: datasety i źródła, przygotowanie, anotacje i review.
- Modele i jakość: wyszukiwanie/rejestr, trening, ewaluacje i eksperymenty.
- Aplikacje: agenci, prompty, obserwowalność.
- Workflow: definicje, wykonania, harmonogramy; edytor curation pozostaje osobny od edytora workflow aplikacji.
- Learning: warsztaty, laboratoria, przypisania i dostęp.
- Administracja: organizacja, użytkownicy, zespoły, projekty, audyt.
- System: runtimes, integracje, konfiguracja instancji.

Górny pasek docelowo: organizacja → projekt, wyszukiwanie, profil/konto. Preferencje UI nie stanowią uprawnień. Projekt anotacji jest obiektem domenowym wewnątrz projektu dostępowego, a nie jego zamiennikiem.

## IAM i współpraca — wymagania implementacyjne

Authentik/OIDC jest źródłem tożsamości. Klucz użytkownika: dostawca i stabilny `sub`, nie adres email. Hasła, odzyskiwanie konta i rejestracja należą do IdP. Grupy IdP pozostają tylko do odczytu; AIWatcher zarządza własnymi zespołami i członkostwami bez automatycznego tworzenia grup Authentika.

Role organizacji: owner/admin/member. Role projektu: viewer/editor/admin. Administrator instancji jest osobnym uprawnieniem. Członkostwo organizacji samo nie daje dostępu do projektów. Efektywny dostęp to suma aktywnych grantów bezpośrednich i zespołowych. Wygaszenie grantu warsztatowego nie odbiera niezależnego dostępu stałego; instruktor musi zobaczyć ten wyjątek.

Zaproszenie: jednokrotne, wygasające, weryfikujące odbiorcę po SSO. W pierwszej wersji kopiowanie linku; wysyłka email nie jest wymagana. Przyjęcie i nadanie dostępu w jednej transakcji. Produkcyjny magazyn PostgreSQL, implementacja pamięciowa do testów. Zmiany sesji/API addytywne, klient generowany z OpenAPI.

Każdy odczyt, zapis, eksport, wynik wyszukiwania, query, artefakt, SSE, runtime oraz tożsamość maszynowa musi mieć sprawdzony zakres. Sam filtr w panelu ani prefiks URL nie wystarcza. `auth=none` pozostaje lokalnym trybem, nie trybem bezpiecznej pracy wielu organizacji.

**Stan na 18.09.2026 i podział na dwie połowy.** Połowa autorska jest zrobiona: control plane, rejestry za `for_project`, 63 wpisy tras `orgs/{organization}/projects/{project}` z rzeczywistym sprawdzeniem grantu, trwałe właścicielstwo wykonania i dispatcher. Połowa obserwowalna — log zdarzeń, projektor, read model i żywe strumienie — jest nietknięta, więc **przebieg projektu nie ma dziś żywego widoku, spanu ani foldu**. Plan na tę połowę to [IAM-02](iam-02-data-plane.md); jego bramka M1 brzmi: zalogować się, zobaczyć wyłącznie swoje projekty i słuchać wyłącznie swoich strumieni.

**Czego brakuje do dzielenia projektu.** Trasy zakresowe istnieją i naprawdę sprawdzają granty, więc połowa autorska jest gotowa do dzielenia — brakuje trzech rzeczy i żadna nie jest kosmetyczna:

1. **Zaproszenia.** `Command::Grant` wymaga dokładnej pary `(provider, subject)`, a tej nie da się znać przed pierwszym logowaniem. Zaproszenie musi być jednorazowym, wygasającym tokenem związanym z `(scope, role, window)`, realizowanym po SSO w jednej transakcji z nadaniem grantu. Email jest wskazówką do dostarczenia, nigdy warunkiem przyjęcia ani kluczem tożsamości; powtórzenie i inny odbiorca odmawiają.
2. **Interfejs.** Dziś **żaden plik panelu nie odwołuje się do organizacji ani projektów** — wygenerowany klient ma 82 wpisy, a UI nie istnieje. Potrzebny jest przełącznik organizacji/projektu, strona członków i grantów oraz okno dzielenia. `/account` pokazuje grupy IdP tylko do odczytu i mówi, że to nie są zespoły.
3. **Odwołanie sięgające strumieni i zadań.** TTL ciasteczka *jest* dziś oknem odwołania. Strumień żyjący godzinami musi sprawdzać grant ponownie i zamykać się przy odmowie, a wznowienie przez `Last-Event-ID` musi sprawdzić grant **przed** odtworzeniem — inaczej wznowienie jest sposobem na czytanie po odebraniu dostępu.

Dzielenie obserwowalności (przebiegi, spany, metryki, żywy strumień) wymaga dodatkowo E1–E4 z IAM-02. **Selektora organizacji/projektu nie aktywujemy przed bramką M1.**

## Warsztaty i Learning

Instruktor wybiera wspólny projekt lub indywidualną kopię przypiętej wersji szablonu. Kopiowanie nie przenosi sekretów, członkostw ani prywatnej historii wykonań. Grant zawiera `valid_from`, `edit_until` i opcjonalne `read_until`. Wygaśnięcie obowiązuje bez czekania na wygaśnięcie sesji SSO; aktywne strumienie i zadania respektują odebranie dostępu. Domyślnie zadania kończą pracę z końcem dostępu; instruktor może dopuścić ograniczony czas dokończenia.

UI Learning: lista i szczegół warsztatu, uczestnicy, stan provisioningu i dostępu, 9 slotów laboratoriów. Laboratorium zawiera miejsce na instrukcję, projekt, testy, ewaluacje i metryki. Brakujące kontrakty oznaczamy jako niedostępne, bez wymyślonych materiałów, wyników ani postępu.

## Kolejność wdrażania i migracja danych

1. **Poprawność obecnego panelu.** Domknąć UX-01–08, regresje, stany błędów i ochronę pozostałych szkiców. Nie zmieniać historii danych.
2. **Nowy shell.** Wdrożyć przełącznik rollout/rollback, preferencje, przypięcia i analitykę przejść. Zachować dotychczasowe trasy jako adaptery.
3. **IAM backend i UI.** Najpierw pełne egzekwowanie zakresu i testy izolacji, potem aktywować selektory organizacji/projektu i zarządzanie zespołami. **Zmiana kolejności, 18.09.2026:** połowa autorska IAM jest zrobiona, a połowa obserwowalna ([IAM-02](iam-02-data-plane.md)) została świadomie przesunięta **na koniec** tej listy. Nie tworzy to długu w danych — brak zakresu *jest* stroną globalną, więc nic istniejącego nie wymaga przeróbki — ale każda funkcja zbudowana w międzyczasie to jedna powierzchnia więcej do zakresowania później. Stąd reguła obowiązująca do tego czasu: **nowy zasób autorski dostaje swoją zakresową rodzinę tras od urodzenia** (`ProjectAuthorization` plus `<prefix>/scopes/<organization>/<project>/registry/`). Selektor organizacji/projektu pozostaje nieaktywny, a wdrożenie nie jest opisywane jako multi-tenant safe.
4. **Dry-run migracji.** Jawnie wybrana organizacja/projekt docelowy. Manifest z liczbami obiektów, referencjami i mapowaniem grup. Migracja powtarzalna, wznawialna, idempotentna. Nie zmieniać hashy treści, ID wersji ani historycznych referencji. Nieprzypisane dane pozostają niedostępne.
5. **Cutover zakresu.** Snapshot, czasowe zatrzymanie zapisów/ingestu, wykonanie manifestu, weryfikacja liczników i referencji, wznowienie. Nowe adresy `/orgs/:orgId/projects/:projectId/...`. Stary adres z niejednoznacznym projektem otwiera wybór wyłącznie dostępnych projektów. Adapter zachowuje wersję, filtry i `as_of`.
6. **Przejścia obszarowe.** Obiekt → rewizja → użyty prompt/model/dataset → dowody ewaluacji. Wspólne zakresy czasu i filtry wykresów/tabel. Goldenset przez jawne review. Curation w czasie rzeczywistym z kolejką i kontrolą pochodzenia.
7. **Learning i zakończenie.** UI powiązać z rzeczywistymi grantami warsztatowymi; osobno wdrażać silnik treści i ocen. Usunąć stare menu po pomyślnym okresie przejściowym.

Migracja zapisanych widoków przeglądarki ma wersjonowany schemat; zachowuje oryginał do poprawnej konwersji. Rollback interfejsu nie może przywrócić globalnego dostępu do danych. Rollback danych wymaga snapshotu i uzgodnienia zapisów wykonanych po cutover.

## Kryteria odbioru

- Testy panelu, granice architektury, TypeScript i build.
- Preview i każdy ucięty etap nie publikują zwykłej wersji; kompletna produkcja publikuje z pochodzeniem.
- 0 zakończonych nie daje czerwonego/zielonego procentu; running nie udaje succeeded.
- Kolejne strony list nie gubią filtrów; odświeżenie workflow nie zmienia identyfikatora komendy.
- Draft anotacji pozostaje po zapisie, reload i wejściu z linku; opuszczenie brudnego edytora wymaga decyzji użytkownika.
- Klawiatura, 375 px i szeroki ekran, motyw jasny/ciemny; żadna globalna kontrolka nie znika poza viewportem.
- IAM: próby odczytu/zapisu między projektami i organizacjami; grant przed startem/po wygaśnięciu; replay zaproszenia i inny odbiorca; suma grantów; SSE i zadania po odebraniu dostępu.
- Migracja: liczniki, ID wersji i referencje przed/po; wznowienie po przerwaniu; stare linki i zapisane widoki; bezpieczny rollback.
- Pełne scenariusze: data → anotacja → trening → ewaluacja; agent → historia → prompt/model → dataset; trace → review → goldenset; dokument → workflow → runtime evaluation.

Monitorować niedziałające trasy i referencje, odmowy dostępu, błędy migracji, opóźnienia wygaszania grantów i czas wykonania kluczowych zadań użytkownika. Nie ogłaszać zakończenia migracji na podstawie samej przebudowy menu.


## Walidacja wykonanej części — 14.09.2026

- `npm run test -- --reporter=dot`: 36 plików, 307 testów zakończonych powodzeniem.
- `npm run build`: kontrola granic architektury, Vite i TypeScript zakończone powodzeniem. Vite zgłasza ostrzeżenie o głównym chunku powyżej 500 kB.
- `git diff --check`: bez błędów.
- Lokalny podgląd w przeglądarce: strona startowa na szerokim ekranie i przy 375 px, dostęp do konta z górnego paska, stan lokalnego konta, dziewięć laboratoriów i jawny brak konfiguracji Learning. Przywrócono viewport i zatrzymano tymczasowe procesy.
- Testy regresji obejmują pełny/ucięty wynik curation, downstream notebook, awarię auth/config i retry, wymaganie sesji przy OIDC, paginację Runs z zachowaniem filtrów oraz przypięcie zapisanej rewizji anotacji.
- Nie wykonano testów realnego logowania Authentik, izolacji organizacji, wygasania grantów ani migracji produkcyjnej: ta część backendu pozostaje niewdrożona. Test anotacji zastępuje renderer canvasu, więc nie jest testem rysowania geometrii ani pełnym E2E.

## Kontynuacja — ochrona szkiców UX-05

Wykonana kolejna część etapu 1; pozostałe etapy migracji zachowują dotychczasowy stan.

- Wspólny `useUnsavedChanges`: potwierdzenie odrzucenia lokalnego szkicu albo pozostanie w edytorze. Osobne ostrzeżenie przeglądarki przy reload/zamknięciu. Samo potwierdzenie nie oznacza szkicu jako zapisanego, więc nieudany import nie wyłącza ochrony.
- **Query:** zmiana czasu/filtrów nie usuwa tekstu. Wejście w inny link zapytania pyta o odrzucenie zmian i po akceptacji odtwarza tekst z URL. Pomyślny Run aktualizuje link tylko wtedy, gdy wysłany tekst nadal jest bieżący i strona pozostaje otwarta.
- **One script:** ochrona tekstu, nazwy, opisu i docelowego datasetu; decyzja przed wczytaniem przykładu lub zapisanej receptury. Nieudany zapis pozostawia szkic niezapisany; odpowiedź udanego zapisu potwierdza wyłącznie wysłany stan, zachowując nowsze zmiany.
- **Pipeline:** ochrona definicji, metadanych i zgłaszanego szkicu notebooka; decyzja przed zastąpieniem flow. Późne pobranie listy nie nadpisuje już wpisanej nazwy/opisu pustego canvasu. Save aktualizuje przypięte bloki tylko, jeśli użytkownik ich w międzyczasie nie zmienił. Import i tworzenie nowego notebook flow czasowo blokują edycję; po opuszczeniu strony ich odpowiedź nie przenosi użytkownika z powrotem.
- Dodano komunikaty niezapisanego stanu i dostępne nazwy edytora Query, pól pipeline oraz jego przycisku zapisu.

Walidacja tej kontynuacji:

- `npm test -- --reporter=dot`: **40 plików, 319 testów**, w tym 12 nowych regresji.
- `npm run build`: granice architektury, Vite i TypeScript — powodzenie; pozostaje ostrzeżenie o głównym chunku powyżej 500 kB.
- `git diff --check`: bez błędów.
- Regresje sprawdzają anulowanie/akceptację przejścia, Wstecz i `beforeunload`, zmianę linku Query, zastępowanie szkiców, błąd zapisu, edycję podczas opóźnionego Run/Save oraz pozostanie na wybranej stronie po późnej odpowiedzi.
- Testy używają prawdziwego routera, klienta HTTP i kontrolowanych odpowiedzi API. Wstecz/`beforeunload` są sprawdzane przez browser history w jsdom; systemowe okno potwierdzenia jest stubowane, a renderer canvasu pipeline zastąpiony. Ta iteracja nie obejmowała nowego przeglądu wizualnego ani pełnego E2E z usługami.

Ochrona nawigacji nie jest automatycznym zapisem ani odzyskiwaniem po awarii przeglądarki.

## Kontynuacja — prompty, harmonogramy i review

- **Prompty:** chronione tworzenie pierwszego promptu i publikacja kolejnej wersji — tekst, nazwa/opis lub notatka i opcja produkcji. Anulowanie, zamknięcie formularza przyciskiem otwarcia, zmiana wersji i opuszczenie strony wymagają decyzji, gdy istnieje szkic. Edycja jest blokowana na czas publikacji. Błąd zachowuje formularz; sukces otwiera konkretną zapisaną wersję bez ponownego pytania. Późna odpowiedź nie przywraca opuszczonej strony. Otwarcie edytora przypina wersję bazową w URL.
- **Harmonogram:** przypięty do nazwy zapisanej definicji, więc wpisywanie nowej nazwy pipeline nie przełącza formularza na inny harmonogram. Zastępowanie pipeline uwzględnia szkic harmonogramu; przed zapisem pod inną nazwą trzeba zapisać lub odrzucić zmiany harmonogramu. Pierwszy odczyt blokuje edycję i zapis; błąd odczytu nie udaje pustego harmonogramu. Odpowiedź zapisu potwierdza tylko wysłane ustawienia, zachowując nowsze zmiany. Dostępne odrzucenie szkicu i dostępne nazwy pól.
- **Case review:** jedna decyzja o opuszczeniu strony, ukryciu review lub zmianie datasetu/źródła propozycji obejmuje wszystkie zmienione formularze. Zapis propozycji i oczekiwanej odpowiedzi zachowuje nowszy tekst wpisany podczas żądania. Zmieniona odpowiedź wymaga zapisu przed zatwierdzeniem/odrzuceniem; publikacja czeka na rozstrzygnięcie lokalnych szkiców. Są osobne przyciski odrzucenia zmian propozycji i wiersza review.
- **Conversation review:** chronione notatki i preferencje przy zmianie rozmowy, filtrów i obszaru; kilka zmienionych wiadomości daje jedno potwierdzenie. Bieżąca rozmowa jest przypięta w URL, więc odświeżenie archiwum nie wybiera innej. Formularz blokuje edycję na czas decyzji, a nieudany zapis zachowuje notatkę i preferencję.

Walidacja: **44 pliki, 336 testów**, w tym **17 nowych regresji** tej iteracji; `npm run typecheck`, `npm run build` i `git diff --check` zakończone powodzeniem. Pozostaje ostrzeżenie Vite o głównym chunku ponad 500 kB. Testy nawigacji nowych formularzy używają rzeczywistego routera oraz kontrolowanych odpowiedzi HTTP; testy wcześniejszych, niezwiązanych funkcji zachowują swoje stuby routera. Nie wykonano nowego przeglądu wizualnego ani pełnego E2E z usługami.

Zaplanowaną w tej iteracji kontynuację historii recipe/pipeline i szkiców notebooków opisano poniżej. Pozostałe punkty UX-01/02/06 oraz rollout shella zachowują dotychczasową kolejność.


## Kontynuacja — historia kontekstu i zagnieżdżone notebooki

- **One script:** Wstecz/Dalej oraz wejście w inny link odtwarzają kod, silnik, nazwę, opis i docelowy dataset. Wybór zapisanej receptury i przykładu tworzy wpis historii z pełnym kontekstem. Zmiana czasu zachowuje bieżący szkic. Odrzucenie zmian jest potwierdzane raz; późny zapis poprzedniej receptury nie zmienia aktualnego formularza ani adresu.
- **Pipeline:** nazwa i rewizja określają kontekst edytora. Wstecz/Dalej odtwarzają definicję, metadane i przypiętą rewizję, a zmiana czasu/widoku zachowuje bieżący szkic. Przejście do innego kontekstu uwzględnia definicję, harmonogram i notebooki oraz usuwa wyniki/komunikaty poprzedniego kontekstu. Późne zapisy, importy, utworzenie notebooka i odpowiedzi wykonania nie przywracają poprzedniego edytora.
- **Historyczny odczyt:** addytywny `GET /api/v1/curation-pipelines/{name}/revisions/{revision}` czyta istniejący niezmienny obiekt rejestru. Pozwala otworzyć historyczny link także po reload i przesunięciu head. Brak rewizji zwraca 404; UI pokazuje brak danych zamiast bieżącego pipeline. Nie zmieniono danych ani identyfikatorów zapisanych wersji. Odświeżono OpenAPI i wygenerowany klient panelu.
- **Importowane/nowe flow:** osobny lokalny identyfikator zapobiega pomyleniu ich z zapisanym pipeline o tej samej nazwie. W ramach otwartej strony historia odtwarza wczytany import. Po opuszczeniu strony/reload lokalna definicja nie jest dostępna — UI wymaga ponownego importu lub otwarcia zapisanego pipeline. To nadal ochrona przed odrzuceniem szkicu, bez autosave i odzyskiwania po awarii.
- **Notebooki:** zmiana widoku, wybór innego bloku, zastąpienie pipeline i usunięcie bloku chronią lokalny kod oraz niepoprawny JSON ustawień. Zastąpienie pipeline resetuje edytory nawet przy tych samych ID bloków. Dostępna jest osobna akcja odrzucenia niepoprawnych ustawień. Zapis potwierdza tylko wysłany kod, zachowuje nowszy kod, ustawienia i tytuł bloku; błąd pozostawia szkic. Odpowiedź po odmontowaniu edytora nie zmienia nowego kontekstu. Tworzenie kopii blokuje edycję jej kodu i ustawień do odpowiedzi.

Walidacja tej kontynuacji:

- Panel: **44 pliki, 351 testów**, w tym **15 nowych regresji** historii, importu, historycznych linków i zagnieżdżonych szkiców.
- API: **182 testy HTTP**, w tym nowy scenariusz odczytu starej rewizji po zmianie head, nazw z ukośnikiem, dostępu czytelnika, odmowy bez logowania i braku rewizji.
- `npm run build`: granice architektury, Vite i TypeScript — powodzenie. Pozostaje ostrzeżenie o głównym chunku ponad 500 kB.
- `just openapi-check` oraz `git diff --check`: powodzenie.
- Historia przeglądarki jest sprawdzana przez browser history w jsdom; okno potwierdzenia jest stubowane. Edytory notebooków, router i klient HTTP są rzeczywiste, odpowiedzi usług kontrolowane, renderer canvasu zastąpiony. Nie wykonano nowego przeglądu wizualnego ani pełnego E2E z uruchomionym marimo/Authentikiem.

Publikację próbek UX-01 wykonano w kontynuacji opisanej poniżej. Rozdzielenie stanów w metrykach API (UX-02), operacje anotacji (UX-06) oraz rollout shella (UX-09) wykonano w kontynuacjach poniżej. **Cała migracja nadal pozostaje w toku.**

## Kontynuacja — jawna publikacja próbek UX-01

- **Pipeline i One script:** Preview oraz ucięty wynik pozostają dostępne do osobnej akcji `Publish sample`. Samo wykonanie ich nie publikuje. Przed zapisem widać liczbę wierszy, docelowe `<dataset>/samples`, tryb i ucięte etapy oraz informację, że wynik nie jest losową ani reprezentatywną próbką. Zwykła publikacja nadal wymaga kompletnego wykonania; wynik `Run to here` nie jest traktowany jako wyjście całego pipeline.
- **Pochodzenie:** pipeline zapisuje rewizje notebooków faktycznie użyte w wykonaniu, również gdy ich head zdążył się zmienić. Obcięcie wcześniejszego etapu pozostaje oznaczone, nawet jeśli następny notebook zwrócił pełny wynik. Zmiana kontekstu unieważnia wynik, a błąd publikacji pozwala ponowić zapis bez ponownego wykonania.
- **Rejestr datasetów:** wersja próbki trwale przechowuje `sample.mode` i `sample.truncated_stages`; metadane uczestniczą w identyfikatorze treści i weryfikacji artefaktu. Identyfikatory zwykłych i historycznych wersji pozostają bez zmian. Lista datasetów i eksplorator wersji pokazują oznaczenie próbki także po ponownym otwarciu.
- **API:** dedykowany `POST /api/v1/dataset-samples` wymaga oznaczenia próbki i uprawnień edytora. Starszy backend bez tej trasy zwraca błąd; panel nie ponawia zapisu przez zwykły endpoint, który mógłby zignorować metadane. Odświeżono OpenAPI i wygenerowany klient panelu.

Walidacja tej kontynuacji:

- Panel: **46 plików, 363 testy**, w tym nowe regresje publikacji, retry, nieaktualnych wyników, dokładnych rewizji notebooków, starszego API i ponownego otwarcia próbki.
- API: **183 testy HTTP**, w tym publikacja próbki przez edytora, odmowa dla czytelnika, wymagane metadane i ich zachowanie w katalogu oraz odczycie wierszy.
- Rejestr datasetów: **30 testów**; publikacja po stronie serwera: **4 testy**. Obejmują zachowanie zwykłych wersji, odrębność i idempotencję próbek, walidację metadanych oraz wykrycie ich zmiany w artefakcie.
- `npm run build`: granice architektury, Vite i TypeScript — powodzenie. Pozostaje ostrzeżenie o głównym chunku ponad 500 kB.
- `just openapi-check` oraz `git diff --check`: powodzenie.
- Testy panelu używają rzeczywistego routera i klienta HTTP z kontrolowanymi odpowiedziami; renderer canvasu jest zastąpiony. Nie wykonano nowego przeglądu wizualnego ani pełnego E2E z usługami.


## Kontynuacja — wiarygodność metryk UX-02

- **API i kontrakt:** buckety `GET /api/v1/metrics` zawierają addytywne liczniki `succeeded` i `running` obok istniejących `failed` i `runs`. Statusy są bieżące, przypisane do przedziału rozpoczęcia runu; w każdym buckecie ich suma równa się `runs`. Odświeżono OpenAPI i wygenerowany klient panelu.
- **Spójność tokenów:** kafelki, breakdowny LLM i timeline powstają z tego samego zbioru zachowanych, zakończonych spanów. Filtr modelu obejmuje LLM calls, tokeny i latencję LLM także na wykresie. Brak lub usunięcie spanów nie powoduje już pokazywania na wykresie tokenów z innego źródła niż kafelki. Runy nadal liczą się bez spanów.
- **Zakres filtrów:** zachowano dotychczasowy kontrakt — czas wybiera runy według rozpoczęcia, agent wybiera runy zawierające agenta, a model zawęża wywołania LLM; nie zawęża liczników run/tool/step. Panel opisuje zakres i zachowuje filtry przy zmianie czasu. Zmiana całej obserwowalności na wspólny filtr obiektów pozostaje w FLOW-01.
- **Panel:** osobne serie succeeded/running/failed; procent sukcesu tylko od zakończonych. Zero zakończonych daje neutralne `No data`; brak próbek LLM nie pokazuje pozornej latencji 0 ms. Stos tokenów rozdziela niebuforowane wejście, wyjście i cache będący częścią wejścia. Ostrzeżenie o limicie retencji mówi o możliwej niekompletności, nie deklaruje pewnego usunięcia historii.
- **Rollout:** przy starszym API bez nowych liczników panel pozostawia serię `running or succeeded` z wyjaśnieniem. Pełna spójność filtrowanego wykresu tokenów wymaga wdrożenia nowego backendu. Historia danych nie jest przepisywana.
- **Mały ekran:** legendy obu wykresów otrzymały osobny wiersz, aby nie stykały się z etykietami czasu.

Walidacja tej kontynuacji:

- Panel: **47 plików, 372 testy**, w tym **9 regresji UX-02**: pusto/running-only, mianownik sukcesu, trzy statusy, starsze API, cache, zachowanie filtrów i błąd HTTP.
- Agregacja metryk: **10 testów**; odpowiedź endpointu: **1 nowy test HTTP** obejmujący rzeczywiste zdarzenia run.started/run.completed/run.failed i liczniki bucketów.
- `cargo clippy -p aiwatcher-projector --all-targets -- -D warnings`: powodzenie.
- `npm run build`: granice architektury, Vite i TypeScript — powodzenie; pozostaje wcześniejsze ostrzeżenie o głównym chunku ponad 500 kB.
- Ponowna generacja OpenAPI i porównanie z kontraktem oraz `git diff --check`: powodzenie.
- Lokalny Chromium, kontrolowane odpowiedzi API: 1440 px oraz 375 px, motywy jasny/ciemny. Bez błędów JavaScript i poziomego przepełnienia strony; sprawdzono zrzuty i poprawiono legendy na małym ekranie. To kontrola renderowania panelu, nie pełny E2E z uruchomionym backendem i rzeczywistym ingestem.

**Cała migracja nadal pozostaje w toku.**


## Kontynuacja — historia rysunku i nawigacja obrazów UX-06

- **Undo/redo:** przyciski oraz Ctrl/Cmd+Z, Ctrl/Cmd+Shift+Z i Ctrl+Y cofają/przywracają tworzenie, usunięcie, geometrię, atrybuty i powiązania anotacji. Przeciągnięcie jest jednym krokiem, niezależnie od liczby ruchów wskaźnika. Nowa edycja usuwa gałąź redo, operacja bez zmiany nie dodaje kroku. Historia obejmuje ostatnie 100 zmian i resetuje się przy otwarciu innego obrazu/rewizji; zapis aktualizuje punkt odniesienia i zachowuje undo/redo.
- **Zakres historii:** to lokalna historia rysunku. Nie cofa decyzji review ani zapisanych, niezmiennych rewizji; cofnięcie po zapisie tworzy niezapisany szkic. Nie jest utrwalana po reload ani po opuszczeniu obrazu. Skróty edytora nie przejmują undo w polach tekstowych.
- **Niedokończony rysunek:** canvas zgłasza otwarty kształt do ochrony nawigacji i reload. Zapis oraz `Save draft & leave` są niedostępne do zakończenia lub anulowania kształtu, aby nie zapisać niepełnego szkicu jako kompletnego. `Discard & leave` jawnie odrzuca rozpoczęty kształt. Dotychczasowe Backspace/Remove last point i Escape/Cancel obsługują punkty otwartego rysunku. Pasek rysowania zawija się na wąskim ekranie.
- **Paginacja:** istniejące `offset`/`next_offset` API, strony po 50 obrazów, `useInfiniteQuery` i wirtualizowana lista. Deduplikacja po `image_id`, licznik załadowanych/pasujących i jawne `Load more images`. Błąd kolejnej strony zachowuje już załadowane wyniki i pozwala ponowić żądanie.
- **Poprzedni/następny:** operują w kolejności filtrowanej listy; Next na końcu załadowanej strony pobiera kolejną. Spóźniona odpowiedź nie zmienia obrazu, jeśli użytkownik zmienił kontekst. Wybrany wiersz jest przewijany do widoku. Obraz otwarty poza załadowanymi wynikami ma jawny komunikat; kolejne wyniki można wczytać ręcznie.
- **Stabilny kontekst:** pierwszy obraz jest przypięty w URL. Zmiana filtra lub odświeżenie listy nie podmienia canvasu ani szkicu. Zapis i przejście, odrzucenie oraz pozostanie zachowują dotychczasowy mechanizm blokowania; nieudany zapis utrzymuje szkic i cel przejścia do ponowienia. Zmiana obrazu usuwa lokalną historię poprzedniego. Dodano stany pobierania, błędu i retry szczegółu oraz puste wyniki filtrów.
- **Mały ekran:** skrócona, przewijana lista obrazów, widoczny przycisk kolejnej strony i zawijane kontrolki nawigacji. Aktywny obraz oraz narzędzie mają oznaczenia dostępności; przycisk usuwania jest widoczny przy fokusie i na małym ekranie.

Walidacja tej kontynuacji:

- Panel: **48 plików, 384 testy**, w tym **12 nowych regresji UX-06** obejmujących historię, grupowanie przeciągnięć, zapis, usuwanie, paginację, filtry, spóźnione odpowiedzi, retry, otwarty kształt, Wstecz i `beforeunload`.
- `npm run build`: granice architektury, Vite i TypeScript — powodzenie. Pozostaje wcześniejsze ostrzeżenie o głównym chunku ponad 500 kB.
- `git diff --check`: powodzenie. Backend i kontrakt API nie wymagały zmian w tym etapie.
- Test Chromium używa rzeczywistego canvasu i wirtualnej listy oraz kontrolowanych odpowiedzi API: utworzenie punktu, zapis, przeciągnięcie w wielu ruchach, zapis, pojedyncze undo przywracające pierwotną geometrię, redo, anulowanie/odrzucenie przejścia, granica stron 50→51 i ochrona otwartego wielokąta. Bez błędów JavaScript.
- Obejrzano podgląd przy 1440 px i 375 px, w jasnym i ciemnym motywie; bez poziomego przepełnienia strony. Poprawiono wysokość listy i miejsce na przycisk kolejnej strony na telefonie.
- Testy komponentu w jsdom zastępują canvas i układ wirtualnej listy; scenariusz Chromium sprawdza ich rzeczywiste renderowanie i gesty. Nie wykonano pełnego E2E z backendem, magazynem i rzeczywistym importem obrazów. Lokalna historia nie jest mechanizmem odzyskiwania po awarii.

Rollout/rollback shella, preferencje startu i przypięcia (UX-09) wykonano w kontynuacji poniżej. Cała migracja pozostaje w toku.

## Kontynuacja — rollout i preferencje nawigacji UX-09

- **Dwa układy:** wszystkie obszary w sidebarze albo klasyczne sekcje z lokalnym menu. Przełączenie zmienia nawigację bez remountu aktywnego edytora, zmiany URL, utraty szkicu ani przeładowania strony. Oba układy zachowują bieżące obszary, dostęp do wyszukiwania i konta; mały ekran ma przewijane grupy i zawijane kontrolki.
- **Rollout/rollback:** flaga kompilacji `VITE_AIWATCHER_SHELL=user|new|classic`. Domyślnie `user`: nowy układ z możliwością lokalnego przełączenia. `new` lub `classic` wymuszają układ wdrożenia, zachowując zapisaną preferencję użytkownika do powrotu w trybie `user`. Instrukcja przebudowania panelu i rollbacku znajduje się w `apps/panel/README.md`. Flaga nie zmienia auth, uprawnień API ani zakresu danych.
- **Start:** wybór strony w Your work → Navigation preferences. Preferencja działa przy wejściu na sam `/`; istniejące deep linki, parametry i fragmenty pozostają na swoim miejscu. Logo oraz linki Your work jawnie otwierają `/?start=workspace`, także ze stron błędów. Zmiana preferencji nie opuszcza otwartego przeglądu. Przekierowanie zastępuje wpis historii i wykonuje się jednokrotnie.
- **Przypięcia:** do 20 nazwanych linków. `Pin this view` zapisuje bieżący URL z filtrami, rewizją, `as_of` i fragmentem; lista na stronie startowej i w rozwiniętym sidebarze umożliwia powrót oraz usuwanie. Zapis nie obejmuje niezapisanej treści edytora ani retencji danych; otwieranie nadal podlega obecnym uprawnieniom serwera i ochronie szkicu. Akcje nawigacji używają rzeczywistego routera.
- **Trwałość:** schemat 1 w istniejącym zakresie lokalnych widoków: instancja API oraz dostawca/stabilny subject, osobno dla trybu lokalnego. Zmiana tożsamości usuwa poprzednie przypięcia ze stanu UI. Zdarzenia storage synchronizują karty; zapis scala zmianę z aktualnym kompatybilnym rekordem. Niedostępny magazyn pozwala zastosować preferencję w sesji i pokazuje komunikat.
- **Migracja preferencji:** przy braku nowego rekordu odczytywany jest dawny `aiwatcher.sidebar`; pierwszy zapis utrwala nowy schemat i weryfikuje odczyt. Oryginał pozostaje. Istniejące widoki treningu/ewaluacji oraz wygląd zachowują format i klucze. Uszkodzony lub nieznany schemat nawigacji nie jest nadpisywany; zewnętrzne adresy, nieznany start i duplikaty są odrzucane.
- **Diagnostyka:** agregacja zakończonych przejść między obszarami według układu, do 100 kombinacji w pamięci bieżącej sesji. Bez ID obiektów, URL, tekstu query i tożsamości; zablokowane przejścia i zmiany filtrów w jednym obszarze nie zwiększają liczników. To lokalna diagnostyka, bez serwisu centralnej analityki, wysyłania zdarzeń i automatycznych kohort.

Walidacja tej kontynuacji:

- Panel: **49 plików, 396 testów**, w tym **12 nowych regresji UX-09**: migracja preferencji, nieznany/uszkodzony schemat, błąd magazynu, zmiana tożsamości, druga karta, preferowany start, wymuszony rollback, szkic, odtworzenie przypięcia i liczniki przejść. Testy używają rzeczywistego routera i ochrony szkiców, z kontrolowanym zakresem tożsamości i lokalnym magazynem.
- `npm run build`: granice architektury, Vite i TypeScript — powodzenie. Pozostaje wcześniejsze ostrzeżenie o głównym chunku ponad 500 kB.
- Chromium: 1440 px i 375 px, oba układy, motywy jasny/ciemny; bez poziomego przepełnienia strony. Sprawdzono wyszukiwanie klawiaturą, preferowany start i powrót do przeglądu, zmianę układu na rzeczywistym edytorze Query z niezapisanym tekstem, anulowanie/akceptację wyjścia i odtworzenie przypiętego URL po reload. Bez błędów JavaScript. Obejrzano zrzuty i przeniesiono ustawienia pod obszary pracy, z bezpośrednim linkiem przy tytule strony.
- `git diff --check`: powodzenie. W tym etapie nie zmieniano backendu ani kontraktu API. Przeglądarka korzystała z kontrolowanych odpowiedzi usług; nie jest to pełny E2E z Authentikiem ani wdrożenie produkcyjne.

IAM-01 rozpoczęto w kontynuacji poniżej. Pełne egzekwowanie zakresu z testami izolacji nadal musi poprzedzić aktywowanie selektorów i zarządzania zespołami. Cała migracja pozostaje w toku.

## Kontynuacja — fundament backendu IAM-01

Pierwszy spójny zakres backendu, **nie zakończenie IAM-01 ani wdrożenie bezpiecznej pracy wielu organizacji**. Szczegółowy kontrakt i granice: [README IAM](../crates/aiwatcher-iam/README.md).

- **Model:** osobne ID organizacji, projektów, zespołów i grantów; principal jako dokładna para dostawca/subject. Role organizacji owner/admin/member oraz projektu viewer/editor/admin są niezależne od dotychczasowych ról instancji. Zespoły są lokalne, bez importowania grup IdP.
- **Polityka:** członkostwo organizacji, także owner/admin, samo nie daje odczytu/zapisu projektu. Twórca projektu otrzymuje jawny grant admin w tej samej transakcji. Administrator organizacji zarządza zwykłymi członkami, zespołami i grantami; tylko owner zmienia ownerów/adminów. Administrator projektu zarządza grantami swojego projektu. Nie można usunąć ani zdegradować ostatniego ownera.
- **Granty:** suma aktywnych grantów bezpośrednich i zespołowych. `valid_from` jest włączające, `edit_until`/`read_until` wyłączające; po końcu edycji pozostaje viewer, do końca odczytu. Brak `read_until` oznacza dalszy odczyt. Wygaśnięcie jednego grantu nie usuwa niezależnego dostępu; wynik ujawnia wszystkie aktualne źródła, ich ID i okna. Usunięcie członkostwa usuwa granty bezpośrednie i członkostwa zespołowe, więc ponowne dodanie użytkownika ich nie przywraca.
- **Atomowość:** ta sama polityka w pamięci i PostgreSQL. Uprawnienie jest sprawdzane pod blokadą obejmującą zapis; czas pobierany dopiero po jej uzyskaniu. PostgreSQL przechowuje wersjonowany dokument IAM na organizację, z indeksem GIN członkostw i blokadą wiersza. Limit 4 MiB dotyczy metadanych organizacji, nie zawartości zasobów. Ten wariant serializuje mutacje w jednej organizacji; dalsza normalizacja wymagana przed przekroczeniem tej skali. Migracja schematu ma własną blokadę i rejestr wersji, niezależne od workflow.
- **Powiązanie SSO:** addytywne `Identity.issuer` pochodzi ze zweryfikowanego wystawcy OIDC i jest zachowane w nowych sesjach. `Authenticator::iam_principal` wymaga odpowiedniego issuer, ważnej tożsamości oraz credential Session/Bearer; nie przenosi emaila, grup ani ról instancji do IAM. Zmiana wystawcy przy zachowanym kluczu podpisu nie pozwala użyć nowej sesji poprzedniego wystawcy. Starsze sesje bez issuer pozostają zgodne ze starym API, lecz przed IAM wymagają ponownego logowania. Session używa czasu życia sesji, Bearer czasu życia tokena. Odświeżono OpenAPI i klienta panelu.
- **Ograniczenia:** magazyn IAM nie jest jeszcze podłączony do uruchomionego serwera; nie ma nowych endpointów zarządzania ani selektorów w panelu. Nie ma jeszcze trwałego audytu mutacji IAM. Tryby anonymous/local/proxy i statyczne/attempt credentials nie są automatycznie zamieniane na principal OIDC. Istniejące zasoby, ingest, cache, query/runtimes, SSE/WebSocket i workerzy zachowują dotychczasowy zakres — testy tej części dotyczą metadanych i polityki IAM, nie izolacji wszystkich danych aplikacji. Nie przeprowadzono migracji danych użytkownika ani wdrożenia produkcyjnego.

Walidacja tej kontynuacji:

- `cargo test -p aiwatcher-auth -p aiwatcher-iam`: **82 testy**, w tym 8 nowych scenariuszy kontraktu IAM i 4 nowe scenariusze integracji OIDC z rzeczywistymi podpisami RS256 lokalnego dostawcy testowego.
- Rzeczywisty, osobny PostgreSQL **18.6** w tymczasowym kontenerze: **13 testów**, wszystkie uruchomione jawnie. Obejmują ten sam kontrakt IAM, reconnect i powtarzalną migrację, wyścig usuwania ownerów, nadawanie grantu równocześnie z usunięciem członka, wygaśnięcie uprawnienia podczas oczekiwania na blokadę oraz odmowę odczytu/zapisu nieznanego lub niespójnego dokumentu bez nadpisania go. Kontener testowy usunięto po walidacji.
- `cargo test -p aiwatcher-api --test http`: **184 testy**, powodzenie.
- `cargo clippy -p aiwatcher-auth -p aiwatcher-iam --all-targets --all-features -- -D warnings`: powodzenie.
- `npm run build`: granice architektury, Vite i TypeScript — powodzenie; wcześniejsze ostrzeżenie o głównym chunku ponad 500 kB pozostaje.
- `just openapi-check` oraz `git diff --check`: powodzenie. Nie wykonywano nowego przeglądu wizualnego — interfejs użytkownika nie zmienił się w tym etapie.

Podłączenie magazynu i principal do API wraz z bootstrapem i audytem wykonano w kontynuacji poniżej. Następna bramka IAM-01 to **wymuszanie zakresu w adapterach zasobów**. UI organizacji/projektów pozostaje za bramką pełnych testów izolacji; sama kontrola metadanych nie spełnia tego kryterium.


## Kontynuacja — API i atomowy audyt IAM-01

Drugi etap backendu: **działające, opcjonalne API metadanych IAM**, nadal bez pełnej izolacji danych aplikacji. Kontrakt, przykłady i konfiguracja: [README IAM](../crates/aiwatcher-iam/README.md).

- **Serwer:** `AIWATCHER_IAM_POSTGRES_URL` jawnie włącza magazyn PostgreSQL i jego migracje. Domyślnie wyłączony. Wymagany OIDC oraz feature `aiwatcher-server/postgres`; brak feature lub inny tryb auth powoduje odmowę uruchomienia, bez fallbacku do pamięci. Konfiguracja IAM jest niezależna od magazynu workflow. `.env.example` zawiera opis, ale rzeczywista konfiguracja wdrożenia nie została zmieniona.
- **API:** sześć operacji pod `/api/v1/iam`: organizacje użytkownika, utworzenie organizacji, komendy członkostw/zespołów/projektów/grantów, projekty z aktywnym grantem, aktualny dostęp do projektu oraz stronicowany audyt. OpenAPI i klient panelu zaktualizowane. Nie aktywowano UI organizacji/projektów.
- **Autoryzacja:** principal pochodzi z uwierzytelnionej sesji/bearer OIDC, nigdy z body. Bootstrap wymaga administratora instancji i ustawia jego samego jako pierwszego ownera. Późniejsze operacje korzystają z bieżących ról IAM; rola instancji nie otwiera cudzej organizacji. Cudze zasoby zwracają 404, brak wymaganej roli 403, usunięcie ostatniego ownera 409, błędy magazynu ogólne 503 bez szczegółów bazy. Starsze sesje bez issuer, obcy issuer, wygasła tożsamość i tryby bez principal OIDC są odrzucane.
- **Zapisy z przeglądarki:** wymagany nagłówek `X-AIWatcher-IAM: 1` blokuje zapis przez formularz innego originu z dołączonym cookie; JavaScript innego originu wymaga przejścia jawnej polityki CORS. Odpowiedzi IAM mają `Cache-Control: no-store`. Nieznane pola komend/tworzenia organizacji są odrzucane, w tym podany przez klienta actor/owner.
- **Audyt:** osobna tabela `iam_audit`; organizacja, actor, czas serwera, komenda i wynik oraz kolejny numer w organizacji. Zapis wraz ze zmianą stanu w jednej transakcji — błąd audytu wycofuje również utworzenie organizacji lub zmianę uprawnień. Odczyt wyłącznie przez bieżącego ownera/admina organizacji, ze sprawdzeniem roli w tej samej blokadzie co pobranie strony. `after` jest wyłączające; `limit` 1–100. Migracja 0002 nie przepisuje dokumentów schematu 1 ani nie tworzy fikcyjnej historii wcześniejszych zmian. Audyt dotyczy udanych mutacji, nie prób odrzuconych ani odczytów danych; eksport i retencja pozostają osobnym zakresem.
- **Granica etapu:** trasy zasobów, klucze object store, ingest, projekcje, query, streamy, wykonania i workerzy zachowują dotychczasowe działanie. Utworzenie organizacji/projektu nie przenosi do niego istniejących zasobów. Nie przeprowadzono migracji danych użytkownika ani wdrożenia produkcyjnego.

Walidacja:

- Auth/IAM: **83 testy**, w tym wspólny kontrakt pamięć/PostgreSQL rozszerzony o autoryzację, stronicowanie i atomowość audytu.
- HTTP: **188 testów**, w tym 4 nowe scenariusze IAM z podpisaną sesją przechodzącą przez middleware oraz lokalnym discovery/JWKS OIDC. Sprawdzono bootstrap, odmowy między organizacjami, próby podstawienia actora, brak nagłówka mutacji, cofnięcie i wygaśnięcie grantu w tej samej sesji, audit pagination i no-store. Testy używają routera axum i pamięciowego IAM, nie produkcyjnego Authentika ani całego stosu usług.
- Kontrakt API: **23 testy biblioteki**, bez powielonych operationId i brakujących odwołań do schematów.
- PostgreSQL **18.6**, osobny tymczasowy kontener: **17 testów**. Nowe przypadki obejmują błąd zapisu audytu z rollbackiem bootstrapu i komendy, spójność i kolejność historii przy równoległych zmianach oraz upgrade schematu 1 bez przepisywania danych.
- Serwer: 3 testy konfiguracji/wiringu IAM w buildzie domyślnym (wyłączony magazyn, wymagany OIDC, brak fallbacku bez feature PostgreSQL). Dodatkowo **33 testy konfiguracji** z feature PostgreSQL — powodzenie.
- Clippy API/IAM/server z `--all-targets --features aiwatcher-server/postgres -- -D warnings`: powodzenie.
- Build panelu (architektura, Vite, TypeScript): powodzenie; wcześniejsze ostrzeżenie o głównym chunku ponad 500 kB pozostaje. Bez nowego przeglądu wizualnego, ponieważ UI nie zmieniono.
- `just openapi-check` i `git diff --check`: powodzenie. Tymczasowy kontener PostgreSQL zatrzymano i usunięto po walidacji.

Następny etap IAM-01: **zakres organizacja/projekt w trwałych zasobach i adapterach**, z manifestem migracji istniejących danych oraz rzeczywistymi testami odmowy odczytu i zapisu między projektami. W dalszej kolejności zakres musi objąć query, strumienie, harmonogramy i wykonania wraz z odebraniem dostępu. UI pozostaje za bramką zakończenia tej izolacji.


## Kontynuacja — pierwszy rejestr zasobów z zakresem IAM-01

Data: 15.09.2026. **Pierwszy adapter danych, nie zakończenie izolacji całej aplikacji.**
Szczegóły kontraktu i narzędzia: [README IAM](../crates/aiwatcher-iam/README.md#first-resource-boundary-project-dataset-registry).

- **API:** 11 operacji rejestru pod `/api/v1/orgs/{organization}/projects/{project}`: lista/publikacja datasetów i próbek, odczyt/search/paginacja wierszy, receptury, pipeline’y i ich dokładne rewizje oraz biblioteka bloków. Wspólne handlery i schematy danych z rodziną starych tras; odświeżono OpenAPI i klienta panelu.
- **Autoryzacja:** principal ze zweryfikowanego OIDC i aktualny grant sprawdzany przy każdym żądaniu. Viewer czyta, editor/admin zapisuje niezależnie od roli instancji; sama własność organizacji nie otwiera projektu. Mutacje wymagają `X-AIWatcher-IAM: 1` i ponownego sprawdzenia grantu po odebraniu JSON, aby powolny upload nie przedłużał dostępu. Odmowy dla cudzej organizacji/projektu, grantów przed startem i po wygaśnięciu oraz cofniętych grantów. Odpowiedzi mają `Cache-Control: no-store`.
- **Trwały zakres:** wszystkie cztery kategorie rejestru używają kluczy `<prefix>/scopes/<org>/<project>/registry/...`. Zakres jest poza treścią i hashem; identyczne dane w dwóch projektach mają ten sam identyfikator wersji, lecz osobne obiekty i katalogi. Znajomość nazwy lub hasha nie pozwala odczytać innego projektu. Zachowane są wersje, rewizje, klasyfikacja próbek i pochodzenie. Adapter odmawia przepięcia już związanego rejestru do innego projektu oraz kluczy wychodzących z zakresu.
- **Zgodność przed cutover:** stare adresy nadal obsługują wyłącznie stary, globalny rejestr z dotychczasowymi uprawnieniami instancji. Nowe trasy nie mają fallbacku do tych danych; nieprzypisane zasoby nie pojawiają się w projekcie. Wyłączenie IAM nie ujawnia danych projektowych przez stare API. Utworzenie projektu nie przenosi danych. Panel i istniejące usługi nadal korzystają ze starych tras.
- **Dry-run:** operatorowy przykład `migration_manifest` w `aiwatcher-datasets` czyta istniejący snapshot plikowego object store i przyjmuje jawne UUID organizacji/projektu. Wypisuje JSON ze schematem, licznikami obiektów, mapowaniem kluczy, rozmiarami, SHA-256 oryginalnych bajtów, znanymi referencjami oraz stanem celu `absent/identical/conflict`. Obejmuje heady i historyczne rewizje czterech kategorii; pomija dane projektowe jako źródła. Nie zapisuje ani nie usuwa obiektów.
- **Granice dry-run:** to inwentaryzacja do przygotowania pełnego manifestu, bez wykonawcy migracji, wznowienia kopii, potwierdzenia istnienia celu w IAM, pełnej walidacji referencji, mapowania grup ani analizy zależności w tekście query. Niespójny JSON lub znikający obiekt przerywa odczyt; potrzebny jest snapshot, nie skan zmieniającego się magazynu. Referencje pozostają zapisane dosłownie, nie są przepisywane ani rozwiązywane przez globalny fallback.
- **Pozostałe granice:** zapis scoped pipeline’u nie uruchamia query/notebooka/workflow w zakresie projektu. Ewaluacje, ingest, pozostałe rejestry, cache, strumienie, harmonogramy i workerzy nadal wymagają własnej integracji. Sprawdzenie grantu przed operacją nie anuluje już dopuszczonej operacji object store i nie tworzy wspólnej transakcji IAM + zasoby. Selektory organizacji/projektów pozostają nieaktywne; nie wykonano migracji danych użytkownika ani wdrożenia.

Walidacja:

- `cargo test -p aiwatcher-api -p aiwatcher-datasets`: **249 testów** — 193 HTTP, 23 kontraktu/biblioteki API oraz 33 rejestru; wszystko przeszło. Dodano **5 scenariuszy HTTP** obejmujących podpisane sesje i middleware OIDC, rozdzielenie nazw/wersji/wierszy między projektami i organizacjami, wszystkie trasy curation, granty, nagłówek mutacji, wygasanie podczas uploadu oraz wyłączenie IAM.
- **3 nowe testy rejestru:** rzeczywisty plikowy object store, ponowne otwarcie zakresu i zachowanie hashy; deterministyczny dry-run, oryginalne referencje, częściowo istniejący cel i konflikt bez nadpisania; odmowa niepoprawnego JSON. Kontrole HTTP korzystają z pamięciowego IAM i object store, więc nie są pełnym E2E z Authentikiem, PostgreSQL, S3 i usługami runtime.
- Narzędzie CLI uruchomiono na osobnym snapshotcie testowym: dwa obiekty receptury, poprawne liczniki, niezmienione bajty źródła i brak zapisów do celu. Tymczasowy katalog usunięto po kontroli.
- `cargo clippy -p aiwatcher-api -p aiwatcher-datasets --all-targets -- -D warnings`: powodzenie.
- Panel: **49 plików, 396 testów**, powodzenie. Zmiana UI nie była częścią tego etapu; bez nowego przeglądu wizualnego.
- `npm run build`: granice architektury, Vite i TypeScript — powodzenie. Pozostaje wcześniejsze ostrzeżenie o głównym chunku ponad 500 kB.
- `just openapi-check` i `git diff --check`: powodzenie.

Następny zakres IAM-01: kolejne adaptery zasobów oraz przekazanie i egzekwowanie zakresu w query, notebookach i publikacji przez wykonania. Pełna walidacja manifestu, wykonawca migracji i cutover muszą poprzedzić udostępnienie całej aplikacji wielu organizacjom. **Cała migracja UX pozostaje w toku.**


## Kontynuacja — rejestr promptów z zakresem IAM-01

Data: 15.09.2026. **Drugi adapter danych; IAM-01 i cała migracja pozostają w toku.**
Kontrakt i granice: [README IAM](../crates/aiwatcher-iam/README.md#second-resource-boundary-project-prompt-registry).

- **API:** osiem operacji promptów pod `/api/v1/orgs/{organization}/projects/{project}` — lista z filtrami/paginacją, publikacja, szczegół, dokładna wersja, etykiety, zapis i odczyt optymalizacji oraz odbudowa indeksu. Wspólne handlery ze starymi trasami; kontrakt OpenAPI i klient panelu aktualizowane razem.
- **Autoryzacja:** wspólna z datasetami kontrola principal OIDC i bieżącego grantu. Viewer czyta, editor/admin zapisuje; własność organizacji i rola instancji nie zastępują grantu projektu. Wszystkie mutacje, także PUT etykiety i odbudowa, wymagają `X-AIWatcher-IAM: 1`. Mutacje JSON ponownie sprawdzają grant po odebraniu body. Odpowiedzi tras mają `Cache-Control: no-store`.
- **Magazyn:** osobne klucze `<prompt-prefix>/scopes/<org>/<project>/registry/` obejmują heady, wersje i optymalizacje. Zakres nie zmienia hasha tekstu, metadanych niezmiennej wersji ani ID raportu. Te same teksty w różnych projektach wymagają osobnych publikacji. Rejestr odmawia przepięcia do innego projektu i wyjścia klucza poza zakres.
- **Granice odczytu i zapisu:** znajomość cudzej nazwy, wersji lub ID optymalizacji nie daje odczytu, zmiany etykiety ani możliwości użycia baseline’u. Odbudowa czyta wyłącznie obiekty danego projektu; etykieta w jednym projekcie nie zmienia drugiego. Stare API nadal widzi tylko globalne prompty, również po wyłączeniu IAM. Prawidłowy stary prompt nazwany `scopes` pozostaje dostępny, a zakodowane separatory nie otwierają dostępu do projektu.
- **Granice etapu:** panel, SDK, runtime, cache i kontekst wykonań nadal wymagają integracji zakresu. Referencje datasetów/ewaluacji w optymalizacji i metadane parent/model są zachowane dosłownie, bez walidacji całego lineage. Obecny dry-run datasetów nie obejmuje jeszcze promptów. Nie wykonano migracji danych ani wdrożenia; selektory organizacji/projektów pozostają nieaktywne. Kontrola grantu nie anuluje już dopuszczonej operacji object store.

Walidacja:

- `cargo test -p aiwatcher-api -p aiwatcher-prompts -p aiwatcher-datasets`: **303 testy przeszły** (198 HTTP, 23 biblioteki/kontraktu API, 49 promptów, 33 datasetów). Dodano **5 scenariuszy HTTP i 2 testy magazynu**. **6 istniejących testów integracyjnych RustFS/S3 pominięto** zgodnie z ich jawnym `ignore`; nie uruchamiano usługi RustFS.
- `cargo test -p aiwatcher-server --test evaluation prompts::`: **4 testy**, powodzenie. Adapter ewaluacji obsługuje nowy błąd zakresu jako niedostępne źródło z odmową dostępu.
- Clippy API/promptów/datasetów oraz serwera z `--all-targets -- -D warnings`: powodzenie.
- Panel: **53 pliki, 417 testów**, powodzenie. `npm run build` (granice architektury, Vite, TypeScript): powodzenie; pozostaje wcześniejsze ostrzeżenie o głównym chunku ponad 500 kB. Interfejs nie zmienił się; nie wykonywano nowego przeglądu wizualnego.
- Wygenerowano OpenAPI i klienta panelu; sprawdzono osiem operacji, wymagane parametry organizacji/projektu i nagłówki mutacji, także PUT. `just openapi-check` oraz `git diff --check`: powodzenie.
- Testy HTTP korzystają z rzeczywistego routera i podpisanych sesji OIDC z lokalnym discovery/JWKS oraz pamięciowego IAM. Testy magazynu używają rzeczywistego systemu plików, ponownego otwarcia rejestru i kontroli zachowania wersji/raportów. Nie są pełnym E2E z Authentikiem, PostgreSQL, S3 ani runtime.

Następny zakres IAM-01: pozostałe adaptery zasobów oraz przekazanie i egzekwowanie zakresu w query, notebookach i publikacji przez wykonania. Pełny manifest, wykonawca migracji i cutover pozostają osobnymi bramkami przed udostępnieniem całej aplikacji wielu organizacjom.


## Kontynuacja — treningi i modele z zakresem IAM-01

Data: 15.09.2026. **Trzeci adapter danych; IAM-01 i cała migracja pozostają w toku.**
Kontrakt i granice: [README IAM](../crates/aiwatcher-iam/README.md#third-resource-boundary-project-training-and-model-registry).

- **API:** dziewięć operacji treningów/modeli pod `/api/v1/orgs/{organization}/projects/{project}` — lista i szczegół treningu, start rekordu, postęp, zakończenie, lista i szczegół modelu, rejestracja wersji i zmiana etykiety. Obie rodziny tras korzystają ze wspólnych handlerów.
- **Role:** viewer czyta, editor zapisuje wyniki treningu i rejestruje model, **admin projektu zmienia etykiety modeli**. Wspólna kontrola zakresu zachowuje wymaganą rolę do ponownego sprawdzenia po odebraniu JSON. Wygaśnięcie admina nie pozwala na promocję nawet przy niezależnym grancie editor. Stare trasy zachowują role instancji. Mutacje w trasach projektu wymagają nagłówka IAM, a odpowiedzi tych tras wyłączają cache.
- **Magazyn:** osobne rekordy i podsumowania treningów oraz heady i wersje modeli w `<training-prefix>/scopes/<org>/<project>/registry/`. Powtarzające się nazwy i ID nie łączą danych między projektami. Model czerpie pochodzenie z treningu w tym samym zakresie; nie odczytuje globalnego treningu. Zakres nie zmienia hashy modeli, krzywych, raportów ani zapisanych referencji. Rejestr odmawia przepięcia do innego projektu.
- **Zgodność i granice:** brak wskazanej wersji znanego modelu zwraca lokalny head bez pola `current`, zgodnie z dotychczasowym API. Nie ma zastąpienia wersją z innego projektu. Stare trasy nie ujawniają danych projektowych po wyłączeniu IAM. Parametry wersji i klucze magazynu odrzucają próby wyjścia poza zakres również na starych trasach.
- **Promocja:** nadal wymaga niezmiennej referencji datasetu i pomiaru held-out; administrator projektu nie omija tego warunku. To kontrola istniejących metadanych, bez dowodu wykonania ewaluacji ani weryfikacji dostępu do źródłowego datasetu.
- **Pozostały zakres:** URI checkpointów/profili i pakietów są zapisanymi referencjami, nie izolowanym magazynem bajtów modeli. Panel, SDK, poświadczenia maszynowe, runtime, cache i zatrzymywanie zadań wymagają własnej integracji. Endpoint start/finish zmienia rekord treningu, nie uruchamia ani nie zatrzymuje procesu. Nie wykonano migracji danych ani wdrożenia; selektory UI pozostają nieaktywne.

Walidacja:

- `cargo test -p aiwatcher-api -p aiwatcher-training`: **252 testy**, powodzenie (203 HTTP, 23 biblioteki/kontraktu API, 26 rejestru treningów/modeli). Dodano **5 scenariuszy HTTP i 2 testy magazynu**. Zestaw HTTP obejmuje również regresje wcześniejszych zakresów datasetów i promptów po rozszerzeniu wspólnej kontroli ról.
- `cargo clippy -p aiwatcher-api -p aiwatcher-training --all-targets -- -D warnings`: powodzenie.
- Panel: **53 pliki, 417 testów**, powodzenie. `npm run build`: granice architektury, Vite i TypeScript — powodzenie; pozostaje wcześniejsze ostrzeżenie o głównym chunku ponad 500 kB. UI nie zmieniono; bez nowego przeglądu wizualnego.
- Wygenerowano OpenAPI i klienta panelu; sprawdzono dziewięć operacji, wymagane parametry zakresu i nagłówki mutacji. `just openapi-check`, `cargo fmt --all --check` oraz `git diff --check`: powodzenie.
- Testy HTTP korzystają z rzeczywistego routera, podpisanych sesji OIDC i lokalnego discovery/JWKS oraz pamięciowego IAM. Testy magazynu używają osobnego katalogu plikowego, ponownego otwarcia rejestru i kontroli historycznej tożsamości modeli. Katalogi testowe usunięto po kontroli. Nie wykonywano E2E z Authentikiem, PostgreSQL, S3 ani procesami treningu.

Następne bramki: pozostałe rejestry, zakres w query/notebookach/wykonaniach oraz dostęp do artefaktów; następnie pełny manifest i wykonawca migracji, testy odebrania dostępu oraz cutover.


## Kontynuacja — anotacje i pliki obrazów z zakresem IAM-01

Data: 15.09.2026. **Czwarty adapter danych; IAM-01 i cała migracja pozostają w toku.**
Kontrakt i granice: [README IAM](../crates/aiwatcher-iam/README.md#fourth-resource-boundary-project-annotations-and-image-bytes).

- **API:** czternaście lokalnych operacji anotacji pod `/api/v1/orgs/{organization}/projects/{project}` — projekty anotacyjne, obrazy i ich filtrowana/paginowana lista, rysunki, review, eksporty, COCO oraz zapis i odczyt plików obrazów. Nazwa projektu anotacyjnego jest osobnym identyfikatorem wewnątrz projektu IAM; nazwy z ukośnikiem pozostają obsługiwane.
- **Autoryzacja:** viewer czyta, editor/admin zapisuje. Mutacje projektu wymagają nagłówka IAM i ponownego sprawdzenia grantu po odebraniu JSON lub bajtów obrazu. Autor rysunku i reviewer pochodzą z tożsamości wywołującego. Odpowiedzi scoped mają `no-store`, także pliki obrazów, które na starych trasach zachowują dotychczasowy cache.
- **Magazyn:** `<annotation-prefix>/scopes/<org>/<project>/registry/` obejmuje schematy, heady obrazów, rewizje, eksporty oraz **bajty obrazów i metadane ich typu**. Ten sam hash w innym projekcie wymaga osobnego uploadu; nie daje odczytu cudzych bajtów. Deduplication plików działa tylko między kolekcjami w jednym projekcie IAM. Klucze magazynu są sprawdzane, a rejestru nie można przepiąć do innego zakresu.
- **Referencje i historia:** rejestracja lokalnego `aiwatcher://blob/<hash>` wymaga zgodności image ID i obecności pliku w projekcie. Hashe schematów, rewizji i eksportów pozostają niezmienione; zachowane są autorzy, prawa użycia, przypięcia review i podziały danych. COCO i weryfikowany odczyt biblioteczny korzystają z tych samych ograniczonych zasobów. Review w jednym projekcie nie zmienia drugiego.
- **Zgodność:** stare trasy nie odczytują plików ani anotacji projektowych, również po wyłączeniu IAM. URI blobu zachowuje dotychczasowy format; jego interpretacja wymaga przekazania zakresu klienta. Panel, SDK i konsumenci eksportów nadal wymagają tego podłączenia.
- **Pozostały zakres:** import z pobieraniem zewnętrznych obrazów, katalog źródeł i kolejki/workerzy importu pozostają przy starych trasach. Nie dodano do nich pozornych aliasów scoped. Wymagają własnych poświadczeń zakresu, kontroli podczas wykonywania i przerwania po odebraniu dostępu. Nie wykonano migracji danych ani wdrożenia; selektory UI pozostają nieaktywne.

Walidacja:

- `cargo test -p aiwatcher-annotations -p aiwatcher-api`: **305 testów**, powodzenie. Dodano **5 scenariuszy HTTP i 2 testy magazynu** obejmujące izolację, role, wygaśnięcie i cofnięcie grantów, ponowną kontrolę po odebraniu body oraz zachowanie historii i hashy.
- `cargo clippy -p aiwatcher-api -p aiwatcher-annotations --all-targets -- -D warnings`: powodzenie.
- Panel: **53 pliki, 417 testów**, powodzenie. `npm run build`: granice architektury, Vite i TypeScript — powodzenie; pozostaje wcześniejsze ostrzeżenie o głównym chunku ponad 500 kB. UI nie zmieniono; bez nowego przeglądu wizualnego.
- Wygenerowano OpenAPI i klienta panelu; sprawdzono czternaście operacji, wymagane parametry zakresu, nagłówki mutacji i unikalność identyfikatorów operacji. `just openapi-check`, `cargo fmt --all --check` oraz `git diff --check`: powodzenie.
- Testy HTTP używają rzeczywistego routera, podpisanych sesji OIDC z lokalnym discovery/JWKS oraz pamięciowego IAM. Testy magazynu zapisują rzeczywiste pliki, ponownie otwierają rejestr i sprawdzają bajty, metadane, rewizje, manifesty oraz COCO. Nie wykonywano E2E z Authentikiem, PostgreSQL, S3 ani workerami importu.

Następne bramki: pozostałe rejestry, importy i zadania, zakres w query/notebookach/wykonaniach oraz konsumenci artefaktów; następnie pełny manifest, wykonawca migracji i cutover.


## Kontynuacja — formularze, oceny i karty ewaluacji z zakresem IAM-01

Data: 15.09.2026. **Piąty adapter danych; IAM-01 i cała migracja pozostają w toku.**
Kontrakt i granice: [README IAM](../crates/aiwatcher-iam/README.md#fifth-resource-boundary-project-forms-assessments-and-scorecards).

- **API:** jedenaście operacji pod `/api/v1/orgs/{organization}/projects/{project}` — publikacja/lista/szczegół formularzy ocen, zapis/lista/historia ocen oraz publikacja/lista/szczegół/wersje/porównanie kart pomiarowych. Obie rodziny tras korzystają ze wspólnych handlerów i schematów.
- **Role i sesje:** viewer czyta, editor/admin zapisuje. Mutacje wymagają nagłówka IAM i ponownej kontroli grantu po odebraniu JSON. Autor publikacji i osoba zapisująca ocenę pochodzą z sesji. Odpowiedzi projektowe wyłączają cache; role instancji nie zastępują grantów projektu.
- **Magazyn:** trzy rodziny obiektów mieszczą się w `evaluation-scopes/<org>/<project>/registry/` skonfigurowanego magazynu ewaluacji. Adapter zachowuje atomowe tworzenie, sprawdza klucze i wyniki listowania. Odmawia przepięcia do innego projektu i dostępu do pozostałych rodzin obiektów. Stare API zachowuje swój magazyn; próby przejścia ścieżką do innego zakresu są odrzucane również tam.
- **Referencje i historia:** ocena oraz karta z sędzią wymagają formularza obecnego w tym samym projekcie. Zachowane są hashe wersji, metadane publikacji, identyfikatory celu i autora oceny oraz numery rewizji. Zmiana oceny lub karty nie zmienia historii w innym projekcie; powtórzona publikacja tych samych treści zachowuje hashe.
- **Granice:** cel oceny pozostaje zapisaną referencją, bez potwierdzenia dostępu do źródłowego wyniku/trace/span. Rejestr projektowy odłącza globalny resolver źródeł. Publikacja kart wymagających zewnętrznego katalogu scorerów jest jawnie odrzucana do czasu objęcia katalogu polityką projektu. Karty z wbudowanymi scorerami i sędzią opartym na lokalnym formularzu są obsługiwane; zapis nie uruchamia pomiaru.
- **Pozostały zakres:** wyniki, dowody, approvals, review, kohorty, nagrania, kalibracje i wykonania wymagają osobnych adapterów. Panel/SDK nadal korzystają ze starych tras, selektory UI pozostają nieaktywne. Nie wykonano migracji danych ani wdrożenia.

Walidacja:

- `cargo test -p aiwatcher-evaluation -p aiwatcher-api`: **328 testów**, powodzenie. Dodano **5 scenariuszy HTTP i 2 testy magazynu** obejmujące wszystkie jedenaście operacji, izolację referencji i historii, role, wygaśnięcie i cofnięcie grantów oraz odmowę zapisu po odebraniu spóźnionego body.
- `cargo test -p aiwatcher-server --test evaluation`: **128 testów**, powodzenie; **1 test RustFS/S3 jawnie pominięty**, ponieważ wymaga osobnej usługi. Kontrola obejmuje zgodność dotychczasowych ewaluacji po dodaniu walidacji kluczy magazynu.
- `cargo clippy -p aiwatcher-api -p aiwatcher-evaluation --all-targets -- -D warnings`: powodzenie.
- Panel: **53 pliki, 417 testów**, powodzenie. `npm run build`: granice architektury, Vite i TypeScript — powodzenie; pozostaje wcześniejsze ostrzeżenie o głównym chunku ponad 500 kB. UI nie zmieniono; bez nowego przeglądu wizualnego.
- Wygenerowano OpenAPI i klienta panelu; sprawdzono jedenaście operacji, wymagane parametry zakresu, nagłówki mutacji i unikalność identyfikatorów. `just openapi-check`, `cargo fmt --all --check` oraz `git diff --check`: powodzenie.
- Testy HTTP używają rzeczywistego routera, podpisanych sesji OIDC z lokalnym discovery/JWKS oraz pamięciowego IAM. Testy plikowe obejmują ponowne otwarcie rejestru, porównanie surowych bajtów niezmiennych dokumentów i paginację historii. Nie wykonywano pełnego E2E z Authentikiem, PostgreSQL, S3 ani workerami.

Następne bramki: pozostałe rejestry i referencje między zasobami, importy i zadania, zakres w query/notebookach/wykonaniach; następnie pełny manifest, wykonawca migracji i cutover.


## Kontynuacja — definicje workflow z zakresem IAM-01

Data: 15.09.2026. **Szósty adapter danych; IAM-01 i cała migracja pozostają w toku.**
Kontrakt i granice: [README IAM](../crates/aiwatcher-iam/README.md#sixth-resource-boundary-project-workflow-definitions).

- **API:** lista, szczegół z opcjonalną rewizją oraz publikacja definicji workflow pod `/api/v1/orgs/{organization}/projects/{project}`. Nazwy z ukośnikiem pozostają obsługiwane. Obie rodziny tras współdzielą handlery i kontrakt.
- **Role:** viewer czyta, editor/admin publikuje. Mutacja wymaga nagłówka IAM i ponownego sprawdzenia grantu po odebraniu JSON. Autor rejestracji pochodzi z sesji; odpowiedzi projektowe wyłączają cache.
- **Magazyn:** heady i niezmienne wersje pod `workflows/scopes/<org>/<project>/registry/`. Brak przepięcia rejestru do innego projektu i fallbacku do globalnych danych. Nazwy są hashowane, rewizje zachowują walidację SHA-256. Listowanie sprawdza adres obiektu przed odczytem oraz zgodność treści z nazwą i rewizją, również na starych trasach.
- **Historia:** zakres nie zmienia definicji, hasha rewizji ani tożsamości skompilowanego planu. Zachowane są autor i czas rejestracji, parametry, przypięte wersje zadań i bramki akceptacji. Zmiana heada nie zmienia starszej wersji ani sąsiedniego projektu.
- **Walidacja i uruchamianie:** publikacja nadal wymaga poprawnego grafu i zgodności z szablonami podów wdrożenia. Nie uruchamia workflow i nie nadaje dostępu do kolejki ani klastra. Istniejące trasy startu i harmonogramów nie znajdują definicji dostępnej wyłącznie w projekcie; znana rewizja nie daje dostępu przez stare API.
- **Pozostały zakres:** scoped wykonania, harmonogramy, poświadczenia workerów, zasoby wskazane w parametrach, cache i zatrzymywanie pracy po cofnięciu grantu wymagają integracji. Nie dodano aliasów tych tras. UI i wywołania panelu/SDK pozostają przed cutoverem; nie wykonano migracji ani wdrożenia.

Walidacja:

- `cargo test -p aiwatcher-api -p aiwatcher-execution`: **526 testów**, powodzenie. Dodano **5 scenariuszy HTTP i 2 testy magazynu**: izolacja nazw i wersji, role, wygasanie/cofnięcie grantów, odmowa spóźnionej publikacji, walidacja grafu/podów oraz brak dostępu przez stare trasy wykonania i harmonogramów.
- `cargo clippy -p aiwatcher-api -p aiwatcher-execution --all-targets -- -D warnings`: powodzenie.
- Panel: **53 pliki, 417 testów**, powodzenie. `npm run build`: granice architektury, Vite i TypeScript — powodzenie; pozostaje wcześniejsze ostrzeżenie o głównym chunku ponad 500 kB. UI nie zmieniono; bez nowego przeglądu wizualnego.
- Wygenerowano OpenAPI i klienta panelu; sprawdzono trzy operacje, wymagane parametry zakresu, nagłówek publikacji i unikalność identyfikatorów. `just openapi-check`, `cargo fmt --all --check` oraz `git diff --check`: powodzenie.
- Testy HTTP korzystają z rzeczywistego routera, podpisanych sesji OIDC z lokalnym discovery/JWKS oraz pamięciowego IAM i silnika wykonania. Testy plikowe ponownie otwierają rejestr i porównują niezmienne bajty, metadane i skompilowane plany. Niepoprawny adapter listowania potwierdza odmowę odczytu cudzych kluczy przed pobraniem bajtów. Nie uruchamiano pełnego E2E z Authentikiem, PostgreSQL, S3, workerami ani klastrem.

Następne bramki: pozostałe rejestry, referencje między zasobami i autoryzacja wykonań, następnie pełny manifest, wykonawca migracji i cutover.


## Kontynuacja — review przypadków i publikacja do datasetu z zakresem IAM-01

Data: 15.09.2026. **Siódmy adapter danych; IAM-01 i cała migracja pozostają w toku.**
Kontrakt i granice: [README IAM](../crates/aiwatcher-iam/README.md#seventh-resource-boundary-project-case-reviews-and-dataset-publication).

- **API:** pięć operacji pod `/api/v1/orgs/{organization}/projects/{project}` — kolejka datasetu, propozycja przypadku, wyszukanie review po celu, zapis oczekiwanej odpowiedzi/akceptacja/odrzucenie oraz publikacja zatwierdzonych przypadków. Wspólne handlery ze starymi trasami, zaktualizowane OpenAPI i klient panelu.
- **Role:** viewer czyta, editor/admin zapisuje i publikuje zatwierdzone przypadki. Akceptacja treści `observed` wymaga **administratora projektu**. Rola administratora instancji nie wystarcza; wygaśnięcia grantu admin podczas wysyłania formularza nie zastępuje niezależny grant editor. Mutacje wymagają nagłówka IAM i ponownej kontroli po odebraniu JSON, odpowiedzi projektowe mają `no-store`.
- **Magazyn:** rewizje review i indeksy celów pod `evaluation-scopes/<org>/<project>/registry/`. Zachowane są identyfikatory, oryginalne bajty rewizji, autorzy, referencje celu/oceny i podziały danych. Rejestr sprawdza klucze oraz odrzuca cudze wyniki listowania przed odczytem bajtów, również przy znanym ID review.
- **Publikacja:** rejestr review i rejestr datasetów otrzymują ten sam zakres z trasy. Dopisywane są wyłącznie zatwierdzone przypadki oraz wcześniejsze wiersze datasetu tego projektu. Brak lokalnego datasetu tworzy nowy; nie korzysta z globalnego ani sąsiedniego heada. Review zapisuje rzeczywistą wersję datasetu i autora publikacji. Niezatwierdzone propozycje nie trafiają do wyniku, a zmiana oczekiwanej odpowiedzi nadal usuwa wcześniejszą akceptację.
- **Konflikt podczas publikacji:** poprawiono także stare trasy — późniejsza edycja/odrzucenie review nie zostaje oznaczone jako opublikowane we wcześniejszym snapshotcie. Zmieniona rewizja lub przypisanie do innej opublikowanej wersji daje 409; ponowne oznaczenie tej samej wersji jest idempotentne. Nie jest to transakcja między rejestrami: awaria lub konflikt po zapisie datasetu może pozostawić wersję bez wszystkich oznaczeń review. Trwała koordynacja i odzyskiwanie publikacji oraz równoległe aktualizacje heada datasetu pozostają osobnym zakresem.
- **Granice źródeł:** propozycja projektowa wymaga jawnie dostarczonego tekstu `written` lub `observed`. Pobieranie słów z wyniku ewaluacji i deklarowanie ich jako `measured` są odrzucane do czasu izolacji dowodów i resolverów. Referencje trace/result/assessment pozostają metadanymi pochodzenia, bez potwierdzenia dostępu do źródła. Globalny resolver nie jest używany.
- **Pozostały zakres:** panel i SDK nadal korzystają ze starych tras, selektory UI pozostają nieaktywne. Nie wykonano migracji ani wdrożenia; dry-run datasetów nie obejmuje jeszcze review i indeksów. Kontrola grantu nie anuluje dopuszczonej wcześniej operacji magazynu.

Walidacja:

- `cargo test -p aiwatcher-api -p aiwatcher-evaluation -p aiwatcher-datasets`: **374 testy**, powodzenie. Dodano **5 scenariuszy HTTP i 3 testy rejestru**; istniejące regresje API i datasetów również przeszły. Osobno potwierdzono wszystkie 5 testów plików/zakresu ewaluacji, w tym konflikt publikacji.
- `cargo clippy -p aiwatcher-api -p aiwatcher-evaluation -p aiwatcher-datasets --all-targets -- -D warnings`: powodzenie.
- Panel: **53 pliki, 417 testów**, powodzenie. `npm run build`: granice architektury, Vite i TypeScript — powodzenie; pozostaje wcześniejsze ostrzeżenie o głównym chunku ponad 500 kB. UI nie zmieniono; bez nowego przeglądu wizualnego.
- Wygenerowano OpenAPI i klienta panelu; sprawdzono pięć operacji, wymagane parametry zakresu i nagłówki mutacji oraz odpowiedź 409 publikacji. `just openapi-check`, `cargo fmt --all --check` oraz `git diff --check`: powodzenie.
- Testy HTTP używają rzeczywistego routera, podpisanych sesji OIDC z lokalnym discovery/JWKS oraz pamięciowego IAM. Testy magazynu obejmują rzeczywiste pliki, ponowne otwarcie, niezmienne bajty rewizji i indeksów oraz odmowę odczytu cudzych kluczy. Nie wykonano pełnego E2E z Authentikiem, PostgreSQL, S3 ani workerami.

Następne bramki: izolacja wyników/dowodów ewaluacji i ich resolverów, pozostałe rejestry, autoryzacja query/notebooków/wykonań i strumieni; pełny manifest, wykonawca migracji oraz cutover. Publikacja review wymaga jeszcze trwałej koordynacji między rejestrami i odzyskiwania po częściowym zapisie.


## Kontynuacja — kohorty z datasetów i anotacji z zakresem IAM-01

Data: 15.09.2026. **Ósmy adapter danych; IAM-01 i cała migracja pozostają w toku.**
Kontrakt i granice: [README IAM](../crates/aiwatcher-iam/README.md#eighth-resource-boundary-project-cohorts-from-native-datasets).

- **API:** tworzenie kohorty oraz odczyt zapisanych metadanych po hashu przypadków pod `/api/v1/orgs/{organization}/projects/{project}/evaluation-cohorts`. Wspólne handlery i schematy ze starymi trasami; kontrakt i klient panelu zaktualizowane.
- **Role:** viewer odczytuje, editor/admin tworzy. Mutacja wymaga nagłówka IAM i ponownej kontroli aktualnego grantu po odebraniu JSON; odpowiedzi projektowe mają `no-store`. Rola instancji nie zastępuje dostępu do projektu.
- **Resolver:** nowa jawna fabryka `SourceAuthority::for_project_cohorts` domyślnie odmawia. Adapter serwera wiąże rejestry datasetów i anotacji z projektem. Nie zachowuje dostępu do katalogu na dysku, globalnych pakietów ewaluacyjnych, rozmów, promptów ani modeli. Odmawia zmiany zakresu i rozwiązywania pełnego manifestu ewaluacji. Działa z istniejącym podłączeniem natywnych rejestrów serwera, bez nowej konfiguracji wdrożenia.
- **Źródła:** każda próba utworzenia ponownie weryfikuje konkretną wersję datasetu lub eksportu anotacji, nawet gdy metadane kohorty już istnieją. Zmiana heada nie zmienia starych przypięć; brak lub uszkodzenie źródła powoduje odmowę bez szukania danych globalnych. Anotacje zachowują weryfikację plików obrazów, dataset — wybór splitu, limit pierwszych N przypadków i licznik wierszy bez splitu.
- **Magazyn i historia:** `evaluation-scopes/<org>/<project>/registry/evaluation-cohorts/`. Zachowane hashe przypadków i schematów, URI przypięć, wersje źródeł, pierwszy autor i czas utworzenia. Identyczne dane zapisane niezależnie w dwóch projektach dają te same przypięcia i osobne metadane. Znany hash/wersja nie otwiera cudzych ani globalnych danych.
- **Odczyt historyczny:** GET zwraca zapis pochodzenia, nie aktualne potwierdzenie istnienia wszystkich bajtów źródła. URI `aiwatcher://` pozostaje przypięciem interpretowanym w tym samym projekcie, nie adresem pobierania plików przez nowe API.
- **Granice etapu:** korpusy rozmów są niedostępne także dla administratora. Nie udostępniono projektowych wyników, approvals, pakietów, nagrań, deklaracji pomiarów, workerów ani automatycznego pobierania tekstu z wyników do review. Panel/SDK nadal korzystają ze starych tras, selektory UI pozostają nieaktywne. Nie wykonano migracji ani wdrożenia; inwentaryzacja migracji nie obejmuje jeszcze kohort. Kontrola grantu nie anuluje już dopuszczonej operacji źródła/magazynu.

Walidacja:

- `cargo test -p aiwatcher-api -p aiwatcher-evaluation -p aiwatcher-datasets`: **378 testów**, powodzenie. Dodano **4 scenariusze HTTP** obejmujące oba endpointy, źródłowe wersje i metadane między projektami/organizacjami, stare trasy, role, nagłówki, przedziały grantów, cofnięcie dostępu i spóźniony JSON.
- `cargo test -p aiwatcher-server --test evaluation`: **131 testów**, powodzenie; **1 test RustFS/S3 jawnie pominięty** zgodnie z jego `ignore`, bez uruchamiania zewnętrznej usługi. Dodano **3 testy rzeczywistych adapterów źródeł** — plikowy magazyn, ponowne otwarcie, oryginalne bajty i historyczne przypięcia, split/limit, uszkodzenie/usunięcie źródła, eksporty i obrazy anotacji oraz odłączenie globalnych możliwości resolvera.
- `cargo clippy -p aiwatcher-api -p aiwatcher-evaluation -p aiwatcher-server --all-targets -- -D warnings`: powodzenie.
- Panel: **53 pliki, 417 testów**, powodzenie. `npm run build`: granice architektury, Vite i TypeScript — powodzenie; pozostaje wcześniejsze ostrzeżenie o głównym chunku ponad 500 kB. UI nie zmieniono; bez nowego przeglądu wizualnego.
- Wygenerowano OpenAPI i klienta panelu; sprawdzono dwie operacje, wymagane parametry zakresu oraz nagłówek mutacji. `just openapi-check`, `cargo fmt --all --check` i `git diff --check`: powodzenie.
- HTTP korzysta z rzeczywistego routera, podpisanych sesji OIDC i lokalnego discovery/JWKS, pamięciowego IAM oraz testowego adaptera źródła nad rzeczywistym rejestrem datasetów. Testy serwera osobno sprawdzają produkcyjny `LocalSource` i natywne rejestry. Nie wykonano pełnego E2E z Authentikiem, PostgreSQL, S3 ani workerami.

Następne bramki: zakres pełnych dowodów i wyników ewaluacji wraz z approvals/pakietami i resolverami promptów/modeli, autoryzacja wykonań, pozostałe rejestry i strumienie; następnie pełny manifest, wykonawca migracji i cutover. Trwała koordynacja publikacji review nadal pozostaje otwarta.


## Kontynuacja — nagrania odpowiedzi z zakresem IAM-01

Data: 15.09.2026. **Dziewiąty adapter danych; IAM-01 i cała migracja pozostają w toku.**
Kontrakt i granice: [README IAM](../crates/aiwatcher-iam/README.md#ninth-resource-boundary-project-recorded-answers).

- **API:** zapis dokumentu odpowiedzi pod `/api/v1/orgs/{organization}/projects/{project}/evaluation-recordings/{name}` oraz pobranie oryginalnych bajtów pod `evaluation-recordings/{digest}/content`. Zapis współdzieli handler ze starą trasą; odczyt jest addytywny w obu rodzinach tras. OpenAPI i klient panelu aktualizowane razem.
- **Role:** viewer czyta, editor/admin zapisuje. PUT wymaga nagłówka IAM i ponownej kontroli aktualnego grantu po odebraniu body; limit uploadu pozostaje 100 MiB. Odpowiedzi projektowe mają `no-store`, podobnie jak pobranie nagrania przez starą trasę. Rola instancji nie zastępuje grantu projektu, a stare API zachowuje wymaganie roli editor do uploadu.
- **Magazyn:** oryginalne dokumenty pod `evaluation-scopes/<org>/<project>/registry/evaluation-recordings/<digest>.json`. Nazwa jest metadanymi wyświetlania, dopuszcza ukośnik i nie wyznacza klucza. Hash obliczany z odebranych bajtów, rozmiar i URI `evaluation://recordings/<digest>` pozostają niezmienione. Identyczne nagranie w innym projekcie wymaga osobnego uploadu; znajomość hasha nie otwiera danych projektowych przez globalny magazyn ani po wyłączeniu IAM.
- **Odczyt i integralność:** odpowiedź zawiera oryginalne bajty JSON, w tym odstępy i zapis liczb, po sprawdzeniu SHA-256. Niepoprawny digest jest odrzucany przed odczytem magazynu; brak lokalnego obiektu daje 404, uszkodzenie daje 503 bez zwrócenia treści. Odczyt biblioteczny nagrań korzysta z tej samej weryfikacji. Rejestr nie zachowuje globalnego resolvera i odmawia przepięcia zakresu.
- **Granice:** nagranie to treść dostarczona przez wywołującego, bez potwierdzenia wykonania pomiaru. ID przypadków/run/trace/span oraz zużycie pozostają metadanymi producenta, bez weryfikacji dostępu do wskazanych źródeł. Nie dodano projektowych deklaracji pomiarów, approvals, kalibracji, wyników ani dostępu workerów. URI wymaga interpretacji w tym samym projekcie. Panel/SDK nadal używają starych tras; selektory UI pozostają nieaktywne. Nie wykonano migracji danych ani wdrożenia; inwentaryzacja migracji nie obejmuje jeszcze nagrań. Kontrola grantu nie anuluje operacji magazynu dopuszczonej przed cofnięciem dostępu; retencja i zatrzymywanie aktywnej pracy pozostają osobnym zakresem.

Walidacja:

- `cargo test -p aiwatcher-api -p aiwatcher-evaluation`: **350 testów**, powodzenie. Dodano **4 scenariusze HTTP i 1 test plikowego magazynu**. HTTP sprawdza obie operacje, izolację projektów/organizacji/starych tras, identyczne treści, dokładne bajty pobrania, role, nagłówki, przedziały grantów, cofnięcie dostępu, upload po wygaśnięciu grantu, brak auth/IAM i błędne dane.
- `cargo test -p aiwatcher-server --test evaluation`: **131 testów**, powodzenie; **1 test RustFS/S3 jawnie pominięty**, bez uruchamiania zewnętrznej usługi.
- `cargo clippy -p aiwatcher-api -p aiwatcher-evaluation --all-targets -- -D warnings`: powodzenie.
- Panel: **53 pliki, 417 testów**, powodzenie. `npm run build`: granice architektury, Vite i TypeScript — powodzenie; pozostaje wcześniejsze ostrzeżenie o głównym chunku ponad 500 kB. UI nie zmieniono; bez nowego przeglądu wizualnego.
- Wygenerowano OpenAPI i klienta panelu; sprawdzono dwie operacje projektowe, wymagane parametry zakresu, nagłówek PUT i unikalność identyfikatorów wszystkich operacji. `just openapi-check`, `cargo fmt --all --check` i `git diff --check`: powodzenie.
- Test magazynu ponownie otwiera rejestr, porównuje oryginalne bajty i tożsamość artefaktu oraz sprawdza odmowę przepięcia zakresu, niepoprawnych digestów i odczytu po uszkodzeniu/usunięciu lokalnego obiektu. Testy HTTP używają podpisanych sesji OIDC, rzeczywistego routera, lokalnego discovery/JWKS i pamięciowego IAM; nie wykonano pełnego E2E z Authentikiem, PostgreSQL, S3 ani workerami.

Następne bramki: zakres pełnych dowodów i wyników ewaluacji wraz z approvals/pakietami i resolverami promptów/modeli, kalibracje i deklaracje pomiarów, autoryzacja wykonań, pozostałe rejestry i strumienie; następnie pełny manifest, wykonawca migracji i cutover. Trwała koordynacja publikacji review nadal pozostaje otwarta.


## Kontynuacja — pliki pakietów dowodów z zakresem IAM-01

Data: 15.09.2026. **Dziesiąty adapter danych; IAM-01 i cała migracja pozostają w toku.**
Kontrakt i granice: [README IAM](../crates/aiwatcher-iam/README.md#tenth-resource-boundary-project-approval-bundle-files).

- **API:** trzy operacje pod `/api/v1/orgs/{organization}/projects/{project}/evaluation-approvals/{approval_id}/bundle` — lista nazw/rozmiarów, usunięcie plików pakietu oraz upload pojedynczego pliku pod `/{name}`, także `model-artifacts/<file>`. Wspólne handlery ze starymi trasami; OpenAPI i klient panelu aktualizowane razem.
- **Role:** viewer listuje, **admin projektu** przesyła i usuwa. PUT i DELETE wymagają nagłówka IAM; po odebraniu body uploadu ponownie sprawdzany jest bieżący grant admin. Wygaśnięcia admina nie zastępuje niezależny editor ani rola instancji. Odpowiedzi projektowe mają `no-store`; stara rodzina tras zachowuje role instancji. Wspólny generator OpenAPI obejmuje teraz także nagłówki i parametry zakresu DELETE oraz jego unikalny identyfikator operacji.
- **Magazyn:** fabryka `ApprovalBundles::for_project` domyślnie odmawia. Adapter serwera wiąże pliki z `evaluation-scopes/<org>/<project>/bundles/<approval_id>/`, bez katalogu hosta i bez globalnych rejestrów datasetów, anotacji, rozmów, promptów czy modeli. Odmawia przepięcia do innego projektu. Korzysta z istniejącej konfiguracji magazynu pakietów, bez dodatkowej konfiguracji wdrożenia.
- **Tożsamość i operacje:** zachowane oryginalne bajty, nazwy plików i ID pary wariant/kontekst. Ten sam ID w różnych projektach oznacza osobne pliki. Upload zastępuje wyłącznie plik własnego projektu; usunięcie nie narusza innego projektu ani innego ID pakietu, a powtórzone usunięcie pustego pakietu jest idempotentne. Brak pakietu daje pustą listę; odczyt biblioteczny nie korzysta z globalnych plików ani katalogu hosta, także gdy istnieje tam ten sam ID.
- **Walidacja kluczy:** adres pary i nazwa pliku są sprawdzane przed dostępem. Cała lista z magazynu musi należeć do dokładnego prefiksu pary i zawierać prawidłowe nazwy, zanim zostanie zwrócona lub rozpocznie się usuwanie. Cudzy albo niepoprawny klucz odrzuca całą listę przed pierwszym usunięciem. Ta kontrola obejmuje również stare trasy.
- **Granice:** upload nie zatwierdza pakietu, nie wykonuje scorera, nie pobiera URL-i ani nie sprawdza wszystkich przypięć manifestu. Usunięcie plików nie wycofuje approval ani nie usuwa opublikowanego wyniku. Projektowe approvals, pełny resolver źródeł, wyniki, kalibracje, deklaracje pomiarów i wykonania pozostają do wdrożenia. UI i SDK nadal używają starych tras; selektory są nieaktywne. Nie wykonano migracji danych ani wdrożenia; inwentaryzacja migracji nie obejmuje pakietów. Usuwanie wielu plików nie jest transakcyjne i po awarii magazynu może wymagać ponowienia. Cofnięcie grantu nie anuluje wcześniej dopuszczonej operacji magazynu.

Walidacja:

- `cargo test -p aiwatcher-api -p aiwatcher-evaluation`: **354 testy**, powodzenie. Dodano **4 scenariusze HTTP** wszystkich trzech operacji: izolacja projektów/organizacji/starych tras, nagłówki PUT/DELETE, admin kontra editor/rola instancji, przedziały grantów, cofnięcie dostępu, opóźniony upload po wygaśnięciu admina przy aktywnym editorze, brak auth/IAM/fabryki.
- Dodano **2 testy adaptera serwera**: rzeczywisty plikowy magazyn i ponowne otwarcie, oryginalne bajty tekstu/binarnych artefaktów, te same ID w różnych projektach, brak fallbacku do globalnych plików/katalogu hosta i odmowa przepięcia; osobno błędny adapter listowania potwierdza odmowę przed jakimkolwiek usunięciem, również na starej ścieżce.
- `cargo test -p aiwatcher-server --test evaluation`: **133 testy**, powodzenie; **1 test RustFS/S3 jawnie pominięty**, bez uruchamiania zewnętrznej usługi.
- `cargo clippy -p aiwatcher-api -p aiwatcher-evaluation -p aiwatcher-server --all-targets -- -D warnings`: powodzenie.
- Panel: **53 pliki, 417 testów**, powodzenie. `npm run build`: granice architektury, Vite i TypeScript — powodzenie; pozostaje wcześniejsze ostrzeżenie o głównym chunku ponad 500 kB. UI nie zmieniono; bez nowego przeglądu wizualnego.
- Wygenerowano OpenAPI i klienta panelu; sprawdzono trzy operacje projektowe, parametry zakresu, wymagane nagłówki PUT/DELETE i unikalność identyfikatorów wszystkich operacji. `just openapi-check`, `cargo fmt --all --check` i `git diff --check`: powodzenie.
- Testy HTTP używają podpisanych sesji OIDC, rzeczywistego routera, lokalnego discovery/JWKS i pamięciowego IAM z testowym adapterem pakietów. Testy serwera sprawdzają produkcyjny `LocalSource`. Nie wykonano pełnego E2E z Authentikiem, PostgreSQL, S3 ani workerami.

Fundament projektowego resolvera manifestów producenta wykonano w kontynuacji poniżej. Następne bramki: podłączenie resolvera do projektowych approvals, izolacja wyników oraz kalibracji, deklaracje pomiarów i autoryzacja wykonań; pozostałe rejestry i strumienie, pełny manifest migracji, wykonawca migracji i cutover. Trwała koordynacja publikacji review pozostaje otwarta.


## Kontynuacja — fundament projektowego resolvera dowodów IAM-01

**Etap biblioteczny, bez nowych tras HTTP; IAM-01 i cała migracja pozostają w toku.**
Kontrakt: [README IAM](../crates/aiwatcher-iam/README.md#producer-evidence-resolver-foundation).

- **Fabryka:** `SourceAuthority::for_project_evidence` domyślnie odmawia. Adapter serwera buduje nowy resolver z rejestrami datasetów, anotacji, promptów i modeli związanymi z jednym projektem oraz projektowym prefiksem pakietów. Nie przenosi katalogu hosta ani archiwum rozmów. Odmawia przepięcia zakresu, również gdy przekazany rejestr źródłowy należy już do innego projektu.
- **Weryfikacja:** manifest producenta przechodzi istniejącą kontrolę przypiętych plików, pakietu modelu i jego artefaktów, wersji promptu/modelu oraz przypadków i ich kolejności. Obsługiwane źródła to zewnętrzne przypadki w pakiecie, natywny dataset i eksport anotacji. Brak lub uszkodzenie lokalnej wersji nie uruchamia fallbacku do identycznych danych globalnych, innego projektu ani dysku hosta. URI nie jest pobierane. Hashe i formaty danych pozostają bez zmian.
- **Odmowy:** rozmowy, datasety ocen, sędziowie i konteksty `aiwatcher.scoring` są odrzucane; wymagają dalszych właścicieli zakresu i polityki wykonania. Resolver zawężony do tworzenia kohort nie odzyskuje zdolności rozwiązywania pełnych manifestów.
- **Granica integracji:** fabryka nie jest jeszcze wywoływana przez produkcyjne trasy approvals/wyników. Nie uwierzytelnia argumentu `subject` ani nie sprawdza grantów IAM — przyszły wywołujący musi sprawdzić bieżący dostęp oraz użyć rejestru z projektowymi approvals, wynikami i retencją. Obecne trasy kohort i plików pakietów zachowują swoje węższe możliwości. Nie aktywowano selektorów UI, nie wykonano migracji ani wdrożenia.

Walidacja:

- `cargo test -p aiwatcher-server --test evaluation --quiet`: **138 testów przeszło, 1 istniejący test RustFS/S3 pominięty**. Dodano **5 testów**: rzeczywisty plikowy magazyn i ponowne otwarcie, projekty/organizacje/globalne dane, osobna obecność identycznych wersji, uszkodzenie/usunięcie źródła, projektowe prompty i modele, integralność artefaktów modelu oraz wszystkich przypiętych plików zewnętrznego pakietu, odmowa przepięcia i domyślna odmowa fabryki.
- `cargo test -p aiwatcher-evaluation --quiet`: **96 testów**, powodzenie.
- `cargo clippy -p aiwatcher-server -p aiwatcher-evaluation --all-targets -- -D warnings`: powodzenie.
- `cargo fmt --all --check` i `git diff --check`: powodzenie.
- Nie zmieniano UI ani kontraktu HTTP; w tej iteracji nie regenerowano klienta i nie uruchamiano testów panelu. Testy adapterów nie są testami autoryzowanej trasy projektowych approvals/wyników ani pełnym E2E z Authentikiem, PostgreSQL, S3 i workerami.

Projektowy rejestr approvals i wyników podłączono w kontynuacji poniżej. Kalibracje, pomiary serwerowe, wykonania, strumienie i cutover pozostają osobnymi bramkami.


## Kontynuacja — projektowe approvals, wyniki producentów i retencja IAM-01

**Jedenasty adapter danych; IAM-01 i cała migracja pozostają w toku.**
Kontrakt: [README IAM](../crates/aiwatcher-iam/README.md#eleventh-resource-boundary-project-producer-approvals-and-results).

- **API:** 10 operacji pod `/api/v1/orgs/{organization}/projects/{project}`: lista/zatwierdzanie/wycofanie approvals, publikacja/katalog/szczegół/usunięcie wyników, strony przypadków i oba porównania. Wspólne handlery ze starymi trasami. OpenAPI i wygenerowany klient panelu zaktualizowane razem.
- **Role:** viewer czyta, editor publikuje, admin projektu zatwierdza, wycofuje i usuwa. Mutacje wymagają nagłówka IAM i ponownego sprawdzenia wymaganej roli po odebraniu body. Wygasłego admina nie zastępuje niezależny editor ani rola instancji. Odpowiedzi projektowe mają `no-store`; brak/cofnięcie dostępu odrzuca żądanie przed odczytem rejestru.
- **Izolacja:** `Registry::for_project_evidence` podłącza jawny resolver i magazyn `evaluation-scopes/<org>/<project>/registry/evaluations/`: approvals, claimy, shardy, nagłówki, katalog, znaczniki wycofania/usunięcia i raporty retencji. Znajomość ID nie otwiera wyniku ani baseline’u w innym projekcie. Identyczne ID wymagają osobnej publikacji i zatwierdzenia. Brak fallbacku do globalnych approvals, wyników ani telemetrycznej projekcji, również po wyłączeniu IAM. Zachowano atomowy create, oryginalne bajty i adresy treści.
- **Retencja:** istniejący worker wykrywa zakresy zawierające dowody i stosuje tę samą regułę sweep/collection osobno dla każdego, z osobnymi trwałymi raportami. Usuwanie nie zależy od aktywnego grantu człowieka. Collection pozostaje godzinowe; termin ostatniej kolekcji jest odczytywany z raportu również po restarcie. Koszt jest jawny: wykrywanie listuje prefiks `evaluation-scopes/` co minutę i rośnie z liczbą obiektów; indeksowana/paginowana lista zakresów pozostaje pracą skalującą. Błąd wykrywania/wiązania jest logowany, błąd uruchomionego sweepu trafia do raportu projektu.
- **Granice:** wyłącznie dowody mierzone przez producenta. Kalibracje, sędziowie, rozmowy, deklaracje pomiarów serwerowych, approval lines, projektowe gate/experiments i połączenia z obserwacjami pozostają nieudostępnione. Bezstanowe obliczanie adresu approval pozostaje na istniejącej trasie. ID trace/span/execution w dowodach są metadanymi producenta, nie dowodem dostępu ani wykonania. Cofnięcie grantu nie anuluje operacji magazynu już dopuszczonej; nie ma wspólnej transakcji IAM i object store. UI/SDK nadal używają starych tras; selektory są nieaktywne. Nie wykonano migracji ani wdrożenia.

Walidacja:

- `cargo test -p aiwatcher-api -p aiwatcher-evaluation --quiet`: **358 testów**, powodzenie (239 HTTP, 23 kontraktu/biblioteki API, 96 ewaluacji). **4 nowe scenariusze HTTP** obejmują 10 operacji, izolację projektów/organizacji/starych tras, granty przed startem/po wygaśnięciu/cofnięciu, role projektu kontra instancji, nagłówki, spóźniony upload po utracie editor/admin, brak auth/IAM/fabryki, wycofanie i niezależne usuwanie.
- `cargo test -p aiwatcher-server --test evaluation --quiet`: **140 testów przeszło, 1 istniejący test RustFS/S3 pominięty**. **2 nowe scenariusze** sprawdzają produkcyjne adaptery: plikowy magazyn i reopen z identycznymi receiptami/bajtami oraz osobnymi approvals, a także rzeczywistą pętlę retencji wykrywającą projekt, usuwającą wygasłe shardy i zapisującą osobny raport.
- Clippy API/ewaluacji/serwera z `--all-targets -- -D warnings`, `cargo fmt --all --check`, `just openapi-check` i `git diff --check`: powodzenie.
- Panel: **53 pliki, 417 testów**, powodzenie. Build (architektura, Vite, TypeScript): powodzenie; pozostaje ostrzeżenie o głównym chunku ponad 500 kB. Bez nowego przeglądu wizualnego — UI nie zmieniono.
- HTTP używa podpisanych sesji OIDC, lokalnego discovery/JWKS, rzeczywistego routera, pamięciowego IAM i jawnego testowego resolvera. Testy serwera osobno sprawdzają produkcyjny `LocalSource`. Nie jest to pełny E2E z Authentikiem, PostgreSQL, S3 ani workerami wykonującymi pomiary.

Kalibracje producentów podłączono w kontynuacji poniżej. Następne bramki: deklaracje pomiarów z ich właścicielami, autoryzacja wykonań i strumieni, pozostałe rejestry; pełny manifest migracji, wykonawca i cutover. Trwała koordynacja publikacji review pozostaje otwarta.

## Kontynuacja — projektowe kalibracje IAM-01

**Dwunasty adapter danych; IAM-01 pozostaje w toku.** Kontrakt:
[README IAM](../crates/aiwatcher-iam/README.md#twelfth-resource-boundary-project-calibration-sets).

- Dwie operacje pod `/api/v1/orgs/{organization}/projects/{project}/evaluation-calibrations`: POST tworzy niezmienny zbiór ocen ludzi, GET `/{version}` odczytuje go po adresie treści. Wspólne handlery zachowują stare trasy.
- Editor tworzy, viewer czyta; POST wymaga nagłówka IAM i ponownego sprawdzenia grantu po odebraniu body. Admin instancji nie zastępuje roli projektu, odpowiedzi mają `no-store`.
- Wynik producenta, jego approvals, rubryki i oceny muszą być dostępne w tym samym projekcie. Zbieżność ID z globalnym lub sąsiednim zasobem nie wystarcza. Magazyn otwiera wyłącznie `evaluation-judges/calibrations/` pod prefiksem projektowego rejestru — nie otwiera konfiguracji/odpowiedzi sędziów ani deklaracji pomiarów. Rejestr authored-only nadal odmawia kalibracji.
- Zachowano oryginalny hash, format, idempotencję pierwszego zapisu i weryfikację przy odczycie. Zmiana oceny człowieka tworzy nowy zbiór, a nie przepisuje stary. Kalibracja zawiera wartości ocen i referencje, nie pytania/odpowiedzi; istniejący zbiór pozostaje czytelny po wycofaniu wyniku, ale stworzenie nowego wymaga aktualnie czytelnego źródła. To istniejący kontrakt, nie mechanizm usuwania metadanych ocen.
- Granice: wyłącznie kalibracje z projektowych wyników producentów. Rozmowy, wyniki serwerowe/sędziowskie, deklaracje pomiarów i ich wykonania pozostają zamknięte. Utworzenie kalibracji nie uruchamia sędziego. Odczyt wielu stron nie jest wspólną transakcją IAM/magazynu; cofnięcie grantu nie przerywa już dopuszczonej pracy. Bez migracji danych, wdrożenia i aktywacji selektorów UI.

Walidacja:

- API: **23 testy biblioteki/kontraktu i 241 HTTP**, powodzenie. **2 nowe scenariusze** sprawdzają obie trasy, izolację projektów/organizacji/globalnych danych oraz każdej zależności, niezmienność po zmianie oceny, wycofanie, przedziały/cofnięcie grantów, role instancji kontra projektu, nagłówek, spóźniony upload, brak uwierzytelnienia/IAM i `no-store`.
- Ewaluacje: **96 testów**, powodzenie. Serwer (`--test evaluation`): **141 przeszło, 1 istniejący RustFS/S3 pominięty**. **1 nowy test** używa produkcyjnego resolvera i plikowego magazynu: reopen, idempotencja, brak fallbacku, wykrywanie uszkodzenia, usunięcie i odmowa pozostałych rodzin judge/scoring.
- Clippy API/ewaluacji/serwera z `--all-targets -- -D warnings`, format i `git diff --check`: powodzenie. OpenAPI i klient wygenerowane, `just openapi-check`: powodzenie.
- Panel: **53 pliki, 417 testów**, build architektury/Vite/TypeScript — powodzenie. UI bez zmian; bez nowego przeglądu wizualnego. Nadal ostrzeżenie o chunku ponad 500 kB.
- Nie wykonano pełnego E2E Authentik/PostgreSQL/S3 ani pomiaru na workerze. Testy HTTP korzystają z rzeczywistego routera, podpisanych sesji i pamięciowego IAM; resolver produkcyjny sprawdzany osobno w teście serwera.

Fundament deklaracji nad nagraniami wykonano poniżej. Projektowe trasy deklaracji, pomiary z kalibracjami i autoryzacja uruchamiania oraz pracy wykonawców pozostają otwarte, podobnie jak pozostałe bramki IAM-01, migracja i cutover.

## Kontynuacja — fundament projektowych deklaracji nad nagraniami IAM-01

**Etap biblioteczny, bez nowych tras HTTP i bez uruchamiania pomiarów.** IAM-01 i cała migracja pozostają w toku. Kontrakt: [README IAM](../crates/aiwatcher-iam/README.md#project-recording-declarations-library-foundation).

- Projektowy rejestr dowodów zapisuje niezmienne deklaracje pod `evaluation-scopes/<org>/<project>/registry/evaluation-runs/`. Zakres nie zmienia hasha; identyczna deklaracja w dwóch projektach ma ten sam ID, ale niezależnego pierwszego autora i czas. Węższe rejestry authored/cohort nadal odmawiają dostępu do deklaracji.
- Przed zapisem wymagane są lokalna przypięta karta, zweryfikowane bajty nagrania i lokalne metadane dokładnie tej kohorty. Resolver ponownie wyprowadza przypięcia z właściciela datasetu/anotacji; sam wcześniejszy zapis pochodzenia kohorty nie zastępuje obecności źródła. Pełny manifest jest walidowany przed zapisem. Brak lub uszkodzenie nie uruchamia fallbacku do globalnych danych, sąsiedniego projektu ani katalogu hosta.
- Odczyt deklaracji oraz ponowiony zapis sprawdzają jej adres treści i zapisane ID, również na starej ścieżce bibliotecznej. Pełny widok ponownie sprawdza zależności; surowe metadane deklaracji pozostają czytelne po utracie źródła, bez udawania dopuszczonego pomiaru.
- Obsługiwany zakres to nagrania, natywne kohorty curation/anotacji i wbudowane scorery. Sędziowie, zewnętrzni scorerzy/kalibracje, archiwum, zewnętrzne kohorty i generowanie odpowiedzi pozostają odrzucane. Resolver nadal odmawia zatwierdzania `aiwatcher.scoring`, a widok deklaracji pokazuje `admitted: false`.
- Granice: referencje kodu, konfiguracji generowania, promptu/modelu/workflow przechodzą walidację struktury manifestu, nie pełną weryfikację bajtów i właścicieli — ta należy do przyszłego projektowego admission. Argument autora nie jest autoryzacją IAM. Nie dodano HTTP, wykonawcy, polityki retencji deklaracji ani wspólnej transakcji IAM/magazynu. Nie aktywowano selektorów UI, nie migrowano danych i nie zmieniano wdrożenia.

Walidacja:

- `cargo test -p aiwatcher-evaluation -p aiwatcher-api --quiet`: **360 testów**, powodzenie (96 ewaluacji, 23 biblioteki/kontraktu API, 241 HTTP).
- `cargo test -p aiwatcher-server --test evaluation --quiet`: **143 przeszły, 1 istniejący RustFS/S3 pominięty**. Dwa nowe testy używają produkcyjnego resolvera; obejmują reopen plikowego magazynu, niezależność projektów/organizacji/globalnych danych, wszystkie lokalne zależności obsługiwanego pomiaru, idempotencję, uszkodzenie deklaracji i brak/uszkodzenie źródeł, odmowy nowych uprawnień wykonawczych i węższych rejestrów.
- Clippy API/ewaluacji/serwera z `--all-targets -- -D warnings`, `cargo fmt --all --check` i `git diff --check`: powodzenie. Nie zmieniono kontraktu HTTP ani UI; bez regeneracji klienta, testów panelu i nowego przeglądu wizualnego w tej iteracji.
- Testy biblioteczne nie zastępują projektowej autoryzacji deklaracji przez HTTP ani pełnego E2E z Authentikiem, PostgreSQL, S3 i workerami.

Projektowe trasy deklaracji dodano w kolejnym etapie poniżej. Pomiary sędziowskie i kalibracje, weryfikacja pełnego wariantu przy admission, autoryzacja uruchamiania/wykonawców i strumieni pozostają otwarte. Pozostałe rejestry, pełny manifest migracji, wykonawca, cutover i trwała koordynacja publikacji review nadal wymagają wdrożenia.

## Kontynuacja — projektowe API deklaracji IAM-01

Dodano `POST /api/v1/orgs/{organization}/projects/{project}/evaluation-runs` i `GET .../evaluation-runs/{id}`. Kontrakt: [README IAM](../crates/aiwatcher-iam/README.md#thirteenth-resource-boundary-project-recording-declarations). **IAM-01 nadal w toku; deklaracja nie uruchamia ani nie zatwierdza pomiaru.**

- Te same handlery obsługują projektowe i stare trasy. Odczyt wymaga bieżącego grantu viewer, deklarowanie editor oraz `X-AIWatcher-IAM: 1`. Po odebraniu JSON grant editor jest sprawdzany ponownie. Rola administratora instancji nie zastępuje uprawnień projektu; autor pochodzi z uwierzytelnionej tożsamości, nie z body.
- Projektowe odpowiedzi mają `Cache-Control: no-store`. Zakres jest wiązany przez extractor, bez fallbacku do globalnych deklaracji. Nie dodano projektowej trasy `/start` ani katalogu zewnętrznych scorerów. Globalne `/start` nie odnajduje deklaracji zapisanej wyłącznie w projekcie.
- Zachowano ograniczenia poprzedniego etapu: nagrania, natywne kohorty i wbudowane scorery. Sędziowie, zewnętrzni scorerzy, generowanie, archiwum i autoryzacja workerów pozostają poza wdrożeniem. Kontrola grantu jest dopuszczeniem operacji, nie wspólną transakcją IAM/magazynu ani anulowaniem zapisu już rozpoczętego przed cofnięciem grantu.
- Dwa testy HTTP pokrywają izolację projektów/organizacji/globalnych danych, niezależne identyczne ID, idempotencję, role i okna czasowe, cofnięcie dostępu, brak nagłówka, administratora instancji bez grantu, utratę roli podczas uploadu bez zapisu deklaracji, odmowę generowania i uruchamiania oraz brak IAM/uwierzytelnienia. Resolver kohort HTTP jest dublem; produkcyjne sprawdzanie źródeł i bajtów pokrywają testy serwera z poprzedniego etapu.

Walidacja: `cargo test -p aiwatcher-api --quiet` — **266 testów** (23 biblioteki/kontraktu, 243 HTTP); Clippy API z `--all-targets -- -D warnings` — powodzenie. `just openapi` odświeżył kontrakt i klienta panelu; `npm run build` panelu przeszedł wraz z kontrolą architektury i TypeScriptem (ostrzeżenie Vite o dużym chunku). Nie zmieniono interfejsu ani nie aktywowano selektorów. Brak nowego wizualnego odbioru i pełnego E2E z Authentikiem/PostgreSQL/S3/workerami; migracja danych i cutover nadal niewykonane.

Następne bramki: projektowe pomiary sędziowskie/zewnętrzne, następnie uruchamianie oraz autoryzacja wykonawców. Admission wariantu dla ograniczonego zakresu pomiarów wdrożono poniżej. Nie przenosić globalnego `/start` pod projektowy prefiks bez izolacji ścieżek wykonania.

## Kontynuacja — admission wariantu i zamknięta granica wykonawców IAM-01

**Częściowe wykonanie etapu: admission działa dla nagrań/natywnych kohort/wbudowanych scorerów; autoryzowane projektowe wykonania nadal niewdrożone.** Kontrakt i następne wymagania: [README IAM](../crates/aiwatcher-iam/README.md#project-admission-for-built-in-recording-measurements).

- Istniejąca projektowa trasa zatwierdzania (rola project admin) może dopuścić `aiwatcher.scoring` w tym zakresie. Rejestr weryfikuje wersję skompilowanego scorera, lokalną kartę i dokładnie jej metryki. Resolver sprawdza projektowy pakiet wariantu, bajty kodu/konfiguracji/schematów/narzędzi/workflow, właścicieli promptu/modelu oraz natywną kohortę. Pomija wyłącznie producenckie `suite.json`/`scorer.py`, zastąpione kartą i binarium. Przypięcie workflow jest nadal weryfikacją bajtów, nie zgodą na jego wykonanie.
- Admission i pełny widok projektowego pomiaru ponownie sprawdzają lokalne źródło i zgodność pakietu po znalezieniu niewycofanego approval. Uszkodzenie po zatwierdzeniu nie oznacza już `admitted: true`; może zakończyć odczyt błędem. Koszt: ponowne rozwiązywanie źródła w istniejących limitach adaptera, bez cache pomiędzy żądaniami.
- Wspólne przygotowanie trzech ścieżek wykonawców scoringu odmawia projektowego rejestru jako `Policy` przed odczytaniem deklaracji. Samo przekazanie zatwierdzonego projektowego rejestru do `ScoreExecutor` nie daje uprawnień wykonawczych. To blokada braku autoryzacji, **nie implementacja autoryzowanego projektowego wykonania**.
- Powód pozostawienia blokady: historia wykonania, artefakty i strumienie nadal mają globalne ścieżki. Zgoda na start bez ich izolacji otworzyłaby drugą drogę do projektowych danych. Nie dodano `/start`, nie uruchomiono workerów i nie aktywowano selektorów UI.

Nowy test produkcyjnego adaptera: staging zależności, lokalny prompt mimo identycznego globalnego, idempotencja i reopen magazynu, uszkodzenie kodu/konfiguracji/workflow po zatwierdzeniu, odmowa innej wersji scorera i kierunku metryki, izolacja projektu/organizacji/globalnych danych, withdrawal oraz odmowa wykonawcy bez publikacji wyniku. Istniejące testy wspólnego resolvera obejmują właścicieli promptów/modeli; istniejące testy HTTP wymagają project admin do zatwierdzania.

Walidacja: testy API i ewaluacji — **362 przeszły**; testy integracyjne `aiwatcher-server --test evaluation` — **144 przeszły, 1 RustFS/S3 pominięty**. Nie zmieniono kształtów HTTP ani klienta. Brak pełnego E2E Authentik/PostgreSQL/S3/worker.

Następny etap wykonawczy musi zapisać zaufany principal i scope razem z wykonaniem, odizolować identyfikator/historię/komendy/artefakty/strumienie oraz sprawdzać aktualne granty przy podejmowaniu pracy i publikacji, z jawną polityką cofnięcia dostępu podczas commit. Dopiero wtedy można otworzyć projektowe `/start`. Jawna autoryzacja samego wykonawcy jest kolejnym etapem poniżej.

## Kontynuacja — jawna autoryzacja wykonawcy nagrań IAM-01

**Etap biblioteczny wykonawcy, nie uruchamianie managed runs. IAM-01 nadal otwarte.** `ScoreExecutor::for_project` przyjmuje jawną `ProjectAuthority`: magazyn IAM, scope, dokładny principal provider/subject, identyfikator wykonania i deklaracji. Typ nie jest serializowany ani odbierany przez HTTP. Przyszły dispatcher musi odczytać te dane z zaufanego, trwałego właścicielstwa wykonania — nie z planu, parametrów, nazwy workera czy autora deklaracji. **Trwałe właścicielstwo i dispatcher nadal niewdrożone.**

- Przed odczytem deklaracji: zgodność scope/wykonania/deklaracji/klucza kroku, runtime nagrania i aktualny grant co najmniej Editor. Pełny widok deklaracji sprawdza lokalne zależności i ograniczony zakres pomiarów. Approval pozostaje osobną zgodą na dowody.
- Po obliczeniu wyniku, przed `Committing` i publikacją: ponowne pytanie do IAM. Cofnięcie grantu lub koniec okna edycji daje Policy, niedostępność magazynu IAM — Transient. Wynik zapisuje się wyłącznie w projekcie i nie jest cacheowalny.
- Jawna granica: cofnięcie dostępu podczas odczytu blokuje publikację; po dopuszczeniu zapisu commit kończy się w całości. Nie jest to transakcja IAM + object store ani natychmiastowe przerywanie wszystkich odczytów/obliczeń. Kolejna próba ponownie wymaga grantu.
- Standardowy executor z projektowym rejestrem nadal odmawia. Projektowy executor nie może użyć globalnych artefaktów, sędziego ani scorer service. Nie dodano go do globalnego rejestru wykonawców — ten nie jest jeszcze projektowym dispatcherem, a jego cache lookup poprzedza wykonanie.

Cztery testy integracyjne używają produkcyjnego resolvera, plikowego object store i rzeczywistej polityki memory IAM: błędny principal/wykonanie/deklaracja/runtime, rebinding, lokalna publikacja i brak cache, withdrawal approval, cofnięcie/wygaśnięcie grantu w trakcie zatrzymanego odczytu, cofnięcie podczas zatrzymanego commit i odmowa kolejnej próby. Właściciel organizacji bez aktywnego projektowego grantu nie może wykonywać pomiaru.

Walidacja: **148 testów integracyjnych ewaluacji i 147 testów biblioteki serwera przeszło**, 1 test RustFS/S3 pominięty; Clippy serwera `--all-targets -D warnings`, formatowanie i `git diff --check` poprawne. Kontrakt HTTP nie zmieniony.

Następna bramka: trwała tożsamość i scope wykonania oraz izolacja claimów, historii, artefaktów i strumieni, potem dispatcher i `/start`. Brak nowych tras, selektorów UI, migracji/cutover i pełnego E2E wdrożenia. Szczegółowy kontrakt: [README IAM](../crates/aiwatcher-iam/README.md#project-recording-executor-explicit-authority-not-yet-a-start-path).

## Kontynuacja — projektowe bajty artefaktów i receipty prób IAM-01

**Wydzielona część bramki magazynu wykonania, nie trwałe właścicielstwo ani uruchamianie projektowych zadań. IAM-01 nadal otwarte.** Kontrakt: [README IAM](../crates/aiwatcher-iam/README.md#project-artifact-bytes-and-attempt-receipts-storage-foundation).

- `Artifacts::for_project` wiąże tabele, ich oryginalny JSON, logi podów i receipty z `artifacts/scopes/<org>/<project>/registry/`. Te same bajty zachowują digest, ale mają osobny URI w każdym projekcie. Te same klucze prób nie współdzielą receiptów. Przepięcie związanego magazynu do innego zakresu jest odrzucane.
- Odczyt bajtów i sprawdzenie obecności wymagają dokładnego klucza danych zgodnego z zakresem, rodzajem i SHA-256 referencji. Odmowa następuje przed I/O, również dla innych rejestrów, receiptów, traversal i zakodowanych separatorów. Port workerów korzysta z tej samej reguły. **Stary globalny czytnik również odmawia projektowych URI**, bez fallbacku; poprawne referencje dotychczasowego writera zachowują działanie.
- Zapis receiptu odmawia cudzej referencji; odczyt sprawdza także zgodność z żądanym kluczem próby. Podmieniony receipt nie staje się wynikiem innego wykonania. Nieczytelny JSON pozostaje cache miss zgodnie z wcześniejszym kontraktem. Zachowano kolejność dane → receipt i weryfikację hasha odczytanych bajtów.
- Granice: to biblioteczna izolacja magazynu, bez samodzielnej autoryzacji IAM/lease. Dispatcher musi dopiero odczytywać scope z trwałego, zaufanego właścicielstwa. Katalog metadanych, lineage/cache, rozliczanie retencji, historia, claimy i strumienie nadal wymagają izolacji; nie wolno podłączyć projektowych bajtów do globalnego katalogu/reactora. Nie otwarto `/start`, nie aktywowano UI, nie wykonano migracji ani wdrożenia.

Walidacja: `cargo test -p aiwatcher-server --quiet` — **300 testów przeszło** (147 biblioteki, 148 ewaluacji, 5 nowych integracji artefaktów), **1 istniejący RustFS/S3 pominięty**. Nowe testy obejmują rzeczywisty magazyn plikowy i reopen, identyczne hashe i klucze prób w projektach/organizacjach/globalnie, dokładny zapis dużych liczb, niezależne usunięcie, podmianę referencji/receiptów oraz brak I/O przy odmowie. Clippy serwera `--all-targets -- -D warnings`, format i `git diff --check`: powodzenie. Bez zmiany kontraktu HTTP/UI i bez pełnego E2E Authentik/PostgreSQL/S3/workerów.

Następne kroki nadal obejmują trwały principal/scope zapisany razem z wykonaniem, izolację pozostałych ścieżek i dopiero potem dispatcher oraz projektowe `/start`. **Cała migracja pozostaje w toku.**

## Kontynuacja — cztery równoległe strumienie IAM-01, scalone

Data: 18.09.2026. **Etap biblioteczny czterech strumieni naraz. Projektowe `/start` pozostaje zamknięte, selektory UI nieaktywne, cutover niewykonany — IAM-01 i cała migracja są nadal w toku.**

Cztery strumienie pracowały równolegle z jednego snapshotu; integrator scalił je i ich raporty. Decyzja, którą trzy z nich wspólnie realizują, to [ADR_0033](ADR/ADR_0033_PROJECT_SCOPED_STORAGE.md); kontrakt jest w [README IAM](../crates/aiwatcher-iam/README.md), procedura operatora w [runbooku migracji](iam-migration-runbook.md), a to, co zostało do zrobienia — w [kickoffie IAM-01](iam-01-kickoff.md).

- **A — trwałe właścicielstwo wykonania.** Wykonanie ma `ExecutionOwnership { scope, principal, definition }`, zapisywane w tej samej transakcji, w której powstaje, i nigdy więcej. Jest niezmienne: kolejna komenda, replay, retry i powtórzony start potwierdzają je, a start nazywający innego principala jest odmową **przed** inboxem, nie duplikatem. Zakres wiąże **magazyn** (`WorkflowStore::for_project`), więc nieobjęty zakresem uchwyt odmawia z drugiej strony — reactor, worker, launcher, tick timerów, publikator outboxa i sweep retencji nie wymagały żadnej zmiany, bo magazyn, który trzymają, nie widzi pracy projektu. Identyfikator wykonania rozróżnia projekt, a identyfikatory globalne nie drgnęły co do bajtu. Checkpointy procesorów i sloty harmonogramu są instancyjne i są odmawiane po nazwie. Migracja PostgreSQL 0011 jest addytywna i nic nie backfilluje.
- **B — projektowy katalog artefaktów, lineage i cache.** `aiwatcher_execution::artifact::layout` jest jedynym miejscem, które mówi, jak wygląda klucz — czyta go katalog i writer bajtów, więc manifest i opisywany obiekt nie trafią pod dwie reguły. Wszystkie sześć metod katalogu jest izolowanych w obie strony; `ProjectArtifacts::bind` buduje obie połowy z jednego zakresu i porównuje prefiksy po fakcie. Pomiar magazynu dzieli przejście na `global`, per projekt, `unattributed` i `elsewhere`; kolekcji nie ma i jest to jawna odmowa, bo dla obszaru zakresowego nie istnieje jeszcze źródło prawdy o osiągalności.
- **C — projektowe pomiary sędziowskie i zewnętrzne.** Karta projektu może nazwać rubrykę sędziego albo metrykę frameworka; przypięta konfiguracja sędziego i trzymane odpowiedzi obu usług przechodzą pod prefiks projektu, a katalog scorerów pozostaje deploymentowy — czytany z projektu, zapisywany tylko przez rolę `work`. Archiwum rozmów jest zamknięte po obu stronach i po nazwie. Produkcyjny `ScoreExecutor` nadal odmawia projektowego rejestru.
- **D — manifest migracji i jego wykonawca.** `aiwatcher-migration`: właściciel podaje klucze, manifest jest czystą funkcją snapshotu, nic nie jest brane na wiarę z pliku (`execute` planuje ponownie i porównuje `manifest_id`), a nieobecność jest odpowiedzią — `empty`, `unsupported`, `blocked`, `damaged`, `foreign` i `unknown prefix` to sześć różnych stanów i żaden z nich nie jest zerem. Wspierane są cztery rodziny (prompty, datasety, treningi, anotacje); rozmowy są **blocked** (kopia bajtów ich nie otworzy, ADR_0021), a jedenaście pozostałych prefiksów jest nazwanych z liczbą obiektów. Zapis wymaga roli **admin** w projekcie; `--iam fixture:` znakuje przebieg jako nieautorytatywny. **Mapowanie danych do projektu nikomu nie nadaje do nich dostępu.**

Uzgodnienia integratora — trzy miejsca, w których strumienie się stykały:

- **Jeden `StoreError::OutOfScope`, nie dwa.** A odmawiał wykonania, do którego magazyn nie jest związany, B — rekordu, referencji lub klucza z innego zakresu. Odpowiadają na to samo pytanie tak samo (`says_the_same_next_time == true`), więc został jeden wariant niosący zdanie.
- **Odmowa zakresu to 404, nie 503 ani 502.** B nazwał 503 obietnicą powrotu po trwałą odmowę; 502 czytałoby się jako awaria czegoś wyżej, a nic nie zawiodło. Przebieg, do którego ktoś nie sięga, to przebieg, którego nie ma.
- **Prośba C do A nie została wykonana i to jest właściwa odpowiedź na tym etapie.** C potrzebuje, by `ProjectAuthority::authorize` dopuszczał `JudgeEvaluation` i `ExternalEvaluation`, a pierwszy strażnik `ScoreExecutor::execute` odmawiał wyłącznie przy `artifacts`. Obie zmiany są bezpieczne dopiero po bramce A, która nie jest zamknięta — więc kontrakt jest zapisany, a strażnik pozostaje zamknięty.

Walidacja scalonego drzewa: `cargo test --workspace --all-targets` — **1784 testy przeszły, 0 niepowodzeń, 7 pominiętych**; `cargo clippy --workspace --all-targets --all-features -- -Dwarnings`, `cargo fmt --all --check`, `git diff --check` i `scripts/check-rust-boundaries.py` — czysto; `contracts/openapi.json` zregenerowany i identyczny, więc klienta panelu nie ruszano. Integracja znalazła jedną rzecz, której żaden strumień nie mógł zobaczyć sam: nowy crate `aiwatcher-migration` nie był zarejestrowany w `scripts/rust-boundaries.json`, co przewracało kontrolę granic — dopisany z jego rzeczywistymi zależnościami. PostgreSQL każdy strumień weryfikował na własnej jednorazowej bazie; nie uruchomiono Authentika, S3/RustFS, klastra, workerów ani żadnego E2E, nie dotknięto danych użytkownika.

Następna bramka jest jedna i wspólna: **dispatcher**, który czyta scope i principala z trwałego właścicielstwa, buduje `ProjectArtifacts` i sprawdza aktualny grant **przed** lookupem w cache oraz ponownie przed publikacją. Dopiero po nim — projektowe `/start`, ścieżka faktów/outboxa/projekcji albo jawna decyzja, że fakty projektu nie trafiają na log, zakresowy sweep retencji, oraz granty na każdym odczycie, komendzie, artefakcie i trasie workera. Do tego czasu żaden produkcyjny caller nie tworzy związanego magazynu.

## Kontynuacja — dispatcher projektowy i plan data plane IAM-02

Data: 18.09.2026. **Zamknięta bramka biblioteczna, nie wdrożenie. Projektowe `/start` pozostaje zamknięte, selektory UI nieaktywne, cutover niewykonany.**

Dispatcher — jedyna bramka, na którą czekało wszystko inne z IAM-01 — stoi i jest przetestowany. Kontrakt: [README IAM](../crates/aiwatcher-iam/README.md#the-project-dispatcher-the-gate-everything-else-was-waiting-on), granica: [ADR_0033](ADR/ADR_0033_PROJECT_SCOPED_STORAGE.md).

- `aiwatcher_execution::authority` — nowy port `ExecutionAuthority` z `Admitting { Work, Publication }`. Dostaje rekord z `store.ownership` i **nic poza nim**, więc nie ma skąd wziąć zakresu ani principala z planu, parametru, nazwy claimanta czy autora deklaracji. To jest pierwsza reguła ADR_0033 zapisana jako własność typu, nie jako konwencja.
- `Reactor` pyta w `take` **przed** wczytaniem strumienia — a więc przed lookupem w cache i przed `step.started` — oraz w `settle_at` po sprawdzeniu lease i **przed** zapisem do katalogu. Odmowa ponawialna przed pracą zostawia próbę do wzięcia i nie wydaje budżetu retry; przed publikacją każda klasa jest raportowana, bo lease należy do tego przebiegu i milczenie zgubiłoby wynik. `Policy` kończy przebieg, zamiast pozwalać, by był claimowany i porzucany co pass w nieskończoność. Awaria IAM to `Transient`, nigdy zgoda.
- `ProjectDispatcher` wiąże magazyn, buduje obie połowy artefaktów przez `ProjectArtifacts::bind` i **buduje executor per próba** z właściciela tej próby — więc nic projektowego nie trafia do procesowego `ExecutorRegistry`. `Reactor::perform` i `settle_at` stały się publiczne; `resume` odpowiada `None`, gdy jest authority.
- Po zielonej bramce zdjęto dwie blokady strumienia C: `ProjectAuthority::authorize` dopuszcza `ScoreEvaluation`, `JudgeEvaluation` i `ExternalEvaluation` (nadal porównując przypiętą deklarację), a pierwszy strażnik `ScoreExecutor::execute` odmawia wyłącznie przy globalnym `artifacts`. Sędzia i usługa scorerów to klienci deploymentu odpowiadający na pytanie ułożone z karty projektu; czytnik artefaktów rozwiązuje `object://` w przestrzeni deploymentu, czyli cudze bajty.

Walidacja: `cargo test --workspace --all-targets` — **1800 testów, 0 niepowodzeń, 7 pominiętych**; z `--all-features` **1809 / 0 / 44**. Clippy `--workspace --all-targets --all-features -- -Dwarnings`, `cargo fmt --all --check`, `git diff --check`, `scripts/check-rust-boundaries.py` — czysto; `scripts/lint-comments.py` zgłasza te same sześć nagłówków co `main`, żadnego nowego. PostgreSQL na osobnej, jednorazowej bazie: kontrakt magazynu (5), suita upgrade'u (7), suita IAM (17). `contracts/openapi.json` zregenerowany i identyczny — kontrakt HTTP nie ruszony, klient panelu nie ruszony. Nie uruchomiono Authentika, S3/RustFS, klastra, workerów ani żadnego E2E.

**Plan na drugą połowę: [IAM-02 — data plane](iam-02-data-plane.md).** Jego bramka M1 to dokładnie to, czego dziś brakuje użytkownikowi: zalogować się, zobaczyć wyłącznie swoje projekty i słuchać wyłącznie swoich strumieni. Spina się na jednej decyzji — `EventEnvelope` niesie opcjonalny `ProjectScope`, pisany przy ingeście z poświadczenia i nigdy nie honorowany od producenta; log per projekt odrzucono, bo `AIWATCHER_LASER_PARTITIONS > 1` jest zakazane, dopóki skalarny `Checkpoint` nie stanie się kursorem per partycja. Fold zostaje **jeden**, z kluczem zakresu w wierszu — fold per tenant byłby przesłanką do rewizji ADR_0033, a nie jego realizacją.

Do dzielenia projektu brakuje trzech rzeczy wymienionych wyżej w „IAM i współpraca": zaproszeń, interfejsu i odwołania sięgającego strumieni. Połowa autorska jest gotowa do dzielenia od razu po nich; obserwowalność wymaga E1–E4. **Selektora nie aktywujemy przed M1.**

## Kolejność do pierwszego testu permissionów i lekcji — 18.09.2026

Cel: **jak najszybciej dać się przetestować uprawnieniom, udostępnianiu lekcji i nowemu UX-owi**. To nie jest pełne IAM i nie udaje, że jest — [IAM-02](iam-02-data-plane.md) pozostaje na końcu.

Dwie obserwacje z kodu skracają tę drogę bardziej, niż wynikałoby z planu:

- **Lekcja to projekt plus okno grantu.** Nie ma i nie musi powstać pojęcie „lekcji" w backendzie. `GrantWindow { valid_from, edit_until, read_until }` istnieje i jest dokładnie prymitywem warsztatowym: dostęp od, edycja do, odczyt do. Wygaśnięcie edycji zrzuca rolę do Viewer, a niezależny grant stały nie znika — to już jest własność polityki, nie rzecz do zbudowania.
- **Zaproszenia nie blokują testowania uprawnień.** Blokują zapraszanie kogoś, kto nigdy się nie logował. Dwa konta, które raz przeszły przez SSO, wystarczą: drugi `subject` odczytuje się z audytu, a grant nadaje ręcznie. Zaproszenia przesuwają się o jeden krok.

| # | Krok | Co odblokowuje | Uwaga |
|---|---|---|---|
| 0 | `just authentik-up` + `just run-sso` | Tożsamość, a więc principal, a więc grant | **Jedyna pozycja o nieznanym koszcie — nigdy nie uruchamiana.** Cały przepływ OIDC jest w kodzie (`/api/v1/auth/login`, `/callback`, `/me`, `/logout`, `/account` w panelu). Justfile ostrzega: issuer kończy się slugiem **aplikacji**, nie nazwą providera |
| 1 | Panel: minimalny IAM | Ręczne testowanie uprawnień bez curla | Czysty frontend. Backend gotowy i naprawdę sprawdza granty; wygenerowany klient ma 82 wpisy, których dziś nikt nie woła |
| — | **Tu testujesz** | Dwa konta, projekt = lekcja, okno grantu = udostępnienie | Sprawdzane: widoczność cudzych projektów, `edit_until` → Viewer, `read_until` → odcięcie, odebranie grantu, suma grantów z dwóch źródeł |
| 2 | Zaproszenia | Zapraszanie spoza już-zalogowanych | Jednorazowy, wygasający token `(scope, role, window)`, realizowany po SSO w jednej transakcji z nadaniem grantu |
| 3 | Nowy shell UX | Krok 2 z „Kolejności wdrażania" | **Zrobione** jako przegląd, nie przebudowa: UX-09 dowiozło całość. Zostają System (brak kontraktu), Administracja jako obszar (świadomie nie), „ostatnie" na starcie, agenci i harmonogramy. Naprawiono fokus w nawigacji mobilnej |
| 4 | Learning UI | Warsztaty, uczestnicy, 9 slotów laboratoriów | **Zrobione** nad grantami: warsztat = projekt, uczestnik = grant, zapis = zaproszenie. Silnik treści i ocen osobno |
| 5 | IAM-02 | Obserwowalność projektu | Na końcu, świadomie |

**Czego ta ścieżka nie obejmuje, i trzeba to powiedzieć przed startem.** Uprawnienia będą przetestowane na **danych autorskich** — prompty, datasety, anotacje, treningi, ewaluacje, definicje workflow, review, kohorty, bundle, approvale, deklaracje. **Nie** na obserwowalności: przebiegi, spany, metryki i żywy strumień pozostają instancyjne do IAM-02. Udostępniona lekcja ma więc materiały, a nie ma historii wykonań ani podglądu na żywo. Selektor organizacji/projektu z kroku 1 jest **narzędziem testowym**, nie ogłoszeniem multi-tenancy.

### Prompt dla sesji realizującej tę kolejność

> Pracujesz w repozytorium AIWatcher nad **ścieżką do pierwszego testu uprawnień, udostępniania lekcji i nowego UX-u**. Przeczytaj `docs/ux-migration-plan-2026-09-14.md` (sekcja „Kolejność do pierwszego testu permissionów i lekcji"), `crates/aiwatcher-iam/README.md`, `apps/panel/CLAUDE.md` i instrukcje repozytorium. Zweryfikuj stan w kodzie — dokumentacja opisuje 18.09.2026 i mogła się zdezaktualizować.
>
> **IAM-02 (`docs/iam-02-data-plane.md`) jest świadomie ostatnie. Nie zaczynaj go.** Obowiązuje za to jedna reguła: nowy zasób autorski dostaje swoją zakresową rodzinę tras od urodzenia — `ProjectAuthorization` plus `<prefix>/scopes/<organization>/<project>/registry/`.
>
> Idź krokami i **nie przechodź dalej, dopóki poprzedni nie działa naprawdę**; po każdym kroku zdaj raport i powiedz, czego nie uruchomiłeś.
>
> **Krok 0 — SSO lokalnie.** `just authentik-up`, potem `just run-sso`. Doprowadź do stanu, w którym logujesz się w panelu i `/account` pokazuje tożsamość oraz grupy IdP tylko do odczytu. To nigdy nie było uruchamiane, więc traktuj to jak zadanie badawcze, nie konfiguracyjne: issuer kończy się slugiem **aplikacji**, nie nazwą providera; `ProviderMetadata::discover` odmawia rozjazdu z konfiguracją; ciasteczko `Secure` wywodzi się ze schematu redirect URL, więc po http nie zostanie zapisane i logowanie zapętli się bez błędu. Zapisz w `docs/` dokładne kroki, które zadziałały — następny człowiek nie ma ich skąd wziąć.
>
> **Krok 1 — panel: minimalny IAM.** Selektor organizacji/projektu, strona członków i grantów, formularz nadania grantu z `valid_from`/`edit_until`/`read_until`, oraz widoczne rozróżnienie: grupy IdP **nie są** zespołami. To czysty frontend nad wygenerowanym klientem; **nie zmieniaj kontraktu HTTP** i nie wołaj `just openapi` bez rzeczywistej zmiany API. Trzymaj konwencje `apps/panel/CLAUDE.md`: filtry w URL, nigdy w stanie komponentu, `src/api/generated` nietykalne, obszary opisane w `navigation.ts`.
>
> **Krok 1b — przetestuj uprawnienia ręcznie i opisz wynik.** Dwa konta w authentiku, oba raz przez SSO. Projekt jest lekcją, okno grantu jest udostępnieniem. Sprawdź: że B nie widzi projektów A bez grantu; że `edit_until` w przeszłości zrzuca rolę do Viewer, a nie odbiera odczytu; że `read_until` odcina; że odebranie grantu działa od razu na następnym żądaniu; że dwa niezależne granty sumują się i wygaśnięcie jednego nie zasłania drugiego; że właściciel organizacji **nie ma** domyślnego dostępu do projektu. Nieudany scenariusz zapisz jako scenariusz, nie jako usterkę do obejścia.
>
> **Krok 2 — zaproszenia.** Dopiero gdy krok 1b przechodzi. Jednorazowy, wygasający token związany z `(scope, role, window)`, realizowany **po** SSO w jednej transakcji tworzącej grant dla principala, który go zrealizował. Email jest wskazówką do dostarczenia, nigdy kluczem tożsamości; powtórzenie i inny odbiorca odmawiają.
>
> **Krok 3 — nowy shell UX.** Nie zależy od IAM i może iść równolegle od początku: przełącznik rollout/rollback, preferencje, przypięcia, dotychczasowe trasy jako adaptery.
>
> **Krok 4 — Learning UI.** Obszar `learning` istnieje z placeholderem. Lista i szczegół warsztatu, uczestnicy, stan dostępu, 9 slotów laboratoriów. **Brakujące kontrakty oznaczaj jako niedostępne** — żadnych wymyślonych materiałów, wyników ani postępu. Silnik treści i ocen to osobna praca.
>
> **Czego nie wolno.** Nie otwieraj projektowego `/start`, nie rejestruj dispatchera w produkcyjnym `spawn`, nie zakresuj logu zdarzeń ani strumieni — to jest IAM-02. Nie opisuj tego wdrożenia jako multi-tenant safe: selektor z kroku 1 jest narzędziem testowym. Nie zmieniaj istniejących migracji SQL; nowe są addytywne, z rolling upgrade i rollbackiem starego binarium.
>
> Pracuj na osobnej gałęzi z `main`, commituj po ścieżkach, jeden krótki konwencjonalny nagłówek, bez trailera współautora.
>
> **Testy na koniec każdego kroku.** Panel: `cd apps/panel && npm run test`, `npm run lint`, `npm run build` (to `check:architecture` + `vite build` + pełne `tsc -b`) — i sprawdź panel klawiaturą, przy 375 px i na szerokim ekranie, w motywie jasnym i ciemnym; żadna globalna kontrolka nie może wyjść poza viewport. Rust, jeśli tknąłeś którykolwiek crate: `cargo test --workspace --all-targets`, `cargo clippy --workspace --all-targets --all-features -- -Dwarnings`, `cargo fmt --all --check`, `git diff --check`, `python3 scripts/check-rust-boundaries.py`, `python3 scripts/lint-comments.py`. PostgreSQL wyłącznie na osobnej, jednorazowej bazie. **Jawnie wypisz, czego nie uruchomiłeś** — a w kroku 0 także to, co uruchomiłeś ręcznie i czego żaden test nie pilnuje.

## Kontynuacja — logowanie lokalne i panel IAM (kroki 0 i 1)

Data: 18.09.2026. **Kroki 0 i 1 z „Kolejności do pierwszego testu permissionów"
wykonane i zweryfikowane na żywym serwerze. Zaproszenia (krok 2), nowy shell
(krok 3), Learning (krok 4) i IAM-02 (krok 5) nietknięte. Cała migracja nadal
w toku.** Gałąź `iam-01/local-sso-and-panel`, dwa commity, niepushowane.

**Krok 0 — SSO lokalnie, po raz pierwszy uruchomione.** Cały przepływ OIDC
działa: `just authentik-up`, `just run-sso`, logowanie w panelu na :5173,
`/account` pokazuje tożsamość i grupy IdP tylko do odczytu. Dokładne kroki są
w [docs/local-sso.md](local-sso.md) — pisane dla następnej osoby, nie jako
sprawozdanie. Cztery rzeczy, których plan nie mógł przewidzieć, bo wychodzą
dopiero przy pierwszym uruchomieniu:

- **Pierwsze uruchomienie nie musi być klikaniem.** `AUTHENTIK_BOOTSTRAP_PASSWORD`,
  `_TOKEN` i `_EMAIL` w compose są czytane raz, przy pierwszym starcie na pustej
  bazie, przez własny `blueprints/system/bootstrap.yaml` authentika. Dzięki temu
  `just authentik-secret` czyta sekret klienta przez API zamiast przepisywania go
  z panelu admina, a `just authentik-seed` zakłada dwie osoby, których wymaga
  test grantu: `teacher` (w `aiwatcher-admins`, czyli instancyjny admin) i
  `student` (bez grup, czyli viewer).
- **Health wyprzedza blueprinty.** `/-/health/ready/` odpowiada 200, zanim worker
  zastosuje blueprinty; żądanie z tokenem bootstrapowym w tej szczelinie dostaje
  `Token invalid/expired`. To nie jest błąd konfiguracji i kosztowało jedno
  fałszywe śledztwo.
- **Issuer i tak trzeba sprawdzić dyskretnie.** Dokument discovery pod
  `…/application/o/aiwatcher/.well-known/openid-configuration` jest jedynym
  dowodem, że slug jest aplikacji, a nie providera.
- **Dwie osoby nie mieszczą się w jednym profilu przeglądarki.**
  `scripts/sso-session.py` przechodzi cały authorization code + PKCE przez API
  egzekutora flow authentika i oddaje ciasteczko sesji aiwatchera. Pułapka jest
  jedna: egzekutor chce query stronę flow jako **jeden** parametr `query`;
  przekazane wprost `next=` gubi się, a flow kończy na stronie użytkownika
  authentika zamiast wrócić na callback — bez żadnego błędu, bo nic nie zawiodło.

Do tego `just run-sso-iam`: ten sam serwer z płaszczyzną kontroli IAM na
**własnej** bazie `aiwatcher_iam` (nie magazynu workflow), bo IAM jest
oidc-only i odmawia startu na innym trybie zamiast schodzić do pamięci.

**Krok 1b — wykonany przed panelem, skryptem.** `scripts/iam-permission-check.py`
zadaje serwerowi szesnaście pytań dwiema prawdziwymi sesjami OIDC i **wszystkie
trzymają**: że viewer nie założy organizacji; że nie-członek nie potrafi nawet
nazwać cudzej organizacji (404, nie 403); że założenie projektu nadaje twórcy
jawny grant i że to jedyny grant, który ktoś dostaje bez nadania; że **właściciel
organizacji nie ma dostępu do cudzego projektu**; że grant od jutra nie daje nic
dziś; że po `edit_until` rola spada do Viewer i czytanie zostaje; że po
`read_until` projekt znika; że dwa żywe źródła sumują się do maksimum i odebranie
jednego zostawia drugie — na następnym żądaniu; że komenda bez `X-AIWatcher-IAM`
jest odmawiana; i że komenda nazywająca własnego aktora jest odrzucana (422 —
ciało odmawia nieznanych pól, więc nigdy nie dociera do handlera).

**Krok 1 — panel.** `/account` stało się obszarem z dwoma widokami: dotychczasowy
**Profile** i nowy **Organizations & projects**. Jest tam selektor organizacji i
projektu (w URL), świeża decyzja o dostępie z każdym źródłem i jego oknem,
formularz udostępnienia z `valid_from` / `edit_until` / `read_until`, komendy
członkostwa i zespołów, oraz historia z audytu z akcją odebrania grantu.
Weryfikacja na żywo: konto zalogowane w przeglądarce dostało grant `editor` z
oknem od teacher-a przez skrypt i **zobaczyło lekcję po stronie odbiorcy** — z
rolą, oknem i zdaniem, że to migawka, a nie klucz.

Trzy rzeczy, które ten krok ustalił i które wchodzą do `apps/panel/CLAUDE.md`:

- **Nie ma przełącznika organizacji w nagłówku.** Selektor zakresujący cały panel
  byłby ogłoszeniem multi-tenancy, którego płaszczyzna danych nie utrzyma; wybór
  siedzi w URL tej jednej strony i znaczy „rzecz, którą administruję".
- **Odpowiedź o dostępie nie jest cache'owana** (`staleTime: 0`). `ProjectAccess`
  niesie `evaluated_at` i jest decyzją, nie zdolnością — odebrany grant nie może
  dalej być prawdziwy dlatego, że react-query go jeszcze trzymał.
- **Historia to audyt i jest tak podpisana.** I to jest znalezisko kroku 1:
  **API nie ma trasy czytającej członków organizacji, jej zespoły ani granty
  projektu.** `projects` i `access` odpowiadają o *wołającym*, co jest właściwym
  kształtem dla sprawdzenia uprawnienia i niewłaściwym dla listy administratora.
  Panel więc nie odtwarza stanu z historii — złożenie go w przeglądarce byłoby
  drugą implementacją polityki, błędną przy pierwszym wygasłym grancie. Stąd też
  bierze się id grantu do odebrania. To jest brakujący kontrakt dla kroku 2:
  zaproszenia i tak go potrzebują, więc rosną razem.

Co uruchomiono: panel `npm run build` (czyli `check:architecture` + vite + pełne
`tsc -b`) i `npm run test` — **422 testy, 0 niepowodzeń**; `prettier` na nowych
plikach; strona sprawdzona klawiaturą, przy 375 px (bez poziomego przewijania —
to wymusiło skracanie 64-znakowych subjectów) i w obu motywach. Kontrakt HTTP
**nietknięty**, `just openapi` nie wołane, `src/api/generated` nietknięte, żaden
crate Rusta nietknięty.

Czego **nie** uruchomiono i co trzeba powiedzieć wprost: ręcznego przejścia kroku
1b **w panelu jako dwie osoby** — jedna przeglądarka to jedna sesja authentika, a
drugiej nie zakładano; zaproszeń, nowego shella, Learning UI i IAM-02; żadnego
E2E, klastra, workerów, S3/RustFS. `npm run lint` w tym repozytorium **nie
działa i nie działało wcześniej** — `apps/panel` nie ma konfiguracji eslint ani
samego eslinta w zależnościach, a CI go nie woła (woła `build` i `test`). To
osobna usterka, nie skutek tej zmiany.

## Kontynuacja — roster i zaproszenia (krok 2)

Data: 18.09.2026. **Krok 2 wykonany: kontrakt HTTP rozszerzony o sześć tras,
zaproszenia działają od wystawienia do zrealizowania, na żywym serwerze i w
panelu. Nowy shell (krok 3), Learning (krok 4) i IAM-02 (krok 5) nietknięte.**
Ta sama gałąź, dwa kolejne commity.

**Najpierw odczyt rosteru, bo bez niego zaproszenia nie mają gdzie się pokazać.**
Krok 1 skończył się znaleziskiem: API odpowiadało wyłącznie o wołającym, więc
panel pokazywał historię zamiast stanu. Doszły dwie trasy i jedna zasada, która
je rozdziela: `roster` i `grants` mówią o **organizacji**, `projects` i `access`
o **wołającym**, i nigdy się nie mieszają — rola w rosterze to co ktoś dostał, a
co komukolwiek wolno teraz, mówi wyłącznie świeża decyzja o dostępie. Lista
grantów nie filtruje po zegarze: wygasły wiersz jest zwykle tym, po co ktoś ją
otworzył. `roster` widzi też projekty, do których **sam nie ma grantu** — bo
administrator organizacji może nadać dostęp do projektu, którego nie potrafi
otworzyć, i bez tego nie umiałby go nawet nazwać.

**Zaproszenie jest ofertą złożoną sekretowi.** Grant nazywa parę
`(provider, subject)`, a subjectu obcego człowieka nikt nie zna, dopóki jego
provider go nie wystawi — to jest cała przyczyna, dla której ten krok istnieje.
Reguły i powód każdej:

- **Token istnieje raz.** Mintuje go magazyn (244 bity z dwóch UUID v4), trzyma
  wyłącznie `sha256`; jawny tekst jest w odpowiedzi, która ofertę stworzyła, i
  nigdzie indziej. Wykradziony dokument daje skróty.
- **`label` to wskazówka do dostarczenia, nigdy klucz tożsamości.** Nie jest z
  nikim porównywany; realizuje ten, kto ma token. Sprawdzanie etykiety byłoby
  uwierzytelnianiem niezweryfikowanym stringiem.
- **Jednorazowo, ktokolwiek pyta.** Druga realizacja to 409 — i dla tego, kto
  użył, i dla każdego innego.
- **Dwa zegary, świadomie.** `expires_at` mówi, ile ktoś ma na przyjęcie; okno
  grantu jest tym, co dostaje, gdy przyjmie. Oferta, która przepadła, to 410 dla
  jej posiadacza — jedynej osoby, która może o nią zapytać.
- **Realizacja nie nazywa organizacji.** Kto ma token, nie wie, do której należy,
  a trasa, która kazałaby mu to powiedzieć, potwierdzałaby zgadywanie. Skrót
  znajduje wiersz przez indeks GIN i ta sama instrukcja go blokuje, więc dwoje
  ludzi ścigających się o jedną ofertę szereguje się, a drugie słyszy, że jest
  zużyta.
- **Wycofanie zrealizowanej oferty jest odmawiane, nie cichym cofnięciem.** To,
  co powstało, jest grantem i odbiera się je `revoke_grant`.
- **Oferty są sprzątane 30 dni po tym, jak przestały cokolwiek znaczyć.** Trwały
  ślad jest we wpisie audytu, którego nie ma w agregacie i który nie jest
  sprzątany.

Zaproszenia siedzą w agregacie organizacji, nie w osobnej tabeli, więc realizacja
to jedna blokada wiersza na ofertę i grant, który z niej powstaje. Pole jest
addytywne i czytane jako nieobecne w dokumentach sprzed niego; starsze binarium
spotkawszy nowszy dokument **odmawia** na `deny_unknown_fields`, zamiast po cichu
zgubić oferty, których nie rozumie. Migracja 0003 dokłada wyłącznie indeks,
którego potrzebuje wyszukanie po skrócie — i została zastosowana na żywej bazie,
która miała już 1 i 2.

W panelu: karta **Invite somebody to…** (rola, notatka, okno i osobna data
wygaśnięcia oferty), token pokazany raz z wprost napisanym „nic tego nie pokaże
ponownie", lista otwartych i zużytych ofert z wycofaniem, oraz **Redeem an
invitation** — pole do wklejenia, nie link. Token w URL-u to sekret w historii
przeglądarki, w nagłówku referer i w czacie, do którego go wklejono; jedno
kliknięcie zamiast jednego wklejenia tego nie kupuje. Doszła też karta **People
and teams** (prawdziwy roster, z „Grant to…", które wypełnia formularz, żeby
64-znakowy subject nigdy nie był przepisywany ręcznie) i **Who may reach this
project** z odbieraniem grantu.

Walidacja: `scripts/iam-permission-check.py` urósł do **28 pytań i wszystkie
trzymają** — dwanaście nowych o zaproszenia i o to, czego roster nie pokazuje.
Jedno z nich najpierw nie przeszło i **to test był w błędzie**, nie serwer:
student był w tym scenariuszu właścicielem organizacji, więc 201 było właściwą
odpowiedzią; scenariusz rozpisano na dwa — właściciel może wszędzie, a edytor na
własnym projekcie nie może. `cargo test --workspace --all-targets` — **1802
testy, 0 niepowodzeń, 7 pominiętych**; clippy `--all-features -Dwarnings`, `cargo
fmt --all --check`, `git diff --check`, `scripts/check-rust-boundaries.py`,
`just openapi-check` — czysto. Kontrakt magazynu IAM na osobnej, jednorazowej
bazie PostgreSQL: **19 testów**, w tym dwie nowe własności sprawdzane na obu
adapterach i test rolling upgrade'u. Panel: `npm run build` i `npm run test` —
**422 testy**. Ręcznie w przeglądarce: wystawienie zaproszenia, token raz,
wklejenie go w drugim miejscu i wyjście z tego z rolą i oknem.

Czego **nie** uruchomiono: dostarczania zaproszeń (nie ma wysyłki poczty i nie
miało być — `label` jest wskazówką, nie adresatem); kroku 1b jako dwóch osób w
dwóch przeglądarkach; kroków 3–5; żadnego E2E, klastra ani workerów.

### Prompt dla sesji po kroku 2

> Pracujesz w repozytorium AIWatcher. Kroki 0–2 z „Kolejności do pierwszego testu
> permissionów i lekcji" są **zrobione i zweryfikowane na żywym serwerze**:
> lokalne SSO, panel IAM pod `/account/access`, matryca uprawnień (28/28),
> roster i zaproszenia. Stoją na gałęzi `iam-01/local-sso-and-panel`, sześć
> commitów, niepushowane. Przeczytaj `docs/ux-migration-plan-2026-09-14.md` (dwie
> ostatnie kontynuacje i „Kolejność do pierwszego testu permissionów"),
> `docs/local-sso.md`, `crates/aiwatcher-iam/README.md` i `apps/panel/CLAUDE.md`.
> **Zweryfikuj stan w kodzie — dokumentacja opisuje 18.09.2026 i mogła się
> zdezaktualizować.**
>
> **Środowisko**, wszystko lokalnie i jednorazowe: `just authentik-up`,
> `just authentik-seed`, `just authentik-secret` (do `.env`), `just postgres-up`,
> `just run-sso-iam`, `just panel`. Ludzie to `teacher` / `teacher-dev` (instancyjny
> admin) i `student` / `student-dev` (viewer). Dwie sesje naraz bez dwóch
> przeglądarek daje `python3 scripts/sso-session.py <kto>`, a całą matrycę
> `python3 scripts/iam-permission-check.py`. PostgreSQL **wyłącznie** na osobnej,
> jednorazowej bazie.
>
> **Krok 3 — sprawdź, nie buduj od nowa.** Nowy shell w większości istnieje:
> UX-09 dowiózł dwa układy, flagę `VITE_AIWATCHER_SHELL`, preferowany start,
> przypięcia i liczniki przejść, z regresjami w `navigation-preferences.test.tsx`.
> Twoje zadanie to przejść „Kryteria odbioru" i **zapisać, czego naprawdę
> brakuje** względem „Docelowej architektury informacji" — w `navigation.ts` nie
> ma obszarów **Administracja** ani **System**, a selektor organizacji/projektu w
> górnym pasku ma pozostać **nieaktywny**: to, co zbudowano w kroku 1, jest
> narzędziem testowym pod `/account`, nie ogłoszeniem multi-tenancy. Nie
> aktywuj go.
>
> **Krok 4 — Learning UI, i to jest właściwa praca.** Jedno zdanie zmienia jej
> zakres: **warsztat to projekt, uczestnik to grant, zapis to zrealizowane
> zaproszenie.** Cała połowa dostępowa już istnieje i działa — nie buduj dla niej
> backendu i nie wymyślaj pojęcia „warsztatu" w Ruście. Zbuduj nad tym, co jest:
> lista warsztatów z projektów organizacji (`roster` widzi też te, do których
> prowadzący nie ma grantu), strona warsztatu z uczestnikami (`grants` na
> projekcie) i ich stanem dostępu wprost z okna — jeszcze nie zaczął się, edycja
> do, tylko odczyt, skończył się — zapisy przez zaproszenia (token pokazany raz,
> uczestnik wkleja u siebie), oraz dziewięć slotów laboratoriów.
>
> **Czego o warsztacie nie wolno wymyślić.** Treść instrukcji, testy, ewaluacje,
> metryki i postęp nie mają żadnego kontraktu — oznacz je jako **niedostępne** i
> nie podstawiaj danych zastępczych; `AreaPlaceholder` istnieje właśnie po to.
> Silnik treści i ocen to osobna praca. Popraw za to zdanie z obecnego
> placeholdera, które przestało być prawdziwe: „timed access are not available"
> — dostęp czasowy jest dostępny i właśnie nim jest okno grantu.
>
> **Czego nie wolno.** Nie zaczynaj IAM-02 (`docs/iam-02-data-plane.md`) — jest
> ostatnie świadomie. Nie otwieraj projektowego `/start`, nie rejestruj
> dispatchera w produkcyjnym `spawn`, nie zakresuj logu zdarzeń ani strumieni.
> Nie opisuj wdrożenia jako multi-tenant safe. Nie zmieniaj istniejących migracji
> SQL; nowe są addytywne. Obowiązuje reguła: **nowy zasób autorski dostaje swoją
> zakresową rodzinę tras od urodzenia** (`ProjectAuthorization` plus
> `<prefix>/scopes/<organization>/<project>/registry/`).
>
> **Testy na koniec każdego kroku.** Panel: `npm run test`, `npm run build` (to
> `check:architecture` + `vite build` + pełne `tsc -b`), przejście klawiaturą,
> 375 px i szeroki ekran, motyw jasny i ciemny, żadnego poziomego przewijania
> strony. `npm run lint` **nie działa w tym repozytorium i nie działało wcześniej**
> — `apps/panel` nie ma konfiguracji eslint ani samego eslinta; nie goń tego, to
> osobna usterka. Rust, jeśli tkniesz którykolwiek crate: `cargo test --workspace
> --all-targets`, `cargo clippy --workspace --all-targets --all-features
> -Dwarnings`, `cargo fmt --all --check`, `git diff --check`,
> `python3 scripts/check-rust-boundaries.py`. Po zmianie trasy lub typu w
> kontrakcie: `just openapi` i commit obu stron.
>
> Pracuj na gałęzi z `main` (albo kontynuuj `iam-01/local-sso-and-panel`),
> commituj po ścieżkach, jeden krótki konwencjonalny nagłówek, bez trailera
> współautora. Po każdym kroku zdaj raport i **jawnie wypisz, czego nie
> uruchomiłeś**.

## Kontynuacja — shell sprawdzony i Learning nad grantami (kroki 3 i 4)

Data: 18.09.2026. **Krok 3 przeszedł jako przegląd, nie jako przebudowa; krok 4
jest zrobiony i obejrzany na żywym serwerze dwiema prawdziwymi sesjami OIDC.
IAM-02 (krok 5) nietknięte.** Ta sama gałąź `iam-01/local-sso-and-panel`.

### Krok 3 — co shell naprawdę ma, i czego naprawdę nie ma

Nic tu nie budowano od nowa, bo UX-09 dowiozło całość: dwa układy,
`VITE_AIWATCHER_SHELL`, preferowany start, przypięcia, liczniki przejść i
dwanaście regresji w `navigation-preferences.test.tsx`. Przegląd „Kryteriów
odbioru" wobec „Docelowej architektury informacji" zostawia cztery braki i
**jedną znalezioną usterkę**:

- **Obszar System nie istnieje i nie ma z czego go zbudować.** Runtimes,
  integracje i konfiguracja instancji nie mają ani jednej trasy w
  `contracts/openapi.json` (226 ścieżek). To, co pokazałby, jest już mówione tam,
  gdzie ma znaczenie — 501 z nazwą zmiennej w promptach, datasetach i hubach —
  więc nie jest to ekran do dorysowania, tylko kontrakt do napisania. Placeholder
  z wymyśloną listą runtime'ów byłby dokładnie tym, czego zakazuje reguła
  `AreaPlaceholder`.
- **Obszar Administracja istnieje, ale świadomie nie jest obszarem.** Siedzi pod
  `/account/access`, poza `navigation.ts`, bo wejście do sidebara robiłoby z
  zarządzania grantami miejsce pracy zamiast miejsca, do którego idzie się
  celowo. To decyzja z kroku 1, nie luka — ale **audyt IAM ma tylko odczyt
  mutacji**; nie ma eksportu ani retencji, i to jest luka.
- **„Ostatnie" na stronie startowej nie ma.** Są przypięcia i preferowany start,
  nie ma historii ostatnio otwartych obiektów. Wymagałoby albo lokalnego zapisu
  (kolejny schemat w `local-views`), albo kontraktu po stronie serwera; żadnego
  z nich nie ma, i nie zgadywano który.
- **Agenci i harmonogramy nie mają własnych miejsc.** „Aplikacje: agenci" to
  wciąż wymiar w Explore, nie strona obiektu (FLOW-01), a harmonogram siedzi w
  `data-curation`, nie w obszarze Workflow. Oba są zgodne z dzisiejszym API i oba
  są przeprowadzką, nie brakiem danych.
- **Usterka, znaleziona klawiaturą przy 375 px i naprawiona.** Rzędy nawigacji
  mobilnej to `overflow-x-auto`, a Chrome **nie** przewija do widoku linku, na
  którym stanął fokus: na `/datasets` fokus lądował na „Conversations" przy
  x=373..505 przy `scrollLeft` 0 — pierścień fokusu całkowicie poza ekranem, a
  jedyną wskazówką był dwupikselowy skrawek. To łamało wprost jedno z kryteriów
  odbioru („żadna globalna kontrolka nie znika poza viewportem"). Naprawa to
  `onFocus` w czterech rzędach `shell.tsx` ze `scrollIntoView({ block: 'nearest',
  inline: 'nearest' })`; regresja w `navigation-preferences.test.tsx` pilnuje
  samego podpięcia, bo jsdom nie przewija. Usterka była **wcześniejsza** —
  powtarza się na `/datasets` tak samo jak na `/learning`.

Selektor organizacji/projektu w górnym pasku **pozostaje nieaktywny** i nie
został dodany.

### Krok 4 — Learning, bez pojęcia „warsztatu" w backendzie

Jedno zdanie jest całym projektem tego obszaru: **warsztat to projekt, uczestnik
to grant, a zapis to zrealizowane zaproszenie.** Nie dopisano ani jednej trasy,
nie zmieniono kontraktu HTTP, `just openapi` nie było wołane, `src/api/generated`
nietknięte, żaden crate Rusta nietknięty.

- **Lista warsztatów.** Dwa odczyty, które odpowiadają na różne pytania:
  `projects` to granty wołającego (z werdyktem serwera na każdym), roster to
  wszystkie projekty organizacji i dostaje go tylko jej administrator. Pierwsza
  sekcja nazywa się **„Open to you now"**, bo `Policy::projects` zostawia wyłącznie
  projekty z grantem **w mocy teraz**. Druga to **„Also in this organization"** —
  i tak właśnie brzmi, bo trafiają tam trzy różne rzeczy naraz: warsztat, który
  się administruje bez grantu, taki, który jeszcze się nie zaczął, i taki, który
  się skończył. Kto nie czyta rosteru, dostaje zdanie mówiące, czego ta lista nie
  pokazuje — bo nie ma trasy odpowiadającej „na co jestem zapisany", jest tylko
  „do czego mam teraz dostęp".
- **Strona warsztatu.** Twoje miejsce (werdykt serwera, każde źródło osobno, z
  `evaluated_at` i zdaniem, że to migawka), uczestnicy z `grants`, zapis przez
  zaproszenie i dziewięć slotów laboratoriów.
- **Faza czytana z okna, nigdy sumowana.** `phaseOf` czyta daty **jednego**
  grantu: `not open yet`, `open`, `read-only`, `closed` — dokładnie cztery
  przypadki `GrantWindow::role_at`, z przedziałami półotwartymi. Viewer po
  `edit_until` zostaje `open`, bo dla niego nic się nie zmieniło, a ogłaszanie mu
  przejścia, którego nie było, byłoby gorsze niż milczenie. Efektywna rola to
  maksimum po żywych grantach, serwer bierze ją per żądanie i odpowiada wyłącznie
  o wołającym — więc panel nie sumuje wierszy, nie ukrywa wygasłych i nie
  warunkuje żadnej kontrolki tym, co wyliczył. Granty jednej osoby są grupowane w
  jeden wiersz, bo „stały grant plus warsztatowy" to przypadek, wokół którego cały
  ten projekt się kręci.
- **Laboratoria zostają puste.** Treść instrukcji, testy, ewaluacja i wynik nie
  mają żadnego kontraktu w tej instancji — każdy z dziewięciu slotów mówi
  `no contract` i nic nie udaje. Poprawione zostało zdanie placeholdera, które
  przestało być prawdziwe: dostęp czasowy **jest** dostępny i jest nim okno
  grantu.
- **Refaktor, którego wymusiła granica architektury.** `features/account/iam.ts`
  → `shared/lib/iam.ts`, a karta zaproszeń i karta realizacji tokenu →
  `shared/components/`, sparametryzowane wyłącznie na tytule i zdaniu wstępu.
  Drugi komplet hooków pod Learning byłby drugim kompletem kluczy cache dla tych
  samych odpowiedzi: zaproszenie wystawione na jednej stronie zostawiałoby nieświeżą
  listę na drugiej.
- **Jedna poprawka zachowania przy okazji.** `GET …/access` odpowiada 404, gdy
  wołający nie ma żywego grantu — to **odpowiedź**, nie awaria, i zwykły stan
  administratora patrzącego na warsztat, którego nie prowadzi. Było rysowane na
  czerwono przez `answerOf`; jest `answerOrNone` i zdanie po ludzku, w Learning i
  na `/account/access`.

Walidacja:

- Panel: **56 plików, 441 testów**, w tym **19 nowych regresji** — osiem
  scenariuszy czytania okna i grupowania uczestników, dziewięć scenariuszy strony
  (rozdział dwóch list, dwa granty jednej osoby, editor kontra viewer po
  `edit_until`, 404 jako odpowiedź, brak kontrolki dla kogoś bez prawa, token
  pokazany raz i nigdy nie odczytany ponownie, dziewięć pustych slotów, warsztat
  w URL, brak dostawcy tożsamości), zdanie dla uczestnika bez rosteru i regresja
  fokusu w nawigacji mobilnej. Ta ostatnia **sprawdzona, że łapie** — po usunięciu
  `onFocus` z rzędu obszarów test pada.
- `npm run build`: granice architektury, Vite i pełne `tsc` — powodzenie.
  Pozostaje wcześniejsze ostrzeżenie o głównym chunku ponad 500 kB.
- `git diff --check`: czysto. Backend, kontrakt i migracje SQL nietknięte.
- Żywy serwer (`just authentik-up`, `just postgres-up`, `just run-sso-iam`,
  `just panel`) z trzema warsztatami w trzech fazach i dwiema sesjami OIDC:
  headless Chromium przy **1440 px i 375 px**, w motywie jasnym i ciemnym, jako
  prowadzący i jako uczestnik — **bez poziomego przewijania strony**, bez błędów
  JavaScript, a po naprawie **każdy z 79 przystanków Tab przy 375 px mieści się w
  viewporcie**. Obejrzano zrzuty wszystkich czterech faz okna.

Czego **nie** uruchomiono: `npm run lint` (nie działa w tym repozytorium i nie
działało wcześniej — `apps/panel` nie ma eslinta ani jego konfiguracji; osobna
usterka); żadnych testów Rusta, bo żaden crate nie był tknięty; kroku 5 (IAM-02);
E2E, klastra, workerów, S3/RustFS; dostarczania zaproszeń pocztą. W przeglądarce
jedynym „błędem" w konsoli są odpowiedzi serwera, które są odpowiedziami: 404 z
`access` dla kogoś bez żywego grantu i 403 z `roster`/`grants` dla kogoś, kto nie
administruje organizacją — obie narysowane zdaniem, nie czerwienią.

**Cała migracja pozostaje w toku.** To, co zostało, jest podzielone na
strumienie, które mogą iść równolegle, wraz z mapą kolizji i promptem na każdy:
[docs/parallel-streams-2026-09-18.md](parallel-streams-2026-09-18.md). Cztery
startują od razu — System, FLOW-01, treść warsztatu i IAM-02/A (projekt na
kopercie); reszta IAM-02 jest łańcuchem za nimi, z bramką M1 pośrodku.

## Kontynuacja — treść warsztatu, czyli LEARN-02

Data: 18.09.2026. **Pierwsze zadanie nie było kodem i zmieniło rozmiar całej
reszty.** Cztery rzeczy, których brakowało w dziewięciu pustych slotach —
brief, testy, oddanie pracy i wynik — zostały przyłożone do tego, co ta
instancja już ma, zanim powstał choćby jeden plik. Trzy z czterech już były,
pod nazwami, które wzięły się z mierzenia modelu, a nie z uczenia kogokolwiek.

### Co z czego wynikło

- **Testy to karta i kohorta.** `Scorecard` jest dokładnie tym, czego szuka
  lab: nazwany, wersjonowany treścią zestaw 1–32 pomiarów, każdy z nazwą
  metryki, wskaźnikami JSON w odpowiedź, oczekiwanie i pytanie, oraz scorerem z
  domkniętego enuma. Kierunek każdej metryki **wyprowadza karta**, autor go
  nigdy nie wysyła. Przypadki to `Cohort` z `POST /api/v1/evaluation-cohorts`,
  przypięta digestem trzech plików. Obie rodziny tras są już projektowe.
- **Oddanie pracy to nagranie albo wygenerowane odpowiedzi.** `PUT
  /api/v1/evaluation-recordings/{name}` przypina odpowiedzi digestem bajtów;
  `Answers::Generated` nazywa zadanie workera, które odpowiada na każdy
  przypadek pod promptem i modelem wariantu, trzymane do nich przez trace'y
  przebiegów. To, co uczestnik *zbudował*, to `VariantManifest`. Projektowe.
- **Wynik to opublikowany `EvaluationResult`** — a widok całej grupy nie
  potrzebuje niczego nowego, i to jest znalezisko, które zdecydowało o
  kształcie. `context_id` jest adresem treści kohorty, splitu, suity, scorera i
  definicji metryk **razem**, więc każdy wynik zmierzony na przypadkach jednego
  laboratorium jego kartą ma ten sam. `GET
  /api/v1/evaluation-results?context_id=…` to już „wyniki wszystkich".
- **Brief nie istnieje** — żadna z 226 tras nie trzyma autorskiego tekstu
  instrukcji. I, mniej oczywiste, nie istnieje **wiązanie**: nigdzie nie jest
  napisane, że *laboratorium 3 tego warsztatu to ten brief, mierzony tą kartą
  na tych przypadkach*. `EvaluationContext` jest tym wiązaniem bez briefu, ale
  jest **wyprowadzany przy publikacji z deklaracji**, a deklaracja potrzebuje
  wariantu — czyli pracy uczestnika, której nie ma, gdy prowadzący pisze lab.

### Co powstało

`aiwatcher-labs` — szósty rejestr autorski, kształt ADR_0011 co do joty
([ADR_0034](ADR/ADR_0034_WORKSHOP_LABS.md)), plus moduł `aiwatcher-api/src/labs.rs`.
Lab to `{name, title, brief, position?, tests?}`, wersja to digest całego
dokumentu, `published` to etykieta, którą czyta uczestnik, a publikacja bez
etykiety to szkic. `LabTests` przypina kartę **na wersji** i kohortę digestem.
Pięć tras, każda z bliźniaczą rodziną projektową od urodzenia: lista,
publikacja, lab, wersja, etykieta — i szósta, `GET
/labs/{name}/measurement`, która odpowiada `context_id`, bo digest po
kanonizowanym dokumencie to dokładnie to, co druga implementacja w TypeScripcie
zrobiłaby subtelnie inaczej (precedens: `POST /evaluation-approvals/address`).

Dwie decyzje warte zapisania. **Przypięcia są sprawdzane tam, gdzie lab jest
pisany**: publikacja rozwiązuje kartę i kohortę w rejestrze *tego* projektu i
odmawia 422 `lab_unpinned`, zamiast zapisać lab, który czyta się dobrze i nic
nie mierzy. I **karta z sędzią jest odmawiana po nazwie metryki**
(`lab_unmeasurable`): sędzia i zestaw kalibracyjny są deklarowane per przebieg,
więc dwie prace nie muszą być ocenione przez tego samego, a ich wyniki nie
dzielą kontekstu — powiedzenie tego jest lepsze niż odpowiedzenie
identyfikatorem, który znaczy mniej, niż wygląda.

Jedna zmiana poza własną granicą, wypisana wprost: `EvaluationContext::id` w
`aiwatcher-evaluation` — adres treści kontekstu, który `Evaluation::prepare`
liczył inline i teraz woła, więc implementacja została jedna. To właściwe
miejsce: tożsamość kontekstu należy do kontekstu.

### Panel

`features/learning/screens/overview/labs.tsx` przestało być dziewięcioma
pustymi slotami i jest czytelnikiem czterech zakresowych tras
(`lib/labs.ts`). Lab otwiera się w miejscu: brief tak, jak go napisano (bez
renderera Markdown — panel go nie ma, a wybranie go byłoby decyzją podjętą przy
okazji rysowania lekcji), karta i kohorta, metryki z kierunkiem **serwera**, i
wyniki dzielące kontekst laboratorium. Czego nie ma: postępu, punktów,
rankingu, terminu. Lab bez przypiętych testów rysuje zdanie serwera
(`unavailable`), a warsztat bez laboratoriów mówi, że ich nie ma — dziewięć
atrap zniknęło, bo rysowanie prawdopodobnego ćwiczenia to ta sama atrapa, przed
którą chroniły, tylko z drugiej strony.

Które laboratorium jest otwarte, jest **jedynym** wyborem w tym panelu, którego
nie ma w URL-u, i to jest świadome: lab jest sekcją strony warsztatu, a nie
osobnym widokiem. Przeniesie się do URL-a tego dnia, w którym dostanie własną
trasę — formularz oddania pracy albo własną ocenę.

### Czego nie zbudowano, i dlaczego to jest widoczne

- **`POST /evaluation-runs/{id}/start` nie ma bliźniaka projektowego** — z
  reguły samego ADR_0033, że żaden projektowy `/start` nie jest otwierany przed
  IAM-02/D. Lab można więc napisać, przeczytać i pobrać jego przypadki w
  projekcie, a przebieg, który ocenia oddaną pracę, startuje pod autoryzacją
  instancji. Panel mówi to zdaniem, zamiast rysować przycisk, który dostałby
  odmowę.
- **`/api/v1/experiments` jest tylko legacy**, więc widok grupy stoi na
  zakresowym `evaluation-results?context_id=`, a nie na wierszach eksperymentu.
  Te same liczby, jeden odczyt naraz.
- Żadnego drugiego pojęcia dostępu: kto widzi laboratorium, decyduje grant na
  projekcie warsztatu, i nic innego.

### Walidacja

- `aiwatcher-labs`: **8 testów** (idempotencja publikacji, etykieta kontra
  szkic, etykieta wskazująca na nieistniejącą wersję, kolejność listy z
  nieumieszczonymi na końcu, kontekst jako klucz zgłoszeń, odmowa karty z
  sędzią po nazwie metryki, izolacja projektów przy tym samym digeście,
  odmowy przed zapisem).
- `aiwatcher-api`: **4 testy HTTP** na rodzinie zakresowej — lab czytany
  wyłącznie przez swój projekt, przypięcia cudzego projektu odmówione tam,
  gdzie lab jest pisany (z pełnym łańcuchem dataset → kohorta → karta → lab →
  `context_id`), lab bez testów jako odpowiedź a nie awaria, nagłówek mutacji,
  brak grantu jako 404 i `Cache-Control: no-store`.
- Panel: **20 testów** w obszarze (12 na stronie), w tym trzy nowe —
  brak laboratoriów rysuje pustkę zamiast slotów, testy i oceny czytane z
  serwera z **asercją, że `context_id` w zapytaniu to ten, który serwer
  odpowiedział** (sprawdzona przez psucie: podmiana na wyliczony w
  przeglądarce wywala test), i zdanie serwera dla laboratorium bez pomiaru.
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets
  --all-features -Dwarnings`, `python3 scripts/check-rust-boundaries.py`,
  `npm run build` (granice architektury, Vite, pełne `tsc`) — zielone.
- Drzewo *dokładnie* z tymi zmianami (`git checkout-index` z indeksu, bez
  plików innych sesji) zbudowane osobno: `cargo test --workspace --all-targets
  --no-run` — powodzenie, a wygenerowany z niego kontrakt ma 236 ścieżek, czyli
  226 plus dziesięć laboratoryjnych.

Czego **nie** uruchomiono: pełnego `cargo test --workspace` (dwie inne sesje
pracują w tym samym katalogu roboczym i ich niezacommitowane zmiany są w
drzewie); żywego serwera z SSO i przeglądarki; klastra, workerów, S3/RustFS;
`npm run lint` (nadal nie istnieje — PORZ-01). `npm run test` w panelu ma jedną
porażkę, **nie z tej pracy**: `commands.test.ts` wymaga schematu dla `/system`,
którego SYS-01 jeszcze nie dopisało.

---

## SYS-01 — obszar System: co ta instancja ma skonfigurowane

Data: 18.09.2026. Strumień SYS-01 z
[podziału na strumienie](parallel-streams-2026-09-18.md) zamknięty. Plan mówił
„System nie ma kontraktu i nie ma z czego go zbudować"; pierwsza połowa była
prawdą, druga nie. Fakty siedziały w `crates/aiwatcher-api/src/state.rs` prawie
w komplecie — brakowało **jednej** rzeczy i jednej trasy nad nimi.

### Co powstało

- **`GET /api/v1/system`**, moduł `crates/aiwatcher-api/src/system.rs` — facada
  jak każda inna w tym crate. **28 zdolności**, każda z: stanem
  (`configured` / `not_configured`), zmiennymi, które o niej decydują,
  wartościami, które nie są sekretem, i jednym zdaniem o tym, co ta instancja
  robi bez niej. Pogrupowane w pięć: `identity`, `observability`, `storage`,
  `runtime`, `integration`.
- **Rola: `admin`**, i to jest decyzja, nie domyślność. To inwentarz wdrożenia —
  które magazyny są podpięte, do kogo ta instancja wychodzi na zewnątrz, komu
  ufa jako dostawcy tożsamości — więc przydaje się operatorowi i przydaje się
  atakującemu, a czytającemu przebieg nie przydaje się wcale. Siedzi tam, gdzie
  rerun i czytanie treści rozmowy.
- **Obszar `system` w panelu** (`apps/panel/src/features/system/`): szósta
  sekcja w `navigation.ts`, jedna strona, `Cache-Control: no-store` po stronie
  trasy. 403 rysowane zdaniem, nie awarią — odmowa jest tu odpowiedzią.

### Jedyna rzecz, której w `AppState` nie było

`AIWATCHER_POD_RUNTIME`. Reguła tego strumienia brzmiała „zdolność, o której
`AppState` nic nie mówi, nie pojawia się na liście", a runtime podów był w
`aiwatcher-server::config` i nigdzie wyżej. Rozwiązaniem nie było zgadywanie ani
drugi enum: `PodRuntime` przeniósł się do `aiwatcher_execution::pods`, obok
`PodTemplates` — czyli tam, gdzie reszta tego, czym jest pod, już była — a
konfiguracja re-eksportuje go i nadal sama parsuje zmienną i nazywa ją w
odmowie. `AppState` dostał jedno pole.

### Czego ta trasa nie mówi, i dlaczego to była cała trudność

**Fakt „skonfigurowane" nie jest sekretem, wartość bywa.** Nigdy nie wychodzi
żadne poświadczenie i nigdy żaden **adres**: URL bazy, endpoint object store'u,
host sędziego czy runtime'u notebooków to rekonesans dla kogoś, kto już jest w
środku. Raportowana jest **nazwa zmiennej**, bo to i tak jest to, czego czytelnik
potrzebuje. Jeden wyjątek, świadomy: **issuer**, bo „na którego authentika to
wskazuje" jest inaczej nieodpowiadalne bez powłoki na podzie, a `/auth/config`
podaje go publicznie, zanim ktokolwiek się zaloguje.

Regresje, obie sprawdzone, że **łapią** (wstrzyknięty wyciek: cały szablon poda
zamiast jego nazwy — obie padły, nazywając zmienną):

- `crates/aiwatcher-server/tests/system.rs` — konfiguruje wdrożenie z
  **rozpoznawalną wartością w każdej wrażliwej zmiennej** (27 igieł: sekrety
  OIDC i sesji, tokeny ingestu, klucz poświadczeń poda, klucze S3, klucze
  archiwum rozmów, Kaggle, Hugging Face, sędzia, scorery, notebooki, query,
  runner, connection stringi PostgreSQL i brokera, adres API poda), przepuszcza
  je przez `aiwatcher_server::build` — to samo okablowanie, którego używa
  binarka — i pada, jeśli którakolwiek wróci. Do tego **kontrola pozytywna**:
  odpowiedź musi nadal nazywać te zmienne i nieść issuera, żeby trasa, która
  przestała odpowiadać, nie przeszła milczeniem.
- `crates/aiwatcher-api/tests/http.rs` — statyczna połowa tej samej reguły
  (żadne ustawienie nie drukuje wartości obok zmiennej z listy zakazanej),
  rola, stany wobec konfiguracji i nazwa szablonu poda bez jego treści.

### Czego nie zbudowano, świadomie

Żadnego zapisu — zmiana ustawienia to zmienna środowiskowa i restart, i tak ma
zostać. Żadnego zdrowia usług zewnętrznych: „czy skonfigurowany serwis
odpowiada teraz" to inne pytanie, z inną częstotliwością i innym trybem awarii,
a inwentarz świecący na czerwono, bo ktoś inny się restartuje, czytałby się jak
awaria tej instancji. I żadnego zgadywania: `AIWATCHER_QUERY_URL` decyduje o
tym, czy silnik zapytań w ogóle odpowiada, ale trzyma go rola `work`, więc
trasa mówi tylko, **dla którego silnika** plan musi być napisany.

Weryfikacja: `cargo test -p aiwatcher-api -p aiwatcher-server -p aiwatcher-execution`,
clippy i `cargo fmt` na dotkniętych crate'ach; panel `npm run test` (467) i
`npm run build`; żywy serwer na :18080 z podpiętym archiwum rozmów, szablonami
podów, tablicą cen, sędzią, scorerami i hubami — 28 wierszy, `cache-control:
no-store`, zero adresów i zero poświadczeń w ciele; headless Chromium przy
**1440 px i 375 px**, w motywie jasnym i ciemnym, **bez poziomego przewijania
strony** i z każdym z 87–88 przystanków Tab w viewporcie.

Czego **nie** uruchomiono: `npm run lint` (PORZ-01); PostgreSQL, IAM i brokera —
ich connection stringi są w teście jako igły, ale nie są okablowane, bo
wymagałyby bazy i cargo feature'a; klastra, workerów i S3/RustFS. W drzewie
roboczym pracują równolegle inne strumienie i ich niezacommitowane zmiany są
obecne; jedna porażka w `cargo test -p aiwatcher-api --lib`
(`every_module_facade_reaches_the_document`) jest **nie z tej pracy** — LEARN-02
dopisało `labs` do `document()`, a jeszcze nie do listy modułów w tym teście.

## IAM-02/A dowieziony — projekt na kopercie i fold, który go zna (18.09.2026)

E1 i E2 z [planu data plane](iam-02-data-plane.md) są zrobione: koperta niesie
opcjonalny `ProjectScope`, trasa ingestu nadpisuje go zakresem poświadczenia, a
jeden fold — przebiegi, wymiary, spany, okresy, `asked`, `measured`, journal —
trzyma klucz zakresu w wierszu. Brak znaczy stronę globalną, więc nic z
dotychczasowych danych się nie ruszyło, a identyfikatory globalne nie drgnęły co
do bajtu. Pomiar pamięci: 176 MB bez projektu, 183 MB z pięćdziesięcioma; limit
512 MB zostaje. Szczegóły i to, czego etap **nie** robi, są w sekcji 8 tamtego
planu. Następny jest IAM-02/B — odczyty i żywy strumień po grantach, czyli
bramka M1.

---

## FLOW-01 dowieziony — jeden filtr, strony agentów, lineage i dwa przedziały (19.09.2026)

Cztery rzeczy z [mapy strumieni](parallel-streams-2026-09-18.md), w kolejności z
[inwentaryzacji](flow-01-inventory-2026-09-18.md): panel, serwer, kontrakt,
panel. Inwentaryzacja jest nadal ważna jako uzasadnienie — tu jest wyłącznie to,
co powstało, i to, co się w trakcie okazało nieprawdą.

**Krok A i B (18.09) — słownik i read model.** `shared/lib/object-filter.ts`
stał się jedynym miejscem, w którym oś (`agent`, `runtime`, `workflow`,
`session`, `variant`, `trace`, `model`, `tool`, `status`) zamienia się w
parametr trasy; `dimensions::compute`, `metrics::compute` i lista przebiegów
przeszły na jeden predykat `crate::selection`, dostały `as_of` i oś `prompt`
(`DimensionKind::Prompt`, `SpanRow::prompt_name`). Strony agentów powstały jako
osobny obszar, a `ModelVersionReference` dołączył do `lineage-reference.tsx`.

**Krok D (19.09) — to, co zostało, i jedna rzecz do poprawienia po drodze.**

- **Panel nadał wszystkie osie.** Tabela tłumaczeń miała trzy wiersze pełne i
  dwa prawie puste — trasa wymiarów „zawęża wyłącznie po agencie", trasa metryk
  „bierze agenta, sesję i model". Po kroku B obie biorą wszystkie dziesięć, więc
  ostrzeżenie „Not applied here" zniknęło stamtąd i **zostało tylko na liście
  spanów**, gdzie jest faktem o spanie, a nie o tym, czego jeszcze nie
  napisano: span nie nosi sesji, workflow, runtime'u ani wariantu swojego
  przebiegu, a jego `ok | error` nie jest statusem przebiegu.
- **Zdanie pod filtrem było nieprawdziwe i jest poprawione.** Krok A napisał
  regułę 3 tak, jakby każdy odczyt zawężał liczniki jak metryki. Zawęża je
  **wyłącznie `/metrics`**, bo tylko on liczy coś mniejszego niż przebieg.
  Wiersz `/runs` i wiersz wymiaru niosą własne sumy przebiegu — złożone przy
  ingeście i takie same niezależnie od filtra — więc przy `model=X` „LLM calls"
  w wierszu to **wszystkie** wywołania tego przebiegu. Strona mówi teraz to,
  co jest prawdą u niej, a lista spanów nie mówi nic, bo jej wiersze *są* tymi
  wywołaniami.
- **Prompty agenta.** Kartka „Prompts it runs on" mówiła, że pytanie jest bez
  odpowiedzi. Było to prawdą do kroku B i jest całym powodem, dla którego oś
  `prompt` powstała. Nazwa, nigdy wersja (rejestr jest kluczowany nazwą), i
  **link tylko wtedy, gdy rejestr potwierdzi, że taki prompt trzyma** —
  `PromptNameLink` obok `PromptRefLink`, bo nazwa na spanie to telemetria pod
  retencją, a tekst jest autorski i ją przeżywa, więc obie połowy mogą się
  rozjechać w obie strony. 501 z nieskonfigurowanego rejestru to brak linku, nie
  awaria.
- **Porównanie przedziałów.** `?compare=previous` na Metrics i na stronie
  agenta: ten sam filtr, dwa odczyty tej samej trasy. Cała decyzja jest w tym,
  **gdzie kończy się drugi przedział**: sekundę przed początkiem okna, które
  zwrócił *pierwszy odczyt*, a nie `teraz − okno` policzone w przeglądarce.
  Okno względne rozwiązuje zegar serwera, więc przeglądarka spiesząca się o
  kilka minut poprosiłaby o przedział zachodzący na sąsiedni — i nic w
  odpowiedzi by tego nie pokazało. Sekunda, bo oba końce okna są domknięte.
  Para jest **względna**, jak samo okno: link znaczy „przedział przed tym, na
  którym się otworzy". „All" nie ma przedziału przed sobą, więc przełącznik
  jest wyłączony i mówi dlaczego.
- **Zmiana nie jest kolorowana.** Jedyne miejsce w panelu, gdzie delta ma
  kolor, to przypięty kontekst ewaluacji, który deklaruje kierunek metryki.
  Nikt nie deklaruje kierunku dla liczby przebiegów ani dla rachunku, więc
  rysowany jest znak, a odczyt zostaje przy czytającym. Współczynnik rusza się
  w **punktach**, brak wartości w przedziale wcześniejszym czyta się jako
  „nothing reported before", a nie jako zero, i zero nie jest mianownikiem.

**Czego świadomie nie zrobiono.** Wykresy i rozbicia (`by_model`, `by_tool`,
`by_step`) zostają jednookresowe — porównanie stoi przy agregatach, a strona
mówi to jednym zdaniem zamiast zostawiać domysł. Drzewo Explore nie porównuje
przedziałów: to listy stronicowane kursorem, więc wiersze dwóch okien nie
odpowiadają sobie po kluczu bez drugiego zapytania na wiersz. `prompt` nie jest
atrybutem **budowniczego zapytań** ani osią **żywego strumienia**: katalog
silników (`services/query/contract/catalog.json`) nie projektuje
`prompt_name`, a zdarzenie nie niesie faktu spanowego (ADR_0003) — czyli
dokładnie ta sama przyczyna, dla której nie ma tam `variant`. Dołożenie tych
dwóch kolumn do katalogu i do trzech silników to osobna praca z własnymi
testami zgodności.

**Weryfikacja.** `npm run test` (487, w tym 20 nowych), `npm run build`
(`check:architecture` + Vite + pełne `tsc -b`), `npm run lint` — zielone.
Kontraktu **nie** regenerowano, bo nie zmieniono żadnej trasy: wszystkie
parametry, których ten krok używa, weszły w kroku B i są już w
`contracts/openapi.json` (`/api/v1/dimensions/{kind}` i `/api/v1/metrics` niosą
`as_of` i wszystkie dziesięć osi).

Czego **nie** uruchomiono: żywego serwera ani przeglądarki (krok jest panelowy,
a dane, na których porównanie ma sens, wymagają dwóch pełnych okien przebiegów);
`cargo test` — nie tknięto żadnego crate'a; klastra, workerów, S3/RustFS.
Równolegle w tym samym repozytorium idzie IAM-02/B, którego praca dotyka
`aiwatcher-projector` i `aiwatcher-api`; ten krok nie wchodzi do żadnego z nich.
