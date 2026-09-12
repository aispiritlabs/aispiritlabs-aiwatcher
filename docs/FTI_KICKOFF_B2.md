# FTI — kickoff: domknięcie B2 (paczki B2e–B2i)

Przygotowano: 2026-09-12. Repozytorium: `/Users/mkubaszek/Projects/ai_spirit/aiwatcher`.
Poprzedni kickoff — [FTI_KICKOFF.md](FTI_KICKOFF.md) — opisuje wydanie A i siedem
kontynuacji B2, które są już dostarczone. Ten dokument zastępuje jego „następny
zakres”: adaptery źródeł są zrobione, a judge **nie jest** następną rzeczą.

## Cel tej sesji

Doprowadzić trwałe dowody ewaluacji do stanu, w którym B3 ma z czego zbudować
porównanie: instancja trzyma naraz baseline i kandydata, odczyt katalogu nie
kosztuje całego korpusu, protokół publikacji jest dowiedziony na rzeczywistym
magazynie w CI, funkcję da się włączyć chartem, a wynik widać na ekranie.

Rezultat dla użytkownika: w panelu widać opublikowane dowody wraz z ich stanem i
terminem retencji, a dwa warianty jednej suite są czytelne jednocześnie.

## Przeczytaj na początku

1. [CLAUDE.md](../CLAUDE.md) — komendy, architektura, konwencje modułów, kontrakty,
   panel i guardrails. Polecenia shell uruchamiaj przez `rtk`, w razie potrzeby
   `rtk proxy`.
2. [FTI_IMPLEMENTATION_PLAN.md](FTI_IMPLEMENTATION_PLAN.md): etap B punkty 8–11 i
   blok „Zmiany wizualne etapu B” w sekcji 4, tabela paczek w sekcji 5, tabela
   odbioru w sekcji 6, AR2/AR3 w sekcji 7. **Sekcja 17 jest uzasadnieniem** — ma
   wskazania miejsc w kodzie dla każdego punktu niżej.
3. Sekcje 9–16 planu — co jest już dostarczone i czego **nie wolno pisać drugi
   raz**: fasada i kontrakty, trwały zapis, orphan GC oraz pięć adapterów źródeł.
4. [ADR 0030](ADR/ADR_0030_EVALUATION_EVIDENCE.md) i
   [README Evaluation](../crates/aiwatcher-evaluation/README.md) — obowiązujący
   kontrakt i format bundle'a operatora.

Kod w checkoutcie rozstrzyga, co działa. Jeśli od 2026-09-12 coś się zmieniło,
zaktualizuj plan na podstawie różnicy zamiast powtarzać badanie.

## Stan wejściowy

- HEAD w chwili pisania: `ab4d998`. Niezacommitowana jest tylko zmiana
  `docs/FTI_IMPLEMENTATION_PLAN.md` z tego przeglądu.
- **Inna sesja pracuje równolegle** na `crates/aiwatcher-server/src/execution/pods/`
  i `scripts/e2e-pod-steps.py` (w tym nieśledzony `pods/docker.rs`). Sprawdź
  `git status` i diff przed edycją, oddziel te zmiany od własnych i je zachowaj.
  Nie wykonuj `reset`, `clean` ani `stash` całego katalogu.
- Dostarczone i **zamknięte**: B1, trwały zapis B2, orphan GC, adaptery Curation,
  promptów, modeli Training, Annotations i Conversations — z szyfrowaniem kopii,
  rolą Admin i wiążącą retencją.
- Otwarte: B2e–B2i poniżej, potem B3. AR3 jest niezależny i pozostaje warunkiem C0.

## Kolejność realizacji

Kolejność wewnątrz B2e–B2i jest wymienna poza trzema ograniczeniami: B2e
poprzedza B3, B2h następuje po rozstrzygnięciu w ADR, a B2i po B2e i B2f.

### B2e — zatwierdzenie źródła jako zasób

Dziś `LocalSource::resolve` w [evaluation.rs](../crates/aiwatcher-server/src/evaluation.rs)
porównuje publikowany manifest z **jednym** `manifest.json` z katalogu wskazanego
przez `AIWATCHER_EVALUATION_SOURCE_DIR` i odrzuca jako `forbidden` wszystko o
innym `variant_id`/`context_id`. Skutki: instancja trzyma naraz jedną parę
wariant/kontekst, podmiana katalogu ukrywa wcześniejsze wyniki, a każda
publikacja z wykonania wymaga człowieka na hoście.

Zrób z zatwierdzenia wersjonowany zasób: wiele przypięć naraz, adresowanych
treścią, z zapisem kto i kiedy zatwierdził oraz z wycofaniem. Właścicielem jest
Evaluation; adapter nadal **nie czyta prywatnych kluczy** innych rejestrów.
Rozstrzygnij przy tym usunięcie pojedynczego dowodu: albo trasa z uprawnieniem,
albo zapisane w ADR 0030 stwierdzenie, że dowód znika wyłącznie przez usunięcie
źródła i retencję — dziś nie ma `DELETE`, a dla źródeł `external` nie ma czego
usunąć.

