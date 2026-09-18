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

### E1 — projekt na kopercie

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

### E2 — fold zna zakres

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

### E3 — odczyty odpowiadają pytającemu

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

### E4 — żywy odczyt — **to jest M1**

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

**Co może pójść wcześniej, jeśli okaże się pilne.** E5 dla połowy autorskiej —
zaproszenia, interfejs członków i okno dzielenia — nie zależy od E1–E4. Trasy
zakresowe już sprawdzają granty, więc dzielenie promptów, datasetów, anotacji,
treningów i ewaluacji da się otworzyć bez ruszania logu zdarzeń.
