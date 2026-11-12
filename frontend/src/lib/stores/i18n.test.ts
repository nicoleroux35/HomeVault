/**
 * i18n — the translate helper and language store.
 *
 * The contract that matters: a missing key falls back to the key itself
 * (never an empty label), placeholders are substituted, and an explicit user
 * choice wins over browser/OS detection. A failure here blanks labels across
 * the whole appliance.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';

async function loadI18n(): Promise<typeof import('$lib/stores/i18n')> {
  vi.resetModules();
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => ({ ok: true, json: async () => ({}) })) as unknown as typeof fetch,
  );
  return import('$lib/stores/i18n');
}

describe('i18n', () => {
  beforeEach(() => {
    localStorage.clear();
    document.documentElement.lang = 'en';
  });

  it('falls back to the key when a translation is missing', async () => {
    const { translate } = await loadI18n();
    expect(translate('missing.key')).toBe('missing.key');
  });

  it('substitutes %name% placeholders', async () => {
    const { translationsStore, translate } = await loadI18n();
    translationsStore.set({ greeting: 'Hello %name%, you have %count% files' });

    expect(translate('greeting', { name: 'Ada', count: '3' })).toBe('Hello Ada, you have 3 files');
  });

  it('prefers the explicit saved language over the browser default', async () => {
    localStorage.setItem('hv-language', 'fr');
    const { languageStore } = await loadI18n();
    expect(get(languageStore)).toBe('fr');
    expect(document.documentElement.lang).toBe('fr');
  });

  it('persists a language choice and updates the document lang', async () => {
    const { languageStore } = await loadI18n();

    languageStore.setLanguage('de');
    expect(get(languageStore)).toBe('de');
    expect(localStorage.getItem('hv-language')).toBe('de');
    expect(document.documentElement.lang).toBe('de');
  });

  it('ignores an unsupported saved value', async () => {
    localStorage.setItem('hv-language', 'it');
    const { languageStore } = await loadI18n();
    expect(get(languageStore)).toBe('en');
  });
});
