<!-- ========================================================================
     File          : Directory.svelte
     Author        : Nicolas Le Roux
     Created       : 01/11/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     Directory and single sign-on (ROADMAP P3.1, stages 1 and 2). An admin
     picks one of two providers — an LDAP or Active Directory server, or an
     OpenID Connect identity provider — says which attribute or claim holds the
     login name and which groups mean "administrator", and people then sign in
     with the credentials they already have.

     The screen is deliberately one page with one Save: connection, who may
     sign in, and roles-from-groups are the three things that must agree, and
     splitting them would let an admin save a server without a base DN. The
     attribute names, the claims and the caches live behind "Advanced" so the
     default path stays short. A two-button segment picks the provider; the
     account list below is shared, because both providers feed the same
     `remote_identities` table and the same role resolution.

     The banner says, at all times, whether directory sign-in is on and how
     many local administrators keep their own password — the reassurance that
     mis-configuring this can never lock anybody out.

     Dependencies:
       - svelte (>= 5), theme + app composites, $lib/services/api

     Usage:
       Registered as the "Directory sign-in" desktop app (admin only).
     ======================================================================== -->
<script lang="ts">
  /**
   * Directory — LDAP/AD settings, group→role mapping and the principal list.
   *
   * Accessibility: the status banner is a `role="status"` live region, every
   * input has a visible (or `sr-only`) label, compact icon buttons carry an
   * `aria-label` plus the shared 44px `.hv-ic` hit square, the mapping rows
   * are real buttons, and "Forget" goes through the focus-trapped
   * `ConfirmDialog` — never a bare click.
   */
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { translate } from '$lib/stores/i18n';
  import { Spinner, Button, Toggle, Alert, Badge, Checkbox } from '$lib/components/theme';
  import { PageLayout, SectionCard, EmptyState, ConfirmDialog } from '$lib/components/app';
  import { notifyError, notifySuccess } from '$lib/utils/notify';
  import { createConfirmState } from '$lib/utils/confirmDialog.svelte';
  import { cn } from '$lib/utils.js';
  import {
    IconUsers,
    IconServer,
    IconKey,
    IconCheck,
    IconPlus,
    IconTrash,
    IconPlugConnected,
    IconInfoCircle,
    IconAlertTriangle,
    IconRefresh,
  } from '$lib/icons/lucide';

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

  interface Status {
    enabled: boolean;
    usable: boolean;
    problem: string | null;
    cache: { hits: number; misses: number; entries: number };
  }

  interface Counts {
    total: number;
    admins: number;
    users: number;
    disabled: number;
  }

  interface Payload {
    settings: Settings;
    status: Status;
    identities: Identity[];
    counts: Counts;
    break_glass: { local_admins: number; policy: string };
  }

  /** The OpenID Connect half of the screen (ROADMAP P3.1, stage 2). */
  interface OidcSettings {
    enabled: boolean;
    label: string;
    issuer: string;
    client_id: string;
    /** Write-only on the wire: the server answers with this flag instead. */
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

  interface OidcStatus {
    enabled: boolean;
    usable: boolean;
    problem: string | null;
  }

  interface OidcPayload {
    settings: OidcSettings;
    status: OidcStatus;
    counts: Counts;
    break_glass: { local_admins: number };
  }

  /** The SAML 2.0 half of the screen (ROADMAP P3.1, stage 3). */
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

  interface SamlStatus {
    enabled: boolean;
    usable: boolean;
    problem: string | null;
    pending_requests: number;
  }

  interface SamlPayload {
    settings: SamlSettings;
    status: SamlStatus;
    counts: Counts;
    break_glass: { local_admins: number };
  }

  const LDAP_DEFAULTS = {
    user_filter: '(&(objectClass=person)(uid={username}))',
    attr_username: 'uid',
    attr_display_name: 'cn',
    attr_member_of: 'memberOf',
  };
  const AD_DEFAULTS = {
    user_filter: '(&(objectClass=user)(sAMAccountName={username}))',
    attr_username: 'sAMAccountName',
    attr_display_name: 'displayName',
    attr_member_of: 'memberOf',
  };

  let settings = $state<Settings>(emptySettings());
  let status = $state<Status>({ enabled: false, usable: false, problem: null, cache: { hits: 0, misses: 0, entries: 0 } });
  let identities = $state<Identity[]>([]);
  let counts = $state<Counts>({ total: 0, admins: 0, users: 0, disabled: 0 });
  let breakGlass = $state<{ local_admins: number; policy: string }>({ local_admins: 0, policy: '' });

  let loading = $state(true);
  let busy = $state<'save' | 'test' | 'identity' | null>(null);
  let error = $state('');

  /** Typed but not yet saved: the server never echoes it back. */
  let bindPassword = $state('');
  /** Set by "Clear the stored password"; maps to `bind_password: ""`. */
  let clearBindPassword = $state(false);

  let testUsername = $state('');
  let testPassword = $state('');
  let testResult = $state<{ ok: boolean; message: string; role?: string } | null>(null);
  let testing = $state<'service' | 'signin' | null>(null);

  /**
   * Two providers, one screen. A directory (LDAP/AD) and an OpenID Connect
   * provider are alternatives, not a ladder: an appliance configures one and
   * the accounts it produces land in the single list at the bottom. A tab
   * strip keeps each form short instead of stacking two full forms on top of
   * each other.
   */
  let tab = $state<'directory' | 'oidc' | 'saml'>('directory');
  let oidc = $state<OidcSettings>(emptyOidc());
  let oidcStatus = $state<OidcStatus>({ enabled: false, usable: false, problem: null });
  let oidcCounts = $state<Counts>({ total: 0, admins: 0, users: 0, disabled: 0 });
  let oidcLocalAdmins = $state(0);
  /** Typed but not yet saved: the server never echoes it back. */
  let oidcSecret = $state('');
  /** Set by "Clear the stored secret"; maps to `client_secret: ""`. */
  let clearOidcSecret = $state(false);
  let oidcTesting = $state(false);
  let oidcTestResult = $state<{ ok: boolean; message: string } | null>(null);

  // SAML 2.0: the third provider. Same screen, same account list, but the
  // identity provider's document is imported rather than typed — a certificate
  // copied by hand is the most common way a SAML integration fails.
  let saml = $state<SamlSettings>(emptySaml());
  let samlStatus = $state<SamlStatus>({ enabled: false, usable: false, problem: null, pending_requests: 0 });
  let samlCounts = $state<Counts>({ total: 0, admins: 0, users: 0, disabled: 0 });
  let samlLocalAdmins = $state(0);
  let samlTesting = $state(false);
  let samlTestResult = $state<{ ok: boolean; message: string } | null>(null);
  let samlImported = $state<{ entity_id: string; sso_url: string; certificates: string[] } | null>(null);
  let samlSpMetadata = $state('');

  const dlg = createConfirmState();

  const fieldLabel = 'text-c-label font-semibold text-[var(--hv-shell-text-strong)]';
  const hint = 'm-0 text-c-label text-[var(--hv-shell-text-2)]';
  const inp =
    'w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-c-base text-[var(--hv-shell-text-strong)] outline-none focus-visible:border-[var(--hv-primary)] focus-visible:shadow-[var(--hv-focus-ring)]';
  const iconBtn =
    'grid place-items-center w-9 h-9 flex-none rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-shell-border-soft)] hover:text-[var(--hv-shell-text-strong)] hv-ic';

  const serverIsLdaps = $derived(settings.server_url.trim().toLowerCase().startsWith('ldaps://'));

  // `ldaps://` is already encrypted, so StartTLS cannot apply to it. Unchecking
  // the box as the address changes keeps the disabled checkbox honest — without
  // this, an admin could carry `start_tls: true` over from an ldap:// address
  // and have every save refused with no way to clear it.
  $effect(() => {
    if (serverIsLdaps && settings.start_tls) settings.start_tls = false;
  });

  onMount(() => {
    // Both halves load together: the account list is shared, and the toolbar
    // asks the active tab which Save it is.
    void refreshAll();
  });

  function emptySettings(): Settings {
    return {
      enabled: false,
      provider: 'ldap',
      server_url: '',
      start_tls: false,
      verify_tls: true,
      bind_dn: '',
      has_bind_password: false,
      base_dn: '',
      user_filter: LDAP_DEFAULTS.user_filter,
      group_filter: '',
      attr_username: LDAP_DEFAULTS.attr_username,
      attr_display_name: LDAP_DEFAULTS.attr_display_name,
      attr_email: 'mail',
      attr_member_of: LDAP_DEFAULTS.attr_member_of,
      default_role: 'user',
      group_roles: [],
      cache_ttl_seconds: 300,
      updated_at: null,
      updated_by: null,
    };
  }

  /**
   * The callback URL for this appliance, offered as a starting point.
   *
   * The provider matches this string exactly, so making the admin assemble it
   * from the address bar is a reliable way to fail on the scheme or the port.
   * It is only a default: an `https://…` address behind a reverse proxy, or a
   * loopback address, is what the server itself accepts.
   */
  function defaultCallbackUrl(): string {
    if (typeof window === 'undefined') return '';
    const here = new URL(window.location.href);
    const loopback = ['localhost', '127.0.0.1', '[::1]', '::1'].includes(here.hostname);
    if (here.protocol !== 'https:' && !loopback) return '';
    return `${here.origin}/api/auth/oidc/callback`;
  }

  function emptyOidc(): OidcSettings {
    return {
      enabled: false,
      label: '',
      issuer: '',
      client_id: '',
      has_client_secret: false,
      redirect_url: '',
      scopes: 'openid profile email',
      attr_username: 'preferred_username',
      attr_display_name: 'name',
      attr_email: 'email',
      attr_groups: 'groups',
      default_role: 'user',
      group_roles: [],
      updated_at: null,
      updated_by: null,
    };
  }

  /** The ACS URL this appliance would use, offered as a starting point for the
   *  same reason as the OIDC callback: the provider matches it exactly. */
  function defaultAcsUrl(): string {
    if (typeof window === 'undefined') return '';
    const here = new URL(window.location.href);
    const loopback = ['localhost', '127.0.0.1', '[::1]', '::1'].includes(here.hostname);
    if (here.protocol !== 'https:' && !loopback) return '';
    return `${here.origin}/api/auth/saml/acs`;
  }

  function defaultSpEntityId(): string {
    if (typeof window === 'undefined') return '';
    const here = new URL(window.location.href);
    const loopback = ['localhost', '127.0.0.1', '[::1]', '::1'].includes(here.hostname);
    if (here.protocol !== 'https:' && !loopback) return '';
    return `${here.origin}/api/auth/saml/metadata`;
  }

  function emptySaml(): SamlSettings {
    return {
      enabled: false,
      label: '',
      idp_metadata_url: '',
      idp_entity_id: '',
      idp_sso_url: '',
      idp_certificates: [],
      sp_entity_id: '',
      acs_url: '',
      attr_display_name: 'urn:oid:2.16.840.1.113730.3.1.241',
      attr_email: 'urn:oid:0.9.2342.19200300.100.1.3',
      attr_groups: 'http://schemas.xmlsoap.org/claims/Group',
      default_role: 'user',
      group_roles: [],
      updated_at: null,
      updated_by: null,
    };
  }

  async function load() {
    loading = true;
    error = '';
    try {
      const resp = await api.get('/directory');
      if (resp.success && resp.data) {
        applyPayload(resp.data as Payload);
      } else {
        error = resp.message || resp.error || translate('directory.load_failed');
      }
    } catch {
      error = translate('directory.load_failed');
    } finally {
      loading = false;
    }
  }

  function applyPayload(data: Payload) {
    settings = data.settings;
    status = data.status;
    identities = data.identities ?? [];
    counts = data.counts;
    breakGlass = data.break_glass ?? { local_admins: 0, policy: '' };
    bindPassword = '';
    clearBindPassword = false;
  }

  // ------------------------------------------------------------------
  // Single sign-on (OpenID Connect)
  // ------------------------------------------------------------------

  async function loadOidc() {
    try {
      const resp = await api.get('/oidc');
      if (resp.success && resp.data) applyOidc(resp.data as OidcPayload);
    } catch {
      // The directory payload's banner already reports a failed load; a second
      // one saying the same thing would just be noise.
    }
  }

  function applyOidc(data: OidcPayload) {
    oidc = { ...emptyOidc(), ...data.settings };
    // A blank saved callback means "never configured": offer this server's.
    if (!oidc.redirect_url) oidc.redirect_url = defaultCallbackUrl();
    oidcStatus = data.status;
    oidcCounts = data.counts;
    oidcLocalAdmins = data.break_glass?.local_admins ?? 0;
    oidcSecret = '';
    clearOidcSecret = false;
  }

  async function refreshAll() {
    await Promise.all([load(), loadOidc(), loadSaml()]);
  }

  /** The settings as the API expects them. The secret is write-only: absent
   *  keeps the stored one, an empty string clears it. */
  function oidcBody() {
    return {
      enabled: oidc.enabled,
      label: oidc.label,
      issuer: oidc.issuer,
      client_id: oidc.client_id,
      client_secret: clearOidcSecret ? '' : oidcSecret || undefined,
      redirect_url: oidc.redirect_url,
      scopes: oidc.scopes,
      attr_username: oidc.attr_username,
      attr_display_name: oidc.attr_display_name,
      attr_email: oidc.attr_email,
      attr_groups: oidc.attr_groups,
      default_role: oidc.default_role,
      group_roles: oidc.group_roles,
    };
  }

  async function saveOidc() {
    busy = 'save';
    error = '';
    try {
      const resp = await api.put('/oidc/settings', oidcBody());
      if (resp.success && resp.data) {
        applyOidc(resp.data as OidcPayload);
        notifySuccess(translate('oidc.saved'));
      } else {
        error = resp.message || resp.error || translate('oidc.save_failed');
      }
    } catch {
      error = translate('oidc.save_failed');
    } finally {
      busy = null;
    }
  }

  /** Read the provider's configuration with the details as typed, saving
   *  nothing — the failure an admin is most likely to hit is a wrong issuer. */
  async function testOidc() {
    oidcTesting = true;
    oidcTestResult = null;
    try {
      const resp = await api.post('/oidc/test', { settings: oidcBody() });
      oidcTestResult = resp.success
        ? { ok: true, message: translate('oidc.test_ok') }
        : { ok: false, message: resp.message || resp.error || translate('oidc.test_failed') };
    } catch {
      oidcTestResult = { ok: false, message: translate('oidc.test_failed') };
    } finally {
      oidcTesting = false;
    }
  }

  function addOidcMapping() {
    oidc.group_roles = [...oidc.group_roles, { group: '', role: 'user' }];
  }

  function removeOidcMapping(index: number) {
    oidc.group_roles = oidc.group_roles.filter((_, at) => at !== index);
  }

  // ------------------------------------------------------------------
  // Single sign-on (SAML 2.0)
  // ------------------------------------------------------------------

  async function loadSaml() {
    try {
      const resp = await api.get('/saml');
      if (resp.success && resp.data) applySaml(resp.data as SamlPayload);
    } catch {
      // Same reasoning as the OIDC half: one load error is enough.
    }
  }

  function applySaml(data: SamlPayload) {
    saml = { ...emptySaml(), ...data.settings };
    if (!saml.acs_url) saml.acs_url = defaultAcsUrl();
    if (!saml.sp_entity_id) saml.sp_entity_id = defaultSpEntityId();
    samlStatus = data.status;
    samlCounts = data.counts;
    samlLocalAdmins = data.break_glass?.local_admins ?? 0;
    samlImported = null;
    samlSpMetadata = '';
  }

  /** The settings as the API expects them. */
  function samlBody() {
    return {
      enabled: saml.enabled,
      label: saml.label,
      idp_metadata_url: saml.idp_metadata_url,
      idp_entity_id: saml.idp_entity_id,
      idp_sso_url: saml.idp_sso_url,
      idp_certificates: saml.idp_certificates,
      sp_entity_id: saml.sp_entity_id,
      acs_url: saml.acs_url,
      attr_display_name: saml.attr_display_name,
      attr_email: saml.attr_email,
      attr_groups: saml.attr_groups,
      default_role: saml.default_role,
      group_roles: saml.group_roles,
    };
  }

  async function saveSaml() {
    busy = 'save';
    error = '';
    try {
      const resp = await api.put('/saml/settings', samlBody());
      if (resp.success && resp.data) {
        applySaml(resp.data as SamlPayload);
        notifySuccess(translate('saml.saved'));
      } else {
        error = resp.message || resp.error || translate('saml.save_failed');
      }
    } catch {
      error = translate('saml.save_failed');
    } finally {
      busy = null;
    }
  }

  /** Reads the provider's metadata document with the address as typed and
   *  offers the values — it saves nothing, so the admin keeps the last word. */
  async function testSaml() {
    samlTesting = true;
    samlTestResult = null;
    samlImported = null;
    try {
      const resp = await api.post('/saml/test', { settings: samlBody() });
      if (resp.success && resp.data) {
        const data = resp.data as {
          entity_id: string;
          sso_url: string;
          certificates: string[];
          warnings?: string[];
        };
        samlImported = {
          entity_id: data.entity_id,
          sso_url: data.sso_url,
          certificates: data.certificates,
        };
        const warnings = data.warnings ?? [];
        samlTestResult = {
          ok: true,
          message: warnings.length
            ? `${translate('saml.test_ok')} ${warnings.join(' ')}`
            : translate('saml.test_ok'),
        };
      } else {
        samlTestResult = {
          ok: false,
          message: resp.message || resp.error || translate('saml.test_failed'),
        };
      }
    } catch {
      samlTestResult = { ok: false, message: translate('saml.test_failed') };
    } finally {
      samlTesting = false;
    }
  }

  /** Fill the form from the imported document. Still unsaved: the admin sees
   *  the values, then presses Save. */
  function applyImported() {
    if (!samlImported) return;
    saml.idp_entity_id = samlImported.entity_id;
    saml.idp_sso_url = samlImported.sso_url;
    saml.idp_certificates = [...samlImported.certificates];
    samlImported = null;
  }

  async function loadSpMetadata() {
    try {
      const resp = await api.get('/saml/metadata');
      if (resp.success && resp.data) {
        samlSpMetadata = (resp.data as { xml: string }).xml;
      } else {
        error = resp.message || resp.error || translate('saml.metadata_failed');
      }
    } catch {
      error = translate('saml.metadata_failed');
    }
  }

  async function copySpMetadata() {
    if (!samlSpMetadata) return;
    try {
      await navigator.clipboard.writeText(samlSpMetadata);
      notifySuccess(translate('saml.metadata_copied'));
    } catch {
      // A browser without clipboard permission can still select the text.
    }
  }

  function addSamlMapping() {
    saml.group_roles = [...saml.group_roles, { group: '', role: 'user' }];
  }

  function removeSamlMapping(index: number) {
    saml.group_roles = saml.group_roles.filter((_, at) => at !== index);
  }

  // ------------------------------------------------------------------
  // Form
  // ------------------------------------------------------------------

  /** Switching the type swaps the *default* attributes, never a typed one. */
  function setProvider(provider: string) {
    settings.provider = provider;
    if (provider === 'ad') {
      if (settings.user_filter === LDAP_DEFAULTS.user_filter) settings.user_filter = AD_DEFAULTS.user_filter;
      if (settings.attr_username === LDAP_DEFAULTS.attr_username) settings.attr_username = AD_DEFAULTS.attr_username;
      if (settings.attr_display_name === LDAP_DEFAULTS.attr_display_name) {
        settings.attr_display_name = AD_DEFAULTS.attr_display_name;
      }
    } else {
      if (settings.user_filter === AD_DEFAULTS.user_filter) settings.user_filter = LDAP_DEFAULTS.user_filter;
      if (settings.attr_username === AD_DEFAULTS.attr_username) settings.attr_username = LDAP_DEFAULTS.attr_username;
      if (settings.attr_display_name === AD_DEFAULTS.attr_display_name) {
        settings.attr_display_name = LDAP_DEFAULTS.attr_display_name;
      }
    }
  }

  function addMapping() {
    settings.group_roles = [...settings.group_roles, { group: '', role: 'user' }];
  }

  function removeMapping(index: number) {
    settings.group_roles = settings.group_roles.filter((_, i) => i !== index);
  }

  /** The wire body. `bind_password` is `null` (keep), `""` (clear) or a value. */
  function body() {
    let bind_password: string | null = null;
    if (clearBindPassword) bind_password = '';
    else if (bindPassword !== '') bind_password = bindPassword;
    return {
      enabled: settings.enabled,
      provider: settings.provider,
      server_url: settings.server_url.trim(),
      start_tls: settings.start_tls,
      verify_tls: settings.verify_tls,
      bind_dn: settings.bind_dn.trim(),
      bind_password,
      base_dn: settings.base_dn.trim(),
      user_filter: settings.user_filter.trim(),
      group_filter: settings.group_filter.trim(),
      attr_username: settings.attr_username.trim(),
      attr_display_name: settings.attr_display_name.trim(),
      attr_email: settings.attr_email.trim(),
      attr_member_of: settings.attr_member_of.trim(),
      default_role: settings.default_role,
      group_roles: settings.group_roles
        .filter((m) => m.group.trim() !== '')
        .map((m) => ({ group: m.group.trim(), role: m.role })),
      cache_ttl_seconds: Number(settings.cache_ttl_seconds),
    };
  }

  async function save() {
    busy = 'save';
    error = '';
    try {
      const resp = await api.put('/directory/settings', body());
      if (resp.success && resp.data) {
        applyPayload(resp.data as Payload);
        notifySuccess('directory.saved');
      } else {
        error = resp.message || resp.error || translate('directory.save_failed');
        notifyError({ message: error });
      }
    } catch {
      error = translate('directory.save_failed');
    } finally {
      busy = null;
    }
  }

  // ------------------------------------------------------------------
  // Connection test
  // ------------------------------------------------------------------

  async function test(kind: 'service' | 'signin') {
    testing = kind;
    testResult = null;
    try {
      const payload: Record<string, unknown> = { settings: body() };
      if (kind === 'signin') {
        payload.username = testUsername.trim();
        payload.password = testPassword;
      }
      const resp = await api.post('/directory/test', payload);
      if (resp.success && resp.data) {
        testResult = {
          ok: true,
          message: resp.data.message ?? translate('directory.test_ok'),
          role: resp.data.role,
        };
      } else {
        testResult = {
          ok: false,
          message: resp.message || resp.error || translate('directory.test_failed'),
        };
      }
    } catch {
      testResult = { ok: false, message: translate('directory.test_failed') };
    } finally {
      testing = null;
    }
  }

  // ------------------------------------------------------------------
  // Principals
  // ------------------------------------------------------------------

  async function setDisabled(identity: Identity, disabled: boolean) {
    busy = 'identity';
    try {
      const resp = await api.post(`/directory/identities/${identity.id}/disabled`, { disabled });
      if (resp.success && resp.data) {
        applyPayload(resp.data as Payload);
        notifySuccess(disabled ? 'directory.identity_disabled_done' : 'directory.identity_enabled_done');
      } else {
        notifyError(resp);
      }
    } catch {
      notifyError({ message: translate('directory.save_failed') });
    } finally {
      busy = null;
    }
  }

  async function forget(identity: Identity) {
    const ok = await dlg.ask(
      translate('directory.forget_confirm', { name: identity.display_name || identity.username }),
      { danger: true },
    );
    if (!ok) return;
    busy = 'identity';
    try {
      const resp = await api.delete(`/directory/identities/${identity.id}`);
      if (resp.success && resp.data) {
        applyPayload(resp.data as Payload);
        notifySuccess('directory.forget_done');
      } else {
        notifyError(resp);
      }
    } catch {
      notifyError({ message: translate('directory.save_failed') });
    } finally {
      busy = null;
    }
  }

  function roleLabel(roleId: number): string {
    return roleId === 2 ? translate('directory.role_admin') : translate('directory.role_user');
  }

  function lastLogin(identity: Identity): string {
    if (!identity.last_login_at) return translate('directory.identity_never');
    const when = new Date(identity.last_login_at);
    if (Number.isNaN(when.getTime())) return translate('directory.identity_never');
    return translate('directory.identity_last_login', { when: when.toLocaleString() });
  }

  function formatUpdated(at: string | null, by: string | null): string {
    if (!at) return translate('directory.never_saved');
    const when = new Date(at);
    const rendered = Number.isNaN(when.getTime()) ? at : when.toLocaleString();
    return by
      ? translate('directory.saved_by', { when: rendered, by })
      : translate('directory.saved_at', { when: rendered });
  }
</script>

<PageLayout title={translate('directory.title')} data-testid="directory-page">
  {#snippet icon()}
    <IconUsers strokeWidth={1.8} />
  {/snippet}

  {#snippet toolbarRight()}
    <div class="flex items-center gap-2">
      <Button variant="secondary" data-testid="directory-reload" disabled={busy !== null} onclick={refreshAll}>
        <IconRefresh size={16} strokeWidth={1.8} /> {translate('common.refresh')}
      </Button>
      <Button
        variant="primary"
        data-testid={tab === 'directory'
          ? 'directory-save'
          : tab === 'oidc'
            ? 'oidc-save'
            : 'saml-save'}
        loading={busy === 'save'}
        disabled={busy !== null}
        onclick={tab === 'directory' ? save : tab === 'oidc' ? saveOidc : saveSaml}
      >
        <IconCheck size={16} strokeWidth={1.8} /> {translate('common.save')}
      </Button>
    </div>
  {/snippet}

  {#if loading}
    <div data-testid="directory-loading" class="flex items-center justify-center gap-3 py-16 text-[var(--hv-shell-text)]">
      <Spinner />
      <span>{translate('common.loading')}</span>
    </div>
  {:else}
    <div data-testid="directory-content" class="flex flex-col gap-4 max-w-[880px]">
      {#if error}
        <div data-testid="directory-error">
          <Alert variant="error">{error}</Alert>
        </div>
      {/if}

      <!-- Two providers, one screen. They are alternatives, not a ladder:
           an appliance configures one, and the accounts either produces land
           in the same list below. A segmented control (not a tablist: the
           arrow-key pattern would be half-implemented) keeps each form short.
           `aria-pressed` is what screen readers announce. -->
      <div
        class="flex flex-wrap gap-1 p-1 rounded-xl border border-[var(--hv-shell-border)] bg-[var(--hv-shell-border-soft)]"
        role="group"
        aria-label={translate('oidc.provider_tabs')}
        data-testid="directory-tabs"
      >
        <button
          type="button"
          class={cn(
            'flex-1 min-w-[200px] h-11 px-4 rounded-lg border-none cursor-pointer text-c-label font-semibold transition-colors focus-visible:outline-none focus-visible:shadow-[var(--hv-focus-ring)]',
            tab === 'directory'
              ? 'bg-[var(--hv-primary)] text-white'
              : 'bg-transparent text-[var(--hv-shell-text-2)] hover:text-[var(--hv-shell-text-strong)]',
          )}
          aria-pressed={tab === 'directory'}
          data-testid="directory-tab-directory"
          onclick={() => (tab = 'directory')}
        >{translate('oidc.tab_directory')}</button>
        <button
          type="button"
          class={cn(
            'flex-1 min-w-[200px] h-11 px-4 rounded-lg border-none cursor-pointer text-c-label font-semibold transition-colors focus-visible:outline-none focus-visible:shadow-[var(--hv-focus-ring)]',
            tab === 'oidc'
              ? 'bg-[var(--hv-primary)] text-white'
              : 'bg-transparent text-[var(--hv-shell-text-2)] hover:text-[var(--hv-shell-text-strong)]',
          )}
          aria-pressed={tab === 'oidc'}
          data-testid="directory-tab-oidc"
          onclick={() => (tab = 'oidc')}
        >{translate('oidc.tab_oidc')}</button>
        <button
          type="button"
          class={cn(
            'flex-1 min-w-[200px] h-11 px-4 rounded-lg border-none cursor-pointer text-c-label font-semibold transition-colors focus-visible:outline-none focus-visible:shadow-[var(--hv-focus-ring)]',
            tab === 'saml'
              ? 'bg-[var(--hv-primary)] text-white'
              : 'bg-transparent text-[var(--hv-shell-text-2)] hover:text-[var(--hv-shell-text-strong)]',
          )}
          aria-pressed={tab === 'saml'}
          data-testid="directory-tab-saml"
          onclick={() => (tab = 'saml')}
        >{translate('saml.tab_saml')}</button>
      </div>

      {#if tab === 'directory'}
        <!-- State, always visible: is it on, is it usable, and can the admin
             still get in with a local password? -->
        <div
          data-testid="directory-status"
          role="status"
          class={cn(
            'flex items-start gap-3 p-4 rounded-xl border',
            status.problem || (!settings.enabled && breakGlass.local_admins === 0)
              ? 'bg-[var(--hv-warning-bg)] border-[var(--hv-warning)]'
              : settings.enabled && status.usable
                ? 'bg-[var(--hv-success-bg)] border-[var(--hv-success)]'
                : 'bg-[var(--hv-shell-border-soft)] border-[var(--hv-shell-border)]',
          )}
        >
          {#if status.problem}
            <IconAlertTriangle size={20} strokeWidth={1.8} class="flex-none text-[var(--hv-warning)]" aria-hidden="true" />
          {:else if settings.enabled && status.usable}
            <IconCheck size={20} strokeWidth={1.8} class="flex-none text-[var(--hv-success)]" aria-hidden="true" />
          {:else}
            <IconInfoCircle size={20} strokeWidth={1.8} class="flex-none text-[var(--hv-shell-text-2)]" aria-hidden="true" />
          {/if}
          <div class="flex flex-col gap-1 min-w-0">
            {#if status.problem}
              <span class="font-semibold text-[var(--hv-shell-text-strong)]" data-testid="directory-status-problem">
                {translate('directory.status_problem')}
              </span>
              <span class={hint}>{status.problem}</span>
            {:else if !settings.enabled}
              <span class="font-semibold text-[var(--hv-shell-text-strong)]" data-testid="directory-status-off">
                {translate('directory.status_off')}
              </span>
              <span class={hint}>{translate('directory.status_off_hint')}</span>
            {:else}
              <span class="font-semibold text-[var(--hv-shell-text-strong)]" data-testid="directory-status-on">
                {translate('directory.status_on', { count: String(counts.total) })}
              </span>
              <span class={hint}>{translate('directory.status_on_hint')}</span>
            {/if}

            <span
              class="flex items-center gap-1.5 text-c-label"
              data-testid="directory-break-glass"
              class:text-[var(--hv-danger)]={breakGlass.local_admins === 0}
              class:text-[var(--hv-shell-text-2)]={breakGlass.local_admins > 0}
            >
              <IconKey size={14} strokeWidth={1.8} aria-hidden="true" />
              {breakGlass.local_admins > 0
                ? translate('directory.break_glass', { count: String(breakGlass.local_admins) })
                : translate('directory.break_glass_none')}
            </span>
          </div>
        </div>

        <SectionCard
          title={translate('directory.enable_title')}
          description={translate('directory.enable_desc')}
          data-testid="directory-enable"
        >
          <div class="flex items-center justify-between gap-4 flex-wrap">
            <div class="flex flex-col gap-0.5 min-w-0">
              <span class="flex items-center gap-2">
                <!-- The visible label names the switch through `aria-labelledby`;
                     passing it as `label` too would announce it twice. -->
                <span id="directory-enabled-label" class={fieldLabel}>
                  {translate('directory.enabled')}
                </span>
                <Badge variant={settings.enabled ? 'success' : 'secondary'} size="sm">
                  {settings.enabled ? translate('directory.state_on') : translate('directory.state_off')}
                </Badge>
              </span>
              <span class={hint}>{translate('directory.enabled_hint')}</span>
            </div>
            <div data-testid="directory-enabled-toggle" class="flex-none">
              <Toggle bind:checked={settings.enabled} ariaLabelledby="directory-enabled-label" />
            </div>
          </div>
        </SectionCard>

        <SectionCard
          title={translate('directory.server_title')}
          description={translate('directory.server_desc')}
          data-testid="directory-server"
        >
          <div class="flex flex-col gap-4">
            <label class="flex flex-col gap-1.5">
              <span class={fieldLabel}>{translate('directory.provider')}</span>
              <select
                class={inp}
                value={settings.provider}
                data-testid="directory-provider"
                onchange={(e) => setProvider((e.currentTarget as HTMLSelectElement).value)}
              >
                <option value="ldap">{translate('directory.provider_ldap')}</option>
                <option value="ad">{translate('directory.provider_ad')}</option>
              </select>
              <span class={hint}>{translate('directory.provider_hint')}</span>
            </label>

            <label class="flex flex-col gap-1.5">
              <span class={fieldLabel}>{translate('directory.server_url')}</span>
              <input
                class={cn(inp, 'font-mono')}
                type="text"
                bind:value={settings.server_url}
                placeholder="ldap://192.168.1.10:389"
                spellcheck="false"
                autocomplete="off"
                data-testid="directory-server-url"
              />
              <span class={hint}>{translate('directory.server_url_hint')}</span>
            </label>

            <div class="flex flex-col gap-3">
              <Checkbox
                bind:checked={settings.start_tls}
                disabled={serverIsLdaps}
                label={translate('directory.start_tls')}
                description={serverIsLdaps
                  ? translate('directory.start_tls_ldaps')
                  : translate('directory.start_tls_hint')}
              />
              <Checkbox
                bind:checked={settings.verify_tls}
                label={translate('directory.verify_tls')}
                description={translate('directory.verify_tls_hint')}
              />
            </div>

            <label class="flex flex-col gap-1.5">
              <span class={fieldLabel}>{translate('directory.base_dn')}</span>
              <input
                class={cn(inp, 'font-mono')}
                type="text"
                bind:value={settings.base_dn}
                placeholder="dc=example,dc=com"
                spellcheck="false"
                autocomplete="off"
                data-testid="directory-base-dn"
              />
              <span class={hint}>{translate('directory.base_dn_hint')}</span>
            </label>

            <label class="flex flex-col gap-1.5">
              <span class={fieldLabel}>{translate('directory.bind_dn')}</span>
              <input
                class={cn(inp, 'font-mono')}
                type="text"
                bind:value={settings.bind_dn}
                placeholder="cn=homevault,dc=example,dc=com"
                spellcheck="false"
                autocomplete="off"
                data-testid="directory-bind-dn"
              />
              <span class={hint}>{translate('directory.bind_dn_hint')}</span>
            </label>

            <label class="flex flex-col gap-1.5">
              <span class={fieldLabel}>{translate('directory.bind_password')}</span>
              <input
                class={inp}
                type="password"
                bind:value={bindPassword}
                autocomplete="new-password"
                disabled={clearBindPassword}
                data-testid="directory-bind-password"
                placeholder={settings.has_bind_password || clearBindPassword
                  ? translate('directory.bind_password_stored')
                  : ''}
              />
              <span class={hint}>
                {clearBindPassword
                  ? translate('directory.bind_password_will_clear')
                  : settings.has_bind_password
                    ? translate('directory.bind_password_keep')
                    : translate('directory.bind_password_hint')}
              </span>
              {#if settings.has_bind_password && !clearBindPassword}
                <button
                  type="button"
                  class="self-start text-c-label font-semibold text-[var(--hv-primary)] bg-transparent border-none cursor-pointer p-0 underline"
                  data-testid="directory-clear-password"
                  onclick={() => {
                    clearBindPassword = true;
                    bindPassword = '';
                  }}
                >
                  {translate('directory.bind_password_clear')}
                </button>
              {:else if clearBindPassword}
                <button
                  type="button"
                  class="self-start text-c-label font-semibold text-[var(--hv-primary)] bg-transparent border-none cursor-pointer p-0 underline"
                  data-testid="directory-keep-password"
                  onclick={() => {
                    clearBindPassword = false;
                  }}
                >
                  {translate('directory.bind_password_undo')}
                </button>
              {/if}
            </label>
          </div>
        </SectionCard>

        <!-- Test panel: uses the form as typed, so an admin can find out it is
             wrong before saving it. -->
        <SectionCard
          title={translate('directory.test_title')}
          description={translate('directory.test_desc')}
          data-testid="directory-test"
        >
          <div class="flex flex-col gap-3">
            <div class="flex items-end gap-2 flex-wrap">
              <Button
                variant="secondary"
                data-testid="directory-test-service"
                loading={testing === 'service'}
                disabled={testing !== null || busy !== null}
                onclick={() => test('service')}
              >
                <IconPlugConnected size={16} strokeWidth={1.8} /> {translate('directory.test_service')}
              </Button>
            </div>

            <div class="grid gap-3 sm:grid-cols-2">
              <label class="flex flex-col gap-1.5">
                <span class={fieldLabel}>{translate('directory.test_username')}</span>
                <input
                  class={inp}
                  type="text"
                  bind:value={testUsername}
                  autocomplete="off"
                  spellcheck="false"
                  data-testid="directory-test-username"
                />
              </label>
              <label class="flex flex-col gap-1.5">
                <span class={fieldLabel}>{translate('directory.test_password')}</span>
                <input
                  class={inp}
                  type="password"
                  bind:value={testPassword}
                  autocomplete="new-password"
                  data-testid="directory-test-password"
                />
              </label>
            </div>
            <div class="flex items-center gap-2">
              <Button
                variant="secondary"
                data-testid="directory-test-signin"
                loading={testing === 'signin'}
                disabled={testing !== null || busy !== null || testUsername.trim() === ''}
                onclick={() => test('signin')}
              >
                <IconKey size={16} strokeWidth={1.8} /> {translate('directory.test_signin')}
              </Button>
            </div>

            {#if testResult}
              <p
                role="alert"
                class={cn(
                  'm-0 p-3 rounded-lg text-c-label',
                  testResult.ok
                    ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]'
                    : 'bg-[var(--hv-error-bg)] text-[var(--hv-danger)]',
                )}
                data-testid="directory-test-result"
              >
                {testResult.message}
                {#if testResult.role}
                  <span class="block font-semibold">
                    {translate('directory.test_role', { role: roleLabel(testResult.role === 'admin' ? 2 : 1) })}
                  </span>
                {/if}
              </p>
            {/if}
          </div>
        </SectionCard>

        <SectionCard
          title={translate('directory.users_title')}
          description={translate('directory.users_desc')}
          data-testid="directory-users"
        >
          <div class="flex flex-col gap-4">
            <label class="flex flex-col gap-1.5">
              <span class={fieldLabel}>{translate('directory.user_filter')}</span>
              <input
                class={cn(inp, 'font-mono')}
                type="text"
                bind:value={settings.user_filter}
                spellcheck="false"
                autocomplete="off"
                data-testid="directory-user-filter"
              />
              <span class={hint}>{translate('directory.user_filter_hint')}</span>
            </label>

            <label class="flex flex-col gap-1.5">
              <span class={fieldLabel}>{translate('directory.group_filter')}</span>
              <input
                class={cn(inp, 'font-mono')}
                type="text"
                bind:value={settings.group_filter}
                spellcheck="false"
                autocomplete="off"
                data-testid="directory-group-filter"
              />
              <span class={hint}>{translate('directory.group_filter_hint')}</span>
            </label>

            <details class="rounded-lg border border-[var(--hv-shell-border)] p-3" data-testid="directory-advanced">
              <summary class="cursor-pointer text-c-label font-semibold text-[var(--hv-shell-text-strong)]">
                {translate('directory.advanced')}
              </summary>
              <div class="flex flex-col gap-4 pt-3">
                <div class="grid gap-3 sm:grid-cols-2">
                  <label class="flex flex-col gap-1.5">
                    <span class={fieldLabel}>{translate('directory.attr_username')}</span>
                    <input
                      class={cn(inp, 'font-mono')}
                      type="text"
                      bind:value={settings.attr_username}
                      spellcheck="false"
                      data-testid="directory-attr-username"
                    />
                  </label>
                  <label class="flex flex-col gap-1.5">
                    <span class={fieldLabel}>{translate('directory.attr_display_name')}</span>
                    <input
                      class={cn(inp, 'font-mono')}
                      type="text"
                      bind:value={settings.attr_display_name}
                      spellcheck="false"
                      data-testid="directory-attr-display-name"
                    />
                  </label>
                  <label class="flex flex-col gap-1.5">
                    <span class={fieldLabel}>{translate('directory.attr_email')}</span>
                    <input
                      class={cn(inp, 'font-mono')}
                      type="text"
                      bind:value={settings.attr_email}
                      spellcheck="false"
                      data-testid="directory-attr-email"
                    />
                  </label>
                  <label class="flex flex-col gap-1.5">
                    <span class={fieldLabel}>{translate('directory.attr_member_of')}</span>
                    <input
                      class={cn(inp, 'font-mono')}
                      type="text"
                      bind:value={settings.attr_member_of}
                      spellcheck="false"
                      data-testid="directory-attr-member-of"
                    />
                  </label>
                </div>

                <label class="flex flex-col gap-1.5">
                  <span class={fieldLabel}>{translate('directory.cache_ttl')}</span>
                  <input
                    class={inp}
                    type="number"
                    min="0"
                    max="3600"
                    step="30"
                    bind:value={settings.cache_ttl_seconds}
                    data-testid="directory-cache-ttl"
                  />
                  <span class={hint}>{translate('directory.cache_ttl_hint')}</span>
                  <span class="text-c-label text-[var(--hv-shell-text-2)]" data-testid="directory-cache-stats">
                    {translate('directory.cache_stats', {
                      hits: String(status.cache.hits),
                      misses: String(status.cache.misses),
                      entries: String(status.cache.entries),
                    })}
                  </span>
                </label>
              </div>
            </details>
          </div>
        </SectionCard>

        <SectionCard
          title={translate('directory.roles_title')}
          description={translate('directory.roles_desc')}
          data-testid="directory-roles"
        >
          <div class="flex flex-col gap-4">
            <label class="flex flex-col gap-1.5">
              <span class={fieldLabel}>{translate('directory.default_role')}</span>
              <select class={inp} bind:value={settings.default_role} data-testid="directory-default-role">
                <option value="user">{translate('directory.role_user')}</option>
                <option value="admin">{translate('directory.role_admin')}</option>
              </select>
              <span class={hint}>{translate('directory.default_role_hint')}</span>
            </label>

            <div class="flex flex-col gap-2">
              {#if settings.group_roles.length === 0}
                <EmptyState
                  compact
                  icon="key"
                  title={translate('directory.mappings_empty')}
                  description={translate('directory.mappings_empty_hint')}
                  data-testid="directory-mappings-empty"
                />
              {:else}
                {#each settings.group_roles as mapping, index (index)}
                  <div
                    class="flex items-end gap-2 flex-wrap p-3 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]"
                    data-testid="directory-mapping-{index}"
                  >
                    <label class="flex flex-col gap-1.5 flex-1 min-w-[220px]">
                      <span class={fieldLabel}>{translate('directory.mapping_group')}</span>
                      <input
                        class={cn(inp, 'font-mono')}
                        type="text"
                        bind:value={mapping.group}
                        placeholder="cn=admins,ou=groups,dc=example,dc=com"
                        spellcheck="false"
                        autocomplete="off"
                        data-testid="directory-mapping-group-{index}"
                      />
                    </label>
                    <label class="flex flex-col gap-1.5 w-[170px]">
                      <span class={fieldLabel}>{translate('directory.mapping_role')}</span>
                      <select class={inp} bind:value={mapping.role} data-testid="directory-mapping-role-{index}">
                        <option value="user">{translate('directory.role_user')}</option>
                        <option value="admin">{translate('directory.role_admin')}</option>
                      </select>
                    </label>
                    <button
                      type="button"
                      class={cn(iconBtn, 'hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)]')}
                      aria-label={translate('directory.mapping_remove_named', {
                        group: mapping.group || translate('directory.mapping_unnamed'),
                      })}
                      data-testid="directory-mapping-remove-{index}"
                      onclick={() => removeMapping(index)}
                    ><IconTrash size={16} strokeWidth={1.8} /></button>
                  </div>
                {/each}
              {/if}
              <div>
                <Button variant="secondary" size="sm" data-testid="directory-add-mapping" onclick={addMapping}>
                  <IconPlus size={15} strokeWidth={1.8} /> {translate('directory.mapping_add')}
                </Button>
              </div>
              <p class={hint}>{translate('directory.mapping_hint')}</p>
            </div>
          </div>
        </SectionCard>

      {:else if tab === 'oidc'}
        <!-- State, always visible: is single sign-on on, does the provider
             answer, and can an administrator still get in with a local
             password? -->
        <div
          data-testid="oidc-status"
          role="status"
          class={cn(
            'flex items-start gap-3 p-4 rounded-xl border',
            oidcStatus.problem || (!oidc.enabled && oidcLocalAdmins === 0)
              ? 'bg-[var(--hv-warning-bg)] border-[var(--hv-warning)]'
              : oidc.enabled && oidcStatus.usable
                ? 'bg-[var(--hv-success-bg)] border-[var(--hv-success)]'
                : 'bg-[var(--hv-shell-border-soft)] border-[var(--hv-shell-border)]',
          )}
        >
          {#if oidcStatus.problem}
            <IconAlertTriangle size={20} strokeWidth={1.8} class="flex-none text-[var(--hv-warning)]" aria-hidden="true" />
          {:else if oidc.enabled && oidcStatus.usable}
            <IconCheck size={20} strokeWidth={1.8} class="flex-none text-[var(--hv-success)]" aria-hidden="true" />
          {:else}
            <IconInfoCircle size={20} strokeWidth={1.8} class="flex-none text-[var(--hv-shell-text-2)]" aria-hidden="true" />
          {/if}
          <div class="flex flex-col gap-1 min-w-0">
            {#if oidcStatus.problem}
              <span class="font-semibold text-[var(--hv-shell-text-strong)]" data-testid="oidc-status-problem">
                {translate('oidc.status_problem')}
              </span>
              <span class={hint}>{oidcStatus.problem}</span>
            {:else if !oidc.enabled}
              <span class="font-semibold text-[var(--hv-shell-text-strong)]" data-testid="oidc-status-off">
                {translate('oidc.status_off')}
              </span>
              <span class={hint}>{translate('oidc.status_off_hint')}</span>
            {:else}
              <span class="font-semibold text-[var(--hv-shell-text-strong)]" data-testid="oidc-status-on">
                {translate('oidc.status_on')}
              </span>
              <span class={hint}>{translate('oidc.status_on_hint')}</span>
            {/if}

            <span
              class="flex items-center gap-1.5 text-c-label"
              data-testid="oidc-break-glass"
              class:text-[var(--hv-danger)]={oidcLocalAdmins === 0}
              class:text-[var(--hv-shell-text-2)]={oidcLocalAdmins > 0}
            >
              <IconKey size={14} strokeWidth={1.8} aria-hidden="true" />
              {oidcLocalAdmins > 0
                ? translate('oidc.break_glass', { count: String(oidcLocalAdmins) })
                : translate('directory.break_glass_none')}
            </span>
          </div>
        </div>

        <SectionCard
          title={translate('oidc.enable_title')}
          description={translate('oidc.enable_desc')}
          data-testid="oidc-enable"
        >
          <div class="flex items-center justify-between gap-4 flex-wrap">
            <div class="flex flex-col gap-0.5 min-w-0">
              <span class="flex items-center gap-2">
                <span id="oidc-enabled-label" class={fieldLabel}>{translate('oidc.enabled')}</span>
                <Badge variant={oidc.enabled ? 'success' : 'secondary'} size="sm">
                  {oidc.enabled ? translate('directory.state_on') : translate('directory.state_off')}
                </Badge>
              </span>
              <span class={hint}>{translate('oidc.enabled_hint')}</span>
            </div>
            <div data-testid="oidc-enabled-toggle" class="flex-none">
              <Toggle bind:checked={oidc.enabled} ariaLabelledby="oidc-enabled-label" />
            </div>
          </div>
        </SectionCard>

        <SectionCard
          title={translate('oidc.provider_title')}
          description={translate('oidc.provider_desc')}
          data-testid="oidc-provider"
        >
          <div class="flex flex-col gap-4">
            <label class="flex flex-col gap-1.5">
              <span class={fieldLabel}>{translate('oidc.label_field')}</span>
              <input
                class={inp}
                type="text"
                bind:value={oidc.label}
                placeholder="Sign in with Authentik"
                autocomplete="off"
                data-testid="oidc-label"
              />
              <span class={hint}>{translate('oidc.label_hint')}</span>
            </label>

            <label class="flex flex-col gap-1.5">
              <span class={fieldLabel}>{translate('oidc.issuer')}</span>
              <input
                class={cn(inp, 'font-mono')}
                type="url"
                bind:value={oidc.issuer}
                placeholder="https://auth.example.com/application/o/homevault"
                spellcheck="false"
                autocomplete="off"
                data-testid="oidc-issuer"
              />
              <span class={hint}>{translate('oidc.issuer_hint')}</span>
            </label>

            <label class="flex flex-col gap-1.5">
              <span class={fieldLabel}>{translate('oidc.client_id')}</span>
              <input
                class={cn(inp, 'font-mono')}
                type="text"
                bind:value={oidc.client_id}
                spellcheck="false"
                autocomplete="off"
                data-testid="oidc-client-id"
              />
            </label>

            <label class="flex flex-col gap-1.5">
              <span class={fieldLabel}>{translate('oidc.client_secret')}</span>
              <input
                class={inp}
                type="password"
                bind:value={oidcSecret}
                autocomplete="new-password"
                disabled={clearOidcSecret}
                placeholder={oidc.has_client_secret || clearOidcSecret
                  ? translate('oidc.secret_stored')
                  : ''}
                data-testid="oidc-client-secret"
              />
              <span class={hint}>
                {clearOidcSecret
                  ? translate('oidc.secret_will_clear')
                  : oidc.has_client_secret
                    ? translate('oidc.secret_keep')
                    : translate('oidc.secret_hint')}
              </span>
              {#if oidc.has_client_secret && !clearOidcSecret}
                <button
                  type="button"
                  class="self-start text-c-label font-semibold text-[var(--hv-primary)] bg-transparent border-none cursor-pointer p-0 underline"
                  data-testid="oidc-clear-secret"
                  onclick={() => {
                    clearOidcSecret = true;
                    oidcSecret = '';
                  }}
                >
                  {translate('oidc.secret_clear')}
                </button>
              {:else if clearOidcSecret}
                <button
                  type="button"
                  class="self-start text-c-label font-semibold text-[var(--hv-primary)] bg-transparent border-none cursor-pointer p-0 underline"
                  data-testid="oidc-keep-secret"
                  onclick={() => {
                    clearOidcSecret = false;
                  }}
                >
                  {translate('oidc.secret_undo')}
                </button>
              {/if}
            </label>

            <label class="flex flex-col gap-1.5">
              <span class={fieldLabel}>{translate('oidc.redirect')}</span>
              <input
                class={cn(inp, 'font-mono')}
                type="url"
                bind:value={oidc.redirect_url}
                spellcheck="false"
                autocomplete="off"
                data-testid="oidc-redirect"
              />
              <span class={hint}>{translate('oidc.redirect_hint')}</span>
            </label>

            <details class="rounded-lg border border-[var(--hv-shell-border)] p-3" data-testid="oidc-claims">
              <summary class="cursor-pointer text-c-label font-semibold text-[var(--hv-shell-text-strong)]">
                {translate('oidc.claims_summary')}
              </summary>
              <div class="flex flex-col gap-4 pt-3">
                <div class="grid gap-3 sm:grid-cols-2">
                  <label class="flex flex-col gap-1.5">
                    <span class={fieldLabel}>{translate('oidc.attr_username')}</span>
                    <input
                      class={cn(inp, 'font-mono')}
                      type="text"
                      bind:value={oidc.attr_username}
                      spellcheck="false"
                      data-testid="oidc-attr-username"
                    />
                  </label>
                  <label class="flex flex-col gap-1.5">
                    <span class={fieldLabel}>{translate('oidc.attr_display_name')}</span>
                    <input
                      class={cn(inp, 'font-mono')}
                      type="text"
                      bind:value={oidc.attr_display_name}
                      spellcheck="false"
                      data-testid="oidc-attr-display-name"
                    />
                  </label>
                  <label class="flex flex-col gap-1.5">
                    <span class={fieldLabel}>{translate('oidc.attr_email')}</span>
                    <input
                      class={cn(inp, 'font-mono')}
                      type="text"
                      bind:value={oidc.attr_email}
                      spellcheck="false"
                      data-testid="oidc-attr-email"
                    />
                  </label>
                  <label class="flex flex-col gap-1.5">
                    <span class={fieldLabel}>{translate('oidc.attr_groups')}</span>
                    <input
                      class={cn(inp, 'font-mono')}
                      type="text"
                      bind:value={oidc.attr_groups}
                      spellcheck="false"
                      data-testid="oidc-attr-groups"
                    />
                  </label>
                </div>

                <label class="flex flex-col gap-1.5">
                  <span class={fieldLabel}>{translate('oidc.scopes')}</span>
                  <input
                    class={cn(inp, 'font-mono')}
                    type="text"
                    bind:value={oidc.scopes}
                    spellcheck="false"
                    autocomplete="off"
                    data-testid="oidc-scopes"
                  />
                  <span class={hint}>{translate('oidc.scopes_hint')}</span>
                </label>
              </div>
            </details>
          </div>
        </SectionCard>

        <!-- Test panel: uses the form as typed, so a wrong issuer is found
             before it is saved. -->
        <SectionCard
          title={translate('oidc.test_title')}
          description={translate('oidc.test_desc')}
          data-testid="oidc-test"
        >
          <div class="flex flex-col gap-3">
            <div>
              <Button
                variant="secondary"
                data-testid="oidc-test-run"
                loading={oidcTesting}
                disabled={oidcTesting || busy !== null}
                onclick={testOidc}
              >
                <IconPlugConnected size={16} strokeWidth={1.8} /> {translate('oidc.test_button')}
              </Button>
            </div>
            {#if oidcTestResult}
              <p
                role="alert"
                class={cn(
                  'm-0 p-3 rounded-lg text-c-label',
                  oidcTestResult.ok
                    ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]'
                    : 'bg-[var(--hv-error-bg)] text-[var(--hv-danger)]',
                )}
                data-testid="oidc-test-result"
              >
                {oidcTestResult.message}
              </p>
            {/if}
          </div>
        </SectionCard>

        <SectionCard
          title={translate('oidc.roles_title')}
          description={translate('oidc.roles_desc')}
          data-testid="oidc-roles"
        >
          <div class="flex flex-col gap-4">
            <label class="flex flex-col gap-1.5">
              <span class={fieldLabel}>{translate('directory.default_role')}</span>
              <select class={inp} bind:value={oidc.default_role} data-testid="oidc-default-role">
                <option value="user">{translate('directory.role_user')}</option>
                <option value="admin">{translate('directory.role_admin')}</option>
              </select>
              <span class={hint}>{translate('directory.default_role_hint')}</span>
            </label>

            <div class="flex flex-col gap-2">
              {#if oidc.group_roles.length === 0}
                <EmptyState
                  compact
                  icon="key"
                  title={translate('directory.mappings_empty')}
                  description={translate('directory.mappings_empty_hint')}
                  data-testid="oidc-mappings-empty"
                />
              {:else}
                {#each oidc.group_roles as mapping, index (index)}
                  <div
                    class="flex items-end gap-2 flex-wrap p-3 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]"
                    data-testid="oidc-mapping-{index}"
                  >
                    <label class="flex flex-col gap-1.5 flex-1 min-w-[220px]">
                      <span class={fieldLabel}>{translate('directory.mapping_group')}</span>
                      <input
                        class={cn(inp, 'font-mono')}
                        type="text"
                        bind:value={mapping.group}
                        placeholder="admins"
                        spellcheck="false"
                        autocomplete="off"
                        data-testid="oidc-mapping-group-{index}"
                      />
                    </label>
                    <label class="flex flex-col gap-1.5 w-[170px]">
                      <span class={fieldLabel}>{translate('directory.mapping_role')}</span>
                      <select class={inp} bind:value={mapping.role} data-testid="oidc-mapping-role-{index}">
                        <option value="user">{translate('directory.role_user')}</option>
                        <option value="admin">{translate('directory.role_admin')}</option>
                      </select>
                    </label>
                    <button
                      type="button"
                      class={cn(iconBtn, 'hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)]')}
                      aria-label={translate('directory.mapping_remove_named', {
                        group: mapping.group || translate('directory.mapping_unnamed'),
                      })}
                      data-testid="oidc-mapping-remove-{index}"
                      onclick={() => removeOidcMapping(index)}
                    ><IconTrash size={16} strokeWidth={1.8} /></button>
                  </div>
                {/each}
              {/if}
              <div>
                <Button variant="secondary" size="sm" data-testid="oidc-add-mapping" onclick={addOidcMapping}>
                  <IconPlus size={15} strokeWidth={1.8} /> {translate('directory.mapping_add')}
                </Button>
              </div>
              <p class={hint}>{translate('oidc.mapping_hint')}</p>
            </div>
          </div>
        </SectionCard>
      {:else}
        <!-- SAML 2.0: the third provider. Same shape as the other two, with
             one extra step — the identity provider's document is imported
             rather than typed, because a certificate copied by hand is the
             most common way a SAML integration fails. -->
        <div
          data-testid="saml-status"
          role="status"
          class={cn(
            'flex items-start gap-3 p-4 rounded-xl border',
            samlStatus.problem || (saml.enabled && !samlStatus.usable)
              ? 'bg-[var(--hv-warning-bg)] border-[var(--hv-warning)]'
              : samlStatus.usable
                ? 'bg-[var(--hv-success-bg)] border-[var(--hv-success)]'
                : 'bg-[var(--hv-shell-border-soft)] border-[var(--hv-shell-border)]',
          )}
        >
          <span class="flex-none mt-0.5">
            {#if samlStatus.problem || (saml.enabled && !samlStatus.usable)}
              <IconAlertTriangle size={18} strokeWidth={1.8} />
            {:else}
              <IconInfoCircle size={18} strokeWidth={1.8} />
            {/if}
          </span>
          <div class="flex flex-col gap-1 min-w-0">
            <span class="font-semibold text-[var(--hv-shell-text-strong)]">
              {#if samlStatus.problem}
                {translate('saml.status_problem')}
              {:else if samlStatus.usable}
                {translate('saml.status_on')}
              {:else if saml.enabled}
                {translate('saml.status_incomplete')}
              {:else}
                {translate('saml.status_off')}
              {/if}
            </span>
            {#if samlStatus.problem}
              <span class="text-c-label text-[var(--hv-shell-text-2)]">{samlStatus.problem}</span>
            {:else if samlStatus.usable}
              <span class="text-c-label text-[var(--hv-shell-text-2)]">
                {samlCounts.total} {translate('saml.status_accounts')}
              </span>
            {:else}
              <span class="text-c-label text-[var(--hv-shell-text-2)]">{translate('saml.status_off_hint')}</span>
            {/if}
            <span
              class={cn(
                'text-c-label font-medium',
                samlLocalAdmins === 0 ? 'text-[var(--hv-error)]' : 'text-[var(--hv-shell-text-2)]',
              )}
            >
              {samlLocalAdmins} {translate('directory.break_glass_count')}
            </span>
          </div>
        </div>

        <SectionCard
          title={translate('saml.enable_title')}
          description={translate('saml.enable_desc')}
          data-testid="saml-enable"
        >
          <div class="flex items-center justify-between gap-4 flex-wrap">
            <div class="flex flex-col gap-0.5 min-w-0">
              <span class="flex items-center gap-2">
                <span id="saml-enabled-label" class={fieldLabel}>{translate('saml.enabled')}</span>
                <Badge variant={saml.enabled ? 'success' : 'secondary'} size="sm">
                  {saml.enabled ? translate('directory.state_on') : translate('directory.state_off')}
                </Badge>
              </span>
              <span class={hint}>{translate('saml.enabled_hint')}</span>
            </div>
            <div data-testid="saml-enabled-toggle" class="flex-none">
              <Toggle bind:checked={saml.enabled} ariaLabelledby="saml-enabled-label" />
            </div>
          </div>
        </SectionCard>

        <SectionCard
          title={translate('saml.import_title')}
          description={translate('saml.import_desc')}
          data-testid="saml-import"
        >
          <div class="flex flex-col gap-3">
            <label class="flex flex-col gap-1">
              <span class={fieldLabel}>{translate('saml.metadata_url')}</span>
              <input
                class={inp}
                type="url"
                inputmode="url"
                autocomplete="off"
                spellcheck="false"
                placeholder="https://idp.example.com/app/metadata"
                bind:value={saml.idp_metadata_url}
                data-testid="saml-metadata-url"
              />
            </label>
            <div class="flex items-center gap-2 flex-wrap">
              <Button
                variant="secondary"
                data-testid="saml-test"
                disabled={samlTesting || !saml.idp_metadata_url.trim()}
                onclick={testSaml}
              >
                {#if samlTesting}<Spinner size="sm" />{/if}
                {translate('saml.test_button')}
              </Button>
              {#if samlImported}
                <Button variant="primary" data-testid="saml-apply-import" onclick={applyImported}>
                  <IconCheck size={15} strokeWidth={1.8} /> {translate('saml.apply_import')}
                </Button>
              {/if}
            </div>
            {#if samlTestResult}
              <div data-testid="saml-test-result">
                <Alert variant={samlTestResult.ok ? 'success' : 'error'}>{samlTestResult.message}</Alert>
              </div>
            {/if}
            {#if samlImported}
              <div class="flex flex-col gap-1 p-3 rounded-lg bg-[var(--hv-shell-border-soft)]">
                <span class="text-c-label text-[var(--hv-shell-text-2)]">
                  {translate('saml.import_entity')}: <strong>{samlImported.entity_id}</strong>
                </span>
                <span class="text-c-label text-[var(--hv-shell-text-2)]">
                  {translate('saml.import_sso')}: <strong>{samlImported.sso_url}</strong>
                </span>
                <span class="text-c-label text-[var(--hv-shell-text-2)]">
                  {translate('saml.import_certificates')}: <strong>{samlImported.certificates.length}</strong>
                </span>
              </div>
            {/if}
          </div>
        </SectionCard>

        <SectionCard
          title={translate('saml.provider_title')}
          description={translate('saml.provider_desc')}
          data-testid="saml-provider"
        >
          <div class="flex flex-col gap-4">
            <label class="flex flex-col gap-1">
              <span class={fieldLabel}>{translate('saml.label')}</span>
              <input
                class={inp}
                type="text"
                maxlength="64"
                placeholder={translate('saml.label_placeholder')}
                bind:value={saml.label}
                data-testid="saml-label"
              />
              <span class={hint}>{translate('saml.label_hint')}</span>
            </label>
            <label class="flex flex-col gap-1">
              <span class={fieldLabel}>{translate('saml.idp_entity_id')}</span>
              <input
                class={inp}
                type="text"
                autocomplete="off"
                spellcheck="false"
                placeholder="https://idp.example.com/metadata"
                bind:value={saml.idp_entity_id}
                data-testid="saml-idp-entity-id"
              />
            </label>
            <label class="flex flex-col gap-1">
              <span class={fieldLabel}>{translate('saml.idp_sso_url')}</span>
              <input
                class={inp}
                type="url"
                inputmode="url"
                autocomplete="off"
                spellcheck="false"
                placeholder="https://idp.example.com/sso"
                bind:value={saml.idp_sso_url}
                data-testid="saml-idp-sso-url"
              />
            </label>
            <label class="flex flex-col gap-1">
              <span class={fieldLabel}>{translate('saml.certificates')}</span>
              <textarea
                class={cn(inp, 'h-32 py-2 font-mono text-c-label resize-y')}
                spellcheck="false"
                placeholder="-----BEGIN CERTIFICATE-----"
                value={saml.idp_certificates.join('\n')}
                oninput={(event) => {
                  const text = (event.currentTarget as HTMLTextAreaElement).value;
                  saml.idp_certificates = text.trim() ? [text] : [];
                }}
                data-testid="saml-certificates"
              ></textarea>
              <span class={hint}>{translate('saml.certificates_hint')}</span>
            </label>
            <div class="grid gap-4 md:grid-cols-2">
              <label class="flex flex-col gap-1">
                <span class={fieldLabel}>{translate('saml.sp_entity_id')}</span>
                <input
                  class={inp}
                  type="text"
                  autocomplete="off"
                  spellcheck="false"
                  bind:value={saml.sp_entity_id}
                  data-testid="saml-sp-entity-id"
                />
              </label>
              <label class="flex flex-col gap-1">
                <span class={fieldLabel}>{translate('saml.acs_url')}</span>
                <input
                  class={inp}
                  type="url"
                  inputmode="url"
                  autocomplete="off"
                  spellcheck="false"
                  bind:value={saml.acs_url}
                  data-testid="saml-acs-url"
                />
              </label>
            </div>
            <p class={hint}>{translate('saml.acs_hint')}</p>
          </div>
        </SectionCard>

        <SectionCard
          title={translate('saml.roles_title')}
          description={translate('saml.roles_desc')}
          data-testid="saml-roles"
        >
          <div class="flex flex-col gap-4">
            <label class="flex flex-col gap-1 max-w-[320px]">
              <span class={fieldLabel}>{translate('directory.default_role')}</span>
              <select class={inp} bind:value={saml.default_role} data-testid="saml-default-role">
                <option value="user">{translate('directory.role_user')}</option>
                <option value="admin">{translate('directory.role_admin')}</option>
              </select>
            </label>
            <div class="flex flex-col gap-2" data-testid="saml-mappings">
              {#each saml.group_roles as mapping, index (index)}
                <div class="flex items-center gap-2 flex-wrap">
                  <input
                    class={cn(inp, 'flex-1 min-w-[200px]')}
                    type="text"
                    autocomplete="off"
                    spellcheck="false"
                    placeholder={translate('directory.mapping_group_placeholder')}
                    bind:value={mapping.group}
                    aria-label={translate('directory.mapping_group')}
                    data-testid={`saml-mapping-group-${index}`}
                  />
                  <select
                    class={cn(inp, 'w-[180px]')}
                    bind:value={mapping.role}
                    aria-label={translate('directory.mapping_role')}
                    data-testid={`saml-mapping-role-${index}`}
                  >
                    <option value="user">{translate('directory.role_user')}</option>
                    <option value="admin">{translate('directory.role_admin')}</option>
                  </select>
                  <button
                    type="button"
                    class={iconBtn}
                    aria-label={translate('directory.mapping_remove')}
                    data-testid={`saml-mapping-remove-${index}`}
                    onclick={() => removeSamlMapping(index)}
                  >
                    <IconTrash size={16} strokeWidth={1.8} />
                  </button>
                </div>
              {/each}
              <div>
                <Button variant="secondary" size="sm" data-testid="saml-add-mapping" onclick={addSamlMapping}>
                  <IconPlus size={15} strokeWidth={1.8} /> {translate('directory.mapping_add')}
                </Button>
              </div>
              <p class={hint}>{translate('saml.mapping_hint')}</p>
            </div>
          </div>
        </SectionCard>

        <SectionCard
          title={translate('saml.sp_metadata_title')}
          description={translate('saml.sp_metadata_desc')}
          data-testid="saml-sp-metadata"
        >
          <div class="flex flex-col gap-3">
            <div class="flex items-center gap-2 flex-wrap">
              <Button variant="secondary" data-testid="saml-show-metadata" onclick={loadSpMetadata}>
                <IconRefresh size={15} strokeWidth={1.8} /> {translate('saml.sp_metadata_button')}
              </Button>
              {#if samlSpMetadata}
                <Button variant="secondary" data-testid="saml-copy-metadata" onclick={copySpMetadata}>
                  {translate('saml.metadata_copy')}
                </Button>
              {/if}
            </div>
            {#if samlSpMetadata}
              <pre
                class="m-0 p-3 rounded-lg bg-[var(--hv-shell-border-soft)] text-c-label overflow-auto max-h-64 whitespace-pre-wrap break-all"
                data-testid="saml-metadata-xml">{samlSpMetadata}</pre>
            {/if}
            <p class={hint}>{translate('saml.sp_metadata_hint')}</p>
          </div>
        </SectionCard>
      {/if}

      <SectionCard
        title={translate('directory.identities_title')}
        description={tab === 'directory'
          ? translate('directory.identities_desc')
          : tab === 'oidc'
            ? translate('oidc.identities_desc')
            : translate('saml.identities_desc')}
        data-testid="directory-identities"
      >
        {#snippet actions()}
          <span class="text-c-label text-[var(--hv-shell-text-2)]" data-testid="directory-counts">
            {translate('directory.counts', {
              admins: String(counts.admins),
              users: String(counts.users),
              disabled: String(counts.disabled),
            })}
          </span>
        {/snippet}
        <div class="flex flex-col gap-2">
          {#if identities.length === 0}
            <EmptyState
              icon="users"
              title={translate('directory.identities_empty')}
              description={translate('directory.identities_empty_hint')}
              data-testid="directory-identities-empty"
            />
          {:else}
            {#each identities as identity (identity.id)}
              <div
                class={cn(
                  'flex items-start justify-between gap-3 flex-wrap p-3 rounded-lg border',
                  identity.disabled
                    ? 'bg-[var(--hv-shell-border-soft)] border-[var(--hv-shell-border)] opacity-70'
                    : 'bg-[var(--hv-shell-border-soft)] border-[var(--hv-shell-border)]',
                )}
                data-testid="directory-identity-{identity.id}"
              >
                <div class="flex flex-col gap-0.5 min-w-0">
                  <span class="flex items-center gap-2 flex-wrap">
                    <span class="font-semibold text-[var(--hv-shell-text-strong)]">{identity.username}</span>
                    <Badge variant={identity.role_id === 2 ? 'warning' : 'secondary'} size="sm">
                      {roleLabel(identity.role_id)}
                    </Badge>
                    {#if identity.provider === 'oidc'}
                      <Badge variant="secondary" size="sm">{translate('oidc.provider_badge')}</Badge>
                    {:else if identity.provider === 'saml'}
                      <Badge variant="secondary" size="sm">{translate('saml.provider_badge')}</Badge>
                    {/if}
                    {#if identity.disabled}
                      <Badge variant="error" size="sm">{translate('directory.identity_disabled_badge')}</Badge>
                    {/if}
                  </span>
                  {#if identity.display_name && identity.display_name !== identity.username}
                    <span class="text-c-label text-[var(--hv-shell-text-2)]">{identity.display_name}</span>
                  {/if}
                  <span class="text-c-label text-[var(--hv-shell-text-2)] font-mono break-all">{identity.subject}</span>
                  <span class="text-c-label text-[var(--hv-shell-text-2)]">
                    {lastLogin(identity)}
                    <span class="px-1" aria-hidden="true">&middot;</span>
                    {translate('directory.identity_groups', { count: String(identity.groups.length) })}
                  </span>
                </div>
                <div class="flex items-center gap-1 flex-none">
                  <Button
                    variant="ghost"
                    size="sm"
                    data-testid="directory-identity-toggle-{identity.id}"
                    disabled={busy !== null}
                    aria-label={identity.disabled
                      ? translate('directory.identity_enable_named', { name: identity.username })
                      : translate('directory.identity_disable_named', { name: identity.username })}
                    onclick={() => setDisabled(identity, !identity.disabled)}
                  >
                    {identity.disabled
                      ? translate('directory.identity_enable')
                      : translate('directory.identity_disable')}
                  </Button>
                  <button
                    type="button"
                    class={cn(iconBtn, 'hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)]')}
                    aria-label={translate('directory.identity_forget_named', { name: identity.username })}
                    data-testid="directory-identity-forget-{identity.id}"
                    disabled={busy !== null}
                    onclick={() => forget(identity)}
                  ><IconTrash size={16} strokeWidth={1.8} /></button>
                </div>
              </div>
            {/each}
          {/if}
        </div>
      </SectionCard>
    </div>
  {/if}

  {#snippet footer()}
    {#if !loading}
      <span data-testid="directory-footer" class="flex items-center gap-2 flex-wrap">
        {#if tab === 'directory'}
          <span>{formatUpdated(settings.updated_at, settings.updated_by)}</span>
          <span class="text-[var(--hv-shell-text-3)]">&middot;</span>
          <span>{translate('directory.footer_counts', {
            total: String(counts.total),
            local: String(breakGlass.local_admins),
          })}</span>
        {:else if tab === 'oidc'}
          <span>{formatUpdated(oidc.updated_at, oidc.updated_by)}</span>
          <span class="text-[var(--hv-shell-text-3)]">&middot;</span>
          <span>{translate('oidc.footer_counts', {
            total: String(oidcCounts.total),
            local: String(oidcLocalAdmins),
          })}</span>
        {:else}
          <span>{formatUpdated(saml.updated_at, saml.updated_by)}</span>
          <span class="text-[var(--hv-shell-text-3)]">&middot;</span>
          <span>{translate('saml.footer_counts', {
            total: String(samlCounts.total),
            local: String(samlLocalAdmins),
          })}</span>
        {/if}
      </span>
    {/if}
  {/snippet}
</PageLayout>

<ConfirmDialog {dlg} onconfirm={() => dlg.settle(true)} oncancel={() => dlg.settle(false)} />
