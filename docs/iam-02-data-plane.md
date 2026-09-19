# IAM-02 — plan data plane

Data: 18.09.2026. Kontynuacja [IAM-01](iam-01-kickoff.md), której granica jest
zapisana w [ADR_0033](ADR/ADR_0033_PROJECT_SCOPED_STORAGE.md), a kontrakt w
[README IAM](../crates/aiwatcher-iam/README.md).

IAM-01 domknął **połowę autorską**: control plane, rejestry za `for_project`,
wykonanie z trwałym właścicielem i dispatcher. Nietknięta została **połowa
obserwowalna** — log zdarzeń, projektor, read model i żywe strumienie — czyli
rdzeń produktu. Ten dokument jest planem na tę połowę i na to, co poza nią
zostało w data plane.

**Ten etap jest świadomie ostatni** — sekcja 7 mówi, co to znaczy i czego nie
kosztuje.

---

## 1. Cel tego etapu

**M1 — „mój projekt, mój strumień".** Zalogowany użytkownik widzi wyłącznie
projekty, do których ma grant; otwiera projekt i widzi **jego** przebiegi, spany
i metryki; podłącza się do SSE/WebSocket i słyszy **tylko swoje** zdarzenia;
odebranie grantu zamyka ten strumień, a nie czeka na wygaśnięcie sesji.

To jest **mniej** niż multi-tenancy i tak ma zostać. Poza M1 zostają świadomie:
silniki zapytań i notebooków, poświadczenia workerów, harmonogramy, archiwum
rozmów, zakresowy sweep retencji, kolekcja artefaktów i dzierżawa w
VictoriaTraces/Metrics. Sekcja 6 wymienia je po nazwie — jako decyzję, nie jako
przeoczenie.

---

## 2. Jedna decyzja, od której zależy cała reszta

Dziś fakty projektu **nie trafiają na log** — nie z wyboru, tylko z konstrukcji:
globalny publikator outboxa nie widzi wierszy projektu. ADR_0033 nazywa to
wprost jako pierwszą rzecz, która może go unieważnić: *„projekt, który
potrzebuje logu zdarzeń… to trzy kolejne granice, a jeśli nie da się ich
zbudować bez per-tenant foldu, to do rewizji jest przesłanka, że jedna instancja
obsługuje wiele projektów"*.

M1 wymaga logu. Rozważone trzy drogi:

| Droga | Werdykt |
|---|---|
| **Zakres w kopercie**, pisany przy ingeście z poświadczenia | **Wybrana.** Jeden log, jeden fold, jeden dodatkowy klucz w wierszu |
| **Log per projekt** (partycje/strumienie) | Odrzucona. `AIWATCHER_LASER_PARTITIONS > 1` jest zakazane, dopóki skalarny `Checkpoint` nie stanie się kursorem per partycja — a bez tego resume żywego strumienia po cichu gubi zdarzenia |
| **Fakty projektu zostają poza logiem** | Odrzucona. Wtedy M1 jest nieosiągalny: przebieg projektu z definicji nie ma żywego widoku, spanu ani foldu |

**Decyzja: `EventEnvelope` niesie opcjonalny `ProjectScope`.** Brak = strona
globalna, więc nic z dotychczasowych danych się nie rusza.

Jedna rzecz, której nie wolno skopiować z sąsiedniego pola. `published_by` jest
`#[serde(skip)]` — ustawiany przy ingeście i **nieprzeżywający szyny**, bo
czytelnikiem jest ten sam proces. Zakres ma innego czytelnika: projektor czyta z
szyny, nie z trasy. Więc zakres **musi być serializowany**, a reguła zaufania
przenosi się o krok:

> Trasa ingestu **zawsze nadpisuje** zakres wartością z poświadczenia; wartość
> podana przez producenta jest odrzucana, nie honorowana. Po ingeście zakres
> jest słowem trasy, nie producenta — dokładnie tak, jak `published_by` jest nim
> w obrębie procesu.

Kto potraktuje to jak zwykłe pole koperty, zrobi z niego prymitywkę do
podszywania się pod cudzy projekt.

---

## 3. Etapy

### E0 — co już stoi

Control plane (organizacje, zespoły, projekty, granty, audyt w tej samej
transakcji), ~13 rodzin zasobów za `for_project`, **63** wpisy tras
`orgs/{organization}/projects/{project}` w kontrakcie z **rzeczywistym**
sprawdzeniem grantu (`ProjectAuthorization::authorize_write`, świeża decyzja na
operację, powtórzona po odebraniu ciała), trwałe `ExecutionOwnership`, związany
`WorkflowStore`, para artefaktów, i dispatcher.

