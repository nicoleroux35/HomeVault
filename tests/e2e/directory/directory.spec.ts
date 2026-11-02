/* =============================================================================
 * File          : directory.spec.ts
 * Author        : Nicolas Le Roux
 * Created       : 01/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Directory sign-in screen end-to-end tests (Playwright): opening the app,
 *   the health banner (on / off / misconfigured / no local administrator),
 *   saving the connection, the write-only service password, the type
 *   defaults, the attributes hidden behind "Advanced", the group→role rows,
 *   the connection-test panel, and disabling or forgetting a principal.
 *   Then the OpenID Connect half: the two-provider segment and its tab-scoped
 *   Save, the write-only client secret, the provider check, the shared account
 *   list, and the sign-in itself — the login-screen button, the trip to the
 *   provider, the one-shot ticket exchange and every refusal path.
 *
 *   Every backend call is intercepted with `page.route`, so the suite never
 *   needs an LDAP server or an identity provider and the assertions are
 *   deterministic. The real endpoints, auth matrix, validation, sealing and
 *   ID-token verification live in `tests/directory_integration.rs` and
 *   `tests/oidc_integration.rs`; the LDAP pipeline, the PKCE/JWKS code and the
 *   group→role resolution live in the `directory.rs` and `oidc.rs` unit tests.
 *
 * Dependencies:
 *   - none (Playwright)
 *
 * Usage:
 *   npx playwright test tests/e2e/directory/directory.spec.ts
 * =============================================================================
 */

import { test, expect, type Page, type APIRequestContext, type Route } from '@playwright/test';

const BASE = process.env.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = process.env.E2E_ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'E2eSecret123';

const BIND_DN = 'cn=homevault,ou=services,dc=example,dc=com';
const ADMIN_GROUP = 'cn=admins,ou=groups,dc=example,dc=com';
const LDAP_USER_FILTER = '(&(objectClass=person)(uid={username}))';
const AD_USER_FILTER = '(&(objectClass=user)(sAMAccountName={username}))';

interface Tokens {
  token: string;
  csrf: string;
}

async function apiLogin(ctx: APIRequestContext): Promise<Tokens> {
  const res = await ctx.post('/api/auth/login', {
    data: { username: ADMIN_USER, password: ADMIN_PASS },
  });
  const body = await res.json().catch(() => ({}));
  return { token: body?.data?.token || '', csrf: body?.data?.csrf_token || '' };
}

async function gotoDesktop(page: Page, tokens: Tokens) {
  await page.addInitScript((t) => {
    try {
      localStorage.setItem('session-token', t.token);
      localStorage.setItem('csrf-token', t.csrf);
    } catch {
      /* storage unavailable — the test will surface it */
    }
  }, tokens);
  await page.goto('/desktop', { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });
  // Monitor widgets overlap the dock on this layout — hide them so clicks land.
  await page
    .addStyleTag({
      content: '[class*="-monitor"]{display:none!important;pointer-events:none!important}',
    })
    .catch(() => {});
}

/** `[data-testid="directory-page"]` is unique to Directory.svelte. */
function win(page: Page) {
  return page.locator('[data-testid="directory-page"]');
}

/** A freshly seeded server answers `GET /api/whats-new` with `seen: false`, so
 *  the desktop opens a bits-ui modal whose full-screen overlay swallows the
 *  first dock click. Close it if it is there. */
async function dismissWhatsNew(page: Page) {
  const modal = page.locator('[data-testid="whats-new-modal"]');
  if (await modal.isVisible().catch(() => false)) {
    await page.locator('[data-testid="whats-new-dismiss"]').click().catch(() => {});
    await modal.waitFor({ state: 'hidden', timeout: 5000 }).catch(() => {});
  }
}

// ---------------------------------------------------------------------------
// Fixtures — the exact shapes `handlers/directory.rs` returns
// ---------------------------------------------------------------------------

interface GroupRole {
  group: string;
  role: string;
}

interface Settings {
  enabled: boolean;
  provider: string;
  server_url: string;
  start_tls: boolean;
  verify_tls: boolean;
  bind_dn: string;
  has_bind_password: boolean;
  base_dn: string;
  user_filter: string;
  group_filter: string;
  attr_username: string;
  attr_display_name: string;
  attr_email: string;
  attr_member_of: string;
  default_role: string;
  group_roles: GroupRole[];
  cache_ttl_seconds: number;
  updated_at: string | null;
  updated_by: string | null;
}

interface Identity {
  id: number;
  provider: string;
  subject: string;
  username: string;
  display_name: string;
  email: string;
  groups: string[];
  role_id: number;
  disabled: boolean;
  last_login_at: string | null;
  created_at: string;
  updated_at: string;
}

function ldapSettings(): Settings {
  return {
    enabled: true,
    provider: 'ldap',
    server_url: 'ldap://192.168.1.10:389',
    start_tls: false,
    verify_tls: true,
    bind_dn: BIND_DN,
    has_bind_password: true,
    base_dn: 'dc=example,dc=com',
    user_filter: LDAP_USER_FILTER,
    group_filter: '',
    attr_username: 'uid',
    attr_display_name: 'cn',
    attr_email: 'mail',
    attr_member_of: 'memberOf',
    default_role: 'user',
    group_roles: [{ group: ADMIN_GROUP, role: 'admin' }],
    cache_ttl_seconds: 300,
    updated_at: '2026-11-01T09:00:00Z',
    updated_by: 'e2eadmin',
  };
}

function alice(): Identity {
  return {
    id: 12,
    provider: 'ldap',
    subject: 'uid=alice,ou=people,dc=example,dc=com',
    username: 'alice',
    display_name: 'Alice Martin',
    email: 'alice@example.com',
    groups: [ADMIN_GROUP],
    role_id: 2,
    disabled: false,
    last_login_at: '2026-11-01T08:12:00Z',
    created_at: '2026-10-20T08:00:00Z',
    updated_at: '2026-11-01T08:12:00Z',
  };
}

function bob(): Identity {
  return {
    id: 13,
    provider: 'ldap',
    subject: 'uid=bob,ou=people,dc=example,dc=com',
    username: 'bob',
    display_name: 'Bob Durand',
    email: 'bob@example.com',
    groups: [],
    role_id: 1,
    disabled: true,
    last_login_at: null,
    created_at: '2026-10-21T08:00:00Z',
    updated_at: '2026-10-30T17:40:00Z',
  };
}

interface OidcSettings {
  enabled: boolean;
  label: string;
  issuer: string;
  client_id: string;
  has_client_secret: boolean;
  redirect_url: string;
  scopes: string;
  attr_username: string;
  attr_display_name: string;
  attr_email: string;
  attr_groups: string;
  default_role: string;
  group_roles: GroupRole[];
  updated_at: string | null;
  updated_by: string | null;
}

interface SamlSettings {
  enabled: boolean;
  label: string;
  idp_metadata_url: string;
  idp_entity_id: string;
  idp_sso_url: string;
  idp_certificates: string[];
  sp_entity_id: string;
  acs_url: string;
  attr_display_name: string;
  attr_email: string;
  attr_groups: string;
  default_role: string;
  group_roles: GroupRole[];
  updated_at: string | null;
  updated_by: string | null;
}

