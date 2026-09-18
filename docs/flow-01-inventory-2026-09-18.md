# FLOW-01 — inwentaryzacja i decyzje

Data: 18.09.2026. To jest krok zerowy strumienia FLOW-01 z
[mapy strumieni](parallel-streams-2026-09-18.md): **co z czterech rzeczy da się
zrobić wyłącznie w panelu nad dzisiejszymi trasami, a co wymaga nowej**, i —
zanim cokolwiek powstanie — **co dokładnie ma znaczyć wspólny filtr obiektów**.

Wynik w jednym zdaniu: trzy z czterech rzeczy są w większości panelowe, czwarta
(porównania przedziałów) jest zablokowana na dwóch brakujących parametrach
zapytania, a jedyna dziura w danych to **prompt jako oś** — span go niesie,
żadna trasa go nie grupuje.

## 1. Co odpowiadają dzisiejsze trasy

Pięć tras czyta read model. Kolumny to osie, po których można zawęzić; `—`
znaczy, że trasa takiego parametru nie ma.

| Oś | `/runs` | `/spans` | `/dimensions/{kind}` | `/metrics` | `/events/stream` |
|---|---|---|---|---|---|
| `agent` | `agent_id` | `agent_id` | `agent_id` | `agent_id` | `agent[]` |
| `session` | `conversation_id` | — | — | `conversation_id` | `session[]` |
| `runtime` | `runtime` | — | — | — | `runtime[]` |
| `workflow` | `workflow` | — | — | — | `workflow[]` |
| `variant` | `variant_id` | — | — | — | — |
| `trace` | `trace_id` | `trace_id` | — | — | — |
| `model` | `model` | `model` | — | `model` ¹ | — |
| `tool` | `tool` | `tool` | — | — | — |
| `status` | `RunStatus` | `SpanOutcome` ² | — | — | — |
| `prompt` | — | — | — | — | — |
| okno | `window_seconds` | `window_seconds` | `window_seconds` | `window_seconds` | — |
| koniec okna | **`as_of`** | **`as_of`** | — | — | — |

¹ i to jest cały problem punktu 4: `model` na `/metrics` **nie wybiera
przebiegów**, tylko pomija spany innych modeli wewnątrz pętli po spanach
(`metrics::compute`, gałąź `CHAT`). Liczniki `runs`, `tool_calls`, `step_calls`
i skuteczność zostają policzone po **wszystkich** przebiegach okna. To jest
kontrakt opisany w UX-02, świadomie zachowany, i jego zmiana jest tą pracą.

² to nie jest ta sama oś: `ok | error` opisuje span, `running | succeeded |
failed` opisuje przebieg. Nieudany span w udanym przebiegu jest rzeczą
codzienną, więc te dwa słowa nie mogą dzielić jednej nazwy w URL.

**Czego panel dziś wysyła.** Mniej, niż trasy przyjmują, i to jest pierwszy
zysk bez żadnej zmiany serwera:

| Widok | W URL | Co posyła | Czego nie posyła, choć trasa przyjmuje |
|---|---|---|---|
| Runs | `window, status, conversation_id, agent_id` | to samo | `runtime, workflow, variant_id, trace_id, model, tool, as_of` |
| Metrics | `window, agent_id, model` | to samo | — (trasa nie ma więcej) |
| Explore — drzewo | `window, by, key, find` | `window, agent_id`(nie), `search` | `agent_id` |
| Explore — oś `span` | jw. | `search, window` | `agent_id, model, tool, step_type, operation, status, min_duration_ms, trace_id` |
| Live | `window` + osie tablicowe | `agent, runtime, workflow, session` | — (mówi, czego nie śledzi) |
| Query | jw. | kompiluje do zapytania | — |

Dwa słowniki filtrów żyją dziś obok siebie: `selection-params.ts`
(`agent | runtime | workflow | session | model | tool | trace | status`, tablice,
używany przez Live i Query) oraz ręcznie wypisane pola w `search.ts` Runs i
Metrics. Pierwszy z nich nazywa osie **dokładnie tak, jak nazywa je trasa
żywego strumienia** i jak nazywa je ADR_0007. To jest słownik, który zostaje.

## 2. Cztery rzeczy, i co która kosztuje

### (1) Dedykowane strony agentów — **panel, poza jednym panelem**

Wszystko, co strona agenta ma pokazać, odpowiada dziś jedna z dwóch tras,
zawężona po `agent_id`:

| Sekcja strony | Skąd | Dziś? |
|---|---|---|
| nagłówek: przebiegi, udane, nieudane, w biegu, koszt, tokeny | `GET /dimensions/agent` + wiersz o kluczu równym agentowi | ✅ |
| przepustowość, skuteczność, latencja p50/p95/p99, koszt w czasie | `GET /metrics?agent_id=` (`timeline`, `latency`, `totals`) | ✅ |
| modele, których używa — z latencją, tokenami i kosztem | `GET /metrics?agent_id=` → `by_model`, i `GET /dimensions/model?agent_id=` | ✅ |
| narzędzia i kroki | `by_tool`, `by_step`, `GET /dimensions/tool?agent_id=` | ✅ |
| workflow, runtime'y i sesje, w których występuje | `GET /dimensions/{workflow,runtime,session}?agent_id=` | ✅ |
| historia przebiegów | `GET /runs?agent_id=` (kursor, okno) | ✅ |
| **prompty, których używa** | — | ❌ |

Prompt jest jedyną dziurą. `SpanRow` niesie `model`, `tool`, `step_type`,
`operation` i `agent_id` — podniesione z atrybutów po to, żeby dało się po nich
filtrować — ale **nie niesie `aiwatcher.prompt.*`**, a `DimensionKind` nie ma
osi `prompt`. Odpowiedzenie na „jakich promptów używa ten agent" bez serwera
znaczyłoby: pobrać stronę przebiegów, dla każdego pobrać szczegół, przejść
spany i policzyć w przeglądarce różne wersje. To jest dokładnie to, czego
zakazuje reguła „nigdy nie licz w przeglądarce tego, co liczy serwer".

### (2) Powiązania wersji prompt/model/dataset — **panel**

`apps/panel/src/shared/components/lineage-reference.tsx` **nie jest reliktem**.
Ma trzy miejsca wywołania (`training/models`, `training/runs`,
`evaluation/overview`), własny test i jasno zapisaną zasadę: rozstrzygnij
rejestr, zanim zrobisz link, bo samo brzmienie odwołania jest wieloznaczne.
Jest **niekompletny**, i to jest cała robota punktu 2:

- `DatasetReference` — jest, rozstrzyga curation kontra anotacje. ✅
- `ExecutionReference` — jest. ✅
- **prompt** — jest, ale **gdzie indziej**: `PromptRefLink` w
  `shared/components/prompt-bits.tsx`, wołany z `span-detail.tsx`. Czyli
  z całej obserwowalności klikalny jest **jeden** widok: panel szczegółu spanu.
  Fala (`waterfall`), wiersz spanu w Explore, nagłówek przebiegu i lista spanów
  nie mówią o promptcie nic.
- **wersja modelu** — **nie ma nigdzie**. `aiwatcher.model.version` jest
  rysowany jako tekst w grupie „Settings", obok `Temperature`. Rejestr modeli
  stoi pod `/training/models?model=&version=`, więc złączenie istnieje w danych
  i nie jest klikalne — dosłownie zdanie z planu.

Jedna pułapka, która decyduje o kształcie tego linku: `aiwatcher.model.version`
powstaje z pola `model_version` w payloadzie, a **dwaj producenci wypełniają je
różnymi rzeczami**. Profil serwujący (`sdk/python/aiwatcher_sdk/serving/server.py`)
wysyła `model.version` z rejestru — SHA-256. Gateway
(`gateway.py`) wysyła `relayed.served_model`, czyli nazwę modelu, którą zwrócił
dostawca. Link musi więc być strzeżony tak samo, jak strzeżony jest link
promptu: **64 znaki heksadecymalne albo żadnego linku**, bo link, który daje
404, jest gorszy niż fakt, którego trzeba poszukać.

### (3) Porównania przedziałów — **wymaga dwóch parametrów**

Porównanie dwóch okien na tych samych filtrach to porównanie **agregatów**:
skuteczności, latencji, kosztu, liczby przebiegów na wiersz wymiaru. Agregaty
liczy serwer i tylko serwer — `/metrics` i `/dimensions/{kind}`.

Obie te trasy **nie przyjmują `as_of`**. Przyjmują je `/runs` i `/spans`, gdzie
`as_of` powstało dla zarządzanego kroku, który po ponowieniu musi przeczytać tę
samą godzinę (`window::bounds`, ADR ‑ komentarz w `crates/aiwatcher-projector/src/window.rs`).

Nie da się tego obejść w panelu uczciwie:

- policzyć drugie okno z listy przebiegów w przeglądarce — to jest liczenie
  tego, co liczy serwer, a do tego lista jest stronicowana kursorem;
- podeprzeć się foldem okresów (`period_fold`, ten od Experiments) — on
  odpowiada **per wariant**, nie per dowolny filtr;