### E1 — projekt na kopercie — **zrobione**

- `EventEnvelope::project: Option<ProjectScope>`, serializowany, domyślnie
  nieobecny.
- `IngestToken` zyskuje zakres; `Identity` go niesie. Rola nadal **twardo
  `Editor`** — token w środowisku agenta nie może urosnąć, a `queues` puste
  znaczy, że producent niczego nie claimuje. Zakres tylko **zawęża**.
- Trasa `POST /api/v1/events` nadpisuje pole z poświadczenia. Token bez zakresu
  pisze globalnie, dokładnie jak dziś.
- `contracts/envelope.schema.json` + `just openapi`. **SDK bez zmian** — nigdy
  tego pola nie wysyłają.
- Testy: producent nazywający projekt jest ignorowany; token projektu A nie
  zapisze do B; koperta bez pola czyta się jak każda historyczna.

### E2 — fold zna zakres — **zrobione**

- **Jeden fold z kluczem zakresu w wierszu, nie fold per tenant.** To jest
  różnica między „dodatkowy wymiar" a „przesłanka do rewizji" z ADR_0033.
- Obejmuje: `RunSummary`, `dimensions::compute` (siedem osi), spany,
  `period_fold`, `asked`, `measured` i journal.
- **Identyfikatory globalne nie drgną co do bajtu** — `TraceId::derive` /
  `SpanId::derive` są czystymi funkcjami `run_id`, a `RunIdentity::of` już
  bierze zakres (ADR_0001, ADR_0033 pkt 5).
- Pamięć i kardynalność: `AIWATCHER_MAX_SPANS_TOTAL` zachowuje znaczenie, ale
  **`just load-test` trzeba przebiec ponownie** i razem z nim ruszyć limit
  kontenera. Metryki rosną o jedną serię na projekt, który cokolwiek zapisał.
- Retencja logu pozostaje instancyjna i tak ma zostać na tym etapie.

### E3 — odczyty odpowiadają pytającemu — **zrobione**

- Każda lista i każdy szczegół filtruje po **efektywnych grantach pytającego**,
  liczonych świeżo na żądanie. `ProjectAccess` to migawka z `evaluated_at`, nie
  zdolność na sesję.
- Odmowa zakresu to **404**, nie 403 i nie 503 — przebieg, do którego ktoś nie
  sięga, to przebieg, którego nie ma (precedens `StoreError::OutOfScope`).
- **Decyzja do podjęcia, nie techniczna:** co widzi członek projektu na
  przebiegu *nieprzypisanym* (globalnym)? **Rekomendacja: bez zmian** — dane
  globalne zostają tam, gdzie są, pod autoryzacją instancji, a dane projektowe
  są addytywne. To jest dokładnie to „nie wchodzić jeszcze całkowicie", i to
  jest też jedyny wariant, w którym nic istniejącego się nie psuje.

### E4 — żywy odczyt — **to jest M1** — **zrobione**

- SSE i WebSocket niosą zakres w subskrypcji; hub filtruje. Tożsamością jest
  **podpisane ciasteczko sesji**, bo przeglądarka nie ustawi nagłówka na żadnej
  z tych dwóch tras — to jest cała racja bytu ADR_0013.
- **Odwołanie grantu w trakcie strumienia.** Dziś TTL ciasteczka *jest* oknem
  odwołania; dla strumienia żyjącego godzinami to za mało. Propozycja: ponowne
  sprawdzenie grantu co **30 s** na strumień i zamknięcie połączenia przy
  odmowie — jedno zapytanie o grant na strumień na pół minuty, a odebranie
  dostępu działa w czasie, w którym człowiek zdąży zauważyć.
- **`Last-Event-ID` sprawdza grant przed odtworzeniem.** Bez tego wznowienie
  strumienia jest sposobem na czytanie po odebraniu dostępu.
- Testy: odwołanie w trakcie strumienia go zamyka; wznowienie po odwołaniu nie
  odtwarza nic; subskrypcja cudzego projektu to 404.