function oidcSettings(): OidcSettings {
  return {
    enabled: true,
    label: 'Sign in with Authentik',
    issuer: 'https://auth.example.test/application/o/homevault',
    client_id: 'homevault',
    has_client_secret: true,
    redirect_url: 'https://nas.example.test/api/auth/oidc/callback',
    scopes: 'openid profile email',
    attr_username: 'preferred_username',
    attr_display_name: 'name',
    attr_email: 'email',
    attr_groups: 'groups',
    default_role: 'user',
    group_roles: [{ group: 'admins', role: 'admin' }],
    updated_at: '2026-11-01T10:00:00Z',
    updated_by: 'e2eadmin',
  };
}

function samlSettings(): SamlSettings {
  return {
    enabled: false,
    label: 'Sign in with our provider',
    idp_metadata_url: '',
    idp_entity_id: 'https://idp.example.test/metadata',
    idp_sso_url: 'https://idp.example.test/sso',
    idp_certificates: ['MIIC-test-certificate'],
    sp_entity_id: 'https://nas.example.test/api/auth/saml/metadata',
    acs_url: 'https://nas.example.test/api/auth/saml/acs',
    attr_display_name: 'displayName',
    attr_email: 'mail',
    attr_groups: 'memberOf',
    default_role: 'user',
    group_roles: [{ group: 'HomeVault Admins', role: 'admin' }],
    updated_at: '2026-11-01T10:00:00Z',
    updated_by: 'e2eadmin',
  };
}

/** A principal mirrored from the identity provider, not from LDAP. */
function carol(): Identity {
  return {
    id: 21,
    provider: 'oidc',
    subject: 'https://auth.example.test#9f8e7d6c',
    username: 'carol',
    display_name: 'Carol Petit',
    email: 'carol@example.com',
    groups: ['admins'],
    role_id: 2,
    disabled: false,
    last_login_at: '2026-11-01T09:30:00Z',
    created_at: '2026-10-28T08:00:00Z',
    updated_at: '2026-11-01T09:30:00Z',
  };
}

/** A principal mirrored from a SAML identity provider. */
function dave(): Identity {
  return {
    id: 22,
    provider: 'saml',
    subject: 'https://idp.example.test#dave',
    username: 'dave',
    display_name: 'Dave Moreau',
    email: 'dave@example.com',
    groups: ['HomeVault Admins'],
    role_id: 2,
    disabled: false,
    last_login_at: '2026-11-01T07:15:00Z',
    created_at: '2026-10-30T08:00:00Z',
    updated_at: '2026-11-01T07:15:00Z',
  };
}

interface Fixture {
  /** Overrides applied on top of `ldapSettings()`. */
  settings?: Partial<Settings>;
  identities?: Identity[];
  /** The sentence `status.problem` carries when the saved config is unusable. */
  problem?: string | null;
  /** Break-glass count; `0` is the alarming case. */
  localAdmins?: number;
  /** An inline username/password test answer. */
  testReply?: { ok: boolean; message: string; role?: string };
  /** Overrides applied on top of `oidcSettings()`. */
  oidc?: Partial<OidcSettings>;
  /** An OpenID Connect "check" answer; `false` refuses with its message. */
  oidcTestReply?: { ok: boolean; message: string };
  /** Overrides applied on top of `samlSettings()`. */
  saml?: Partial<SamlSettings>;
  /** A SAML metadata read answer; `false` refuses with its message. */
  samlTestReply?: { ok: boolean; message: string };
}

interface Recorder {
  /** Bodies sent to `PUT /api/directory/settings`. */
  settings: any[];
  /** Bodies sent to `POST /api/directory/test`. */
  tests: any[];
  /** `{ id, disabled }` for every principal toggle. */
  toggles: { id: number; disabled: boolean }[];
  /** Paths passed to `DELETE /api/directory/identities/{id}`. */
  deletes: string[];
  /** Make the next mutation fail with this status/message. */
  refuse: { status: number; message: string } | null;
  /** Bodies sent to `PUT /api/oidc/settings`. */
  oidcSettings: any[];
  /** Bodies sent to `POST /api/oidc/test`. */
  oidcTests: any[];
  /** Make the next single sign-on save fail. */
  oidcRefuse: { status: number; message: string } | null;
  /** Bodies sent to `PUT /api/saml/settings`. */
  samlSettings: any[];
  /** Bodies sent to `POST /api/saml/test`. */
  samlTests: any[];
  /** Make the next SAML save fail. */
  samlRefuse: { status: number; message: string } | null;
}

