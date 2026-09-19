# IAM-03 — projekty: konta, dzielenie, cykl życia, klonowanie

Data: 19.09.2026, zaktualizowany tego samego dnia po zamknięciu
[IAM-02/D](iam-02-data-plane.md#12-e6-i-e7--co-naprawdę-stanęło-19092026).
Ciąg dalszy [IAM-01](iam-01-kickoff.md) (control plane i połowa autorska) oraz
[IAM-02](iam-02-data-plane.md) (log, foldy, odczyty, strumienie — bramka M1
zaliczona; E6 i E7 zamknięte). Granica danych jest w
[ADR_0033](ADR/ADR_0033_PROJECT_SCOPED_STORAGE.md), kontrakt w
[README IAM](../crates/aiwatcher-iam/README.md).

> **Co zmieniło się pod tym planem, zanim ktokolwiek go zaczął.** IAM-02/D
> dowiozło resztę E2 (foldy workflow i ewaluacji, `/experiments`), zakresowy
> publikator outboxa, `ProjectDispatcher` w produkcyjnym `spawn`, projektowy
> `/start`, zakresowy sweep retencji i projektową stronę jednego przebiegu.
> Skutki dla tego planu, każdy w swoim miejscu niżej: **M6 jest w połowie
> zrobione**, **M7 skurczyło się o trzy pozycje**, **nagłówek D4 wymaga
> korekty** (te rodziny nie odpowiadają już żadnym wierszem projektu), a
> **M0 dostaje graf workflow i `/experiments`**, których plan mu odmawiał.

Tamte dwa dokumenty budowały **mechanizm**: kto jest principalem, co znaczy
grant, gdzie kończy się projekt w kluczu obiektu i w wierszu foldu. Ten
dokument jest o **produkcie na tym mechanizmie**: konto osobiste obok
organizacji, cztery sposoby dzielenia projektu, zapraszanie kogoś, kto jeszcze
nie ma żadnego konta, koniec projektu w czasie, i przenoszenie konfiguracji
między projektami.

Cel bliski jest jeden i jest wąski: **kilka projektów demo dla klientów, każdy
klient widzi swój i tylko swój**, śledzi w nim agentów, robi laby i trenuje
modele. Sekcja 5 zaczyna się od tego; reszta jest kształtem, do którego to
prowadzi.

---

## 1. Co już stoi — i czego w związku z tym nie trzeba budować

Najważniejsza rzecz w tym planie: **większość tego, o co chodzi, już jest**.

| Potrzeba | Gdzie to jest dzisiaj |
|---|---|
| Projekt jako granica danych | `ProjectScope { organization, project }` — w kluczu obiektu, na kopercie zdarzenia, w wierszu foldu, w `ExecutionOwnership`, w etykiecie tokenu ingestu |
| Dostęp „dla wyznaczonych osób" | `Grantee::User(Principal)` |
| Dostęp „per zespół" | `Grantee::Team(TeamId)` |
| Dostęp czasowy, z automatycznym przejściem w read-only | `GrantWindow { valid_from, edit_until, read_until }` — po `edit_until` grant **sam** spada do `Viewer`, po `read_until` przestaje się liczyć. „Po czasie tylko do odczytu" **działa już dziś**, na poziomie grantu |
| Zapraszanie kogoś, kto nigdy się nie logował | `Invitation` — jednorazowy token, przechowywany jako `sha256`, plaintext raz, realizowany po SSO w jednej transakcji z grantem i wpisem audytu |
| Odczyt, który odpowiada jednym projektem | **96 ścieżek zakresowych z 278** w `contracts/openapi.json`; **36 rodzin w pełni, 7 częściowo, 31 wyłącznie instancyjnie** (było 78/260 i 31/6/33) |
| Graf workflow, przejścia i raporty ewaluacji w projekcie | Reszta E2 — fold workflow kluczuje po `(projekt, id)`, fold ewaluacji po pierwszym zdarzeniu; `/workflows`, `/workflow-executions`, `/evaluations`, `/evaluation-suites` i `/experiments` serwują jeden router dwa razy, ze strumieniem grafu włącznie |
| Zarządzany przebieg w projekcie | `ProjectDispatcher` w produkcyjnym `spawn` (jedna pętla nadzorcy, wiązanie raz na projekt), zakresowy publikator outboxa, `POST {SCOPE}/evaluation-runs/{id}/start`, zakresowy sweep retencji |
| Strona jednego przebiegu w projekcie | `GET {SCOPE}/executions/{id}`, historia i cztery komendy nad **związanym** magazynem; wszystkie 19 tras instancyjnych nazywających jedno wykonanie odmawia przebiegu projektu |
| Odwołanie, które dosięga otwartego strumienia | SSE/WS pyta o grant co 30 s i zamyka ramką `revoked` |
| Selektor w nagłówku panelu | `apps/panel/src/app/scope-selector.tsx` + `shared/lib/scope.ts` — jedno przepisanie w transporcie, `reach` na każdym obszarze |
| Strona członków, grantów i zaproszeń | `/account/access` |
| Widok warsztatu | `features/learning` — „warsztat to projekt, uczestnik to grant, zapis to zaproszenie" |
| Audyt każdej zmiany uprawnień | `iam_audit`, w tej samej transakcji co zmiana, z własną retencją i eksportem |

Czyli: **warsztat z 19.09.2026 da się dziś poprowadzić** — jeden projekt, granty
z oknem kończącym się o 17:00, uczestnicy przechodzą w read-only sami. Brakuje
trzech rzeczy: żeby uczestnik miał konto bez zakładania mu go ręcznie, żeby nie
widział **strony nieprzypisanej** wdrożenia, i żeby projekt dało się potem
zamknąć. Cudzych danych **projektowych** już nie widzi: to jest różnica, którą
IAM-02/D zrobiło, i jest zmierzona trzema pytaniami matrycy zadanymi po
odebraniu grantu.

---

## 2. Czego brakuje, po nazwie

| Czego chcesz | Czego nie ma | Gdzie by to było |
|---|---|---|
| Konto osobiste jako gospodarz projektu | `Organization` nie ma rodzaju; tworzenie organizacji to operacja administratora instancji | `iam::model`, `iam::policy`, jedna trasa |
| „Projekt dostępny dla wszystkich w organizacji" | `Grantee` to `User \| Team` i nic więcej | jedno ramię w `policy::effective_role` |
| Dzielenie **poza** organizację | Zrealizowanie zaproszenia czyni z kogoś **zwykłego członka** organizacji | `OrganizationRole::Guest` |
| Klient nie widzi **strony nieprzypisanej** | Każdy, kto się zaloguje, ma co najmniej `Viewer` — a `Viewer` czyta wszystkie trasy instancyjne. Od IAM-02/D **żadna z nich nie odpowiada wierszem projektu**, więc to, co zostaje, to własne dane wdrożenia — nadal za dużo dla klienta | `RoleMapping`, `Identity::role`, `Caller` |
| Konto zakładane z linku zaproszenia | Zaproszenie zakłada, że człowiek **już** ma konto w IdP | nowy port provisioningu + adapter authentika |
| Koniec projektu: read-only albo usunięcie | `Project` ma tylko `scope` i `name` | `iam::model`, `IamStore::access`, zadanie kasujące |
| Klonowanie konfiguracji między projektami | Nic; `aiwatcher-migration` umie tylko global → projekt, całymi rodzinami | `aiwatcher-migration` |
| Token ingestu na projekt, wydawany z panelu | Token ingestu to zmienna środowiskowa i restart | `IamStore`, `Authenticator` |
| Kolektor artefaktów projektu | Bajty rosną i nic ich nie zbiera; pass kasujący to, czego nie nazywa **globalna** historia, skasowałby bajty projektu | `aiwatcher-server/src/execution` — otwarta bramka M6 |
| Retencja deklaracji, ustawień sędziego i trzymanych odpowiedzi | Rosną z liczbą pytań na deklarację i nie mają własnej polityki | `aiwatcher-evaluation` — otwarta bramka M6 |
| Trasy artefaktów w projekcie | `GET {SCOPE}/executions/{id}/artifacts` nie istnieje; instancyjna **odmawia** przebiegu projektu (zamiast kłamać pustą listą), więc członek projektu nie czyta swoich wierszy | `aiwatcher-api/src/artifacts.rs` + `for_project` na porcie `ArtifactCatalog` |

---

## 3. Dziewięć decyzji

Każda jest napisana tak, żeby dało się ją odrzucić osobno. Trzy pierwsze
decydują o kształcie; czwarta jest zawiasem całego demo.

### D1 — konto osobiste to organizacja jednoosobowa, a `ProjectScope` się nie rusza

`ProjectScope` to dwa uuid-y. Ten kształt jest dziś w kluczu S3, na kopercie
zdarzenia, w wierszu read modelu, w `ExecutionOwnership`, w etykiecie tokenu
ingestu, w parametrze ścieżki 78 tras, w `?scope=` panelu, w PostgreSQL i w
czterech SDK. **Trzeci rodzaj gospodarza podwoiłby każdą decyzję `Global |
Project` w systemie** i uczyniłby z niej trójwartościową.

Więc: `Organization` dostaje `kind: shared | personal` (pole addytywne, czytane
jako `shared` z dokumentów zapisanych wcześniej). Organizacja osobista:

- powstaje **sama, przy pierwszym logowaniu**, z jednym właścicielem;
- jest dokładnie jedna na parę `(provider, subject)` — unikalność wymuszona
  w magazynie, nie w kodzie wywołującym;
- nie przyjmuje `SetMember` na `member`/`admin`/`owner` — **tylko gości**, czyli
  dokładnie to, czym jest dzielenie na zewnątrz (D3);
- nie ma zespołów (zespół to konstrukcja organizacji dzielonej);
- poza tym **jest zwykłą organizacją**: projekty, granty, zaproszenia, okna,
  audyt, cykl życia i klonowanie działają bez jednej linii warunku.

Koszt: jedno pole, jeden bootstrap przy logowaniu, jedna reguła w `policy.rs`.
Zysk: żadna warstwa poniżej IAM nie uczy się nowego słowa.

Tworzenie organizacji **dzielonej** zostaje operacją administratora instancji
(dziś `caller.require(Role::Admin)` w `iam.rs:186`). Tworzenie osobistej jest
samoobsługowe, bo nie da się inaczej: to jest twoja własna szuflada.

### D2 — „cała organizacja" to grantee, nie flaga na projekcie

> **Jedno miejsce do dopisania ramienia, poza polityką.**
> `IamAuthority::audience` w `crates/aiwatcher-migration/src/authority.rs`
> wypisuje żywe granty na projekcie docelowym i ma dwuramienny `match` na
> `Grantee` (`oidc:subject`, `team:<id>`). Nowy wariant jest tam **błędem
> kompilacji**, nie cichym pominięciem — i trzeba zdecydować, jak
> organizacja-grantee czyta się w pokwitowaniu migracji, bo to jest zdanie
> „kto sięgnie po skopiowane dane".

`Grantee::Organization` — „każdy **członek** tej organizacji, w tej roli, w tym
oknie". Jedno ramię w maksimum po aktywnych grantach, i wszystko, co już
działa, działa dalej: okno, odwołanie, `ProjectAccess.grants` pokazujące każde
źródło z osobna, panel mówiący, skąd ktoś ma dostęp.

Flaga `public: bool` na projekcie byłaby drugą implementacją polityki obok
grantów, bez okna i bez śladu w audycie o tym, kiedy ją włączono.

**Gościa to nie sięga** — i to jest cały sens D3.

### D3 — dzielenie poza organizację to gość, a gość nie jest członkiem

Dziś zrealizowanie zaproszenia czyni z kogoś **zwykłego członka organizacji**.
Dla klienta zaproszonego do jednego projektu demo to jest źle: klient staje się
kolegą w rosterze i dosięgnie go każdy przyszły grant „cała organizacja".

Więc `OrganizationRole::Guest`, **poniżej** `Member`. Gość:

- trzyma granty projektowe i nic poza tym;
- **nie jest członkiem** — `Grantee::Organization` go nie sięga, zespół go nie
  przyjmuje, roster pokazuje go w osobnej sekcji;
- widzi organizację w selektorze (inaczej nie miałby jak wejść do projektu),
  a `projects()` pokazuje mu dokładnie to, na co ma grant;
- awansuje wyłącznie jawnym `SetMember`, nigdy ścieżką zaproszenia.

`InvitationOffer` dostaje `standing: guest | member`, **domyślnie `guest`**.
Domyślna wartość idzie w stronę węższą, bo żadne wdrożenie jeszcze z tego nie
korzysta, a zaproszenie, które chce kolegi, może to powiedzieć.

### D4 — instancyjny odczyt wymaga roli instancyjnej

To jest zawias całego demo i jedyna rzecz, której dziś **nie da się obejść
konfiguracją**.

Stan faktyczny: `RoleMapping::resolve` daje `default_role` (domyślnie `Viewer`)
każdemu, kto nie trafił w żadną grupę; `AIWATCHER_AUTH_DEFAULT_ROLE=none`
**odmawia logowania w ogóle**, a nie wpuszcza bez roli. `Identity::role()` to
`self.roles.iter().max().unwrap_or(Role::Viewer)` — pusta lista **znaczy
Viewer**. A trasy instancyjne przy odczycie nie sprawdzają żadnej roli (reguła
z CLAUDE.md: *„most read routes check no role"*).

Wniosek, i **węższy, niż był przed IAM-02/D** — bo to, co wtedy było najgorszą
połową tego zdania, zostało zamknięte:

* **Cudzych wierszy projektu już nie czyta.** `/api/v1/evaluations`,
  `/api/v1/workflows`, `/api/v1/workflow-executions` i `/api/v1/experiments`
  mają projekt w wierszu i **nie odpowiadają żadnym**; wszystkie 19 ścieżek
  `/api/v1/executions` odmawia przebiegu projektu, bo nieskopowany magazyn go
  nie trzyma. Zmierzone: trzy pytania `scripts/iam-permission-check.py` zadane
  principalowi **po** odebraniu grantu.
* **Nadal czyta stronę nieprzypisaną**, czyli własne dane wdrożenia, przez 31
  rodzin tras bez bliźniaka. Dla demo dwóch klientów to wciąż za dużo, i to
  jest to, co D4 naprawia.

Komentarz w `scripts/authentik-seed.py` mówi o uczestniku *„in no group →
aiwatcher's viewer, sees nothing until granted"* — to nadal jest intencja, nie
zachowanie.

**I jedna zależność w drugą stronę, którą trzeba znać przed napisaniem kroku
3.** Odmowa w `Caller` poza `ScopedRoute` działa tylko wtedy, gdy każda trasa,
której członek projektu naprawdę potrzebuje, **ma** bliźniaka. Przed IAM-02/D
nie miały go `/workflows`, `/evaluations`, `/experiments` ani strona jednego
przebiegu — więc D4 zamknęłoby klienta przed jego własnym grafem, raportem i
przebiegiem. Teraz mają. Trasy artefaktów wciąż nie mają (odmawiają, nie
kłamią pustą listą), więc po D4 członek projektu nie przeczyta artefaktów
swojego przebiegu **żadną** drogą — to jest do zrobienia w tym samym kroku
albo do nazwania na ekranie.

Zmiana, w czterech krokach i w jednym miejscu każdy:

1. `AIWATCHER_AUTH_DEFAULT_ROLE` dostaje wartość **`project`**: zalogowany,
   bez roli instancyjnej. (`none`/`off` zostaje tym, czym jest — odmową
   logowania — bo zmiana jego znaczenia zmieniłaby ciche zachowanie wdrożenia,
   które już go ustawiło.)
2. `Identity::role()` → `Option<Role>`; pusta lista przestaje znaczyć `Viewer`.
   Bezpieczne, bo **każdy dzisiejszy konstruktor tożsamości podaje rolę jawnie**:
   ingest `[Editor]`, attempt `[Editor]`, local `[self.role]`, anonim
   `[Admin]`, proxy i sesja z mapowania grup.
3. `Caller::from_request_parts` odmawia tożsamości bez roli instancyjnej na
   trasie, która **nie** jest `ScopedRoute`. Jedno miejsce, ten sam znacznik,
   który `project_scope::resolve_required` już czyta — **żadnej tablicy ścieżek
   w middleware**, bo to jest dokładnie to, przed czym ostrzega CLAUDE.md.
4. Wyjątki, wąskie i nazwane: `auth::is_public` (sondy i logowanie) oraz
   rodzina `/api/v1/iam` — ta autoryzuje **sama siebie**, per principal, i bez
   niej selektor nie miałby czego pokazać (`iam::context` nie wymaga dziś
   żadnej roli instancyjnej, i słusznie).

Test, który to trzyma: **każda ścieżka z `contracts/openapi.json`, zapytana
przez principala bez roli instancyjnej i bez grantu, nie zwraca danych.**
Matryca po kontrakcie, nie po liście pisanej ręcznie — tak jak `SCOPED_ROUTES`
w panelu jest brana z kontraktu i sprawdzana testem.

**Dla istniejących wdrożeń to nie zmienia nic**, dopóki ktoś nie ustawi
`project`. To jest warunek, żeby ta zmiana mogła wejść pierwsza.

### D5 — zaproszenie zakłada konto w IdP, a hasła aiwatcher nie widzi

Wybrana ścieżka: **link generuje aiwatcher, konto powstaje w authentiku**,
klient klika i podaje swoje dane. Grant nadal wiąże się ze zweryfikowaną parą
`(provider, subject)` po SSO — czyli żadna reguła tożsamości się nie zmienia,
a dochodzi jeden adapter wychodzący.

```
1. POST .../projects/{p}/invitations            → token, raz (jest dziś)
2. link:  https://<host>/invite#<token>
   Fragment — nie ścieżka i nie query. Fragment nie trafia na serwer ani do
   jego logu dostępu; panel czyta go z location.hash i chowa w sessionStorage,
   zanim cokolwiek przekieruje.
3. POST /api/v1/invitations/preview  { token }   ← NOWE
   Odpowiada: organizacja, nazwa projektu, rola, okno, termin oferty.
   Nie zużywa oferty. Trzeba tego, bo inaczej człowiek zakłada konto,
   nie wiedząc do czego.
4. „Załóż konto" → aiwatcher tworzy zaproszenie **authentika** przez jego API
   (`/api/v3/stages/invitation/invitations/`, prefill z etykiety oferty)
   i przekierowuje na flow rejestracji: /if/flow/<enrollment>/?itoken=…
   Hasło człowiek ustawia na stronie authentika. aiwatcher nie widzi go
   nawet przez chwilę i nie ma gdzie go zgubić.
5. Powrót → zwykły authorization-code flow (ADR_0013) → ciasteczko aiwatchera.
6. Panel wyjmuje token z sessionStorage → POST /invitations/redeem.
   Grant powstaje dla zweryfikowanej pary. Dokładnie jak dziś.
```

Trzy reguły, które to niosą:

- **Nowe konto nie trafia do żadnej grupy aiwatchera.** Flow rejestracji
  przypisuje grupę pustą albo neutralną. Gdyby przypisywał `aiwatcher-viewers`,
  D4 przestałaby cokolwiek znaczyć — cała izolacja klienta siedzi na tym, że
  nie ma roli instancyjnej.
- **Poświadczenie do API authentika jest wąskie i jest Sekretem.** Konto
  serwisowe, które może tworzyć *zaproszenia*, a nie użytkowników i nie
  członkostwa w grupach. `AIWATCHER_AUTH_PROVISION_URL` + token z Sekretu;
  brak zmiennej to **501 nazywające ją** (reguła domu), a panel pokazuje wtedy
  tylko „zaloguj się, jeśli masz konto".
- **Email w ofercie to nadal wskazówka dostarczenia, nigdy klucz tożsamości.**
  Prefill w authentiku to wygoda; kto zrealizuje ofertę, decyduje token.

Adapter jest jedyną nową rzeczą wychodzącą na zewnątrz i należy tam, gdzie
`integrations::fetch` — z limitem czasu, z poświadczeniem, i z odpowiedzią
traktowaną jako dane, nie jako prawda.

### D6 — warsztat to ta sama ścieżka z licznikiem miejsc i PIN-em

Skoro konto powstaje w authentiku (D5), **warsztat nie potrzebuje drugiego
wystawcy tożsamości**. To jest największe uproszczenie w tym planie: uczestnik
dostaje zwykłe konto, a tymczasowy jest **grant**, nie konto.

Czego brakuje ponad D5:

- **Oferta wielomiejscowa.** `Invitation` jest jednorazowe z założenia i tak ma
  zostać. Obok niego `Pass`: `{ scope, role, window, standing, seats,
  redeemed, expires_at, pin_hash, label }`. Jedna realizacja = jedno miejsce +
  jeden grant + jeden wpis audytu, w jednej transakcji z inkrementacją licznika
  — tak samo jak dziś `redeem` serializuje wyścig o jedną ofertę.
- **PIN przed przekierowaniem do rejestracji.** Link idzie na czat, PIN mówisz
  na głos — to jest cały jego sens. Trzeba powiedzieć wprost, co on daje:
  **bezpieczeństwo niesie limit miejsc, okno i limit prób, a nie sekret.**
  Cztery cyfry bez limitu prób padają w sekundy, więc limit prób per `Pass`
  z narastającą zwłoką i twardą blokadą jest częścią funkcji, nie dodatkiem.
- **Link do odtworzenia.** Oferta jednorazowa pokazuje token raz. Link na
  slajdzie pokazany raz i zgubiony to wrogość wobec prowadzącego, więc
  `POST .../passes/{id}/link` mintuje nowy token dla tej samej oferty i
  unieważnia stary. Nadal nic nie jest trzymane jawnie.
- **Koniec warsztatu już działa**: `edit_until` przełącza wszystkich w
  read-only, `read_until` zamyka. Nie trzeba niczego robić o 17:00.

Jedna rzecz do rozstrzygnięcia świadomie: **konto zostaje w authentiku po
warsztacie.** To jest zaleta (uczestnik może wrócić do swojej pracy, jeśli
dostanie nowy grant) i koszt (konta się zbierają). Jeśli mają znikać —
`Pass` dostaje `deactivate_accounts_at` i ten sam tick, co zamiata retencję,
dezaktywuje w authentiku konta, które powstały z tej oferty. Domyślnie:
nie znikają.

### D7 — cykl życia projektu: read-only to przycięcie w `access`, usunięcie to pokwitowanie

`Project` dostaje `state: active | archived | closed`, `created_at`, oraz
opcjonalne `archive_at` / `delete_at`.

- **`archived`** — `IamStore::access` **przycina rolę do `Viewer`**, niezależnie
  od grantów. Jedno miejsce, przez które i tak przechodzi każda trasa zakresowa
  i każdy dispatcher. Zapis odpowiada odmową nazywającą stan projektu, a nie
  brak uprawnień, bo to są dwie różne rzeczy dla czytającego.
  **Skutek uboczny, który trzeba nazwać, bo plan opisuje `archived` jako
  read-only dla ludzi:** `editor_grant` w `ProjectDispatcher` żąda `Editor`, a
  `ProjectGrant::admits` pyta przed każdym claimem — więc zarchiwizowanie
  **zatrzymuje też zarządzaną pracę**, a próba w locie kończy się `Policy`,
  czyli kończy przebieg zamiast zostawić go claimowanym co przebieg pętli. To
  jest prawdopodobnie pożądane; w każdym razie nie jest przypadkiem i ma być w
  zdaniu, które zobaczy operator archiwizujący projekt z biegnącym pomiarem.
- **`closed`** — `access` odpowiada `NotFound` wszystkim poza administracją
  organizacji, która może otworzyć z powrotem albo skasować. 404 jest tu
  zgodne z tym, co już robi `StoreError::OutOfScope`.
- **`delete`** — zadanie w kształcie [ADR_0022](ADR/ADR_0022_STAGED_IMPORT_JOBS.md):
  po rodzinie, `list(prefix)` + `delete` pod `<prefix>/scopes/<org>/<proj>/`,
  kursor przesuwany po skasowanym shardzie, wznawialne. Kończy się
  **pokwitowaniem**, które mówi też, czego **nie** usunięto i dlaczego:
  - **log zdarzeń** — niezmienny; usuwa go retencja. Ta pozycja stała się
    **realna dopiero po IAM-02/D**: do tej pory fakty projektu w ogóle nie
    trafiały na log, bo globalny publikator nie widział jego wierszy outboxa.
    Teraz trafiają, ze stemplem zakresu założonym przez publikator. To, co
    można zrobić od razu, to przestać je czytać: `project` jest w wierszu foldu
    przebiegów, wymiarów, spanów, okresów, `asked`, `measured`, journala **oraz
    foldów workflow i ewaluacji**, więc pominięcie wierszy zamkniętego projektu
    tego samego dnia jest wykonalne wszędzie tam, gdzie klient by je zobaczył.
  - **VictoriaTraces / VictoriaMetrics** — zakres jedzie tam jako atrybut
    zasobu, same magazyny nie są zakresowane (IAM-02 §6). Pokwitowanie mówi to
    zdaniem, a nie milczeniem.
  - **audyt IAM** — celowo nie kasowany na czyjeś żądanie; ma własny zegar.
- `archive_at` / `delete_at` zamiata ten sam tick, co retencję audytu.

To jest dokładnie „po czasie projekt cały może być usunięty albo tylko
readonly" — z tą różnicą, że read-only na **uczestniku** (okno grantu) już
działa, a to jest read-only na **projekcie**.

### D8 — klonowanie to kopia pod innym zakresem; tożsamość się nie rusza

ADR_0033 już gwarantuje to, co czyni klonowanie łatwym: *„identyczna treść w
dwóch projektach ma ten sam identyfikator wersji i osobne obiekty"*. Czyli
sklonowany prompt, recepta kuracji, scorecard czy lab **mają w nowym projekcie
ten sam content address**, a każda referencja wewnątrz nich nadal rozwiązuje
się do tego samego.

Maszyneria też jest: `aiwatcher-migration` czyta inwentarz od właściciela
rodziny, liczy manifest jako czystą funkcję snapshotu, planuje ponownie przed
wykonaniem i odmawia, jeśli źródło się ruszyło. `Source::with_prefix` przyjmuje
dowolny prefiks, więc **prefiks zakresowy da się nazwać jako źródło już dziś**.

Czego brakuje: wybór pozycji (dziś kopiuje się całymi rodzinami), projekt →
projekt jako pierwszoklasowe źródło, trasa i przycisk. Plus dług, który i tak
jest do spłaty: 4 rodziny wspierane (`annotations`, `datasets`, `prompts`,
`training`), 1 zablokowana (`conversations` — ADR_0021, szyfrogram pod nowym
kluczem się nie otwiera), reszta bez adaptera.

Pierwsza wersja: **projekt-szablon**. Projekt z `template: true`, z którego przy
tworzeniu nowego kopiuje się wspierane rodziny. Recepty i pipeline'y kuracji
siedzą w prefiksie `datasets` — czyli **twój przykład działa w pierwszej
wersji**, a ewaluacje i workflow dochodzą razem z resztą adapterów.

### D9 — token ingestu na projekt jest wydawany, nie konfigurowany

Dziś token ingestu to wpis w `AIWATCHER_AUTH_INGEST_TOKENS` i restart. Projekt
już umie być w etykiecie: `name[queue]@<org-uuid>/<projekt-uuid>=secret`.

Dla trzech–pięciu dem to wystarczy i **tak robimy na start**. Docelowo:
`POST .../projects/{p}/ingest-tokens` — precedens zaproszenia dokładnie:
mintowane w magazynie, trzymane jako `sha256`, plaintext w odpowiedzi raz.
Ścieżka gorąca (`POST /api/v1/events`) nie chodzi do bazy na żądanie —
`Authenticator` trzyma zbiór digestów odświeżany co 30 s, **z nazwanym oknem
odwołania** (ta sama liczba, którą już ma strumień). Rola zostaje `Editor`
na twardo; projekt tylko zawęża.

---

## 4. Czego te decyzje **nie** zmieniają

- `ProjectScope` — dwa uuid-y, ta sama pisownia w trzech miejscach.
- Grant nadal nazywa dokładną parę `(provider, subject)`; email i nazwa nigdy
  nie są kluczem tożsamości.
- Grupy IdP nadal nie są zespołami.
- `ProjectAccess` nadal jest migawką z `evaluated_at`, nie zdolnością do
  trzymania.
- Rola tokenu ingestu nadal jest wbita na `Editor`.
- Zakres nadal nie wchodzi do content hasha.
- Dane globalne zostają tam, gdzie są, pod autoryzacją instancyjną.

---

## 5. Kamienie milowe

### M0 — pierwsze demo klienckie

Cel: dwóch klientów, dwa projekty, jedna organizacja dzielona („AI Spirit"),
każdy widzi swoje i **żadne pytanie nie zwraca cudzego wiersza**.

| # | Co | Gdzie |
|---|---|---|
| 0.1 | **D4**: `AUTH_DEFAULT_ROLE=project`, `Identity::role() -> Option<Role>`, odmowa w `Caller` poza `ScopedRoute`, wyjątki `is_public` + `/iam` | `aiwatcher-auth`, `aiwatcher-api/src/auth.rs` |
| 0.2 | **D3** w minimalnym zakresie: `OrganizationRole::Guest`, `InvitationOffer.standing` z domyślnym `guest`, roster z osobną sekcją | `aiwatcher-iam`, `/account/access` |
| 0.3 | **D5**: `POST /invitations/preview`, link z tokenem we fragmencie, port provisioningu + adapter authentika, strona `/invite` | `aiwatcher-auth`, `aiwatcher-api`, panel |
| 0.4 | Lab mierzony **ścieżką autorską**, nie zarządzaną — patrz niżej. Nic do napisania w Ruście; do napisania jest przykład i opis | `sdk/python`, dokumentacja laba |
| 0.5 | Chart uczy się IAM: `AIWATCHER_IAM_POSTGRES_URL`, własna baza (**nie** ta od workflow store), obraz budowany z `aiwatcher-server/postgres` (build-arg `FEATURES` już jest) | `deploy/helm/aiwatcher` |
| 0.6 | Wdrożenie na `vps`, namespace `planner`, z authentikiem i flow rejestracji, który **nie** nadaje grup aiwatchera | `deploy/`, authentik |
| 0.7 | Panel: co widzi ktoś bez roli instancyjnej — obszar instancyjny mówi „to należy do wdrożenia, nie do ciebie", zamiast pokazywać błąd zapytania | `reach-notice.tsx`, `navigation.ts` |
| 0.8 | Token ingestu per klient z konfiguracji (sufiks `@org/proj`), po jednym na projekt | `AIWATCHER_AUTH_INGEST_TOKENS` |

#### Lab bez projektowego `/start`

To jest jedyne miejsce, gdzie pierwsze demo musi się cofnąć o krok — i lepiej
powiedzieć to teraz niż odkryć w trakcie.

**To się zdezaktualizowało: projektowy `/start` istnieje.**
`POST {SCOPE}/evaluation-runs/{id}/start` stoi od IAM-02/D, razem z
dispatcherem w produkcyjnym `spawn`. Zostawiam tu wyjaśnienie, dlaczego to
**nie** była zwykła trasa do dorobienia — w środku woła
`state.executions().start(plan_for(…))`, czyli jest zarządzanym wykonaniem — bo
to nadal tłumaczy, czemu M6 był największym kamieniem i czemu jego otwarcie
wymagało decyzji, a nie tylko routera.

Co z tego wynika dla M0: **lab ma teraz dwie drogi, nie jedną.**

* **Autorska**, i to nią idzie M0, bo nie wymaga niczego nowego: mark to
  opublikowany `EvaluationResult`, a `{SCOPE}/evaluation-results` przyjmuje
  `POST`. Scorecard, kohorta, nagranie, approval i sam lab też są zakresowe.
  Uczestnik liczy swoją miarę **u siebie** — notebook albo `record_evaluation`
  z SDK, ta sama czwórka co przy tracingu (ADR_0010) — i publikuje wynik do
  swojego projektu, a `GET {SCOPE}/evaluation-results?context_id=…` jest
  „oceny całej grupy", bo `context_id` jest content addressem pinów laba
  (ADR_0034).
* **Zarządzana**, jeśli zechcesz jej użyć w demo: niegenerujący scoring run —
  nagranie albo kohorta rozmów — startuje projektowym `/start` i jest liczony
  przez `ProjectDispatcher`, z sędzią i scorer service włącznie.

Czego **nie** ma do końca M6: generowanie odpowiedzi przez workera (odmawiane
po nazwie: token workera nazywa kolejki, nie projekt) i bramka regresji jako
zarządzany przebieg. Czego nie ma do M7: uruchomienie łańcucha kuracji, bo
silnik zapytań czyta trasy instancyjne bez poświadczenia.

**Bramka M0**: `scripts/iam-permission-check.py` rozszerzone o drugiego klienta
— każde pytanie zadane jako klient A o zasób klienta B odpowiada 404/403, i
każde pytanie zadane jako klient A o **trasę instancyjną** też. Uruchomione
przeciwko wdrożeniu na `vps`, nie tylko lokalnie; pytania, których nie dało się
zadać, raportowane jako **niezadane** — jak dziś z `AIWATCHER_M1_SCOPE`.

**Co klient dostaje po M0** — lista urosła o to, co dowiozło IAM-02/D: Explore
(przebiegi, spany, wymiary), metryki, żywy strumień, prompty, datasety,
anotacje, laby, ewaluacje (scorecards, kohorty, wyniki, review), treningi i
modele, **graf workflow z żywym strumieniem, przejścia, `/experiments`,
zarządzane zmierzenie nagrania lub kohorty rozmów i stronę własnego przebiegu**
— wszystko zakresowe. Recepty i łańcuchy kuracji **pisze** w swoim projekcie;
**uruchamia** je dopiero po M7, bo silnik zapytań czyta trasy instancyjne bez
poświadczenia.
**Czego nie dostaje**: alerty i importy z hubów (M7), artefakty własnego
przebiegu (mała bramka M6 — instancyjna trasa odmawia, zakresowej nie ma),
generowanie odpowiedzi przez workera i bramka regresji (reszta M6).

### M1 — dzielenie wewnątrz organizacji

`Grantee::Organization` (D2), okno dzielenia w panelu nazywające, kogo
dopuszcza („wszyscy członkowie — **bez gości**"), roster z podziałem
członkowie / goście / zespoły.

### M2 — konto osobiste

`Organization.kind` (D1), automatyczne utworzenie przy pierwszym logowaniu,
unikalność per principal, selektor pokazujący „Twoje" nad organizacjami.
Plus D9 — mintowany token ingestu, bo od tego momentu projekt zakłada ktoś,
kto nie ma dostępu do zmiennych środowiskowych.

### M3 — warsztat: oferta wielomiejscowa i PIN

`Pass` (D6), strona dołączania, strona prowadzącego (miejsca, kto dołączył,
kiedy się zamyka), limit prób z blokadą, `POST .../passes/{id}/link`.
Opcjonalnie: dezaktywacja kont po warsztacie.

### M4 — cykl życia projektu

`state` + `archive_at` / `delete_at` (D7), przycięcie w `access`, zadanie
kasujące z pokwitowaniem, sweep na ticku retencji, w panelu: co się stanie
i kiedy.

### M5 — klonowanie i projekt-szablon

D8: źródło zakresowe w `aiwatcher-migration`, wybór pozycji, trasa, przycisk
„Użyj jako szablonu", plus adaptery dla rodzin, które ich nie mają.

### M6 — projektowy `/start` — **w połowie zrobione**

Ten kamień był w planie największy i jedyny blokujący funkcję. **IAM-02/D
zdjęło zakaz i dowiozło `/start` wraz z połową bramek**, więc to, co zostaje,
jest mniejsze i innego rodzaju.

| Bramka z [IAM-01 §3](iam-01-kickoff.md) | Stan | Uwaga |
|---|---|---|
| Zakresowy sweep retencji | **zamknięta** | I taniej, niż plan wyceniał: nie iteruje IAM, tylko `WorkflowStore::project_scopes` — zakresy, nigdy wiersze, odmawiane po nazwie na magazynie związanym |
| Granty na trasach wykonania | **w połowie** | 7 z 19 ścieżek `/executions` ma bliźniaka (odczyt, historia, cztery komendy, `input`); **wszystkie 19** odmawiają przebiegu projektu. Bez bliźniaka zostaje połowa decydenta i workera — celowo, bo poświadczenie workera nazywa kolejki, nie projekt — oraz trasy artefaktów |
| Kolektor artefaktów | **otwarta** | |
| Retencja deklaracji, sędziego, trzymanych odpowiedzi | **otwarta** | |

**Sam `/start` stoi**: `POST {SCOPE}/evaluation-runs/{id}/start`, właściciel z
zakresu, który dopuścił grant, i principala, którego zweryfikowała sesja,
zapisany w tej samej transakcji co przebieg; `ProjectDispatcher` zarejestrowany
w produkcyjnym `spawn` przez jedną pętlę nadzorcy, z sędzią i scorer service.

**Trzeba powiedzieć wprost, że `/start` otwarto przy dwóch otwartych
bramkach.** To była świadoma decyzja IAM-02/D, a nie przeoczenie, i uczciwe
rozróżnienie jest takie: obie otwarte bramki to problemy **przyrostu bajtów**,
nie granicy. Nikt przez nie nie czyta cudzych danych — coś rośnie bez polityki.
Zamknięte bramki były tymi, które dotyczyły granicy i widoczności.

Co **naprawdę** zostaje w M6, w kolejności rosnącego rozmiaru:

1. **Trasy artefaktów w projekcie** — mała. Dziś instancyjna odmawia przebiegu
   projektu (zamiast odpowiadać pustą listą, co czytało się jak „nic nie
   wyprodukował"), więc członek projektu nie czyta swoich artefaktów żadną
   drogą. Potrzebne: `for_project` na porcie `ArtifactCatalog` i bliźniak pary
   tras.
2. **Retencja deklaracji, ustawień sędziego i trzymanych odpowiedzi** — mała.
3. **Kolektor artefaktów** — średnia.
4. **Reszta rodziny `executions`** — tylko jeśli projekt ma mieć workera; dziś
   `RuntimeKind::outside_a_project` odmawia `python_task` i `container_job` po
   nazwie, więc to jest praca **razem z** poświadczeniem workera na projekt, a
   nie przed nim.

Po M6: zarządzana kuracja (po zakresowaniu silnika zapytań — M7), generowanie
odpowiedzi przez workera i bramka regresji w projekcie. Scoring run z sędzią i
scorer service **już działa** w projekcie, o ile jego odpowiedzi są nagraniem
albo kohortą rozmów, a nie generowane przez workera.

### M7 — reszta zakresowania → dopiero wtedy „multi-tenant"

**Trzy pozycje z tej listy odpadły**: foldy workflow i ewaluacji (reszta E2)
oraz `/experiments` są zakresowe od IAM-02/D. Zostaje: alerty, importy anotacji
i huby, silnik zapytań, runtime notebooków, poświadczenia workerów,
harmonogramy, archiwum rozmów, dzierżawa w VictoriaTraces/Metrics i w Persesie.

I zmienił się **rodzaj** tego, co zostaje. Pięć z tych pozycji — silnik
zapytań, runtime notebooków, poświadczenia workerów, harmonogramy i archiwum
rozmów — nie jest już „cicho instancyjnych": są **odmawiane po nazwie**
(`RuntimeKind::outside_a_project`, `StoreError::NotInThisScope`, brak rejestru
i brak trasy), każde z powodem. To jest inny stan niż „niezakresowane" i inna
praca: zamiast szukać, gdzie coś przecieka, czyta się odmowę i zdejmuje ją
jedną po drugiej. Alerty czytają stronę globalną **świadomie** (jeden kanał na
wdrożenie, ADR_0035) — tam decyzja jest do podjęcia, nie do wykonania.

**Dopiero po M7 to wdrożenie wolno opisać jako multi-tenant** — i to jest to
„potem", o którym mówimy. Do tego czasu model jest: „jedna organizacja dzielona,
której właściciel ufa sobie; klienci są gośćmi w swoich projektach i nie mają
roli instancyjnej".

---

## 6. Kolejność i zależności

| Etap | Zależy od | Daje |
|---|---|---|
| M0 | — | Demo klienckie na `vps` |
| M1 | M0 (gość) | Projekt dla całej organizacji |
| M2 | M0 (D4) | Konto osobiste, samoobsługowy token |
| M3 | M0 (D5) | Warsztat z linkiem i PIN-em |
| M4 | M0 | Koniec projektu w czasie |
| M5 | M0 | Konfiguracja przenoszona między projektami |
| M6 | — | Reszta zarządzanego przebiegu: artefakty, kolektor, retencja deklaracji |
| M7 | — | Multi-tenant |

M4 i M5 nie zależą od siebie ani od M1–M3. M7 może iść równolegle od początku,
bo to jest praca na foldach, nie na kontroli dostępu.

**M6 przestał być największym kamieniem i przestał cokolwiek blokować.**
Zarządzane mierzenie w projekcie działa od IAM-02/D; to, co zostaje, to cztery
mniejsze rzeczy, żadna nie na ścieżce krytycznej demo — i żadna nie zależy już
od M0. **Największym i jedynym blokującym jest teraz D4 w M0**: dopóki
zalogowany bez grantu czyta stronę nieprzypisaną, nie ma uczciwego demo dla
dwóch klientów, i żadna dalsza praca tego nie zastąpi.

---

## 7. Ryzyka, nazwane zawczasu

- **M6 jest większy, niż wygląda.** Biblioteka stoi i ma testy, ale cztery
  bramki z IAM-01 §3 to praca na retencji, kolektorze artefaktów i 19 trasach
  rodziny `executions`. Jeśli zarządzane mierzenie ma być w demie, M6 musi
  ruszyć **razem** z M0, nie po nim.
- **D4 jest zmianą semantyki pustej listy ról.** Bezpieczna, bo każdy dzisiejszy
  konstruktor podaje rolę jawnie — ale to jest twierdzenie do sprawdzenia
  testem, nie do przyjęcia na słowo.
- **Flow rejestracji w authentiku nadający grupę zabija izolację.** Cała
  szczelność klienta siedzi na tym, że nie ma roli instancyjnej. To jest
  konfiguracja poza tym repozytorium i dlatego jest najłatwiejsza do zepsucia
  cudzą ręką. Bramka M0 musi to sprawdzać po **rzeczywistym** zalogowaniu
  świeżo założonego konta.
- **PIN to jeden słaby czynnik.** Bezpieczeństwo niesie limit miejsc, okno i
  limit prób. Jeśli któregoś zabraknie, `Pass` jest otwartymi drzwiami.
- **Poświadczenie do API authentika to nowa zdolność wychodząca.** Konto
  serwisowe musi móc tworzyć zaproszenia i nic więcej; szerszy token czyni
  z aiwatchera narzędzie do zakładania kont w IdP.
- **„Usunięcie projektu" nie usuwa zdarzeń z logu ani śladów w VT/VM.** To musi
  być w pokwitowaniu, które czyta człowiek, a nie w dokumentacji, której nie
  przeczyta.
- **Alerty urodziły się instancyjne**, łamiąc regułę z IAM-02 §7 („nowy zasób
  autorski dostaje swoją zakresową rodzinę tras od urodzenia"). Po M0 klient
  ich po prostu nie ma; dług spłaca M6 — i reguła obowiązuje dalej.
- **Token ingestu to nadal wspólny sekret w środowisku agenta.** Zakres
  zmniejsza promień rażenia wycieku do jednego projektu i nie usuwa go.
- **`auth=none` i `auth=local` zostają trybami jednego najemcy.** Żaden z nich
  nie staje się bezpiecznym trybem wielu klientów dlatego, że powstały goście.

---

## 8. Czego ten plan nie robi

Foldu per najemca. Dzierżawy w VictoriaTraces/Metrics ani w Persesie. Migracji
historii do projektów. Projektowych harmonogramów. Archiwum rozmów (zamknięte
po obu stronach, ADR_0021). Rozliczeń, limitów i kwot. Samoobsługowej
rejestracji bez zaproszenia. Federacji wielu dostawców tożsamości naraz —
`iam_principal` nadal porównuje wystawcę z **jednym** skonfigurowanym.

I żadne z powyższych nie jest powodem, żeby nazwać to wdrożenie multi-tenant
przed M7.

---

## 8a. M0 — co stanęło (19.09.2026)

Kontrakt dla D4 i D5 jest w [ADR_0013](ADR/ADR_0013_SINGLE_SIGN_ON.md), aneks
z 19.09; D3 jest w [README IAM](../crates/aiwatcher-iam/README.md).

### D4, i dlaczego wyszło węziej, niż wyglądało

`AIWATCHER_AUTH_DEFAULT_ROLE` ma czwartą wartość `project`,
`Identity::role()` zwraca `Option<Role>`, a odmowa siedzi w
`Caller::from_request_parts` — poza `ScopedRoute`, czyli czyta ten sam
znacznik co `project_scope::resolve_required`, bo warstwa uwierzytelniająca
biegnie **przed routingiem** i żadnego znacznika nie widzi. Wyjątki są dwie
rodziny: `/api/v1/auth/` (odmowa `me` zgłasza ważną sesję jako wylogowaną) i
`/api/v1/iam/` (autoryzuje sama siebie i bez niej klient nie zobaczy swojego
projektu). Odmowa to **403 `instance_role_required`**, nie 404: trasa jest w
kontrakcie każdego aiwatchera, więc nie ma czego chować, a panel musi odróżnić
„to nie twoje" od „tego wdrożenia nie ma".

Najciekawsze jest to, co zmierzył test. `instance_reach.rs` przechodzi **każdą**
operację z `contracts/openapi.json` i pyta o nią principalem bez roli
instancyjnej. Odmowa w ekstraktorze `Caller` okazała się niemal totalna —
prawie wszystko i tak idzie przez `project_scope::resolve` — ale **piętnaście
tras nie brało `Caller` w ogóle** i odpowiadało dalej: cztery w `imports`,
cztery w `conversations`, para w `artifacts`, trzy w `context`, dwie w
`schedules`, cztery w `hubs`, `execution_timers`, `decider-lease` i
`annotation-sources`. Dostały `InstanceRead` — `Caller` bez tożsamości, nazwany
po tym, co rozstrzyga — i test trzyma teraz dwie własności naraz: nic poza
dwiema rodzinami nie odpowiada, **i** każda operacja instancyjna odmawia
*po granicy*, a nie z powodu źle zgadniętego parametru. To druga własność jest
regułą o handlerach: **caller przed sparsowaniem żądania**.

**Zależność z promptu sprawdzona i zamknięta, nie nazwana na ekranie.** Trasy
artefaktów przebiegu dostały bliźniaka: `for_project` na porcie
`ArtifactCatalog` (i na `AttemptArtifacts`, bo bajty i katalog wiążą się razem
albo wcale), `RunArtifacts` wiążące trzy magazyny z jednej odpowiedzi
`RunHandle`, i para tras pod `{SCOPE}/executions/{id}/artifacts`. Domyślna
implementacja portu **odmawia po nazwie** — adapter bez formy projektowej mówi
to, zamiast odpowiadać stroną wdrożenia (ADR_0033 pkt 7).

### D3 i D5

`OrganizationRole::Guest` jest **poniżej** `Member` (kolejność wariantów to
kolejność, którą czyta każdy check), `InvitationOffer.standing` domyślnie
`guest`, zespół gościa nie przyjmuje, a roster ma dwie listy zamiast jednej z
rolą w wierszu — bo „czy ten człowiek jest stąd" ma być nie do przeczytania
źle, a nie tylko możliwe do przeczytania dobrze.

D5 to dwie trasy publiczne (`preview`, `enrollment`), port provisioningu z
adapterem authentika i strona `/invite` czytająca token z **fragmentu**.
Blueprint dokłada flow `aiwatcher-enrolment` (etap zaproszenia wymagany, żaden
etap nie nadaje grupy), a `authentik-seed` konto serwisowe z dwoma
uprawnieniami: dodać zaproszenie i przeczytać flow. Sprawdzone ręcznie wobec
prawdziwego authentika: token **tworzy zaproszenie (201)**, **nie utworzy
użytkownika (403)**.

### Bramka M0

`scripts/iam-permission-check.py` urosło z 51 pytań do **94** i przebiegło
**94/94**, zero niezadanych, na żywym serwerze (`just authentik-up`,
`authentik-seed`, `postgres-up`, `run-sso-iam`, `panel`, dwa przebiegi z
`AIWATCHER_M1_SCOPE` i `AIWATCHER_M0_SCOPES`). Nowe pytania, po rodzajach:

* klient loguje się **bez żadnej roli instancyjnej**, redeemuje zaproszenie i
  jest w rosterze gościem, nie członkiem;
* przebiegi klienta A to jego przebiegi i **żaden** z klienta B, w obie strony;
* jedenaście rodzin zakresowych zapytanych o projekt drugiego klienta →
  **404**, każda;
* **żadna trasa instancyjna nie odpowiada klientowi** — 60 ścieżek wziętych z
  kontraktu, nie z listy pisanej ręcznie;
* i cały łańcuch D5: oferta czyta swoje warunki komuś **bez sesji**, konto
  powstaje w authentiku przez jego własny flow, wraca **bez roli i bez grupy**,
  a redeem daje dokładnie ten grant, który oferta deklarowała.

Dwa pytania z M1 trzeba było przepisać, i to jest zmiana wartościowa sama w
sobie: pytały o stronę instancyjną **studentem**, który przed D4 miał `viewer`
z samego zalogowania. Teraz pyta `observer` — widz instancji bez żadnego grantu
— czyli principal, o którym te pytania naprawdę są. Trzecie („człowiek spoza
mapowanych grup") pyta o **konsekwencję**, nie o wartość, więc trzyma się
wdrożenia, które wybrało `viewer`, i wdrożenia, które wybrało `project`.

### Wdrożenie na `vps`, i co ono pokazało

Zrobione, i bramka przebiegła tam **94/94, zero niezadanych** — te same pytania
co lokalnie, przeciwko `https://aiwatcher.159.195.240.156.sslip.io`. Kolejność
jest w [runbooku §10](iam-migration-runbook.md); tu to, czego się przy tym
nauczyliśmy, bo każda z tych rzeczy kosztowała jeden nieudany krok:

* **Slug aplikacji jest cudzy.** `aiwatcher` w authentiku na `vps` to aplikacja
  *forward-auth* plannera, deklarowana w blueprintcie plannera. Wzięcie sluga
  zostałoby nadpisane przy następnym deployu plannera, więc SSO aiwatchera
  dostało własny: `aiwatcher-sso`, i to on jest w issuerze.
* **`grant_types` na providerze OAuth2 jest w 2026.x domyślnie puste**, a puste
  odmawia authorization-code z `invalid_request` — co czyta się jak pomyłka w
  redirect URI i nią nie jest. Blueprint wymienia je teraz jawnie.
* **RBAC zmienił kształt**: 2025.6 przypisywało uprawnienie użytkownikowi,
  2026.8 przypisuje je **roli**, a użytkownika wkłada do roli. Konto serwisowe
  dostaje te same dwa uprawnienia jedną i drugą drogą.
* **`helm upgrade --reuse-values` nie zna nowych domyślnych wartości chartu** —
  bierze za bazę wartości ostatniego wydania, więc `secretKeyRef.key` renderował
  się pusty i API serwera odrzuciło Deployment. Klucze mają teraz `| default`
  w szablonie, co jest poprawką dla każdego wdrożenia, nie tylko tego.
* **Outpost przed aiwatcherem zdjęty**, razem z trasą `/outpost.goauthentik.io/`:
  klient nie jest człowiekiem plannera i nie ma po co przechodzić jego polityki
  przed własnym logowaniem aiwatchera.

**Czego nie dowieźliśmy**: `planner-api` nadal nie ma `AIWATCHER_TOKEN`, więc
jego telemetria trafia na 401 — SDK zawodzi otwarcie, więc planner działa, a
ślady nie przychodzą. To jedna zmienna w Deploymencie plannera (sekret
`aiwatcher-ingest`, klucz `AIWATCHER_TOKEN`) i należy do repozytorium plannera.

---

## 9. Prompt — IAM-03/M0: pierwsze demo klienckie

> Pracujesz w repozytorium AIWatcher nad **IAM-03/M0: pierwszym demo
> klienckim** — dwóch klientów, dwa projekty, jedna dzielona organizacja, i
> żadne pytanie nie zwraca cudzego wiersza. Przeczytaj `CLAUDE.md`,
> `docs/iam-03-projects.md` (całość; D3, D4, D5 i M0 to twoja praca),
> `docs/iam-02-data-plane.md` sekcje 9–12, `ADR_0033` z oboma aneksami,
> `crates/aiwatcher-iam/README.md` i `apps/panel/CLAUDE.md`.
>
> **Zacznij od D4, bo to jest zawias.** `Identity::role()` to
> `roles.iter().max().unwrap_or(Role::Viewer)`, `AIWATCHER_AUTH_DEFAULT_ROLE=none`
> **odmawia logowania** zamiast wpuszczać bez roli, a trasy instancyjne przy
> odczycie nie sprawdzają żadnej roli. Skutek: każdy zalogowany czyta stronę
> nieprzypisaną wdrożenia. Cudzych wierszy **projektu** już nie czyta — to
> zamknęło IAM-02/D i jest zmierzone — więc nie szukaj tam przecieku, tylko
> zrób to, czego D4 żąda: czwarta wartość `project`, `role() -> Option<Role>`,
> odmowa w `Caller::from_request_parts` poza `ScopedRoute`, wyjątki `is_public`
> i `/api/v1/iam`. Bez tablicy ścieżek w middleware — znacznik `ScopedRoute`
> już istnieje i czyta go `project_scope::resolve_required`.
>
> **Jedna zależność, którą musisz sprawdzić, zanim napiszesz krok 3.** Odmowa
> poza `ScopedRoute` działa tylko wtedy, gdy każda trasa potrzebna członkowi
> projektu **ma** bliźniaka. Dziś mają go `/runs`, `/spans`, `/dimensions`,
> `/metrics`, `/live`, `/events/stream`, `/workflows`,
> `/workflow-executions`, `/evaluations`, `/experiments` i strona jednego
> przebiegu. **Nie mają** go trasy artefaktów przebiegu — instancyjna odmawia
> przebiegu projektu, więc po D4 członek projektu nie przeczyta swoich
> artefaktów żadną drogą. Albo dorób bliźniaka (`for_project` na porcie
> `ArtifactCatalog`), albo nazwij to na ekranie; nie zostawiaj tego do
> odkrycia.
>
> **Test, który to trzyma, bierze się z kontraktu, nie z listy pisanej
> ręcznie**: każda ścieżka z `contracts/openapi.json`, zapytana przez
> principala bez roli instancyjnej i bez grantu, nie zwraca danych. Ten sam
> wzorzec, co `SCOPED_ROUTES` w panelu.
>
> **Potem D3 i D5**, w tej kolejności: `OrganizationRole::Guest` (klient nie
> staje się członkiem rosteru), a następnie zaproszenie zakładające konto w
> authentiku przez jego API — token we **fragmencie** URL, nie w ścieżce i nie
> w query, żeby nie trafił do logu serwera. Grant nadal wiąże się ze
> zweryfikowaną parą `(provider, subject)` po SSO; żadna reguła tożsamości się
> nie zmienia.
>
> **Reszta M0 to wdrożenie**: chart uczy się `AIWATCHER_IAM_POSTGRES_URL` na
> **własnej** bazie (nie tej od workflow store), obraz z
> `aiwatcher-server/postgres`, wdrożenie na `vps` w namespace `planner`, token
> ingestu per klient z sufiksem `@org/proj`, i zdanie w panelu dla kogoś bez
> roli instancyjnej.
>
> **Co warto zweryfikować.** Temat z M6 ani M7 — projektowy `/start`
> **już istnieje** i działa dla niegenerującego scoring runu, więc lab w demo
> może iść ścieżką autorską (`{SCOPE}/evaluation-results` + `context_id`) albo
> zarządzaną, i jedno i drugie jest gotowe. Nie opisuj wdrożenia jako
> multi-tenant — to jest dopiero po M7. Nie zmieniaj istniejących migracji SQL.
> Nie wypełniaj ekranu danymi zastępczymi.
>
> **Bramka M0**: `scripts/iam-permission-check.py` rozszerzone o **drugiego
> klienta** — każde pytanie zadane jako klient A o zasób klienta B odpowiada
> 404, i każde zadane jako klient A o trasę instancyjną też. Uruchom je
> przeciwko wdrożeniu na `vps`, nie tylko lokalnie; pytania, których nie dało
> się zadać, raportuj jako **niezadane**, nie zaliczone — tak jak dziś działa
> `AIWATCHER_M1_SCOPE`. Lokalnie: `just authentik-up`, `authentik-seed`,
> `postgres-up`, `run-sso-iam`, `panel`, dwa przebiegi skryptu; serwer
> odpowiada na `/readyz`, nie `/health`.
>
> **Testy na koniec**: `./scripts/check.sh`. Uwaga — `comments` jest tam
> **czerwone od dawna** (15 za długich bloków, żaden z tej pracy); trzymaj swoje
> bloki poniżej 25 linii prozy i nie próbuj naprawiać cudzych. Jawnie wypisz,
> czego nie uruchomiłeś.