**Bramka M1:** logowanie → lista wyłącznie moich projektów → przebiegi, spany i
metryki wyłącznie mojego projektu → żywy strumień wyłącznie mojego projektu →
odwołanie grantu zamyka strumień i odcina odczyty. Weryfikacja przez
**rzeczywiste HTTP**, nie przez test biblioteczny.

### E5 — dzielenie projektu

To drugie z dwóch pytań, i blokuje je jedna brakująca rzecz.

**`Command::Grant` wymaga dokładnej pary `(provider, subject)` — a tej nie da
się znać, zanim człowiek pierwszy raz się zaloguje.** Email nie jest kluczem
tożsamości i nigdy nim nie będzie. Więc zaproszenie:

- jednorazowy, wygasający token związany z `(scope, role, window)`;
- realizowany **po** SSO, w jednej transakcji tworzący grant dla principala,
  który go zrealizował;
- email jest wskazówką do dostarczenia i **niczym więcej** — nie warunkiem
  przyjęcia;
- powtórzenie i inny odbiorca muszą odmówić.

Do tego panel: przełącznik organizacji/projektu, strona członków i grantów,
okno dzielenia. Dziś **żaden plik panelu nie odwołuje się do organizacji ani
projektów** — wygenerowany klient ma 82 wpisy, a interfejsu nie ma w ogóle.
`/account` pokazuje grupy IdP tylko do odczytu i mówi, że to nie są zespoły.

**Co daje się dzielić od razu po E5, bez E1–E4:** cała połowa autorska — prompty,
datasety, anotacje, treningi, ewaluacje, definicje workflow, review przypadków,
kohorty, nagrania, bundle, approvale, kalibracje i deklaracje. Te trasy istnieją
i naprawdę sprawdzają granty. Dzielenie **obserwowalności** wymaga E1–E4.

### E6 — reszta data plane, poza M1

Silniki zapytań (`services/query`) i runtime notebooków
(`services/ml_pipeline`), poświadczenia i kolejki workerów, harmonogramy (dziś
odmawiane po nazwie na związanym magazynie), archiwum rozmów (zamknięte po obu
stronach), zakresowy sweep retencji, kolekcja artefaktów, eksport do
VictoriaTraces/Metrics.

### E7 — migracja i cutover

[Runbook](iam-migration-runbook.md). 4 z 16 rodzin wspierane, 11 bez adaptera,
1 zablokowana (`conversations` — kopia bajtów ich nie otworzy, ADR_0021).
S3/RustFS niezweryfikowane. I rzecz, której żadne narzędzie nie załatwi:
**kto ma dostęp po skopiowaniu, nie jest zdecydowane** — mapowanie danych do
projektu nikomu nie nadaje do nich dostępu.

---

## 4. Kolejność i zależności

| Etap | Zależy od | Daje |
|---|---|---|
| E1 projekt na kopercie | — | Fakty projektu docierają na log |
| E2 fold zna zakres | E1 | Przebiegi, spany, wymiary i okresy mają właściciela |
| E3 odczyty po grantach | E2 | Listy i szczegóły odpowiadają pytającemu |
| E4 żywy odczyt | E3 | **M1** |
| E5 dzielenie | E3 (autorskie: nic) | Zapraszanie, członkowie, selektor |
| E6 reszta data plane | M1 | Zapytania, notebooki, workerzy, harmonogramy, retencja |
| E7 migracja | E6 | Cutover |

E5 dla połowy autorskiej nie czeka na nic i może iść równolegle do E1–E2.

---

## 5. Ryzyka, nazwane zawczasu

- **Pamięć i kardynalność.** Read model jest w procesie i ograniczony
  `AIWATCHER_MAX_SPANS_TOTAL`. Bez powtórzonego `just load-test` E2 jest zmianą
  kontraktu pamięciowego zrobioną na niewidocznie.
- **Opóźnienie odwołania w strumieniu.** 30 s to propozycja, nie pomiar. Trzeba
  ją zderzyć z kosztem zapytania o grant przy realnej liczbie strumieni.
- **Token ingestu to wspólny sekret w środowisku agenta.** Zakres zmniejsza
  promień rażenia wycieku, nie usuwa go. Rola zostaje `Editor` na twardo.
- **Przebiegi globalne zostają widoczne.** To jest decyzja z E3 i ma być
  powiedziana głośno, a nie odkryta przez kogoś w trakcie audytu.
- **`auth=none` i `auth=local` pozostają trybami jednego najemcy.** Żaden z nich
  nie staje się bezpiecznym trybem pracy wielu organizacji dlatego, że powstał
  selektor.