/** Intercepts every directory call and records what the page sent. */
async function mockApi(page: Page, fixture: Fixture = {}): Promise<Recorder> {
  const recorder: Recorder = {
    settings: [],
    tests: [],
    toggles: [],
    deletes: [],
    refuse: null,
    oidcSettings: [],
    oidcTests: [],
    oidcRefuse: null,
    samlSettings: [],
    samlTests: [],
    samlRefuse: null,
  };
  const state = {
    settings: { ...ldapSettings(), ...(fixture.settings ?? {}) } as Settings,
    identities: (fixture.identities ?? [alice(), bob()]).map((i) => ({ ...i })),
    problem: fixture.problem ?? null,
    localAdmins: fixture.localAdmins ?? 1,
  };

  /** The exact `GET /api/directory` document, recomputed after each mutation. */
  function payload() {
    const admins = state.identities.filter((i) => i.role_id === 2).length;
    return {
      settings: state.settings,
      status: {
        enabled: state.settings.enabled,
        usable: state.settings.enabled && state.problem === null,
        problem: state.problem,
        cache: { hits: 4, misses: 1, entries: 3 },
      },
      identities: state.identities,
      counts: {
        total: state.identities.length,
        admins,
        users: state.identities.length - admins,
        disabled: state.identities.filter((i) => i.disabled).length,
      },
      break_glass: {
        local_admins: state.localAdmins,
        policy:
          'A local account always keeps its own password, so directory sign-in can never lock an administrator out.',
      },
    };
  }

  const oidcState = {
    settings: { ...oidcSettings(), ...(fixture.oidc ?? {}) } as OidcSettings,
  };

  /** The exact `GET /api/oidc` document, recomputed after each save. */
  function oidcPayload() {
    const mine = state.identities.filter((i) => i.provider === 'oidc');
    const admins = mine.filter((i) => i.role_id === 2).length;
    return {
      settings: oidcState.settings,
      status: {
        enabled: oidcState.settings.enabled,
        usable: oidcState.settings.enabled && !!oidcState.settings.issuer,
        problem: null,
      },
      counts: {
        total: mine.length,
        admins,
        users: mine.length - admins,
        disabled: mine.filter((i) => i.disabled).length,
      },
      break_glass: {
        local_admins: state.localAdmins,
        policy:
          'A local account always keeps its own password, so single sign-on can never lock an administrator out.',
      },
    };
  }

  const okOidc = (data: any = oidcPayload()) => ({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify({ success: true, data }),
  });

  const samlState = {
    settings: { ...samlSettings(), ...(fixture.saml ?? {}) } as SamlSettings,
  };

  /** The exact `GET /api/saml` document, recomputed after each save. */
  function samlPayload() {
    const mine = state.identities.filter((i) => i.provider === 'saml');
    const admins = mine.filter((i) => i.role_id === 2).length;
    return {
      settings: samlState.settings,
      status: {
        enabled: samlState.settings.enabled,
        usable:
          samlState.settings.enabled &&
          !!samlState.settings.idp_entity_id &&
          !!samlState.settings.idp_sso_url &&
          samlState.settings.idp_certificates.length > 0,
        problem: null,
        pending_requests: 0,
      },
      counts: {
        total: mine.length,
        admins,
        users: mine.length - admins,
        disabled: mine.filter((i) => i.disabled).length,
      },
      break_glass: {
        local_admins: state.localAdmins,
        policy:
          'A local account always keeps its own password, so single sign-on can never lock an administrator out.',
      },
    };
  }

  const okSaml = (data: any = samlPayload()) => ({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify({ success: true, data }),
  });

  const ok = (data: any = payload()) => ({
    status: 200,    contentType: 'application/json',
    body: JSON.stringify({ success: true, data }),
  });
  const refuse = (status: number, message: string) => ({
    status,
    contentType: 'application/json',
    body: JSON.stringify({ success: false, message, data: null }),
  });

  /** `null` keeps the stored password, `''` forgets it, a value replaces it. */
  function applySettings(body: any) {
    const next: any = { ...state.settings };
    for (const key of [
      'enabled',
      'provider',
      'server_url',
      'start_tls',
      'verify_tls',
      'bind_dn',
      'base_dn',
      'user_filter',
      'group_filter',
      'attr_username',
      'attr_display_name',
      'attr_email',
      'attr_member_of',
      'default_role',
      'group_roles',
      'cache_ttl_seconds',
    ]) {
      if (body[key] !== undefined && body[key] !== null) next[key] = body[key];
    }
    if (body.bind_password !== undefined && body.bind_password !== null) {
      next.has_bind_password = body.bind_password !== '';
    }
    next.updated_at = '2026-11-01T12:00:00Z';
    next.updated_by = 'e2eadmin';
    state.settings = next;
  }

  await page.route('**/api/directory**', async (route: Route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const method = request.method();

    if (method === 'GET') {
      return route.fulfill(ok());
    }

    if (recorder.refuse) {
      const { status, message } = recorder.refuse;
      recorder.refuse = null;
      return route.fulfill(refuse(status, message));
    }

    if (method === 'PUT' && path.endsWith('/settings')) {
      const body = request.postDataJSON();
      recorder.settings.push(body);
      applySettings(body);
      return route.fulfill(ok());
    }

    if (method === 'POST' && path.endsWith('/test')) {
      const body = request.postDataJSON();
      recorder.tests.push(body);
      const reply = fixture.testReply ?? { ok: true, message: 'Connected to the directory.' };
      const data: any = { ok: reply.ok, message: reply.message };
      if (reply.role) data.role = reply.role;
      return route.fulfill(reply.ok ? ok(data) : refuse(400, reply.message));
    }

    if (method === 'POST' && path.includes('/identities/') && path.endsWith('/disabled')) {
      const id = Number(path.split('/').slice(-2)[0]);
      const body = request.postDataJSON();
      recorder.toggles.push({ id, disabled: !!body.disabled });
      state.identities = state.identities.map((i) =>
        i.id === id ? { ...i, disabled: !!body.disabled } : i,
      );
      return route.fulfill(ok());
    }

    if (method === 'DELETE' && path.includes('/identities/')) {
      const id = Number(path.split('/').pop());
      recorder.deletes.push(path);
      state.identities = state.identities.filter((i) => i.id !== id);
      return route.fulfill(ok());
    }

    return route.fulfill(ok());
  });

  await page.route('**/api/oidc**', async (route: Route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const method = request.method();

    if (method === 'GET') return route.fulfill(okOidc());

    if (recorder.oidcRefuse) {
      const { status, message } = recorder.oidcRefuse;
      recorder.oidcRefuse = null;
      return route.fulfill(refuse(status, message));
    }

    if (method === 'PUT' && path.endsWith('/settings')) {
      const body = request.postDataJSON();
      recorder.oidcSettings.push(body);
      const next: any = { ...oidcState.settings };
      for (const key of [
        'enabled',
        'label',
        'issuer',
        'client_id',
        'redirect_url',
        'scopes',
        'attr_username',
        'attr_display_name',
        'attr_email',
        'attr_groups',
        'default_role',
        'group_roles',
      ]) {
        if (body[key] !== undefined && body[key] !== null) next[key] = body[key];
      }
      if (body.client_secret !== undefined && body.client_secret !== null) {
        next.has_client_secret = body.client_secret !== '';
      }
      next.updated_at = '2026-11-01T13:00:00Z';
      next.updated_by = 'e2eadmin';
      oidcState.settings = next;
      return route.fulfill(okOidc());
    }

    if (method === 'POST' && path.endsWith('/test')) {
      const body = request.postDataJSON();
      recorder.oidcTests.push(body);
      const reply = fixture.oidcTestReply ?? { ok: true, message: 'Your provider answered.' };
      return route.fulfill(reply.ok ? okOidc() : refuse(400, reply.message));
    }

    return route.fulfill(okOidc());
  });

  await page.route('**/api/saml**', async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const method = request.method();

    if (path.endsWith('/metadata')) {
      return route.fulfill({
        status: 200,
        contentType: 'application/xml',
        body: '<EntityDescriptor entityID="https://nas.example.test/api/auth/saml/metadata" />',
      });
    }

    if (path.endsWith('/test') && method === 'POST') {
      recorder.samlTests.push(request.postDataJSON());
      const reply = fixture.samlTestReply ?? { ok: true, message: 'Your provider answered.' };
      return route.fulfill(
        reply.ok
          ? {
              status: 200,
              contentType: 'application/json',
              body: JSON.stringify({
                success: true,
                data: {
                  entity_id: 'https://idp.example.test/metadata',
                  sso_url: 'https://idp.example.test/sso',
                  certificates: ['MIIC-test-certificate'],
                  warnings: [],
                },
              }),
            }
          : refuse(400, reply.message),
      );
    }

    if (path.endsWith('/settings') && method === 'PUT') {
      const body: any = request.postDataJSON();
      recorder.samlSettings.push(body);
      if (recorder.samlRefuse) {
        const { status, message } = recorder.samlRefuse;
        recorder.samlRefuse = null;
        return route.fulfill(refuse(status, message));
      }
      const next: any = { ...samlState.settings };
      for (const key of [
        'enabled',
        'label',
        'idp_metadata_url',
        'idp_entity_id',
        'idp_sso_url',
        'sp_entity_id',
        'acs_url',
        'attr_display_name',
        'attr_email',
        'attr_groups',
        'default_role',
      ]) {
        if (key in body) next[key] = body[key];
      }
      if (Array.isArray(body.idp_certificates)) {
        next.idp_certificates = body.idp_certificates;
      }
      next.updated_at = '2026-11-02T09:00:00Z';
      next.updated_by = ADMIN_USER;
      samlState.settings = next;
      return route.fulfill(okSaml());
    }

    return route.fulfill(okSaml());
  });

  return recorder;
}

