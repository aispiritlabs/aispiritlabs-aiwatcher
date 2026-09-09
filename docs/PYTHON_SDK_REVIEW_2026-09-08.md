# Ocena SDK według globalnych skilli Python

Zakres: dodane `runtime/`, `worker/`, `workflow.py`, ich testy oraz zmiany
telemetrii i tracera `agentic`. Przegląd z 2026-09-08, następnie poprawki
wdrożone na prośbę użytkownika tego samego dnia.

**Status: R1, R2, R3 oraz wskazany dług zależności i typowania zostały poprawione.**
Opisy reprodukcji niżej zachowano jako zapis stanu sprzed zmian; podane w nich
numery linii odnoszą się do pierwotnego kodu.

## Wdrożone poprawki

- **R1:** nadzorca przechwytuje `BaseException`, zatrzymuje wszystkie pule i
  przekazuje pierwotne wyjątki przez `BaseExceptionGroup` (automatycznie
  `ExceptionGroup` dla zwykłych wyjątków). Rejestruje także nieoczekiwany powrót
  workera i błędy jego zamykania. Testy z rzeczywistymi zadaniami podnoszącymi
  `SystemExit`/`KeyboardInterrupt` sprawdzają wyjście z `serve()`, brak raportu
  `user_code`, zatrzymanie pojemności i odrzucenie dalszego skalowania.
- **R2:** slot jest publikowany dopiero po udanym starcie wątku. Nieudane
  tworzenie/start wątku zamyka utworzonego workera. Błąd startu lub powiększenia
  puli zatrzymuje runtime i opróżnia już uruchomione sloty przed zgłoszeniem
  przyczyny. Cleanup zbiera błędy i wykonuje pozostałe kroki. Testy obejmują
  pierwszy i drugi slot podczas `start()` i `scale()`, zamknięcie własnych
  połączeń HTTP i telemetrii oraz awarię zamykania połączeń.
- **R3:** referencje w atrapie zawierają digest danych. Żądania i odpowiedzi
  atrapy są sprawdzane przez `jsonschema` względem `contracts/openapi.json`,
  także raporty, przydziały i wiersze. Test regresji odrzuca dawną niepełną
  referencję. `jsonschema` i jego stuby są wyłącznie zależnościami deweloperskimi.
- **Czysty model:** deklaracje przeniesiono do `aiwatcher_sdk.task`, a błędy
  zadań do `task_errors`. Stare importy nadal działają. Osobny proces testowy
  potwierdza, że import workflowów i deklaracji nie ładuje HTTP ani workerów.
- **Typy i walidacja:** `Assignment.from_dict` waliduje pola i zagnieżdżony
  JSON; `ArtifactRef`, `JsonObject`, `JsonValue`, `Completed` i `Failed`
  zastępują niekontrolowane słowniki protokołu. Adapter sprawdza również
  wiersze, nazwę wyjścia i odpowiedź settlement. Niepoprawne odpowiedzi są
  błędami protokołu, bez ponowienia raportu lub wysłania sprzecznej porażki.
  `Any` pozostało przy heterogenicznych kolekcjach generycznych zadań.
- **Port i zegar:** `TaskContext` używa `AttemptAPI`; `HttpAttemptAPI` składa
  URL-e i sprawdza dane na granicy HTTP. Wstrzykiwane źródła czasu pozwalają
  sprawdzać deadline i wygaśnięcie lease bez snu. Wspólne scenariusze portu
  uruchamiane są z implementacją pamięciową i adapterem HTTP nad atrapą.

Walidacja końcowa: `just sdk-check` — **274 testy przeszły**, Ruff format/check
i mypy strict bez błędów; `git diff --check` bez uwag.
Test z prawdziwym serwerem Rust i odtwarzaniem workflowu nadal należy do
kolejnego etapu migracji; walidacja schematów nie jest takim testem.

## Użyte instrukcje

W globalnym Claude jest zestaw powiązanych skilli:

- [python-quality](/Users/mkubaszek/.claude/skills/python-quality/SKILL.md): jakość, narzędzia, organizacja.
- [python-architecture](/Users/mkubaszek/.claude/skills/python-architecture/SKILL.md): czysty model, porty, composition root; także `references/layers.md`.
- [python-types](/Users/mkubaszek/.claude/skills/python-types/SKILL.md): inwarianty, ograniczanie `Any`, wersja Pythona.
- [python-testing](/Users/mkubaszek/.claude/skills/python-testing/SKILL.md): testy zachowania, verified fakes; także `references/fakes.md`.
- [python-errors](/Users/mkubaszek/.claude/skills/python-errors/SKILL.md): granice obsługi błędów; także `references/decisions.md`.
- [python-async](/Users/mkubaszek/.claude/skills/python-async/SKILL.md): współbieżność, zamykanie i kontekst.

Zastosowano je z uwzględnieniem zasad repozytorium: SDK wspiera Python 3.11,
więc obecne `TypeVar`/`ParamSpec` są poprawne. Samo istnienie preferencji PEP 695
w skillu nie uzasadnia podniesienia wymagań interpretera. `Runtime` jest świadomie
composition root, więc samo konstruowanie adapterów w tym miejscu nie jest
błędem architektury. API klientów rejestru może podnosić wyjątki; nie wymaga
mechanicznego zastąpienia ich uniami wyników.

## Potwierdzone problemy

### R1 — P1: zakończenie wątku bez powiadomienia runtime’u

Miejsce: `sdk/python/aiwatcher_sdk/runtime/runtime.py:149–157`, uzupełniająco
`_resize:128` i `serve:180–185`.

`Worker` celowo propaguje `BaseException`, ale nadzorujący go runtime łapie
wyłącznie `Exception`. Gdy zadanie wywoła `sys.exit()` lub podniesie
`SystemExit`, wątek kończy pracę, nie ustawiając sygnału zakończenia runtime’u
i nie zapisując awarii. `serve()` pozostaje w oczekiwaniu. Skalowanie do
dotychczasowego rozmiaru nie naprawia sytuacji, ponieważ `_resize()` zalicza
martwy, niewycofany slot do aktywnych.

Reprodukcja przez publiczne API, z prawdziwym wątkiem i atrapą transportu httpx:

```text
zadanie: raise SystemExit(7)
status: desired=1, running=0, draining=0
po scale(concurrency=1): desired=1, running=0, draining=0
liczba claimów nadal 1
close(): bez zgłoszenia błędu
```

Naprawa: nadzorca musi rejestrować każde nieoczekiwane zakończenie slotu,
włącznie z wyjątkami spoza `Exception`, oraz jawnie zatrzymać runtime lub
odtworzyć jego pojemność zgodnie z polityką. Nie zamieniać `SystemExit`
w zwykły błąd kodu zadania. Przy agregacji uwzględnić różnicę między
`ExceptionGroup` i `BaseExceptionGroup`.

### R2 — P2: nieudany start wątku uniemożliwia cleanup

Miejsce: `sdk/python/aiwatcher_sdk/runtime/runtime.py:142–147` i `194–206`.

Slot trafia do `_slots` przed sukcesem `thread.start()`. Jeżeli system odmówi
utworzenia wątku podczas startu lub zwiększania puli, slot zostaje zapisany
jako aktywny. `close()` próbuje wykonać `join()` na nieuruchomionym wątku,
podnosi kolejny wyjątek i nie dochodzi do zamknięcia własnej telemetrii.

Reprodukcja: podstawiono awarię wyłącznie standardowego `Thread.start`,
symulując wyczerpanie zasobów systemowych. Kod SDK nie był zastępowany atrapą.

```text
scale(): can't start new thread
close(): cannot join thread before it is started
własny wątek telemetrii nadal działa
```

Naprawa: uruchamianie slotu powinno mieć rollback, z zamknięciem utworzonego
workera i spójnym stanem puli. Cleanup musi wykonać wszystkie kroki mimo
pojedynczego błędu, a dopiero potem zgłosić zebrane problemy. Potrzebny test
awarii pierwszego i kolejnego slotu podczas częściowo udanego skalowania.

### R3 — P2: atrapa artefaktów nie spełnia kontraktu API

