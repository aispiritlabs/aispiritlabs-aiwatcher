# Panel

React panel organized by product capability (vertical slices). Work on a feature
starts in its directory, with its screens, domain logic, components and tests
together.

```text
src/
  app/                    Application shell and navigation
  routes/                 TanStack route registration and redirects
  features/
    annotations/
    conversations/
    data-curation/
      screens/pipeline/
        page.tsx          Screen orchestration and rendering
        search.ts         URL validation and selection contract
      screens/recipe/
      components/         Curation-specific UI and component tests
      lib/                Curation logic and unit tests
    datasets/
    evaluation/
    experiments/
    observability/        Includes run details at /runs/$runId
    prompts/
    training/
    workflows/
  shared/
    components/           UI used across features, including ui/ and charts/
    lib/                  Auth, streaming, service clients and common utilities
  api/generated/          Generated API contract; regenerate rather than edit
  test/                   Common test setup and server helpers
```

## Adding or changing a feature

1. Put a screen in `features/<feature>/screens/<screen>/page.tsx`. Keep components,
   hooks and domain logic local to that screen until another screen needs them;
   then use that feature's `components/` or `lib/` directory.
2. Define URL validation in the screen's `search.ts`. Keep it independent of the
   page so route validation does not eagerly load the screen.
3. Register the page and schema in `routes/`. Read typed URL state in the page
   using `getRouteApi('/the/route')`; never import a route file into a feature.
   The Vite router plugin still splits the registered screen automatically.
4. Keep API requests and query/mutation behavior with the feature that uses them.
   Import generated endpoint functions directly; service-wide configuration and
   reusable clients belong in `shared/lib/`.
5. Put tests next to the implementation. Use `test/` only for common test support.

Features do not import each other. Shared code cannot import features, routes or
the application shell. Promote code into `shared/` only when multiple features
actually use it (for example, prompt references, hub discovery or scheduling).
Use explicit module imports instead of a feature-wide barrel that loads every
screen. Route files own routing; `app/` owns application composition.

## Verification

```sh
npm run check:architecture
npm run lint
npm run typecheck
npm test
npm run build
```

The architecture check is also part of `build` and `typecheck`, so future changes
cannot quietly reintroduce dependencies from shared code into individual features
or from feature code into route registration.

`eslint.config.js` says which rules this panel keeps and why each switched-off
one is off. Formatting is not among them: `.prettierrc` is the authority there
and `just fmt` is what moves it.

## Navigation rollout (UX-09)

`VITE_AIWATCHER_SHELL` is a **build-time** setting. Set it before `npm run build`
or when starting Vite:

| Value | Behaviour |
| --- | --- |
| `user` or unset | All work areas by default; the user can switch to classic section navigation. |
| `classic` | Force classic navigation for this deployment, including users who saved the new layout. |
| `new` | Force all work areas for this deployment. |

For a UI rollback, rebuild/redeploy the panel with `VITE_AIWATCHER_SHELL=classic`.
Returning to `user` restores each user's choice. Both modes use the same router,
screens, authentication gate and API; this flag does not change authorization,
data scope or stored data. The classic layout retains all current work areas.
The UI switch updates only navigation, without remounting the active editor.

Choose the start page and manage pins from **Your work → Navigation preferences**.
The logo and Your work links use `/?start=workspace` to bypass the preferred
start. The preferred start applies only to a bare root entry; deep links,
root query parameters and hashes keep their destination. Updating the preference
does not immediately leave the overview. Navigation layout is also available
above an open view, beside **Pin this view**.

Preferences use `${scope}:navigation`, where `scope` is the existing local-view
boundary for API instance and identity provider/subject. Schema version 1 stores
layout, sidebar state, start page and up to 20 named internal links. Pins retain
the link's filters, revision and hash; they do not save unsaved edits or retain
the underlying data. They remain subject to current server permissions.

When no scoped preferences exist, the old `aiwatcher.sidebar` value seeds the
sidebar state. The original is retained; the new schema is written and read back
on the first preference change. Saved training/evaluation views and appearance
settings keep their existing formats and keys. Malformed or unknown navigation
schemas are left untouched. Storage errors allow session-only preferences with
an explicit message. Storage events synchronize other tabs; writes read the
latest compatible record before merging a change.

**Navigation diagnostics** shows completed moves between areas, grouped by
layout, for the current session. It aggregates at most 100 distinct combinations
in memory, excluding same-area filter changes and blocked moves. It stores no
object IDs, raw URLs, queries or identity, resets on identity change/reload, and
does not send events to a service. Central rollout metrics and automatic cohort
assignment are not implemented by this local diagnostic.