let tokens: Tokens;

test.beforeAll(async ({ playwright }) => {
  const ctx = await playwright.request.newContext({ baseURL: BASE });
  tokens = await apiLogin(ctx);
  if (tokens.token) {
    // Acknowledge the release once, so the What's New overlay never sits
    // between a test and its first dock click.
    await ctx
      .post('/api/whats-new/seen', {
        headers: { 'X-Session-Token': tokens.token, 'X-CSRF-Token': tokens.csrf },
        data: {},
      })
      .catch(() => null);
  }
  await ctx.dispose();
  if (!tokens.token) {
    throw new Error(`Could not log in as ${ADMIN_USER} — seed this account first.`);
  }
});

test.beforeEach(() => {
  test.setTimeout(60_000);
});

async function openDirectory(page: Page) {
  await gotoDesktop(page, tokens);
  await dismissWhatsNew(page);
  await page.locator('.hv-dock-item[aria-label="Applications"]').click();
  const card = page.locator('.lp-card[data-app-id="directory"]');
  await card.waitFor({ state: 'visible', timeout: 10_000 });
  await card.click();
  await page.waitForSelector('[data-testid="directory-page"]', { timeout: 15_000 });
  await expect(win(page).locator('[data-testid="directory-content"]')).toBeVisible({
    timeout: 15_000,
  });
}

// ===========================================================================
// 1. Load and health
// ===========================================================================

test.describe('1 — Load and health', () => {
  test('1.1 — the app opens from the launcher with the saved server and its accounts', async ({
    page,
  }) => {
    await mockApi(page);
    await openDirectory(page);

    await expect(win(page).locator('[data-testid="directory-server-url"]')).toHaveValue(
      'ldap://192.168.1.10:389',
    );
    await expect(win(page).locator('[data-testid="directory-base-dn"]')).toHaveValue(
      'dc=example,dc=com',
    );
    await expect(win(page).locator('[data-testid="directory-bind-dn"]')).toHaveValue(BIND_DN);
    // The stored password is never returned: the field stays empty and says so.
    await expect(win(page).locator('[data-testid="directory-bind-password"]')).toHaveValue('');

    await expect(win(page).locator('[data-testid="directory-identity-12"]')).toContainText('alice');
    await expect(win(page).locator('[data-testid="directory-identity-12"]')).toContainText(
      'Administrator',
    );
    await expect(win(page).locator('[data-testid="directory-identity-13"]')).toContainText('bob');
    await expect(win(page).locator('[data-testid="directory-counts"]')).toContainText(
      '1 administrator(s)',
    );
    await expect(win(page).locator('[data-testid="directory-counts"]')).toContainText(
      '1 disabled',
    );
  });

  test('1.2 — the banner is a live region that reports directory sign-in as on', async ({
    page,
  }) => {
    await mockApi(page);
    await openDirectory(page);

    const status = win(page).locator('[data-testid="directory-status"]');
    await expect(status).toHaveAttribute('role', 'status');
    await expect(win(page).locator('[data-testid="directory-status-on"]')).toContainText(
      'Directory sign-in is on',
    );
    await expect(win(page).locator('[data-testid="directory-break-glass"]')).toContainText(
      '1 local administrator(s) keep their own password',
    );
  });

  test('1.3 — with the integration off, local accounts are explicitly unchanged', async ({
    page,
  }) => {
    await mockApi(page, { settings: { enabled: false } });
    await openDirectory(page);

    await expect(win(page).locator('[data-testid="directory-status-off"]')).toContainText(
      'Directory sign-in is off',
    );
    await expect(win(page).locator('[data-testid="directory-status"]')).toContainText(
      'Everyone signs in with their HomeVault account, exactly as before.',
    );
    await expect(win(page).locator('[data-testid="directory-enabled-toggle"] [role="switch"]')).toHaveAttribute(
      'aria-checked',
      'false',
    );
  });

  test('1.4 — a saved configuration the server cannot use says what is wrong', async ({ page }) => {
    await mockApi(page, {
      problem: 'The server address must start with ldap:// or ldaps://.',
    });
    await openDirectory(page);

    const status = win(page).locator('[data-testid="directory-status"]');
    await expect(win(page).locator('[data-testid="directory-status-problem"]')).toContainText(
      'The saved configuration needs attention',
    );
    await expect(status).toContainText('must start with ldap://');
  });

  test('1.5 — no local administrator left is called out before the admin saves anything', async ({
    page,
  }) => {
    await mockApi(page, { localAdmins: 0 });
    await openDirectory(page);

    await expect(win(page).locator('[data-testid="directory-break-glass"]')).toContainText(
      'No local administrator can sign in while the directory is down',
    );
  });
});

// ===========================================================================
// 2. Saving the connection
// ===========================================================================

