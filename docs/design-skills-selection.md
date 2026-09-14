# Wybrane skille projektowe

Dobór: 2026-09-13. Kryteria: treść SKILL.md, oryginalny autor, dopasowanie do React 19 + Vite + Tailwind 4, popularność i rozdzielenie odpowiedzialności. To dobór do tych projektów, nie wynik porównawczego benchmarku jakości UI.

| Skill / źródło | Zastosowanie | Instalacje skills.sh (orientacyjnie) |
| --- | --- | --- |
| [frontend-design — Anthropic](https://www.skills.sh/anthropics/skills/frontend-design) | Kierunek wizualny, typografia, układ, treść interfejsu | 882,2 tys. |
| [ui-ux-pro-max — nextlevelbuilder](https://www.skills.sh/nextlevelbuilder/ui-ux-pro-max-skill/ui-ux-pro-max) | Lokalna wyszukiwarka UX, palet, wykresów i wskazówek React | 355,4 tys. |
| [web-design-guidelines — Vercel](https://www.skills.sh/vercel-labs/agent-skills/web-design-guidelines) | Audyt dostępności i zachowania interfejsu | 629,9 tys. |
| [design-system-patterns — wshobson](https://www.skills.sh/wshobson/agents/design-system-patterns) | Tokeny, motywy i biblioteka komponentów | 14,1 tys. |
| [system-design — wondelai](https://www.skills.sh/wondelai/skills/system-design) | Wymagania, przepływy danych, szacowanie obciążenia, niezawodność | 4,9 tys. |
| [vercel-react-best-practices](https://github.com/vercel-labs/agent-skills/tree/main/skills/react-best-practices) | Renderowanie, pobieranie danych, wielkość bundla | Nie odczytywano osobno |
| [vercel-composition-patterns](https://github.com/vercel-labs/agent-skills/tree/main/skills/composition-patterns) | API komponentów i kompozycja React 19 | Nie odczytywano osobno |

## Jak stosować

- Nowy wygląd / większy redesign: frontend-design; UI/UX Pro Max jako źródło konkretnych propozycji, nie drugi niezależny dyrektor artystyczny.
- Wąski problem UX: jeden trafny domain search UI/UX Pro Max, bez generowania całego design systemu.
- Tokeny i motywy: design-system-patterns. Kompozycja komponentów: vercel-composition-patterns. Wydajność: vercel-react-best-practices.
- Audyt gotowej strony: web-design-guidelines. Ten skill pobiera aktualne reguły Vercela przez sieć; przypięcie SKILL.md nie przypina zdalnych reguł. Zapisuj w audycie datę/wersję pobranych zasad.
- Architektura usług: system-design. Punkty jego checklisty są pomocą do rozmowy, nie nakazem dodawania cache, replik czy kolejek; decyzje zależą od rzeczywistego obciążenia i ograniczeń projektu.
- Oba frontendy są SPA Vite. Pomijaj porady o Next.js, RSC, SSR i server actions, o ile osobne zadanie nie zmienia architektury.
- Zachowuj istniejącą tożsamość produktu i komponenty przy zwykłej rozbudowie. Żaden skill nie daje zgody na niezamówiony redesign.

Pliki źródłowe są w `.claude/skills/`; `.agents/skills/` zawiera względne symlinki dla Codexa. Nie instalujemy drugiego globalnego zestawu. Skille są dostępne od następnej wiadomości, gdy aplikacja odświeży katalog.

UI/UX Pro Max: przykłady upstream używają CLAUDE_PLUGIN_ROOT, którego zwykła instalacja repozytoryjna nie musi definiować. Uruchamiaj wyszukiwarkę pełną ścieżką do `<repo>/.claude/skills/ui-ux-pro-max/scripts/search.py`, przez `rtk proxy python3 -B`, np. z argumentami `"keyboard focus modal" --domain ux`. Python -B zapobiega tworzeniu cache w vendored tree.

## Alternatywy i ograniczenia oceny

[Impeccable](https://github.com/pbakaus/impeccable) jest mocną alternatywą do całościowego workflow designu. Sprawdzona wersja 4.3.1 wymaga launchera, który uruchamia dostarczony lub pobierany program. Nie dodano go obok nakładającego się zestawu powyżej.

[Audyt UI/UX Pro Max z 2026-03-10](https://www.skills.sh/nextlevelbuilder/ui-ux-pro-max-skill/ui-ux-pro-max/security/agent-trust-hub) zgłaszał m.in. instalację Pythona przez sudo i persystencję instrukcji. Odczytany aktualny SKILL.md wyklucza instalowanie pakietów i nadpisywanie reguł użytkownika; skontrolowano importy i wrażliwe operacje skryptów wyszukiwania. To ograniczona inspekcja, nie pełny audyt bezpieczeństwa. Przy testach nie używano --persist i nie instalowano zależności.

Repozytorium Vercela w odczytanej wersji nie zawiera osobnego pliku LICENSE. W licenses/vercel-labs_agent-skills.txt zachowano upstream SKILL.md z deklaracją MIT dla React; web-design-guidelines nie ma własnej deklaracji licencji. Nie dopisano mu fikcyjnego tekstu licencji. Pozostałe teksty licencji i pochodzenie zapisano wraz ze skillami.
