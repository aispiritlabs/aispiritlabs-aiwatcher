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
| IAM-01 | Organizacje, zespoły, projekty | W toku: model i magazyny IAM, OIDC issuer/sub, API z bootstrapem i atomowym audytem oraz pierwszy rejestr zasobów z zakresem organizacja/projekt (datasety i curation). Jest dry-run inwentaryzacji migracji. Pozostałe rejestry, query, strumienie, zadania i cutover nadal wymagają izolacji. Selektory UI są nieaktywne. |
| IAM-02 | Zaproszenia i dostęp warsztatowy | Niewdrożone; zależą od IAM-01. |
| FLOW-01 | Pełne przejścia i lineage | Pozostają dedykowane strony agentów, powiązania wersji prompt/model/dataset, porównania przedziałów i wspólne filtry. |
| LEARN-01 | Learning | Wdrożono `/learning`: stan niedostępnych warsztatów oraz 9 slotów laboratoriów bez fikcyjnej treści, wyników i aktywnych operacji. Listy/szczegóły rzeczywistych warsztatów i provisioning pozostają. |

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

## Warsztaty i Learning

Instruktor wybiera wspólny projekt lub indywidualną kopię przypiętej wersji szablonu. Kopiowanie nie przenosi sekretów, członkostw ani prywatnej historii wykonań. Grant zawiera `valid_from`, `edit_until` i opcjonalne `read_until`. Wygaśnięcie obowiązuje bez czekania na wygaśnięcie sesji SSO; aktywne strumienie i zadania respektują odebranie dostępu. Domyślnie zadania kończą pracę z końcem dostępu; instruktor może dopuścić ograniczony czas dokończenia.

UI Learning: lista i szczegół warsztatu, uczestnicy, stan provisioningu i dostępu, 9 slotów laboratoriów. Laboratorium zawiera miejsce na instrukcję, projekt, testy, ewaluacje i metryki. Brakujące kontrakty oznaczamy jako niedostępne, bez wymyślonych materiałów, wyników ani postępu.

## Kolejność wdrażania i migracja danych

1. **Poprawność obecnego panelu.** Domknąć UX-01–08, regresje, stany błędów i ochronę pozostałych szkiców. Nie zmieniać historii danych.
2. **Nowy shell.** Wdrożyć przełącznik rollout/rollback, preferencje, przypięcia i analitykę przejść. Zachować dotychczasowe trasy jako adaptery.
3. **IAM backend i UI.** Najpierw pełne egzekwowanie zakresu i testy izolacji, potem aktywować selektory organizacji/projektu i zarządzanie zespołami.
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