---

## 6. Czego ten plan nie robi

Fold per tenant. Dzierżawa w VictoriaTraces/Metrics — zakres jedzie jako atrybut
zasobu, a same magazyny i Perses nie są zakresowane. Harmonogramy projektowe.
Rozmowy. Migracja historii. I żadne z powyższych nie jest powodem, by aktywować
selektor organizacji/projektu przed bramką M1: **dopóki E1–E4 nie są
zweryfikowane przez rzeczywiste HTTP, ten deployment nie jest opisywany jako
multi-tenant safe.**

---

## 7. Kiedy — i co znaczy „na końcu"

IAM-02 jest **odłożony na koniec roadmapy**, świadomie. To zmienia kolejność z
`ux-migration-plan-2026-09-14.md`, gdzie IAM stał na trzecim miejscu z siedmiu.

**Czego to nie kosztuje.** Nic z istniejących danych nie wymaga przeróbki, bo
brak zakresu *jest* stroną globalną — E1 jest addytywne, identyfikatory globalne
nie drgną co do bajtu, a E3 rekomenduje zostawić dane globalne dokładnie tam,
gdzie są. Odłożenie nie tworzy długu w danych.

**Co to kosztuje.** Każda funkcja zbudowana w międzyczasie to jedna powierzchnia
więcej do zakresowania później, a każdy nowy rejestr to potencjalnie dwunasta
rodzina bez adaptera migracji. Stąd jedna reguła, która obowiązuje póki IAM-02
czeka:

> **Nowy zasób autorski dostaje swoją zakresową rodzinę tras od urodzenia.**
> Wzorzec jest gotowy — `ProjectAuthorization` plus
> `<prefix>/scopes/<organization>/<project>/registry/` — i napisanie go od razu
> to ten sam kod, którym dorabianie go później nie jest.

**Co obowiązuje w międzyczasie.** Deployment pozostaje jednym najemcą w praktyce:
selektor organizacji/projektu jest nieaktywny, przebieg projektu nie ma żywego
widoku, spanu ani foldu, a `auth=none` i `auth=local` są trybami lokalnymi. Nie
opisujemy tego wdrożenia jako multi-tenant safe — i to zdanie nie zmienia się
przez samo istnienie tego planu.