Miejsce: `sdk/python/tests/test_worker.py:69–73`, także fixture wejść w linii 33.

`WorkerApi` zwraca referencję `{name, uri, kind}` bez wymaganego `digest`.
Fixture wejściowa zawiera tylko `name`. W aktualnym `contracts/openapi.json`
`ArtifactRef.required` to `name`, `uri`, `digest`. Atrapa przyjmuje następnie
raport zakończenia bez sprawdzania jego struktury. Test dwóch etapów może więc
przejść dla komunikatów, których rzeczywiste API nie przyjęłoby jako referencji.

To nie jest zarzut wobec użycia `httpx.MockTransport`: podmieniany jest poprawnie
zewnętrzny transport. Problemem jest brak sprawdzenia własnego protokołu.

Naprawa: pełne referencje i kontrola przesyłanych żądań względem kontraktu;
docelowo wspólny zestaw scenariuszy dla atrapy i prawdziwego serwera. Testy
SDK nie powinny samodzielnie deklarować zgodności atrapy ze schematem.

## Dług projektowy według skilli

**Model workflow zależy od infrastruktury wykonania.** Import
`aiwatcher_sdk.workflow` prowadzi przez `worker.task` i inicjalizację pakietu
`worker` do klienta HTTP. W osobnym procesie potwierdzono załadowanie
`httpx`, `tenacity`, `aiwatcher_sdk.api` i `aiwatcher_sdk.worker.worker`.
Warto wydzielić niezależną deklarację zadania i błędy definicji, a następnie
uzależnić od nich worker. Model procesu powinien dać się analizować i testować
bez ładowania transportu. Nie wymaga to kopiowania całej struktury katalogów
hexagonalnej ze skilla.

**`Any` maskuje brak typowanego kontraktu.** `Task[P, R]` poprawnie zachowuje
sygnatury lokalnych wywołań, ale `Assignment.from_dict`, referencje artefaktów
i raporty wykonania nadal operują na `dict[str, Any]`. Dataclass nie waliduje
wartości przypisanych do jej pól. Potrzebne są walidacja na granicy HTTP,
typ referencji artefaktu oraz rozłączne typy raportu sukcesu i porażki.
`Any` dla heterogenicznej kolekcji zadań może być świadomym zatarciem typu;
nie powinien automatycznie oznaczać niekontrolowanej struktury protokołu.

**Nie ma jeszcze portu oddzielającego wykonywanie od transportu.**
`TaskContext` bezpośrednio składa ścieżki HTTP, odnawia lease i przechowuje stan
współbieżności. Mały port obejmujący operacje próby i osobny adapter HTTP
ułatwiłyby testy logiki bez odtwarzania odpowiedzi serwera. Zegar/źródło czasu
również warto wstrzyknąć przy rozbudowie testów lease’u; obecne krótkie deadline’y
oparte na rzeczywistym zegarze zwiększają podatność na obciążenie CI.

## Wynik pierwotnego przeglądu (przed poprawkami)

- `just sdk-check`: **240 testów**, ruff format/check i mypy zakończone sukcesem.
- Dwie reprodukcje awarii wykonano dodatkowo; wszystkie uruchomione zasoby
  posprzątano po zebraniu wyników.
- `ParamSpec`, jawny wybór zadań i `ContextVar` z resetem w `finally` są
  zgodne z praktykami opisanymi w skillach.
- Odróżnienie lease loss od błędu zadania i brak ponawiania niejednoznacznego
  raportu POST są właściwymi decyzjami dla obecnego protokołu.
- Walidacja cykli grafu nie wykonuje funkcji biznesowych.
- Limit lokalnych slotów jest opisany osobno od autoscalingu klastra.

Pierwotnie zalecona kolejność zmian: R1 i R2, potem R3 oraz typowany kontrakt; następnie oddzielenie
modelu definicji i portu wykonania przed dodaniem kompilatora i nowych backendów.
Brak hosted decidera i rejestracji workflowów pozostaje jawnym zakresem kolejnego
etapu, nie nowo wykrytym błędem tego przeglądu.
