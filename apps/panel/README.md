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
npm run typecheck
npm test
npm run build
```

The architecture check is also part of `build` and `typecheck`, so future changes
cannot quietly reintroduce dependencies from shared code into individual features
or from feature code into route registration.
