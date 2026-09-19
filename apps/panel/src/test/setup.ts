import { afterEach } from 'vitest';
import { cleanup } from '@testing-library/react';

import { client } from '@/api/generated/client.gen';

// The app points the client at its own origin, where a relative URL resolves
// against `location`. jsdom's `Request` refuses one outright, so the tests do
// what `lib/api.ts` does and name an origin — the same wiring, made explicit.
client.setConfig({ baseUrl: 'http://panel.test' });

/**
 * What jsdom does not provide and this panel expects the browser to.
 *
 * `EventSource` is the live stream. Every test here drives the REST path, so
 * the stub only has to exist and do nothing — a component that opens a stream
 * must not fail to mount because of it, and a test that asserted on frames
 * would be testing `lib/live.ts` rather than the component.
 */
class SilentEventSource {
  readonly url: string;
  constructor(url: string) {
    this.url = url;
  }
  addEventListener() {}
  close() {}
}

globalThis.EventSource ??= SilentEventSource as unknown as typeof EventSource;

// `scrollIntoView` is not implemented in jsdom at all. The command panel keeps
// its highlighted row in view with it, and a component must not fail to mount
// because the environment cannot scroll — in a browser this is the real thing.
Element.prototype.scrollIntoView ??= function scrollIntoView() {};

// React Testing Library only unmounts by itself when the test framework's
// `afterEach` is global, and it is not here — imports are explicit in this
// codebase. Without this every test after the first renders into a document
// that still holds the previous one.
afterEach(cleanup);

/**
 * And the same for what a test left in the browser's own storage.
 *
 * jsdom gives one `localStorage` per file, so without this a test that pinned
 * a preference — which of the two places a marimo can be, a collapsed section,
 * a remembered filter — hands it to the next test in that file. That reads as
 * an ordering bug in the component and is not one; it also fails only
 * sometimes, because whether the stale value wins depends on which query
 * settles first.
 */
afterEach(() => {
  try {
    window.localStorage.clear();
    window.sessionStorage.clear();
  } catch {
    // A test that stubbed storage owns its own cleanup.
  }
});