- porównać dwa okna względne (np. „ostatnia godzina" kontra „ostatnie 24 h") —
  to nie jest porównanie przedziałów, tylko dwa różne pytania o teraz.

Więc punkt 3 jest po stronie serwera, i jest **mały**: `as_of: Option<i64>` w
`DimensionFilter` i `MetricsFilter`, przepuszczone przez `window::bounds`,
które już istnieje i już jest przetestowane. Dla `/metrics` dochodzi jedna
rzecz do zrobienia porządnie: okno metryk jest osią X wykresu i jest liczone
**od startu przebiegu**, więc `as_of` musi przesunąć również koniec osi, nie
tylko odciąć przebiegi.

### (4) Wspólne filtry tabel i wykresów — **panel, a potem serwer, żeby nie kłamał**

Panel może dziś: jeden słownik w URL, przenoszony między widokami obszaru,
tłumaczony na parametry każdej trasy, i **każdy widok nazywa oś, której nie
zastosował**. To zamyka największą dziurę bez jednej linijki Rusta — dziś Runs
nie wysyła sześciu parametrów, które trasa przyjmuje, a lista spanów w Explore
nie wysyła ośmiu.

Czego panel nie domknie: `/metrics` przyjmuje trzy osie z dziewięciu, a `model`
znaczy tam co innego niż wszędzie indziej. Dopóki tak jest, strona metryk musi
wypisywać, że pięciu osi nie zastosowała — co jest uczciwe i czytelne, i co
jest właśnie powodem do zmiany serwera w drugim kroku.

## 3. Decyzja: co znaczy wspólny filtr obiektów

Pięć zdań. Wszystko, co powstanie, ma być z nich wyprowadzalne.

**1. Jeden słownik, nazwany po wymiarach.** Osie to wymiary ADR_0007 plus status
przebiegu: `agent, runtime, workflow, session, variant, trace, model, tool,
status`. Jedna nazwa na oś, ta sama w URL na każdym widoku obserwowalności i na
stronie agenta. Nazwy są te, których używa już `GET /api/v1/events/stream` i
`selection-params.ts` — nie `conversation_id` i nie `agent_id`, bo te są
nazwami parametrów trasy, a nie nazwami osi. Tłumaczenie oś → parametr jest
jednym miejscem w panelu.

**2. Filtr wybiera przebiegi.** Każda oś nazywa własność, którą przebieg ma —
wprost (`agents`, `runtimes`, `workflow`, `variant_id`, `trace_id`,
`conversation_id`, `status`) albo przez swoje spany (`model`, `tool`), czym
`RunFilter` już się posługuje. Wybrany zbiór to przebiegi mające **wszystkie**
nazwane własności; między osiami koniunkcja. Wiele wartości na jednej osi to
alternatywa i przyjmuje ją dziś wyłącznie żywy strumień — widok, który przyjmie
jedną, mówi, że zawęża do jednej, zamiast po cichu brać pierwszą.

**3. Licznik liczy się po wybranych przebiegach, a zawęża się dalej tylko
wtedy, gdy oś jest o tym, co ten licznik liczy.** Przy `model=X`:

| Licznik | Po czym | Dlaczego |
|---|---|---|
| przebiegi, skuteczność, latencja przebiegu | przebiegi, które wołały X | bo to są przebiegi |
| wywołania LLM, tokeny, koszt, latencja LLM | wywołania X w tych przebiegach | bo wywołanie ma model |
| wywołania narzędzi i kroków | wszystkie w tych przebiegach | bo wywołanie narzędzia nie ma modelu |

I strona to mówi — jednym zdaniem wyprowadzonym z zaznaczenia, nie pisanym per
widok.

To jest zmiana wobec dziś, i jest to dokładnie ta zmiana. Dziś, przy filtrze
modelu, „Runs: 412" i skuteczność obok są liczone po **każdym** przebiegu okna,
a wykres tokenów pod nimi po wywołaniach jednego modelu. Dwie liczby o dwóch
różnych populacjach na jednym ekranie, a jedyne, co je rozróżnia, to akapit
drobnym drukiem. Po zmianie każda liczba na ekranie jest o jednej populacji, a
jedyne zawężenie, które zostaje, daje się powiedzieć jednym zdaniem.

Odrzucone alternatywy:
- *`model` nigdzie nie wybiera przebiegów* (dzisiejsza reguła metryk, rozciągnięta
  na resztę). Wtedy lista przebiegów przestaje odpowiadać na „które przebiegi
  dotknęły gpt-4o", a oś `model` w Explore nie ma czego pokazać pod wierszem.