test.describe('2 — Saving the connection', () => {
  test('2.1 — saving sends the whole document and keeps the stored password', async ({ page }) => {
    const rec = await mockApi(page);
    await openDirectory(page);

    await win(page)
      .locator('[data-testid="directory-server-url"]')
      .fill('ldaps://dc1.example.com:636');
    // An ldaps:// address is already encrypted, so StartTLS is not offered.
    await expect(
      page.getByRole('checkbox', { name: /Upgrade the connection with StartTLS/ }),
    ).toBeDisabled();

    await win(page).locator('[data-testid="directory-save"]').click();
    await expect.poll(() => rec.settings.length).toBe(1);

    const body = rec.settings[0];
    expect(body.server_url).toBe('ldaps://dc1.example.com:636');
    expect(body.base_dn).toBe('dc=example,dc=com');
    expect(body.group_roles).toEqual([{ group: ADMIN_GROUP, role: 'admin' }]);
    expect(body.cache_ttl_seconds).toBe(300);
    // Untouched: `null` means "keep what is sealed", never an empty password.
    expect(body.bind_password).toBeNull();

    await expect(win(page).locator('[data-testid="directory-server-url"]')).toHaveValue(
      'ldaps://dc1.example.com:636',
    );
    await expect(win(page).locator('[data-testid="directory-footer"]')).toContainText('Saved');
  });

  test('2.2 — typing a password replaces the sealed one; clearing it forgets it', async ({
    page,
  }) => {
    const rec = await mockApi(page);
    await openDirectory(page);

    await win(page).locator('[data-testid="directory-bind-password"]').fill('S3cretPassphrase');
    await win(page).locator('[data-testid="directory-save"]').click();
    await expect.poll(() => rec.settings.length).toBe(1);
    expect(rec.settings[0].bind_password).toBe('S3cretPassphrase');

    // Stored now, so the "forget it" affordance appears — and the field is
    // cleared back to "leave empty to keep it".
    const clear = win(page).locator('[data-testid="directory-clear-password"]');
    await expect(clear).toBeVisible();
    await expect(win(page).locator('[data-testid="directory-bind-password"]')).toHaveValue('');

    await clear.click();
    await expect(win(page).locator('[data-testid="directory-bind-password"]')).toBeDisabled();
    await win(page).locator('[data-testid="directory-save"]').click();
    await expect.poll(() => rec.settings.length).toBe(2);
    expect(rec.settings[1].bind_password).toBe('');
    await expect(win(page).locator('[data-testid="directory-clear-password"]')).toHaveCount(0);
  });

  test('2.3 — a refusal keeps what was typed and shows the server sentence', async ({ page }) => {
    const rec = await mockApi(page);
    await openDirectory(page);

    await win(page).locator('[data-testid="directory-server-url"]').fill('ldap://not a url');
    rec.refuse = {
      status: 400,
      message: 'The server address must start with ldap:// or ldaps://.',
    };
    await win(page).locator('[data-testid="directory-save"]').click();

    await expect(win(page).locator('[data-testid="directory-error"]')).toContainText(
      'must start with ldap://',
    );
    // The form is not reset: nothing was saved, so nothing may be lost.
    await expect(win(page).locator('[data-testid="directory-server-url"]')).toHaveValue(
      'ldap://not a url',
    );
  });

  test('2.4 — switching the directory type swaps the untouched defaults only', async ({ page }) => {
    await mockApi(page);
    await openDirectory(page);

    await win(page).locator('[data-testid="directory-provider"]').selectOption('ad');
    await expect(win(page).locator('[data-testid="directory-user-filter"]')).toHaveValue(
      AD_USER_FILTER,
    );
    await expect(win(page).locator('[data-testid="directory-attr-username"]')).toHaveValue(
      'sAMAccountName',
    );
    await expect(win(page).locator('[data-testid="directory-attr-display-name"]')).toHaveValue(
      'displayName',
    );

    // A value the admin typed is never overwritten by a default.
    await win(page).locator('[data-testid="directory-user-filter"]').fill('(sAMAccountName={username})');
    await win(page).locator('[data-testid="directory-provider"]').selectOption('ldap');
    await expect(win(page).locator('[data-testid="directory-user-filter"]')).toHaveValue(
      '(sAMAccountName={username})',
    );
    await expect(win(page).locator('[data-testid="directory-attr-username"]')).toHaveValue('uid');
  });

  test('2.5 — attribute names and the cache stay behind "Advanced"', async ({ page }) => {
    await mockApi(page);
    await openDirectory(page);

    await expect(win(page).locator('[data-testid="directory-attr-username"]')).toBeHidden();
    await expect(win(page).locator('[data-testid="directory-cache-ttl"]')).toBeHidden();

    await win(page).locator('[data-testid="directory-advanced"] summary').click();

    await expect(win(page).locator('[data-testid="directory-attr-username"]')).toBeVisible();
    await expect(win(page).locator('[data-testid="directory-cache-ttl"]')).toHaveValue('300');
    await expect(win(page).locator('[data-testid="directory-cache-stats"]')).toContainText(
      '4 cached',
    );
  });

  test('2.6 — a mapping row with no group is dropped instead of sent as an empty DN', async ({
    page,
  }) => {
    const rec = await mockApi(page);
    await openDirectory(page);

    await win(page).locator('[data-testid="directory-add-mapping"]').click();
    await expect(win(page).locator('[data-testid="directory-mapping-1"]')).toBeVisible();
    await win(page).locator('[data-testid="directory-save"]').click();

    await expect.poll(() => rec.settings.length).toBe(1);
    expect(rec.settings[0].group_roles).toEqual([{ group: ADMIN_GROUP, role: 'admin' }]);
  });

  test('2.7 — mapping rows can be added and removed', async ({ page }) => {
    const rec = await mockApi(page);
    await openDirectory(page);

    await win(page).locator('[data-testid="directory-add-mapping"]').click();
    await win(page)
      .locator('[data-testid="directory-mapping-group-1"]')
      .fill('cn=helpdesk,ou=groups,dc=example,dc=com');
    await win(page).locator('[data-testid="directory-mapping-role-1"]').selectOption('admin');

    await win(page).locator('[data-testid="directory-mapping-remove-0"]').click();
    await expect(win(page).locator('[data-testid="directory-mapping-1"]')).toHaveCount(0);

    await win(page).locator('[data-testid="directory-save"]').click();
    await expect.poll(() => rec.settings.length).toBe(1);
    expect(rec.settings[0].group_roles).toEqual([
      { group: 'cn=helpdesk,ou=groups,dc=example,dc=com', role: 'admin' },
    ]);
  });
});

// ===========================================================================
// 3. The test panel
// ===========================================================================

test.describe('3 — Testing before saving', () => {
  test('3.1 — the service-account test sends the form as typed and saves nothing', async ({
    page,
  }) => {
    const rec = await mockApi(page);
    await openDirectory(page);

    await win(page).locator('[data-testid="directory-server-url"]').fill('ldap://dc2.example.com');
    await win(page).locator('[data-testid="directory-test-service"]').click();

    await expect.poll(() => rec.tests.length).toBe(1);
    expect(rec.tests[0].settings.server_url).toBe('ldap://dc2.example.com');
    expect(rec.tests[0].username).toBeUndefined();
    expect(rec.settings.length).toBe(0);

    await expect(win(page).locator('[data-testid="directory-test-result"]')).toContainText(
      'Connected to the directory.',
    );
  });

  test('3.2 — a sign-in test reports the role the groups would grant', async ({ page }) => {
    const rec = await mockApi(page, {
      testReply: {
        ok: true,
        message: "'alice' would sign in as admin.",
        role: 'admin',
      },
    });
    await openDirectory(page);

    await win(page).locator('[data-testid="directory-test-username"]').fill('alice');
    await win(page).locator('[data-testid="directory-test-password"]').fill('alice-secret');
    await win(page).locator('[data-testid="directory-test-signin"]').click();

    await expect.poll(() => rec.tests.length).toBe(1);
    expect(rec.tests[0].username).toBe('alice');
    expect(rec.tests[0].password).toBe('alice-secret');

    const result = win(page).locator('[data-testid="directory-test-result"]');
    await expect(result).toContainText("'alice' would sign in as admin.");
    await expect(result).toContainText('Would sign in as Administrator');
  });

  test('3.3 — a failed test shows the reason instead of a generic error', async ({ page }) => {
    const rec = await mockApi(page, {
      testReply: { ok: false, message: 'The directory refused the password.' },
    });
    await openDirectory(page);

    await win(page).locator('[data-testid="directory-test-service"]').click();

    await expect(win(page).locator('[data-testid="directory-test-result"]')).toContainText(
      'The directory refused the password.',
    );
    expect(rec.settings.length).toBe(0);
  });

  test('3.4 — the sign-in test needs a name before it can run', async ({ page }) => {
    await mockApi(page);
    await openDirectory(page);

    await expect(win(page).locator('[data-testid="directory-test-signin"]')).toBeDisabled();
    await win(page).locator('[data-testid="directory-test-username"]').fill('alice');
    await expect(win(page).locator('[data-testid="directory-test-signin"]')).toBeEnabled();
  });
});

