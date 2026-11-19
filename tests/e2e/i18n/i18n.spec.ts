/**
 * i18n E2E — the four shipped locales (en / fr / es / de) in a real browser.
 *
 * The app resolves its language from `localStorage['hv-language']` (the
 * explicit choice), then the browser locale, then the host's POSIX locale —
 * see `frontend/src/lib/stores/i18n.ts`. That key, the login field ids
 * (`#username` / `#password`) and the settings language selector (`#language`)
 * are the product surface these tests drive; the expected strings come from
 * `src/i18n/{en,fr,es,de}.toml`, never from a guess.
 */
import { test, expect, request } from '@playwright/test';
import {
  ADMIN_PASS,
  ADMIN_USER,
  apiLogin,
  gotoDesktop,
  openApp,
  type Tokens,
} from '../design/designGates';

const LOCALES = ['en', 'fr', 'es', 'de'] as const;
type Locale = (typeof LOCALES)[number];

/**
 * The string the login call-to-action renders per locale. fr/es/de are the
 * catalog's `sign-in` label verbatim; English is matched case-insensitively
 * because the shipped label is title-cased in the catalog and sentence-cased
 * on the button.
 */
const LOGIN_CTA: Record<Locale, RegExp> = {
  en: /^sign in$/i,
  fr: /^Se connecter$/,
  es: /^Iniciar sesión$/,
  de: /^Anmelden$/,
};

/**
 * `desktop.app_settings_name` per locale — what the Settings window title says
 * (the app name, not `common.settings`; Spanish ships "Ajustes").
 */
const SETTINGS_TITLE: Record<Locale, string> = {
  en: 'Settings',
  fr: 'Paramètres',
  es: 'Ajustes',
  de: 'Einstellungen',
};

/** The login page rendered with the given stored language choice. */
async function gotoLoginIn(page: import('@playwright/test').Page, lang: string) {
  await page.addInitScript((l) => {
    try {
      localStorage.setItem('hv-language', l);
    } catch {
      /* storage unavailable — the assertions below surface it */
    }
  }, lang);
  await page.goto('/');
  await expect(page.locator('#username')).toBeVisible({ timeout: 20_000 });
}