- *Każdy licznik zawęża się do rzeczy, którą oś nazywa.* Przy `tool=t` nie ma
  czegoś takiego jak „wywołania LLM narzędzia t"; to się zwija do zdania 3.

**4. Widok, który osi nie umie zastosować, mówi to.** Precedens jest i jest
sprawdzony: Live nazywa `model`, `tool`, `trace` i `status` jako nieśledzone,
zamiast wysłać je i pokazać ciszę. Ten sam mechanizm, jeden komponent, trzy
powody nieprzyjęcia: trasa nie ma parametru, oś znaczy tu co innego (`status`
na liście spanów), albo wybrano więcej wartości niż widok umie wysłać.

**5. Filtr żyje w URL i przechodzi między widokami obszaru**, tak jak dziś
przechodzi okno czasu (`NavArea.carries`). „Zawęziłem do tego agenta, teraz
metryki dla niego" to jedno pytanie, a przełączenie zakładki, które je gubi,
robi z niego dwa.

## 4. Decyzja: gdzie mieszka strona agenta

**Nowy obszar `features/agents/`, w sekcji Aplikacje, obok Promptów i
Obserwowalności** — tak, jak mówi docelowa architektura informacji w planie.

Za tym, żeby jednak wsadzić ją do `features/observability/`, przemawia jedna
rzecz i jest poważna: agent **nie jest bytem autorskim**. Nie ma rejestru
agentów, nie ma wersji agenta, nie ma niczego poza retencją — agent to klucz,
który przebieg wnosi do foldu. Strona agenta jest więc widokiem logu, a nie
stroną obiektu w tym sensie, w jakim jest nią prompt albo model.

Przeważa co innego: **granica architektury zabrania jednej funkcji importować
drugiej** (`check-architecture.mjs`), a to, co strona agenta dzieli z
obserwowalnością, to wyłącznie słownik filtra — który i tak musi trafić do
`shared/`, bo czytają go trzy widoki obserwowalności plus ta strona. Koszt
osobnego katalogu to więc **jeden plik przeniesiony do `shared/`**, a zysk to
adres, który nie kłamie: `/agents/$agentId` jest stroną agenta, a
`/observability/explore?by=agent&key=…` jest komórką w tabeli — i plan nazywa
tę różnicę wprost.

Konsekwencja zapisana wprost: strona agenta **nie zakłada, że agent istnieje**.
Nie ma trasy, która by to potwierdziła; jest tylko fold, który go widział albo
nie. Agent bez ani jednego przebiegu w oknie to pusty stan mówiący „w tym oknie
nic", nie 404.

## 5. Decyzja: co z `lineage-reference.tsx`

Zostaje i rośnie o `ModelVersionReference`. `PromptRefLink` **zostaje tam,
gdzie jest** (`prompt-bits.tsx`): to jest słownik obszaru promptów, obok reguł
rejestru, które ten link egzekwuje, a przeniesienie go przepisałoby importy w
cudzej funkcji za tyle, ile warte jest wyrównanie nazw plików. W dokumentacji
obu plików staje zdanie, że są dwiema połowami jednej rzeczy.

## 6. Kolejność

Cztery commity. Panel najpierw, wygenerowane artefakty osobno — bo
`contracts/openapi.json` i `src/api/generated` dzielimy z SYS-01 i LEARN-02, a
konflikt na nich rozwiązuje się przegenerowaniem, nie scalaniem.

| # | Co | Dotyka |
|---|---|---|
| **A** | wspólny filtr w `shared/`, wpięty w Runs, Metrics i Explore; strony agentów; `ModelVersionReference` i prompt na fali i w liście spanów | wyłącznie panel |
| **B** | `as_of` na `/dimensions` i `/metrics`; pozostałe osie na obu; `model` wybiera przebiegi; oś `prompt` w `DimensionKind` i w `RunFilter`/`SpanFilter` | `aiwatcher-projector`, `aiwatcher-api` |
| **C** | `just openapi` | kontrakt + wygenerowany klient, nic więcej |
| **D** | porównanie przedziałów; metryki honorują wszystkie osie; panel promptów na stronie agenta | wyłącznie panel |

Co w B jest addytywne: wszystko. Nowy parametr zapytania nieobecny w żądaniu to
dzisiejsze zachowanie; nowy wariant `DimensionKind` nie rusza ośmiu istniejących.
Jedyna zmiana **zachowania** to `model` wybierający przebiegi w `/metrics` — i
to jest jawna zmiana kontraktu UX-02, opisana wyżej, wpisana do planu migracji.