// ===========================================================================
// 4. Directory accounts
// ===========================================================================

test.describe('4 — Directory accounts', () => {
  test('4.1 — disabling an account updates its badge and the counts at once', async ({ page }) => {
    const rec = await mockApi(page);
    await openDirectory(page);

    await win(page).locator('[data-testid="directory-identity-toggle-12"]').click();
    await expect.poll(() => rec.toggles.length).toBe(1);
    expect(rec.toggles[0]).toEqual({ id: 12, disabled: true });

    await expect(win(page).locator('[data-testid="directory-identity-12"]')).toContainText(
      'Disabled',
    );
    await expect(win(page).locator('[data-testid="directory-counts"]')).toContainText(
      '2 disabled',
    );
    // The action flips to the way back.
    await expect(
      win(page).locator('[data-testid="directory-identity-toggle-12"]'),
    ).toHaveText('Enable');
  });

  test('4.2 — forgetting asks first and cancelling changes nothing', async ({ page }) => {
    const rec = await mockApi(page);
    await openDirectory(page);

    await win(page).locator('[data-testid="directory-identity-forget-13"]').click();

    const message = page.locator('[data-testid="confirm-dialog-message"]');
    await expect(message).toContainText('Forget Bob Durand?');
    // The consequence is named, not implied.
    await expect(message).toContainText('The account in your directory is not touched.');

    await page
      .locator('[data-slot="dialog-content"]')
      .getByRole('button', { name: 'Cancel', exact: true })
      .click();

    await expect(message).toBeHidden();
    expect(rec.deletes.length).toBe(0);
    await expect(win(page).locator('[data-testid="directory-identity-13"]')).toBeVisible();
  });

  test('4.3 — confirming removes the account from the list', async ({ page }) => {
    const rec = await mockApi(page);
    await openDirectory(page);

    await win(page).locator('[data-testid="directory-identity-forget-13"]').click();
    await page
      .locator('[data-slot="dialog-content"]')
      .getByRole('button', { name: 'Confirm', exact: true })
      .click();

    await expect.poll(() => rec.deletes.length).toBe(1);
    expect(rec.deletes[0]).toContain('/api/directory/identities/13');
    await expect(win(page).locator('[data-testid="directory-identity-13"]')).toHaveCount(0);
    await expect(win(page).locator('[data-testid="directory-identity-12"]')).toBeVisible();
  });

  test('4.4 — nobody signed in yet reads as an instruction, not a blank table', async ({
    page,
  }) => {
    await mockApi(page, { identities: [] });
    await openDirectory(page);

    const empty = win(page).locator('[data-testid="directory-identities-empty"]');
    await expect(empty).toBeVisible();
    await expect(empty).toContainText('Nobody has signed in through the directory yet');
    await expect(empty).toContainText('An account appears here the first time its owner signs in.');
  });
});

// ===========================================================================
// 5. Accessibility
// ===========================================================================

test.describe('5 — Accessibility', () => {
  test('5.1 — the enable switch is named by its visible label, exactly once', async ({ page }) => {
    await mockApi(page);
    await openDirectory(page);

    const toggle = page.getByRole('switch', { name: 'Allow directory sign-in', exact: true });
    await expect(toggle).toHaveCount(1);
    await expect(toggle).toHaveAttribute('aria-checked', 'true');
    // No duplicate inline label next to the switch.
    await expect(
      win(page).locator('[data-testid="directory-enabled-toggle"]').getByText('Allow directory sign-in'),
    ).toHaveCount(0);
  });

  test('5.2 — icon-only actions carry a name that includes the account', async ({ page }) => {
    await mockApi(page);
    await openDirectory(page);

    await expect(win(page).locator('[data-testid="directory-identity-forget-12"]')).toHaveAttribute(
      'aria-label',
      'Forget alice',
    );
    await expect(win(page).locator('[data-testid="directory-identity-toggle-12"]')).toHaveAttribute(
      'aria-label',
      'Disable alice',
    );
    await expect(win(page).locator('[data-testid="directory-mapping-remove-0"]')).toHaveAttribute(
      'aria-label',
      'Remove the mapping for cn=admins,ou=groups,dc=example,dc=com',
    );
  });

  test('5.3 — every input has a label, and the page has one top-level heading', async ({ page }) => {
    await mockApi(page);
    await openDirectory(page);

    const inputs = win(page).locator('input, select');
    const count = await inputs.count();
    expect(count).toBeGreaterThan(0);
    for (let i = 0; i < count; i += 1) {
      const control = inputs.nth(i);
      if (!(await control.isVisible())) continue;
      const id = await control.getAttribute('id');
      const insideLabel = await control.evaluate(
        (el) => !!el.closest('label'),
      );
      const labelledBy = await control.getAttribute('aria-labelledby');
      expect(
        insideLabel || !!id || !!labelledBy || (await control.getAttribute('aria-label')),
      ).toBeTruthy();
    }
  });
});

// ===========================================================================
// 6. Single sign-on settings
// ===========================================================================