**Pułapka:** dodanie drugiego zatwierdzenia nie może ukryć pierwszego. Wycofanie
ma nadal ukrywać i ta ścieżka jest już przetestowana — zachowaj ją.

**Odbiór:** baseline i kandydat czytelni jednocześnie; trzecie zatwierdzenie nie
rusza dwóch wcześniejszych; wycofanie nadal ukrywa bez przedłużania retencji;
publikacja z workera nie wymaga kroku na hoście.

### B2f — koszt odczytu i porządek katalogu

W [registry.rs](../crates/aiwatcher-evaluation/src/registry.rs): `get` woła
`read_metadata`, które przechodzi po **wszystkich** shardach i odrzuca ich treść,
żeby zwrócić nagłówek; `cases` woła `get`, a potem czyta jeszcze swoją stronę;
`list` woła `get` dla każdego wiersza strony wraz z `authority.resolve`; `sweep`
robi to co 60 s dla każdego zatwierdzonego claimu.

Rozdziel koszt odczytu od jego zakresu: podsumowanie odpowiada z metadanych,
shard weryfikuje się przy czytaniu jego strony, lista nie weryfikuje źródła dla
każdego wiersza, a sprzątanie korzysta z terminów w receipt. Wybierz porządek
katalogu — klucz to `evaluations/{sha256(id)}/`, więc „najnowsze pierwsze”
wymaga dziś pełnego skanu.

**Pułapka:** to nie jest rozluźnienie gwarancji. Digest sharda nadal sprawdzany,
gdy ten shard jest czytany; stan źródła nadal sprawdzany przy wejściu w
szczegół. Zmierz **przed** i **po**, na wartościach startowych: 10 000
przypadków, 100 MiB, strony po 200, 30 dni.

**Odbiór:** cztery liczby przed i po — podsumowanie, strona przypadków, strona
katalogu, jeden przebieg sprzątania — oraz próg, powyżej którego indeks jest
wymagany.

### B2g — CI, obserwowalność i wdrożenie

- **CI na rzeczywistym magazynie.** Poprawność publikacji i GC opiera się na
  atomowym `create`; na S3 to `If-None-Match: *`, a pamięć używa zamka i plik
  twardego dowiązania — żadne z nich niczego o S3 nie dowodzi. Jedyny test jest
  `#[ignore]` za `AIWATCHER_EVALUATION_TEST_S3_ENDPOINT` w
  [tests/evaluation.rs](../crates/aiwatcher-server/tests/evaluation.rs). Wzorzec
  do skopiowania to zadania `laser` i `workflow-store` w
  [ci.yml](../.github/workflows/ci.yml) — usługa w kontenerze i jawne odpalenie
  `--ignored`. To samo zadanie domyka lukę dla podpisu SigV4 w Prompts.
- **Sprzątanie ma raportować.** `evaluation::spawn` odrzuca licznik zwracany
  przez `sweep` i przy błędzie loguje `warn!`. Sweep psujący się od tygodnia
  wygląda identycznie jak działający.
- **Wdrożenie.** Żadna ze zmiennych `AIWATCHER_EVALUATION_*` nie występuje w
  `deploy/helm`, `docs/INSTALL.md` ani w `justfile`; nie ma receptury
  uruchamiającej trwałą ścieżkę. `evaluations/` jest nowym prefiksem we wspólnym
  object store — to także pytanie o NetworkPolicy i retencję. Dopisz też do
  ADR 0030 akapit o tym, co znaczy odtworzenie tego prefiksu z kopii: claim jest
  niezmienny, więc przywrócenie może wskrzesić porzucone ID albo usunąć tombstone.

### B2h — judge

**Najpierw ADR, potem adapter.** Pięć dotychczasowych adapterów dopuszcza źródło
przez ponowny odczyt bajtów u właściciela; wyniku judge'a nikt ponownie nie
potwierdzi. Przeniesienie tamtej reguły albo judge'a zablokuje, albo zostanie
rozluźnione dla wszystkich pozostałych źródeł. Zapisz w ADR 0030 własną regułę:
konfiguracja przypięta treścią, zbiór kalibracyjny i rozbieżność z ocenami ludzi
jako część dowodu, jawne oznaczenie wyniku jako nieodtwarzalnego przez ponowny
odczyt. Dopiero po tym adapter.

### B2i — panel trwałych dowodów

Dziś trwała ścieżka nie ma ekranu: `listResults`, `getResult` i `getCases` są w
wygenerowanym kliencie i nie woła ich nic poza nim. Rozstrzygnięcia są w bloku
„Zmiany wizualne etapu B” w planie; najkrócej:

- Siedem stanów `EvidenceState` to nie są stany błędu — każdy ma własne zdanie i
  własny następny krok, a `forbidden` ma trzy różne przyczyny. Nieudany odczyt
  nie jest stanem pustym; `forbidden` z powodu roli renderuje się jak w
  Conversations, czyli jako wymaganie roli, nie jako awaria.
- `state: partial` i `status: partial` znaczą co innego i nie mogą być dwiema
  plakietkami z tym samym słowem.
- Termin retencji jest faktem z datą i ma być widoczny przy wyniku.
- Status porównywalności z A3 dostaje kontrolkę zamiast czwartego zdania w
  akapicie; wstrzymana delta ma być widocznie wstrzymana, nie po prostu nieobecna.
- Katalog trwały i lista z projekcji to jedna lista z widocznym pochodzeniem
  wiersza. `useInfiniteQuery` i `VirtualList`, filtry w URL — ale **bez** kontrolki
  okresu, dopóki katalog nie ma porządku czasowego.
- Brak konfiguracji to 501 z nazwą zmiennej, nie pusta lista.

Nie implementuj reguł porównywalności ani walidacji drugi raz w TypeScript i nie
koloruj delt metryk — powód jest zapisany w
[search.ts](../apps/panel/src/features/evaluation/screens/overview/search.ts).

## Zasady implementacji

- Reguły i dane mają jednego właściciela. Nie dopisuj polityk jakości do Core ani
  do prywatnego storage innej domeny; nie sięgaj po prywatne klucze cudzych
  rejestrów — wszystko przez publiczne fasady.
- Utrzymuj kompatybilność odczytu starszych danych. Stare wyniki plaintext
  pozostałych rodzajów źródeł zachowują format i odczyt.
- Nie przywracaj fallbacku do telemetrii po tombstone, odmowie dostępu ani
  uszkodzeniu. Zebrane i wycofane ID pozostają trwale niedostępne.
- Feature panelu nie importuje innego feature; filtry żyją w URL; odpowiedzi
  generowanego klienta czytaj przez `src/shared/lib/result.ts`.
- Po zmianie trasy lub typu w kontrakcie uruchom `rtk just openapi` i zacommituj
  kontrakt razem z klientem; nie edytuj wygenerowanych plików ręcznie.
- Realizuj małe, sprawdzalne paczki. Przeszkoda w jednej nie blokuje pozostałych.
  Istotne odstępstwa zapisuj wraz z powodem.

## Odbiór

Testuj zachowanie, nie obecność kodu. Minimum dla każdej paczki: dwa
zatwierdzenia naraz i trzecie bez szkody dla nich (B2e); cztery liczby przed i po
(B2f); nowe zadanie CI faktycznie zielone, a nie tylko dodane (B2g); reguła
dopuszczenia w ADR przed adapterem (B2h); siedem stanów, dwa „partial”, retencja
i wstrzymana delta na ekranie, w obu motywach i z klawiatury (B2i).

Przed scaleniem `rtk just check`. Przy zmianach panelu `rtk npm run typecheck`,
`rtk npm test`, `rtk npm run build` w `apps/panel`. Nie powtarzaj kosztownych
sprawdzeń bez zmiany lub nowej przesłanki. Nie oznaczaj niesprawdzonego
scenariusza jako PASS; istniejący błąd spoza zakresu odróżnij od własnej regresji
i poprzyj wynikiem.

## Czego nie robić

- Nie implementuj ponownie B1, trwałego zapisu B2, orphan GC ani żadnego z pięciu
  adapterów źródeł.
- Nie oznaczaj B2/AR2 jako ukończonych na podstawie samego adaptera syntetycznego.
- Nie modyfikuj danych działających instancji `:8080` i `:18080`. Zachowaj wersję
  demonstracyjną `demo.segmenter/production` zgodnie z wcześniejszą decyzją
  użytkownika. Do odbioru uruchamiaj własną instancję na własnym porcie i własny
  kontener magazynu, a po odbiorze je zatrzymaj.
- Nie zmieniaj niejawnie polityki promocji modelu ani promptu. AW-5 dostarczyło
  już połączenia oceny z wykonaniem i odmowę `production` dla odrzuconego
  kandydata — nie buduj tego drugi raz.
- Nie buduj B3 przed B2e i B2f.

## Na koniec

Dopisz do planu sekcję 18 ze stanem paczek, wykonanymi sprawdzeniami i
pozostałymi problemami. Przenieś trwałe reguły do `CLAUDE.md` i ADR zamiast
zostawiać je w checkpointach — niezmienniki z sekcji 10–16 nadal tam siedzą.
Załóż kartę na [tablicy specyfikacji](specs/BOARD.md) dla pozostałej części FTI.
Jeśli praca przechodzi do kolejnej sesji, dopisz tutaj krótki checkpoint:
ukończone paczki, zmienione pliki, testy i dokładny następny krok.

Odpowiedz po polsku: co użytkownik już może zrobić, co zmieniono, jak sprawdzono
i jakie ograniczenia pozostały.
