// Vitest test setup
//
// `@testing-library/jest-dom/vitest` registers the DOM matchers
// (`toBeInTheDocument`, `toHaveAttribute`, `toBeVisible`, …) on Vitest's
// `expect` — without it every component assertion throws "Invalid Chai
// property". `@testing-library/svelte/vitest` unmounts components between
// tests.
import { vi } from 'vitest';
import '@testing-library/jest-dom/vitest';
import '@testing-library/svelte/vitest';

// Mock window.matchMedia
Object.defineProperty(window, 'matchMedia', {
  writable: true,
  value: vi.fn().mockImplementation((query) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(),
  })),
});

// Mock IntersectionObserver
global.IntersectionObserver = class IntersectionObserver {
  constructor() {}
  disconnect() {}
  observe() {}
  takeRecords() {
    return [];
  }
  unobserve() {}
} as any;

// Mock ResizeObserver
global.ResizeObserver = class ResizeObserver {
  constructor() {}
  disconnect() {}
  observe() {}
  unobserve() {}
} as any;