test.describe('6 — Single sign-on settings', () => {
  test('6.1 — the segment switches forms and Save follows the active one', async ({ page }) => {
    const rec = await mockApi(page);
    await openDirectory(page);

    // The directory form is the default path.
    await expect(win(page).locator('[data-testid="directory-server-url"]')).toBeVisible();
    const sso = win(page).locator('[data-testid="directory-tab-oidc"]');
    await expect(sso).toHaveAttribute('aria-pressed', 'false');
    await expect(win(page).locator('[data-testid="directory-tab-directory"]')).toHaveAttribute(
      'aria-pressed',
      'true',
    );

    await sso.click();
    await expect(sso).toHaveAttribute('aria-pressed', 'true');
    await expect(win(page).locator('[data-testid="directory-server-url"]')).toHaveCount(0);
    await expect(win(page).locator('[data-testid="oidc-issuer"]')).toBeVisible();

    await win(page).locator('[data-testid="oidc-issuer"]').fill('https://auth.example.test');
    await win(page).locator('[data-testid="oidc-save"]').click();

    await expect.poll(() => rec.oidcSettings.length).toBe(1);
    expect(rec.oidcSettings[0].issuer).toBe('https://auth.example.test');
    // A single sign-on Save must not write the directory half.
    expect(rec.settings.length).toBe(0);
    await expect(win(page).locator('[data-testid="directory-footer"]')).toContainText(
      'remote account',
    );
  });

  test('6.2 — the client secret is write-only: stored, replaced, then cleared', async ({ page }) => {
    const rec = await mockApi(page, {
      oidc: { enabled: true, issuer: 'https://auth.example.test', has_client_secret: true },
    });
    await openDirectory(page);
    await win(page).locator('[data-testid="directory-tab-oidc"]').click();

    const field = win(page).locator('[data-testid="oidc-client-secret"]');
    // The stored secret is never returned: the field is empty and says so.
    await expect(field).toHaveValue('');
    await expect(field).toHaveAttribute(
      'placeholder',
      'A secret is stored — type a new one to replace it',
    );
    await expect(win(page).locator('[data-testid="oidc-clear-secret"]')).toBeVisible();

    await field.fill('Replacement!2026');
    await win(page).locator('[data-testid="oidc-save"]').click();
    await expect.poll(() => rec.oidcSettings.length).toBe(1);
    expect(rec.oidcSettings[0].client_secret).toBe('Replacement!2026');
    await expect(field).toHaveValue('');

    // Clearing sends the empty string, which is how the API forgets it.
    await win(page).locator('[data-testid="oidc-clear-secret"]').click();
    await expect(field).toBeDisabled();
    await win(page).locator('[data-testid="oidc-save"]').click();
    await expect.poll(() => rec.oidcSettings.length).toBe(2);
    expect(rec.oidcSettings[1].client_secret).toBe('');
    await expect(win(page).locator('[data-testid="oidc-clear-secret"]')).toHaveCount(0);
  });

  test('6.3 — the check reads the typed configuration and saves nothing', async ({ page }) => {
    const rec = await mockApi(page);
    await openDirectory(page);
    await win(page).locator('[data-testid="directory-tab-oidc"]').click();

    await win(page).locator('[data-testid="oidc-issuer"]').fill('https://auth.example.test');
    await win(page).locator('[data-testid="oidc-test-run"]').click();

    await expect.poll(() => rec.oidcTests.length).toBe(1);
    expect(rec.oidcTests[0].settings.issuer).toBe('https://auth.example.test');
    expect(rec.settings.length).toBe(0);
    expect(rec.oidcSettings.length).toBe(0);
    await expect(win(page).locator('[data-testid="oidc-test-result"]')).toContainText(
      'Your provider answered.',
    );
  });

  test('6.4 — a refused save keeps what was typed and shows the server sentence', async ({
    page,
  }) => {
    const rec = await mockApi(page);
    await openDirectory(page);
    await win(page).locator('[data-testid="directory-tab-oidc"]').click();

    await win(page).locator('[data-testid="oidc-issuer"]').fill('http://auth.example.test');
    rec.oidcRefuse = { status: 400, message: 'The issuer must be an https:// address.' };
    await win(page).locator('[data-testid="oidc-save"]').click();

    await expect(win(page).locator('[data-testid="directory-error"]')).toContainText(
      'must be an https:// address',
    );
    await expect(win(page).locator('[data-testid="oidc-issuer"]')).toHaveValue(
      'http://auth.example.test',
    );
  });

  test('6.5 — one account list, with the provider named and the tab counts', async ({ page }) => {
    await mockApi(page, { identities: [alice(), carol(), dave()] });
    await openDirectory(page);

    // The directory tab does not claim the single sign-on account.
    await expect(win(page).locator('[data-testid="directory-identity-12"]')).not.toContainText(
      'OIDC',
    );

    await win(page).locator('[data-testid="directory-tab-oidc"]').click();
    const sso = win(page).locator('[data-testid="directory-identity-21"]');
    await expect(sso).toContainText('carol');
    await expect(sso).toContainText('OIDC');
    await expect(win(page).locator('[data-testid="directory-identities"]')).toContainText(
      'through your directory or identity provider',
    );
    await expect(win(page).locator('[data-testid="oidc-break-glass"]')).toContainText(
      'local administrator',
    );

    // The SAML tab names its own provider and counts only its own accounts.
    await win(page).locator('[data-testid="directory-tab-saml"]').click();
    const samlRow = win(page).locator('[data-testid="directory-identity-22"]');
    await expect(samlRow).toContainText('Dave Moreau');
    await expect(samlRow).toContainText('SAML');
    await expect(win(page).locator('[data-testid="directory-identities"]')).toContainText(
      'through your identity provider',
    );
    await expect(win(page).locator('[data-testid="directory-footer"]')).toContainText('remote account');
  });

  test('6.6 — the switch is a named group of pressed buttons, not a half tablist', async ({
    page,
  }) => {
    await mockApi(page);
    await openDirectory(page);

    const group = win(page).locator('[data-testid="directory-tabs"]');
    await expect(group).toHaveAttribute('role', 'group');
    await expect(group).toHaveAttribute('aria-label', 'Sign-in method to configure');
    await expect(group.locator('button')).toHaveCount(3);
    // A real tablist would owe the arrow-key pattern; this deliberately is not one.
    await expect(group.locator('[role="tab"]')).toHaveCount(0);
  });

  test('6.7 — the SAML tab saves only the SAML half and can be turned on', async ({ page }) => {
    const rec = await mockApi(page);
    await openDirectory(page);
    await win(page).locator('[data-testid="directory-tab-saml"]').click();

    await expect(win(page).locator('[data-testid="saml-status"]')).toContainText(
      'Single sign-on with SAML is off',
    );
    await win(page).locator('[data-testid="saml-enabled-toggle"] button').click();
    await win(page).locator('[data-testid="saml-idp-sso-url"]').fill('https://idp.example.test/sso');
    await win(page).locator('[data-testid="saml-save"]').click();

    await expect.poll(() => rec.samlSettings.length).toBe(1);
    expect(rec.samlSettings[0].enabled).toBe(true);
    expect(rec.samlSettings[0].idp_sso_url).toBe('https://idp.example.test/sso');
    // A SAML Save must not write the directory or the OpenID Connect half.
    expect(rec.settings.length).toBe(0);
    expect(rec.oidcSettings.length).toBe(0);
    await expect(win(page).locator('[data-testid="saml-status"]')).toContainText(
      'Single sign-on with SAML is on',
    );
  });

  test('6.8 — reading the provider metadata fills the form but saves nothing', async ({ page }) => {
    const rec = await mockApi(page);
    await openDirectory(page);
    await win(page).locator('[data-testid="directory-tab-saml"]').click();

    await expect(win(page).locator('[data-testid="saml-apply-import"]')).toHaveCount(0);
    await win(page)
      .locator('[data-testid="saml-metadata-url"]')
      .fill('https://idp.example.test/metadata');
    await win(page).locator('[data-testid="saml-test"]').click();

    await expect.poll(() => rec.samlTests.length).toBe(1);
    expect(rec.samlTests[0].settings.idp_metadata_url).toBe('https://idp.example.test/metadata');
    await expect(win(page).locator('[data-testid="saml-test-result"]')).toContainText(
      'metadata was read',
    );
    await expect(win(page).locator('[data-testid="saml-apply-import"]')).toBeVisible();
    // Nothing is stored until the administrator says so.
    expect(rec.samlSettings.length).toBe(0);

    await win(page).locator('[data-testid="saml-apply-import"]').click();
    await expect(win(page).locator('[data-testid="saml-idp-entity-id"]')).toHaveValue(
      'https://idp.example.test/metadata',
    );
    expect(rec.samlSettings.length).toBe(0);
  });

  test('6.9 — a refused SAML save keeps the typed values and says why', async ({ page }) => {
    const rec = await mockApi(page);
    await openDirectory(page);
    await win(page).locator('[data-testid="directory-tab-saml"]').click();

    rec.samlRefuse = { status: 400, message: 'The sign-in address must be an https:// address.' };
    await win(page).locator('[data-testid="saml-idp-sso-url"]').fill('http://idp.example.test/sso');
    await win(page).locator('[data-testid="saml-save"]').click();

    await expect(win(page).locator('[data-testid="directory-error"]')).toContainText(
      'must be an https:// address',
    );
    await expect(win(page).locator('[data-testid="saml-idp-sso-url"]')).toHaveValue(
      'http://idp.example.test/sso',
    );
    await expect(win(page).locator('[data-testid="saml-save"]')).toBeEnabled();
  });
});

