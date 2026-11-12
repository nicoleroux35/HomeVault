/**
 * activateOnEnterOrSpace — the WAI-ARIA button keyboard contract shared by
 * every `role="button"` container in the app.
 *
 * Space must be prevented (otherwise the page scrolls while the row opens),
 * key repeats must not re-fire the action, and any other key must be left
 * alone so typing inside a row still works.
 */
import { describe, expect, it, vi } from 'vitest';
import { activateOnEnterOrSpace } from '$lib/utils/activateOnEnterOrSpace';

function keydown(key: string, init: KeyboardEventInit = {}): KeyboardEvent {
  return new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...init });
}

describe('activateOnEnterOrSpace', () => {
  it('activates on Enter and prevents the default action', () => {
    const activate = vi.fn();
    const event = keydown('Enter');

    expect(activateOnEnterOrSpace(event, activate)).toBe(true);
    expect(activate).toHaveBeenCalledTimes(1);
    expect(event.defaultPrevented).toBe(true);
  });

  it('activates on Space (and the legacy Spacebar) without scrolling', () => {
    for (const key of [' ', 'Spacebar']) {
      const activate = vi.fn();
      const event = keydown(key);

      expect(activateOnEnterOrSpace(event, activate)).toBe(true);
      expect(activate).toHaveBeenCalledTimes(1);
      expect(event.defaultPrevented).toBe(true);
    }
  });

  it('ignores key repeats so holding the key fires once', () => {
    const activate = vi.fn();
    const event = keydown('Enter', { repeat: true });

    expect(activateOnEnterOrSpace(event, activate)).toBe(false);
    expect(activate).not.toHaveBeenCalled();
  });

  it('leaves every other key untouched', () => {
    for (const key of ['Tab', 'Escape', 'a', 'ArrowDown']) {
      const activate = vi.fn();
      const event = keydown(key);

      expect(activateOnEnterOrSpace(event, activate)).toBe(false);
      expect(activate).not.toHaveBeenCalled();
      expect(event.defaultPrevented).toBe(false);
    }
  });
});
