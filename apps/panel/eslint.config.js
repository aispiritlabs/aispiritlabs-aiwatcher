import js from '@eslint/js';
import globals from 'globals';
import reactHooks from 'eslint-plugin-react-hooks';
import tseslint from 'typescript-eslint';

/**
 * What `npm run lint` means here.
 *
 * `package.json` has carried a `lint` script since the first commit and there
 * has never been a configuration behind it, so this is the first pass this
 * panel has ever had. That decides the shape: a rule the code already keeps is
 * on, and a rule it does not keep is written down here with what it fires on
 * rather than applied to two hundred files — a lint with standing violations
 * is a lint nobody reads the output of, and it would never report the next one.
 *
 * Formatting is not lint's business at all. `.prettierrc` is the authority and
 * `just fmt` is what moves it; nothing here reformats anything, so the two
 * cannot disagree about where a line breaks.
 */
export default tseslint.config(
  {
    // The two generators' output, for the same reason `.prettierignore` holds
    // them: `just openapi` and the router plugin rewrite these files, so
    // anything said about them would have to be said again next build.
    ignores: ['dist', '.tanstack', 'src/api/generated', 'src/routeTree.gen.ts'],
  },

  {
    files: ['**/*.{js,mjs,ts,tsx}'],
    extends: [js.configs.recommended, tseslint.configs.recommended],
    rules: {
      // `_name` is this codebase's "read out of the shape, deliberately
      // unused" — `const { succeeded: _succeeded, ...bucket } = timeline[0]`
      // drops a field to build the older API's answer. `tsc` already reads the
      // prefix that way (`noUnusedLocals`), which is why that code compiles,
      // and a linter that read it differently would be the one disagreeing.
      '@typescript-eslint/no-unused-vars': [
        'error',
        {
          argsIgnorePattern: '^_',
          varsIgnorePattern: '^_',
          caughtErrorsIgnorePattern: '^_',
        },
      ],
    },
  },

  {
    files: ['src/**/*.{ts,tsx}'],
    languageOptions: { globals: globals.browser },
    plugins: { 'react-hooks': reactHooks },
    rules: {
      // The one hook rule that catches a defect rather than describing a
      // migration: a hook behind a condition or a loop is a component that
      // breaks on the render where the condition flips, and there is no way to
      // write it deliberately.
      'react-hooks/rules-of-hooks': 'error',

      // Off, and this is the one switched-off rule that is not about style.
      // It fires twenty times, and every site is deliberate: effects here list
      // the query-object members they actually read (`history.hasNextPage`,
      // `history.isFetchingNextPage`, `history.fetchNextPage`) instead of the
      // object that changes identity on every fetch, and the resets are keyed
      // on an id (`[runId]`) because re-running them on anything else is the
      // bug. Turning it on is a change of its own with twenty call sites to
      // read, not a line in a configuration.
      'react-hooks/exhaustive-deps': 'off',
    },
  },

  // The plugin's own `recommended` carries the React Compiler rule set as well
  // — `refs`, `set-state-in-effect`, `purity`, `preserve-manual-memoization`,
  // `incompatible-library` and nine more — which is why it is not extended
  // above. Those rules describe what the compiler needs in order to memoize a
  // component, and this panel does not run the compiler: there is no
  // `babel-plugin-react-compiler` in `vite.config.ts`. They report 146 findings
  // on code that works, so they are a migration checklist for the day the
  // compiler is switched on, and that day is when they belong in this file.

  {
    files: ['src/**/*.test.{ts,tsx}', 'src/test/**/*.{ts,tsx}'],
    rules: {
      // A test double captures a request body as JSON and the assertion reads
      // into it — `(server.calls.find(…)?.body as Record<string, any>)
      // .variant.experiment_id`. `unknown` cannot be indexed twice, and typing
      // each captured body would be restating the contract the panel's
      // generated client already holds, in the one place that exists to check
      // the panel against it.
      '@typescript-eslint/no-explicit-any': 'off',
    },
  },

  {
    // The build's own files. Everything under `src` runs in a browser; these
    // run in node, and `process.env` is how the dev proxy is pointed.
    files: ['*.config.{js,ts}', 'eslint.config.js', 'scripts/**/*.{js,mjs}'],
    languageOptions: { globals: globals.node },
  },
);
