/**
 * Shared Vitest setup.
 *
 * - `@testing-library/jest-dom` adds the DOM matchers (`toBeVisible`,
 *   `toHaveTextContent`, …) on top of Vitest's `expect`.
 * - Testing Library is unmounted after every test, otherwise a component that
 *   subscribed to a store leaks into the next file.
 * - jsdom implements neither `matchMedia` nor `ResizeObserver`; the desktop
 *   shell and bits-ui primitives read both at mount time, so a component test
 *   would crash without these small, behaviour-neutral shims.
 */
import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/svelte';
import { afterEach } from 'vitest';

afterEach(() => {
  cleanup();
});

if (!window.matchMedia) {
  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    value: (query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }),
  });
}

if (!globalThis.ResizeObserver) {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
}

// jsdom does not implement scrolling; the desktop chrome calls it on focus.
if (!Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = () => {};
}