test.describe('i18n — locale rendering', () => {
  let tokens: Tokens;

  test.beforeAll(async () => {
    const ctx = await request.newContext({
      baseURL: process.env.BASE_URL || 'http://localhost:8080',
    });
    tokens = await apiLogin(ctx, ADMIN_USER, ADMIN_PASS);
    await ctx.dispose();
    expect(tokens.token, 'the seeded admin must log in — is the server seeded?').toBeTruthy();
  });

  for (const locale of LOCALES) {
    test(`1.${LOCALES.indexOf(locale) + 1} — ${locale.toUpperCase()} login screen renders its own CTA`, async ({
      page,
    }) => {
      await gotoLoginIn(page, locale);
      await expect(page.locator('button[type="submit"]')).toHaveText(LOGIN_CTA[locale]);
      // The document language is what a screen reader announces.
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    });

    test(`2.${LOCALES.indexOf(locale) + 1} — ${locale.toUpperCase()} Settings window title`, async ({
      page,
    }) => {
      await gotoDesktop(page, tokens, locale);
      await openApp(page, 'settings');
      await expect(page.locator('.hv-window').last().locator('.hv-winhdr-title')).toHaveText(
        SETTINGS_TITLE[locale],
      );
    });
  }

  test('3.1 — no raw i18n key is rendered on the login screen', async ({ page }) => {
    for (const locale of LOCALES) {
      await gotoLoginIn(page, locale);
      const text = (await page.locator('body').innerText()).trim();
      // An unresolved key renders as `section.key` — that is the visible
      // failure mode this gate exists for.
      expect(text, `${locale}: a raw translation key is on screen`).not.toMatch(
        /\b(login|common|wizard)\.[a-z_]{2,}\b/,
      );
    }
  });

  test('3.2 — date formatting follows the locale', async ({ page }) => {
    await page.goto('/');
    for (const locale of LOCALES) {
      const formatted = await page.evaluate((loc) => {
        return new Intl.DateTimeFormat(loc).format(new Date('2024-12-25'));
      }, locale);
      // en is month-first, the three others are day-first (de uses dots).
      if (locale === 'en') expect(formatted).toBe('12/25/2024');
      else if (locale === 'de') expect(formatted).toBe('25.12.2024');
      else expect(formatted).toBe('25/12/2024');
    }
  });

  test('3.3 — number formatting follows the locale', async ({ page }) => {
    await page.goto('/');
    for (const locale of LOCALES) {
      const formatted = await page.evaluate((loc) => {
        return new Intl.NumberFormat(loc).format(1234567.89);
      }, locale);
      if (locale === 'en') {
        expect(formatted).toBe('1,234,567.89');
      } else if (locale === 'de' || locale === 'es') {
        // de-DE and es-ES both group with a dot and use a decimal comma.
        expect(formatted).toBe('1.234.567,89');
      } else {
        // fr-FR groups with a space — U+202F (narrow no-break), not ASCII
        // 0x20, so the assertion matches any space code point.
        expect(formatted).toMatch(/^1\s234\s567,89$/);
      }
    }
  });

  test('3.4 — currency formatting follows the locale', async ({ page }) => {
    await page.goto('/');
    for (const locale of LOCALES) {
      const formatted = await page.evaluate((loc) => {
        return new Intl.NumberFormat(loc, {
          style: 'currency',
          currency: loc === 'en' ? 'USD' : 'EUR',
        }).format(1234.56);
      }, locale);
      expect(formatted).toContain(locale === 'en' ? '$' : '€');
    }
  });

  test('3.5 — an unsupported locale falls back to English', async ({ page }) => {
    // HomeVault ships en/fr/es/de only; `ar` is not a locale it can render, so
    // the choice is ignored and detection falls through to English.
    await gotoLoginIn(page, 'ar');
    await expect(page.locator('button[type="submit"]')).toHaveText(LOGIN_CTA.en);
    await expect(page.locator('html')).not.toHaveAttribute('lang', 'ar');
  });

  test('3.6 — the locale choice survives a reload', async ({ page }) => {
    await gotoLoginIn(page, 'fr');
    await expect(page.locator('button[type="submit"]')).toHaveText(LOGIN_CTA.fr);
    await page.reload();
    await expect(page.locator('#username')).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('button[type="submit"]')).toHaveText(LOGIN_CTA.fr);
  });

  test('3.7 — the Settings language selector switches the whole UI', async ({ page }) => {
    await gotoDesktop(page, tokens, 'en');
    await openApp(page, 'settings');

    const settingsWindow = page.locator('.hv-window').last();
    const saveBtn = settingsWindow.locator('[data-testid="settings-save"]');

    try {
      await settingsWindow.locator('#language').selectOption('fr');
      await saveBtn.click();
      await expect(saveBtn).toHaveAttribute('data-state', /^(saved|clean)$/, { timeout: 15_000 });
      // `saveAll()` calls `languageStore.setLanguage`, so the window title
      // re-renders in the new language without a reload.
      await expect(settingsWindow.locator('.hv-winhdr-title')).toHaveText(SETTINGS_TITLE.fr);
      expect(await page.evaluate(() => localStorage.getItem('hv-language'))).toBe('fr');
    } finally {
      // The choice is persisted server-side too; restore it so the rest of the
      // suite runs against the shipped language.
      await settingsWindow.locator('#language').selectOption('en');
      await saveBtn.click();
      await expect(saveBtn).toHaveAttribute('data-state', /^(saved|clean)$/, { timeout: 15_000 });
    }
  });

  test('3.8 — diacritics are encoded, not mojibake, in every catalog', async ({
    page,
    request: requestFixture,
  }) => {
    // `innerText` reflects `text-transform`, so a CSS-uppercased label loses
    // its lower-case accents on screen ("SÉCURISÉE"). The catalogs are the
    // source of truth, so assert the served values: each locale has at least
    // one accented string and none is double-encoded (Ã / â€ / Ã©).
    const accents: Record<Locale, RegExp> = {
      en: /[A-Za-z]/,
      fr: /[éèêàçùôîï]/i,
      es: /[áéíóúñ¿¡]/i,
      de: /[äöüß]/i,
    };
    for (const locale of LOCALES) {
      const res = await requestFixture.get(`/api/translations?lang=${locale}`);
      expect(res.ok(), `catalog ${locale} must be served`).toBe(true);
      const catalog: Record<string, string> = await res.json();
      const values = Object.values(catalog);
      expect(
        values.some((value) => accents[locale].test(value)),
        `${locale} catalog must contain its own accented characters`,
      ).toBe(true);
      const mojibake = values.filter((value) => /Ã|â€|\uFFFD/.test(value));
      expect(mojibake, `${locale} catalog has double-encoded values`).toEqual([]);
    }

    // And the rendered login screen really does show them: French renders
    // "connexion sécurisée", Spanish "Iniciar sesión".
    await gotoLoginIn(page, 'fr');
    expect((await page.locator('body').innerText()).toLowerCase()).toContain('sécurisée');
    await gotoLoginIn(page, 'es');
    expect(await page.locator('body').innerText()).toContain('sesión');
  });
});