// ===========================================================================
// 7. Signing in with single sign-on
// ===========================================================================

test.describe('7 — Signing in with single sign-on', () => {
  /** Answer `GET /api/auth/methods` the way the server would. */
  async function mockMethods(
    page: Page,
    methods: { enabled: boolean; label?: string },
    saml?: { enabled: boolean; label?: string },
  ) {
    await page.route('**/api/auth/methods', (route: Route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          success: true,
          data: {
            password: true,
            oidc: { enabled: methods.enabled, label: methods.label ?? '' },
            saml: { enabled: saml?.enabled ?? false, label: saml?.label ?? '' },
          },
        }),
      }),
    );
  }

  test('7.1 — the button appears only when the appliance offers it, with the configured text', async ({
    page,
  }) => {
    await mockMethods(page, { enabled: false });
    await page.goto('/login');
    await expect(page.getByTestId('login-page')).toBeVisible();
    await expect(page.getByTestId('login-sso')).toHaveCount(0);

    // The most recent route wins: the integration is now on, with a label.
    await mockMethods(page, { enabled: true, label: 'Sign in with Authentik' });
    await page.reload();
    await expect(page.getByTestId('login-sso')).toContainText('Sign in with Authentik');
    await expect(page.getByTestId('login-sso')).toHaveAttribute('type', 'button');
    // A password form is still there: single sign-on is an addition, not a
    // replacement.
    await expect(page.locator('#username')).toBeVisible();
  });

  test('7.2 — clicking it asks the server where to go, then leaves', async ({ page }) => {
    let started = 0;
    await mockMethods(page, { enabled: true });
    await page.route('**/api/auth/oidc/start', (route: Route) => {
      started += 1;
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          success: true,
          data: { authorize_url: `${BASE}/mock-provider`, state: 's1' },
        }),
      });
    });

    await page.goto('/login');
    // No label configured: the generic, translated text is used.
    await expect(page.getByTestId('login-sso')).toContainText('Single sign-on');
    await page.getByTestId('login-sso').click();

    // The browser really left for the URL the server answered with. (A full
    // navigation is served by the service worker, so asserting the URL is
    // steadier than mocking the destination document.)
    await expect(page).toHaveURL(/\/mock-provider$/);
    expect(started).toBe(1);
  });

  test('7.3 — a refused sign-in lands on the login card as a sentence', async ({ page }) => {
    await mockMethods(page, { enabled: true });
    await page.goto('/login?sso_error=denied');

    // The reason sits on the card itself — no modal to dismiss after being
    // bounced back from the provider.
    const notice = page.getByTestId('login-inline-error');
    await expect(notice).toContainText('did not confirm the sign-in');
    // The code is consumed once: the URL is clean and a reload is quiet.
    await expect(page).toHaveURL(/\/login$/);
    await page.reload();
    await expect(page.getByTestId('login-inline-error')).toHaveCount(0);
  });

  test('7.4 — the one-shot ticket is exchanged for the session and the desktop opens', async ({
    page,
  }) => {
    const exchanges: any[] = [];
    await mockMethods(page, { enabled: true });
    await page.route('**/api/auth/oidc/complete', async (route: Route) => {
      exchanges.push(route.request().postDataJSON());
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          success: true,
          data: {
            token: tokens.token,
            csrf_token: tokens.csrf,
            username: ADMIN_USER,
            is_admin: true,
            must_change_password: false,
          },
        }),
      });
    });

    await page.goto('/login#sso=ticket-1');
    await page.waitForSelector('.hv-user-pill', { timeout: 20_000 });

    // The fragment was consumed exactly once, and it was the ticket.
    expect(exchanges).toEqual([{ code: 'ticket-1' }]);
    await expect(page).toHaveURL(/\/desktop/);
  });

  test('7.5 — a valid but non-admin session is refused, not left dangling', async ({ page }) => {
    let loggedOut = 0;
    await mockMethods(page, { enabled: true });
    await page.route('**/api/auth/oidc/complete', (route: Route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          success: true,
          data: {
            token: 'not-a-real-token',
            csrf_token: '',
            username: 'carol',
            is_admin: false,
            must_change_password: false,
          },
        }),
      }),
    );
    await page.route('**/api/auth/logout', (route: Route) => {
      loggedOut += 1;
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ success: true, data: {} }),
      });
    });

    await page.goto('/login#sso=ticket-2');
    await expect(page.getByTestId('login-error-msg')).toContainText(
      'Access restricted to administrators only',
    );
    // The session was ended server-side rather than kept for the next reload.
    expect(loggedOut).toBe(1);
    await expect(page).toHaveURL(/\/login/);
  });

  test('7.6 — a malformed ticket is refused without a network call', async ({ page }) => {
    let called = 0;
    await mockMethods(page, { enabled: true });
    await page.route('**/api/auth/oidc/complete', (route: Route) => {
      called += 1;
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ success: true, data: {} }),
      });
    });

    await page.goto('/login#sso=');
    await expect(page.getByTestId('login-inline-error')).toContainText(
      'Single sign-on did not complete',
    );
    expect(called).toBe(0);
  });

  test('7.7 — two providers produce two buttons, and the SAML one starts SAML', async ({
    page,
  }) => {
    let samlStarts = 0;
    let oidcStarts = 0;
    await mockMethods(
      page,
      { enabled: true, label: 'Sign in with Authentik' },
      { enabled: true, label: 'Sign in with our SAML provider' },
    );
    await page.route('**/api/auth/saml/start', (route: Route) => {
      samlStarts += 1;
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          success: true,
          data: { authorize_url: `${BASE}/mock-saml-provider`, request_id: '_r1' },
        }),
      });
    });
    await page.route('**/api/auth/oidc/start', (route: Route) => {
      oidcStarts += 1;
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ success: true, data: { authorize_url: `${BASE}/never` } }),
      });
    });

    await page.goto('/login');
    // Each provider keeps its own admin-chosen text, and the password form
    // stays available beside them.
    await expect(page.getByTestId('login-sso')).toContainText('Sign in with Authentik');
    await expect(page.getByTestId('login-sso-saml')).toContainText(
      'Sign in with our SAML provider',
    );
    await expect(page.locator('#username')).toBeVisible();

    await page.getByTestId('login-sso-saml').click();
    await expect(page).toHaveURL(/\/mock-saml-provider$/);
    expect(samlStarts).toBe(1);
    // Clicking SAML must not also start an OpenID Connect flow.
    expect(oidcStarts).toBe(0);
  });
});