**Co idzie wcześniej.** E5 dla połowy autorskiej — zaproszenia, interfejs
członków i okno dzielenia — nie zależy od E1–E4. Trasy zakresowe już sprawdzają
granty, więc dzielenie promptów, datasetów, anotacji, treningów i ewaluacji da
się otworzyć bez ruszania logu zdarzeń. Kolejność do pierwszego testu uprawnień
i lekcji, razem z promptem dla sesji, która ją wykona, jest w
[planie UX](ux-migration-plan-2026-09-14.md#kolejność-do-pierwszego-testu-permissionów-i-lekcji--18092026).

---

## 8. E1 i E2 — co naprawdę stanęło (18.09.2026)

Oba etapy są zrobione i zielone; kontrakt jest w
[README IAM](../crates/aiwatcher-iam/README.md) („Połowa obserwowalna się
zaczyna") i w [ADR_0001](ADR/ADR_0001_EVENT_ENVELOPE.md), aneks z 18.09.2026.

**E1.** `EventEnvelope::project` i `RecordedMetadata::project`,
serializowane, domyślnie nieobecne. Trasa ingestu nadpisuje pole zakresem
poświadczenia **zawsze**, także nieobecnością. `IngestToken` niesie zakres w
etykiecie — `name[queue]@<organizacja>/<projekt>=sekret`, przyrostek, więc
każdy dotychczasowy token parsuje się dokładnie jak przedtem. Rola zostaje
twardo `Editor`. `contracts/envelope.schema.json` zaktualizowany; SDK nietknięte.

**E2.** Jeden fold, klucz zakresu w wierszu: `RunSummary` (projekt pierwszego
zdarzenia, nigdy nieprzenoszony), `dimensions::compute` (wiersz to `(projekt,
klucz)`, kursor też), `SpanRow` (z atrybutów, które asembler pisze przy
otwarciu spanu), `period_fold` wraz z układem kluczy w object storze, `asked`,
`measured` i journal. Identyfikatory globalne nie drgnęły co do bajtu.

**Pomiar, nie założenie.** `just load-test`, debug, pełna retencja, 5 000
przebiegów, jeden batch na przebieg: **176 MB** bez projektu, **182 MB** z
jednym, **183 MB** z pięćdziesięcioma. Koszt jest **na zdarzenie**, nie na
projekt — to jest różnica „jeden fold" od „fold per tenant" wyrażona w
megabajtach. Limit 512 MB w `deploy/` zostaje; liczby są zapisane przy
`ReadModelConfig`.

**Czego to nie robi, i to jest ważniejsze niż co robi.** Żaden z tych wierszy
nie jest decyzją o dostępie. Odczyty nadal odpowiadają pod autoryzacją
instancji (E3), SSE i WebSocket są nietknięte (E4), selektor nieaktywny. To
wdrożenie nadal nie jest opisywane jako multi-tenant safe.

**Jedna dziura, nazwana.** Producent publikujący **prosto do brokera** niesie
własne słowo o swoim projekcie — dokładnie tak, jak żaden rekord tam nie nazywa
publikującego (ADR_0001). Trasa jest granicą; broker stanie się nią, gdy zacznie
uwierzytelniać producentów, a adapter poniesie to, czego się dowiedział.

---

## 9. E3 i E4 — co naprawdę stanęło, i bramka M1 (19.09.2026)

Oba etapy są zrobione; kontrakt jest w
[ADR_0033](ADR/ADR_0033_PROJECT_SCOPED_STORAGE.md), aneks z 19.09.2026.

**Decyzja z E3, podjęta: bez zmian, tak jak rekomendował plan.** Dane globalne
zostają tam, gdzie są, pod autoryzacją instancji; dane projektowe są addytywne,
na rodzinie `/api/v1/orgs/{organization}/projects/{project}`. To jedyny wariant,
w którym nic istniejącego się nie psuje — każdy przebieg, jaki ten build
napisał, nie ma projektu, więc trasy instancyjne odpowiadają dokładnie tym, czym
odpowiadały.

Druga połowa tej decyzji jest tą, którą łatwo pominąć i którą trzeba powiedzieć
głośno: **trasy instancyjne nie odpowiadają wierszami projektu.** Członek
projektu widzi przebieg globalny — tak, bez zmian. Widz instancji **nie** widzi
przebiegu projektu, bo inaczej dodatkowa rodzina tras nie dodałaby niczego, a
czytałby projekt, do którego nie ma grantu.

**E3.** `aiwatcher_projector::ReadScope` (`Global | Project`) jest argumentem
każdego odczytu foldu przebiegów — `list`, `run`, `spans`, `dimensions`,
`conversations`, `metrics`, `serving`, `asked_since` — a nie kolejną osią w
`RunSelection`: zakres decyduje, które wiersze w ogóle są, łącznie z licznikami,
które fold bierze *zanim* cokolwiek zawęzi (`runs_retained`, suma niezgrupowanych
w wymiarze). `run_scope::RunRead` rozstrzyga stronę przez to samo
`project_scope::resolve`, czyli **świeże** `IamStore::access` na każde żądanie.
Odmowa zakresu to **404**. `/runs/{id}/events` czyta log, więc stronę bierze z
**pierwszego zdarzenia przebiegu** — tą samą regułą, którą trzyma
`RunSummary::project`; pytanie read modelu zostawiłoby dziurę: przebieg projektu,
którego wiersz został wyparty, stałby się czytelny globalnie.

**E4.** `LiveEvent` niesie `project`, a `stream::Subscription` łączy go z
selekcją. Filtr zostaje na serwerze (ADR_0004, aneks z 11.09). Tożsamością jest
**podpisane ciasteczko sesji** — po raz pierwszy jest to nośne, a nie tylko
opisane (ADR_0013). Strumień zakresowy **pyta o grant ponownie co 30 s** i
zamyka się przy odmowie ramką `LiveFrame::Revoked`; ten sam tik czyta wygaśnięcie
tożsamości. Niedostępny IAM **nie** jest odpowiedzią i nie zamyka połączenia
(`Transient`, jak w `ProjectDispatcher`) — ogranicza to druga połowa: dopóki IAM
nie odpowiada, żaden **nowy** strumień się nie otwiera. `Last-Event-ID` niczego
nie rozszerza: wznowienie to nowe żądanie, więc grant jest sprawdzany przed
odtworzeniem, a odtwarzane ramki idą przez tę samą subskrypcję.

### Bramka M1 — 44/44, rzeczywistym HTTP

`scripts/iam-permission-check.py` urosło z 28 pytań do **44**, na żywym serwerze
(`just authentik-up`, `authentik-seed`, `postgres-up`, `run-sso-iam`, `panel`).
Dwa pytania potrzebują przebiegu po stronie projektu, a tam kładzie go wyłącznie
**poświadczenie** — sesja człowieka nie niesie projektu z założenia. Więc
pierwsze uruchomienie wypisuje linię do ustawienia, a drugie pyta o wszystko:

```
python3 scripts/iam-permission-check.py
AIWATCHER_M1_SCOPE=<organizacja>/<projekt> just run-sso-iam
AIWATCHER_M1_SCOPE=<organizacja>/<projekt> python3 scripts/iam-permission-check.py
```

Bez tego te pytania są **wypisane jako niezadane**, nie zaliczone — zielone,
które liczy niezadane pytanie, nic nie znaczy. Zmierzone: 44/44 z zakresem,
37/37 + 7 niezadanych bez niego.

Bramka, punkt po punkcie: zalogowanie → lista wyłącznie moich projektów →
przebiegi, spany i metryki wyłącznie mojego projektu (i **żadnego** z
instancji, w obie strony) → żywy strumień wyłącznie mojego projektu →
odwołanie grantu zamyka strumień, który ktoś już miał otwarty, i odcina
odczyty → wznowienie po odwołaniu nie odtwarza nic.

### Czego to nie robi, po nazwie

**Fold workflow nie ma projektu w wierszu.** E2 okluczowało fold przebiegów,
wymiary, spany, okresy, `asked`, `measured` i journal — nie ten. Więc
`/api/v1/workflows` i `/api/v1/workflow-executions` nadal odpowiadają
instancyjnie, nie mają rodziny zakresowej, a ich strumień odpowiada **stroną
globalną** — co zawodzi zamknięciem i znaczy, że przebieg projektu nadal nie ma
żywego widoku grafu. To samo dotyczy foldu ewaluacji (`/api/v1/evaluations`) i
`/api/v1/experiments`. Okluczowanie tych foldów to reszta E2 i jest warunkiem
IAM-02/C: **dopóki nie stoi, selektor organizacja/projekt zostaje nieaktywny.**

Poza tym wszystko z sekcji 6 zostaje poza: logi, silniki zapytań i notebooków,
workerzy i ich poświadczenia, harmonogramy, retencja, dzierżawa w
VictoriaTraces/Metrics. **To wdrożenie nadal nie jest opisywane jako
multi-tenant safe.**

### Ryzyko, zmierzone tylko w połowie

30 s to nadal propozycja, nie pomiar wobec realnej liczby strumieni (sekcja 5).
Co wiadomo: jedno zapytanie o grant na strumień na pół minuty, w procesie na
`MemoryIamStore` i jeden zindeksowany wiersz na PostgreSQL. Czego nie wiadomo:
ile strumieni trzyma naraz instalacja z panelem otwartym na wielu biurkach.

---

## 10. E5 — selektor, i jak daleko sięga (19.09.2026)

E5 było zrobione poza jedną rzeczą: **selektorem organizacja/projekt**. Reguła,
która go trzymała poza panelem, brzmiała: *„nie ma przełącznika organizacji w
nagłówku, bo selektor zakresujący cały panel byłby ogłoszeniem multi-tenancy,
którego płaszczyzna danych nie utrzyma"*. Warunkiem jej zdjęcia było, żeby M1
naprawdę trzymało — zmierzone, nie założone.

### Matryca, przebiegnięta jeszcze raz i rozszerzona do 47 pytań

`scripts/iam-permission-check.py` na żywym serwerze: **47/47**. Czterdzieści
cztery to M1 z 19.09 (sekcja 9), bez zmian. Trzy nowe pytają o coś innego i to
one rozstrzygają kształt selektora — zadane **po** odwołaniu grantu, więc każdą
odpowiedź dostaje principal, któremu ten projekt odpowiada 404:

```
ok  a project's workflow graph is still on the instance's list — that fold has no project in its row
ok  and so is its execution, to somebody the project itself answers 404 to
ok  and its evaluation report, on the fold ADR_0010 gave its own projection
```

To nie jest awaria E3. Te foldy **nigdy nie zostały okluczowane** — E2 objęło
fold przebiegów, wymiary, spany, okresy, `asked`, `measured` i journal, i tyle.
Nie ma czego zawężać i nie ma trasy zakresowej do zapytania. Kontrakt mówi to
samo, co do ścieżki: **31 rodzin tras odpowiada za jeden projekt, 6 częściowo,
33 wyłącznie instancyjnie** (78 ścieżek zakresowych z 251).

### Decyzja: selektor tak, ale nie „cały panel"

Reguła zabraniała **ogłoszenia**, którego dane nie utrzymają. Selektor, który
nazywa własny zasięg, żadnego takiego ogłoszenia nie robi — i to jest ta zmiana.
Pięć części:

1. **`?scope=<organizacja>/<projekt>` na trasie *root***, utrzymywany przez
   `retainSearchParams`. Każdy inny parametr należy do widoku i ginie na jego
   granicy (`NavArea.carries` to ta sama myśl piętro niżej); zakres nie należy
   do żadnego, bo decyduje, które wiersze w ogóle są. Czytany w `beforeLoad`
   trasy root — przed każdym loaderem i przed renderem — więc głęboki link nie
   zdąży dostać ani jednej odpowiedzi instancyjnej; przy zmianie zakresu cache
   react-query jest czyszczony, bo klucze nazywają widok, nigdy projekt.
2. **Jedno przepisanie, w transporcie** (`apps/panel/src/shared/lib/scope.ts`).
   Wygenerowanego klienta woła 97 plików; zakres nakładany per wywołanie to
   zakres, o którym ktoś zapomni. `SCOPED_ROUTES` to zbiór tras instancyjnych
   mających bliźniaka zakresowego — **wzięty z `contracts/openapi.json` i
   sprawdzany wobec niego testem**. Trasa bez bliźniaka zostaje nietknięta:
   przepisanie jej dałoby 404, a 404 czyta się jak „nie wolno ci tego widzieć",
   podczas gdy prawdą jest „to nie jest własność projektu".
3. **Każdy obszar deklaruje `reach`** w `navigation.ts` (`project | instance |
   mixed`, per widok tam, gdzie widok się różni), a `reach-notice.tsx` mówi to
   na stronie. Przy wybranym projekcie obszar, do którego granica nie sięga,
   mówi, że odpowiada za całe wdrożenie, **łącznie z pracą innych projektów** —
   i nie da się oznaczyć samych wierszy, bo ten sam brak projektu w wierszu jest
   powodem, dla którego obszar jest instancyjny.
4. **Bez wybranego projektu panel jest stroną nieprzypisaną i tak się nazywa.**
   Przed M1 lista instancyjna *była* wszystkimi przebiegami; już nie jest, a ktoś,
   komu przebiegi jego projektu zniknęły z listy, zasługuje na zdanie, nie na
   zgłoszenie błędu.
5. **Nic z tego nie pojawia się na instancji, której użytkownik nie jest w żadnej
   organizacji.** Wdrożenie, które nigdy nie użyło IAM, ma jedną stronę i nie
   dostaje przełącznika, którego nie ma jak użyć.

Zmiana projektu zachowuje obszar albo widok i porzuca obiekt (`landingFor`):
`/runs/{id}` nazywa przebieg należący do strony, na której został otwarty.

### Co jeszcze zostało domknięte

Ramka `revoked` była zbudowana po stronie serwera w IAM-02/B i **nieosiągalna**
z panelu, bo panel nie otwierał strumienia zakresowego. Teraz otwiera:
`live.ts` zna tę ramkę, zamyka `EventSource` zamiast pozwolić mu wznawiać się w
404, a `StreamBadge` rysuje to jako odmowę. Zmierzone w przeglądarce na żywym
serwerze: odwołanie grantu przy otwartym `/observability/live` zmienia odznakę z
`live` na `access revoked` po **30 sekundach**, czyli na pierwszym ponownym
pytaniu o grant.

### Czego to nadal nie robi

Wszystko z sekcji 6 i z „Czego to nie robi" w sekcji 9 zostaje: foldy workflow i
ewaluacji bez projektu w wierszu, `/experiments`, archiwum rozmów, importy i
źródła anotacji, silnik zapytań i runtime notebooków, workerzy, harmonogramy,
retencja, dzierżawa w VictoriaTraces/Metrics. **To wdrożenie nadal nie jest
opisywane jako multi-tenant safe** — selektor tego nie zmienia i właśnie dlatego
mówi, dokąd sięga.
