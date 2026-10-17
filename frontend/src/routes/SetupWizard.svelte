<!-- ========================================================================
     File          : SetupWizard.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 16/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     The first-run Setup Wizard: admin account (+ optional TOTP enrollment) and
     the final provisioning call — with per-step progress persisted
     server-side so a refresh resumes where the user left off. The interface
     language is detected from the OS (English when unsupported) and changed
     in Settings; storage, RAID and backup restore live in their own apps.
     Local-network only (the backend's setup guard enforces it).

     Dependencies:
       - svelte (>= 5), stores (auth, i18n), $lib/services/api

     Usage:
     Rendered by App.svelte while the app gate is on 'setup'.
     ======================================================================== -->
<script lang="ts">
  import { api } from '$lib/services/api';
  import { onMount, onDestroy, tick } from 'svelte';
  import { authStore } from '$lib/stores/auth';
  import { IconAlertTriangle, IconCheck, IconChevronLeft, IconChevronRight, IconEye, IconEyeOff, IconLoader, IconLock, IconRefresh, IconShield, IconShieldCheckered, IconUser, IconUserCheck } from '@tabler/icons-svelte';
  import { translations, languageStore } from '$lib/stores/i18n';
  import { Button, Alert, Spinner, Switch } from '$lib/components/theme';
  import { cn } from '$lib/utils.js';
  import QRCode from 'qrcode';

  let currentStep = 0;
  const totalSteps = 4;

  let username = '';
  let fullname = '';
  let password = '';
  let confirmPassword = '';
  let showPassword = false;
  let showConfirmPassword = false;
  let isLoading = false;
  let isVerifying = false;
  let verificationResult = { valid: false, errors: [] as string[], warnings: [] as string[] };

  // Real username availability (server-side) used by the verification summary.
  let checkingUsername = false;
  let usernameAvailable: boolean | null = null;

  // Host capability checks (sudo) shown as a warning banner.
  let sudoAvailable = true;
  let capabilitiesLoaded = false;

  // Plain-HTTP warning. The admin password and (when enrolled) the TOTP secret
  // travel in the clear over `http://`, and there is no way for the browser to
  // tell the operator. Shown only for a real network host — on `localhost`
  // nothing leaves the machine. The secure answer is to enable HTTPS in
  // Settings, which the TLS card does in one toggle + restart.
  let insecureConnection = false;

  // ── Reboot-safe progress ─────────────────────────────────────────────
  // The wizard's current step persists server-side (GET/PUT
  // /auth/setup/progress) so a reboot or a closed browser resumes where the
  // operator left off. The payload is always an empty object now: the wizard
  // asks for nothing that is worth remembering (the interface language is
  // detected from the OS and changed in Settings), and secrets — the password,
  // the TOTP secret — are deliberately NOT part of it.
  //
  // This wizard is a legacy-mode (non-runes) Svelte component — its many
  // `$:` reactive statements and `$languageStore` auto-subscription rely on
  // legacy reactivity — so plain `let` + reassignment is used for state and
  // `$:` for derivations. Mixing `$state`/`$derived` in here makes the whole
  // component switch to runes mode and breaks every `$:` in the file.
  let progressLoaded = false;
  let resumedNotice = false;

  function persistProgress() {
    if (!progressLoaded) return;
    api.saveSetupProgress(currentStep, {}).catch(() => {});
  }

  async function loadProgress() {
    try {
      const res = await api.getSetupProgress();
      if (res.success && res.data) {
        if (res.data.step > 0 && res.data.step < totalSteps) {
          currentStep = res.data.step;
          resumedNotice = true;
        }
      }
    } catch {
      /* first boot or unreachable — start at step 0 */
    } finally {
      progressLoaded = true;
    }
  }

  // Optional two-factor authentication enrollment. The TOTP setup moved INTO
  // the wizard at the password step (step 3): the user flips a toggle, scans a
  // QR code rendered client-side from a generated secret, and enters a 6-digit
  // code that the server validates both pre-creation (`/auth/setup/verify-totp`)
  // and again inside the setup call. The recovery code is still generated and
  // shown once AFTER account creation (step 4).
  let setupComplete = false;
  // Mandatory recovery-code screen, shown once right after account creation.
  // The code is the email-free "forgot password" mechanism (see Login.svelte);
  // it is displayed once and never again.
  let showRecovery = false;
  let recoveryCode = '';
  let recoveryError = '';
  let recoveryBusy = false;
  let recoveryAcknowledged = false;

  // TOTP enrollment state at the password step (client-generated secret + QR).
  let totpEnabled = false;
  let totpGenerating = false;
  let totpSecret = '';
  let totpQrDataUrl = '';
  let totpCode = '';
  let totpVerifying = false;
  let totpVerified = false;
  let totpError = '';

  // Brief post-creation summary when TOTP was enrolled at the password step:
  // the one-time backup codes (or a warning when the server could not persist
  // 2FA). `twoFactorEnabled` mirrors the setup response's `totp_enabled`.
  let showTwoFactorSummary = false;
  let twoFactorEnabled = false;
  let twoFactorUnavailable = false;
  let twoFactorBackupCodes: string[] = [];

  // Gateway live clock (24 h), same voice as the login screen.
  let clock = '';
  let dateLabel = '';
  let clockTimer: ReturnType<typeof setInterval>;

  function updateClock() {
    const now = new Date();
    clock = now.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false });
    dateLabel = now.toLocaleDateString([], { weekday: 'long', day: 'numeric', month: 'long', year: 'numeric' });
  }

  onMount(async () => {
    updateClock();
    clockTimer = setInterval(updateClock, 1000);
    await loadCapabilities();
    await loadProgress();
    insecureConnection = isInsecureConnection();
  });

  /// `true` when the page is served over plain HTTP on a non-loopback host.
  function isInsecureConnection(): boolean {
    if (typeof window === 'undefined') return false;
    if (window.location.protocol !== 'http:') return false;
    const host = window.location.hostname;
    return !(host === 'localhost' || host === '127.0.0.1' || host === '[::1]' || host === '::1');
  }

  onDestroy(() => {
    if (clockTimer) clearInterval(clockTimer);
  });

  async function loadCapabilities() {
    try {
      const status = await api.getSetupStatus();
      sudoAvailable = status.sudo_available !== false;
    } catch {
      // If the status call fails, assume available to avoid false alarms.
      sudoAvailable = true;
    } finally {
      capabilitiesLoaded = true;
    }
  }

  // Avatar (DiceBear Miniavs - generated server-side, fully offline).
  // We keep the seed as the source of truth and let the backend render
  // the SVG so nothing about avatar generation ever needs the network.
  let avatarSeed = '';
  let avatarPreview = '';
  let avatarLoading = false;

  function generateSeed(): string {
    const bytes = new Uint8Array(12);
    crypto.getRandomValues(bytes);
    const alphabet = '0123456789abcdefghjkmnpqrstvwxyz';
    let out = '';
    for (const b of bytes) {
      out += alphabet[b % alphabet.length];
    }
    return out;
  }

  async function refreshAvatarPreview(seed: string) {
    avatarLoading = true;
    try {
      avatarPreview = await api.previewAvatarSvg(seed, 128);
    } catch (err) {
      console.warn('Avatar preview failed:', err);
      avatarPreview = '';
    } finally {
      avatarLoading = false;
    }
  }

  function regenerateAvatar() {
    avatarSeed = generateSeed();
    refreshAvatarPreview(avatarSeed);
  }

  // When the user finishes typing a username, suggest a deterministic seed
  // derived from the username so the avatar is "theirs" by default, but
  // still easy to regenerate.
  function suggestAvatarFromUsername() {
    if (!avatarSeed) {
      avatarSeed = generateSeed();
      refreshAvatarPreview(avatarSeed);
    }
  }

  // Follows the user's saved language preference ($languageStore), falling back
  // to English for keys not yet translated in the embedded dictionary.
  $: t = (key: string) =>
    (translations as any)?.[$languageStore]?.[key] || (translations as any)?.en?.[key] || key;

  // Validation states
  let usernameError = '';
  let passwordError = '';
  let confirmPasswordError = '';

  // System account names a new admin account must not collide with. Shared
  // by validateUsername() and verifyAccountCreation() below (was previously
  // duplicated inline in both places).
  const RESERVED_USERNAMES = ['root', 'admin', 'system', 'daemon', 'nobody', 'www-data', 'postgres', 'mysql'];
  const USERNAME_PATTERN = /^[a-zA-Z_][a-zA-Z0-9_]*$/;

  function validateUsername(): boolean {
    if (!username) {
      usernameError = t('setup.validation.username.required');
      return false;
    }
    if (username.length < 3) {
      usernameError = t('setup.validation.username.too_short');
      return false;
    }
    if (!USERNAME_PATTERN.test(username)) {
      usernameError = t('setup.validation.username.invalid_chars');
      return false;
    }
    // Prevent users from choosing system account names that already exist on the server
    if (RESERVED_USERNAMES.includes(username.toLowerCase())) {
      usernameError = t('setup.validation.username.reserved');
      return false;
    }
    usernameError = '';
    return true;
  }

  // Mirror of the backend policy (`SetupService::validate_password`): 12
  // characters minimum, mixed case + digit, no compromised/obvious password and
  // no run of more than 4 identical characters. Kept in sync so the wizard can
  // never enable "Next" for a password the server would reject.
  const MIN_PASSWORD_LEN = 12;
  const MAX_REPEATED_RUN = 4;

  // Obvious / compromised candidates (subset of the backend list — enough to
  // give instant feedback; the server is the authority).
  const BANNED_PASSWORDS = [
    'password', 'passw0rd', 'p@ssw0rd', 'p@ssword', 'qwerty', 'qwertyuiop',
    'azerty', 'letmein', 'welcome', 'iloveyou', 'admin', 'administrator',
    'changeme', 'secret', 'homevault', 'motdepasse', 'passwort', 'contrasena',
  ];

  function isBannedPassword(value: string): boolean {
    const lowered = value.toLowerCase();
    const stripped = lowered.replace(/\d+$/, '');
    const lettersOnly = lowered.replace(/[^a-z]/g, '');
    return BANNED_PASSWORDS.some((b) => b === lowered || b === stripped || b === lettersOnly);
  }

  function hasLongRepeatedRun(value: string): boolean {
    let previous: string | null = null;
    let run = 0;
    for (const c of value) {
      if (c === previous) {
        run++;
        if (run > MAX_REPEATED_RUN) return true;
      } else {
        previous = c;
        run = 1;
      }
    }
    return false;
  }

  function validatePassword(): boolean {
    if (!password) {
      passwordError = t('setup.validation.password.required');
      return false;
    }
    if (password.length < MIN_PASSWORD_LEN || password.length > 128) {
      passwordError = t('setup.validation.password.too_short');
      return false;
    }
    if (!/[A-Z]/.test(password)) {
      passwordError = t('setup.password_no_uppercase');
      return false;
    }
    if (!/[a-z]/.test(password)) {
      passwordError = t('setup.password_no_lowercase');
      return false;
    }
    if (!/\d/.test(password)) {
      passwordError = t('setup.password_no_digit');
      return false;
    }
    if (isBannedPassword(password)) {
      passwordError = t('setup.password_too_common');
      return false;
    }
    if (hasLongRepeatedRun(password)) {
      passwordError = t('setup.password_no_repeats');
      return false;
    }
    passwordError = '';
    return true;
  }

  // Live password requirement checklist — every rule the backend enforces.
  // This list, not `validatePassword()`, is what enables the "Next" button
  // (`canProceed` reads `passwordAllMet`), so a rule missing here would let the
  // wizard advance with a password the server then rejects.
  $: passwordChecks = [
    { label: t('setup.password_min_length'), met: password.length >= MIN_PASSWORD_LEN && password.length <= 128 },
    { label: t('setup.password_uppercase'), met: /[A-Z]/.test(password) },
    { label: t('setup.password_lowercase'), met: /[a-z]/.test(password) },
    { label: t('setup.password_digit'), met: /\d/.test(password) },
    { label: t('setup.password_too_common'), met: password.length > 0 && !isBannedPassword(password) },
    { label: t('setup.password_no_repeats'), met: password.length > 0 && !hasLongRepeatedRun(password) },
  ];
  $: passwordAllMet = passwordChecks.every((c) => c.met);

  function validateConfirmPassword(): boolean {
    if (password !== confirmPassword) {
      confirmPasswordError = t('setup.validation.password.mismatch');
      return false;
    }
    confirmPasswordError = '';
    return true;
  }

  function canGoNext(): boolean {
    // Step 0 (welcome) is an optional choice — always passable. The account
    // and password gates live at steps 1 and 2; verify (3) is always reachable.
    if (currentStep === 1) return validateUsername();
    // Password step: the TOTP toggle (if on) must hold a server-validated code
    // before the step can be left — an enabled-but-unverified enrollment is
    // not allowed to slip through to account creation.
    if (currentStep === 2) return validatePassword() && validateConfirmPassword() && (!totpEnabled || totpVerified);
    return true;
  }

  // Live server-side availability check for the admin username (DB accounts +
  // system accounts). Runs when the user blurs a valid username at the account
  // step AND again as a blocking gate before leaving it — the account step must
  // not be passable while the name is already taken. Returns true only when the
  // name is confirmed available; a network error fails open (the final verify
  // step re-checks server-side).
  async function checkUsernameAvailability(): Promise<boolean> {
    if (!validateUsername()) return false;
    checkingUsername = true;
    usernameAvailable = null;
    try {
      const res = await api.checkUsername(username);
      const available = res.data?.available === true;
      usernameAvailable = available;
      if (!available) {
        usernameError = t('setup.error.username_exists');
      }
      return available;
    } catch {
      // Network/backend issue: cannot determine availability — fail open.
      usernameAvailable = null;
      return true;
    } finally {
      checkingUsername = false;
    }
  }

  // Pure (no error-message side effects), Svelte-trackable mirror of
  // canGoNext(), used only to drive the "Next" button's disabled state.
  // canGoNext() itself can't be called directly from a template expression
  // for this: Svelte's compiler determines a template binding's reactive
  // dependencies from the identifiers referenced in the expression itself,
  // not from what a called function reads internally, so
  // `disabled={!canGoNext()}` never re-evaluated as the user typed —
  // confirmed live (a reserved username like "root" showed its error
  // message but left "Next" visually enabled). Referencing username/
  // password/confirmPassword/currentStep/passwordAllMet directly here
  // makes the dependency real.
  $: usernameFormatValid = !!username
    && username.length >= 3
    && USERNAME_PATTERN.test(username)
    && !RESERVED_USERNAMES.includes(username.toLowerCase());
  $: canProceed = currentStep === 1
    ? usernameFormatValid
    : currentStep === 2
    ? (passwordAllMet && password === confirmPassword && (!totpEnabled || totpVerified))
    : true;

  async function verifyAccountCreation() {
    isVerifying = true;
    checkingUsername = true;
    usernameAvailable = null;
    verificationResult = { valid: false, errors: [], warnings: [] };

    const errors: string[] = [];
    const warnings: string[] = [];

    // Validate username (format)
    if (!username || username.length < 3) {
      errors.push(t('setup.username_too_short'));
      usernameAvailable = false;
    }
    if (!USERNAME_PATTERN.test(username)) {
      errors.push(t('setup.username_invalid_chars'));
      usernameAvailable = false;
    }
    if (RESERVED_USERNAMES.includes(username.toLowerCase())) {
      errors.push(t('setup.username_reserved'));
      usernameAvailable = false;
    }

    // Real, server-side availability check (DB + system accounts)
    if (username && username.length >= 3 && !RESERVED_USERNAMES.includes(username.toLowerCase())) {
      try {
        const res = await api.checkUsername(username);
        if (res.data && !res.data.available) {
          errors.push(t('setup.error.username_exists'));
          usernameAvailable = false;
        } else if (res.data?.available) {
          usernameAvailable = true;
        }
      } catch {
        // Network/backend issue: let the final submit catch it.
        usernameAvailable = null;
      }
    }
    checkingUsername = false;

    // Validate password
    if (!password || password.length < MIN_PASSWORD_LEN) {
      errors.push(t('setup.password_min_length'));
    } else {
      if (!/[A-Z]/.test(password)) errors.push(t('setup.password_no_uppercase'));
      if (!/[a-z]/.test(password)) errors.push(t('setup.password_no_lowercase'));
      if (!/\d/.test(password)) errors.push(t('setup.password_no_digit'));
    }
    if (password !== confirmPassword) {
      errors.push(t('setup.passwords_mismatch'));
    }

    // If the password-step TOTP toggle is on, a server-validated 6-digit code
    // is mandatory before the account can be created.
    if (totpEnabled && !totpVerified) {
      errors.push(t('setup.2fa.verify_required'));
    }

    // Password strength warnings
    const strength = getPasswordStrength(password);
    if (strength < 3 && password.length >= MIN_PASSWORD_LEN) {
      warnings.push(t('setup.verification.weak_password'));
    }

    // Fullname warnings
    if (!fullname || !fullname.trim()) {
      warnings.push(t('setup.verification.no_fullname'));
    }

    verificationResult = {
      valid: errors.length === 0,
      errors,
      warnings
    };

    isVerifying = false;
  }

  // The page-level .hv-login-page scroll container — now this component's
  // own root, since App.svelte renders the wizard directly (it used to sit
  // inside Login.svelte's page div). The wizard card itself never scrolls;
  // the default scroll-anchoring of that container grows/shrinks its
  // scrollTop whenever a step swap changes content height above the fold —
  // confirmed live this could leave scrollTop at 165px right after a step
  // change, pushing the stepper (and sometimes the step title) above the
  // visible area with no indication anything was cut off. Steps should
  // always open scrolled to their own top.
  async function scrollWizardToTop() {
    await tick();
    document.querySelector('.hv-login-page')?.scrollTo({ top: 0, behavior: 'instant' as ScrollBehavior });
  }

  // Advance to the next step; when reaching the verification step, run validation automatically.
  // Leaving the account step (5) is gated on the server-side username
  // availability: an already-taken admin name blocks the transition to the
  // password step.
  async function nextStep() {
    if (currentStep === 1) {
      const available = await checkUsernameAvailability();
      if (!available) return;
    }
    // Password step: an enabled TOTP toggle needs a verified code before the
    // step can be left (the "Next" button is disabled too — this guards the
    // keyboard/enter path and surfaces the reason explicitly).
    if (currentStep === 2 && totpEnabled && !totpVerified) {
      totpError = t('setup.2fa.verify_required');
      return;
    }
    if (!canGoNext()) return;
    resumedNotice = false;
    if (currentStep < totalSteps - 1) {
      currentStep++;
      persistProgress();
      scrollWizardToTop();
      // The QR label embeds the admin name: re-render it when (re)entering the
      // password step in case the user went back and changed the username.
      if (currentStep === 2 && totpEnabled && totpSecret) {
        refreshTotpQr();
      }
      // Run verification when reaching the verification step
      if (currentStep === 3) {
        verifyAccountCreation();
      }
    }
  }

  function prevStep() {
    if (currentStep > 0) {
      currentStep--;
      resumedNotice = false;
      persistProgress();
      scrollWizardToTop();
      // Reset verification when leaving verification step
      if (currentStep === 2) {
        verificationResult = { valid: false, errors: [], warnings: [] };
        usernameAvailable = null;
      }
    }
  }

  async function handleSubmit() {
    if (!verificationResult.valid) {
      return;
    }

    isLoading = true;
    try {
      // api.setupAdmin() (not the generic api.post()) is required here: it
      // sends the request with includeAuth: false. /auth/setup is a public,
      // pre-session endpoint, but a browser that previously held a valid
      // session for this origin (e.g. a data-directory reset reusing the
      // same browser) still has a stale X-Session-Token/X-CSRF-Token pair
      // sitting in ApiClient's in-memory state — with includeAuth left at
      // its default of true, that stale pair gets attached anyway, and the
      // global CSRF middleware then rejects the whole request with a bare
      // 403 before handle_setup ever runs. Confirmed live: this is exactly
      // what broke account creation for a non-clean browser.
      const result = await api.setupAdmin({
        username,
        password,
        fullname: fullname || undefined,
        avatar_seed: avatarSeed || undefined,
        totp_secret: totpEnabled && totpVerified ? totpSecret : undefined,
        totp_code: totpEnabled && totpVerified ? totpCode : undefined,
      });

      if (result.success && result.data?.token) {
        // Set tokens and user directly since we already have a valid session
        api.setTokens(result.data.token, result.data.csrf_token || '');
        authStore.set({
          user: {
            username: result.data.username,
            is_admin: result.data.is_admin,
          },
          isAuthenticated: true,
          isLoading: false,
        });
        // Remember whether 2FA (enrolled at the password step) was persisted,
        // so the post-recovery summary can show the one-time backup codes.
        twoFactorEnabled = result.data.totp_enabled === true;
        if (twoFactorEnabled) {
          twoFactorBackupCodes = result.data.backup_codes || [];
        } else if (totpEnabled && totpVerified) {
          // The admin enrolled TOTP but the server could not persist it — warn
          // on the summary screen instead of silently skipping 2FA.
          twoFactorUnavailable = true;
        }
        // Account created: show the one-time recovery code first; TOTP was
        // already enrolled at the password step, so only a short summary
        // (backup codes) follows.
        setupComplete = true;
        startRecoveryCode();
      } else {
        // Surface the backend error directly in the verification box.
        // result.message covers a normal backend rejection; result.error is
        // what api.post() returns instead when the request never reached
        // the backend at all (e.g. a network failure) — checking only
        // .message silently dropped that case to the generic fallback below.
        const errorMsg = result.message || (result as any).error || '';
        const errors: string[] = [];
        if (errorMsg.toLowerCase().includes('already exists') || errorMsg.toLowerCase().includes('username')) {
          errors.push(t('setup.error.username_exists'));
        } else if (errorMsg.toLowerCase().includes('invalid')) {
          errors.push(t('setup.error.invalid_input'));
        } else if (errorMsg.toLowerCase().includes('network') || errorMsg.toLowerCase().includes('fetch')) {
          errors.push(t('setup.error.network'));
        } else if (errorMsg.toLowerCase().includes('timeout')) {
          errors.push(t('setup.error.timeout'));
        } else if (errorMsg.includes('500') || errorMsg.toLowerCase().includes('server')) {
          errors.push(t('setup.error.server'));
        } else {
          errors.push(errorMsg || t('setup.error'));
        }
        verificationResult = { valid: false, errors, warnings: verificationResult.warnings };
      }
    } catch (error: any) {
      // Handle network and server errors. api.setupAdmin() never actually
      // throws (its underlying post() catches fetch failures internally and
      // returns {success:false} instead), so this branch is a defensive
      // fallback rather than the normal error path — kept in sync with the
      // same specific messages as the `else` branch above in case that
      // assumption ever stops holding (e.g. a future change makes post()
      // rethrow, or an unrelated exception surfaces here).
      const errorMsg = error?.message || '';
      const errors: string[] = [];
      if (errorMsg.includes('fetch') || errorMsg.includes('network')) {
        errors.push(t('setup.error.network'));
      } else if (errorMsg.includes('timeout')) {
        errors.push(t('setup.error.timeout'));
      } else if (errorMsg.includes('500') || errorMsg.includes('server')) {
        errors.push(t('setup.error.server'));
      } else {
        errors.push(t('setup.error'));
      }
      verificationResult = { valid: false, errors, warnings: verificationResult.warnings };
    } finally {
      isLoading = false;
    }
  }

  // After successful account creation, generate and display the one-time
  // numeric recovery code.
  //
  // This is MANDATORY: the recovery code is the only offline way back into the
  // account (there is no email reset), so a failed generation must not be
  // waved through. On error the screen offers a retry and refuses to continue —
  // the admin account already exists, but the code can still be issued from
  // My Account > Recovery code once they are signed in, so the failure is
  // recoverable without stranding them.
  async function startRecoveryCode() {
    recoveryBusy = true;
    recoveryError = '';
    recoveryAcknowledged = false;
    try {
      const res = await api.generateRecoveryCode();
      if (res.success && res.data?.recovery_code) {
        recoveryCode = res.data.recovery_code;
      } else {
        recoveryError = res.message || t('setup.recovery_error');
      }
    } catch {
      recoveryError = t('setup.recovery_error');
    } finally {
      recoveryBusy = false;
      showRecovery = true;
    }
  }

  function continueFromRecovery() {
    showRecovery = false;
    recoveryCode = '';
    if (totpEnabled && totpVerified) {
      // TOTP was already enrolled and verified at the password step; the
      // short summary screen shows the one-time backup codes (or a warning).
      showTwoFactorSummary = true;
    } else {
      finishSetup();
    }
  }

  // ── TOTP enrollment at the password step (before the account exists) ──

  // RFC 4648 base32, identical to AuthService::encode_base32's alphabet: the
  // client-generated secret must decode to the same bytes on the server.
  const TOTP_BASE32 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';

  // 20 cryptographically-random bytes base32-encoded (exactly 32 chars).
  function generateTotpSecret(): string {
    const bytes = new Uint8Array(20);
    crypto.getRandomValues(bytes);
    let out = '';
    let buffer = 0;
    let bits = 0;
    for (const byte of bytes) {
      buffer = (buffer << 8) | byte;
      bits += 8;
      while (bits >= 5) {
        out += TOTP_BASE32[(buffer >>> (bits - 5)) & 31];
        bits -= 5;
      }
    }
    if (bits > 0) out += TOTP_BASE32[(buffer << (5 - bits)) & 31];
    return out;
  }

  // otpauth URI with the same shape AuthService::generate_totp_secret builds
  // (SHA1 / 6 digits / 30 s, issuer "HomeVault") — the QR is therefore usable
  // by any standard TOTP app: Google Authenticator, Microsoft Authenticator,
  // Aegis, Ente Auth, 2FAS, and friends.
  function buildOtpauthUri(secret: string): string {
    return `otpauth://totp/HomeVault:${encodeURIComponent(username)}?secret=${secret}&issuer=HomeVault&algorithm=SHA1&digits=6&period=30`;
  }

  async function renderTotpQr() {
    if (!totpSecret) return;
    try {
      totpQrDataUrl = await QRCode.toDataURL(buildOtpauthUri(totpSecret), {
        margin: 1,
        width: 180,
        errorCorrectionLevel: 'M',
      });
    } catch {
      totpError = t('setup.2fa.gen_error');
    }
  }

  // Re-render the QR (label embeds the admin name) without changing the secret.
  function refreshTotpQr() {
    renderTotpQr();
  }

  // Toggle switch: turning enrollment on generates a fresh secret + QR;
  // turning it off discards the whole enrollment (a new one is generated on
  // the next enable).
  async function onTOTPToggle(enabled: boolean) {
    totpEnabled = enabled;
    if (!enabled) {
      totpSecret = '';
      totpQrDataUrl = '';
      totpCode = '';
      totpVerified = false;
      totpError = '';
      return;
    }
    totpGenerating = true;
    totpError = '';
    await tick();
    try {
      totpSecret = generateTotpSecret();
      await renderTotpQr();
    } catch {
      totpError = t('setup.2fa.gen_error');
    } finally {
      totpGenerating = false;
    }
  }

  // Validate the entered 6-digit code against the client-generated secret via
  // the stateless, first-run-only endpoint. The authoritative check still
  // happens server-side inside /auth/setup at creation time.
  async function verifyTotpCode() {
    if (totpCode.length !== 6) {
      totpError = t('login.validation.totp_invalid');
      return;
    }
    totpVerifying = true;
    totpError = '';
    try {
      const res = await api.verifySetupTotp(totpSecret, totpCode);
      if (res.success && res.data?.valid) {
        totpVerified = true;
      } else {
        totpError = res.message || t('login.error.invalid_totp');
      }
    } catch {
      totpError = t('login.error.invalid_totp');
    } finally {
      totpVerifying = false;
    }
  }

  function finishSetup() {
    window.location.href = '/login';
  }

  // Rate password strength from 0 to 5 based on length, mixed case, digits, and special characters
  function getPasswordStrength(pass: string): number {
    let strength = 0;
    if (pass.length >= 8) strength++;
    if (pass.length >= 12) strength++;
    if (/[a-z]/.test(pass) && /[A-Z]/.test(pass)) strength++;
    if (/\d/.test(pass)) strength++;
    if (/[^a-zA-Z0-9]/.test(pass)) strength++;
    return strength;
  }

  $: passwordStrength = getPasswordStrength(password);
  $: strengthLabel = passwordStrength === 0 ? '' :
                     passwordStrength <= 2 ? t('setup.password_weak') :
                     passwordStrength === 3 ? t('setup.password_medium') :
                     passwordStrength === 4 ? t('setup.password_strong') :
                     t('setup.password_very_strong');
  // Password-strength hue ramp (red → orange → green) matching the mock labels.
  $: strengthColor = passwordStrength <= 2 ? 'var(--hv-wizard-strength-weak)' :
                     passwordStrength === 3 ? 'var(--hv-wizard-strength-mid)' :
                     passwordStrength === 4 ? 'var(--hv-wizard-success)' :
                     'var(--hv-wizard-success-strong)';

  // Stepper labels shown in the card header step dots. The order matches the
  // 5-step flow: welcome → language → account → password → verify.
  // Stepper labels shown in the card header step dots. The order matches the
  // 4-step flow: welcome → account → password → verify.
  $: stepLabels = [
    t('setup.stepper.welcome'),
    t('setup.stepper.account'),
    t('setup.stepper.password'),
    t('setup.stepper.verify'),
  ];

  // Welcome-step feature cards (matching the approved mock).
  $: welcomeFeatures = [
    { icon: IconUser, color: 'var(--hv-login-accent)', title: t('setup.welcome.feature1'), desc: t('setup.welcome.feature1_desc') },
    { icon: IconLock, color: 'var(--hv-wizard-feature-violet)', title: t('setup.welcome.feature2'), desc: t('setup.welcome.feature2_desc') },
    { icon: IconShield, color: 'var(--hv-wizard-feature-emerald)', title: t('setup.welcome.feature3'), desc: t('setup.welcome.feature3_desc') },
  ];

  // base.css ships unlayered global `input { width:100%; border; background;
  // padding; … }` rules that beat Tailwind's layered utilities, so every raw
  // form field carries the trailing `!` important modifier. The field
  // look lives on the wrapper; the inner input is transparent and borderless.
  // The wizard fields are WHITE (the mock) with a hairline shadow, unlike the
  // login's #F8FAFC fields.
  const textInputClass =
    'min-w-0 flex-1 p-0! border-none! bg-transparent! text-[var(--hv-wizard-text-title)] caret-[var(--hv-login-accent)] outline-none placeholder:text-[var(--hv-login-muted)]! focus:shadow-none! disabled:bg-transparent!';
  const fieldWrapperClass =
    'flex h-11 items-center gap-2 rounded-md border border-[var(--hv-login-field-border)] bg-white px-3 text-sm shadow-[var(--hv-wizard-field-shadow)] focus-within:border-[var(--hv-login-accent)] focus-within:shadow-[0_0_0_3px_var(--hv-login-accent-ring)]';
  const ctaClass =
    'bg-[image:var(--hv-login-btn-grad)]! rounded-md! shadow-[var(--hv-login-btn-shadow)] hover:brightness-105 active:scale-[0.98] disabled:opacity-60 disabled:shadow-none';
  const createClass =
    'bg-[image:var(--hv-wizard-create-grad)]! rounded-md! shadow-[var(--hv-wizard-create-shadow)] hover:brightness-105 active:scale-[0.98] disabled:opacity-60 disabled:shadow-none';
  const ghostPrevClass =
    'bg-transparent! border-none! text-[var(--hv-wizard-text-body)]! hover:bg-[var(--hv-login-accent-soft)]! hover:text-[var(--hv-login-accent)]!';
  const errorTextClass = 'text-[var(--hv-wizard-error)]';
  const errorBorderClass = 'border-[var(--hv-wizard-error)]!';
</script>

<div class="hv-login-page relative grid min-h-dvh w-full grid-cols-1 grid-rows-[minmax(auto,1fr)_auto_minmax(3rem,1fr)] justify-items-center overflow-y-auto bg-[image:var(--hv-login-bg-gradient)] px-4 @container" data-testid="setup-container">
  <!-- Blurred orbs (fixed, never scroll) -->
  <div class="pointer-events-none fixed inset-0 overflow-hidden" aria-hidden="true">
    <div class="absolute top-1/4 left-1/3 h-96 w-96 rounded-full bg-[image:var(--hv-login-orb-indigo)] opacity-40 blur-[70px]"></div>
    <div class="absolute right-1/3 bottom-1/4 h-80 w-80 rounded-full bg-[image:var(--hv-login-orb-pink)] opacity-30 blur-[60px]"></div>
  </div>

  <!-- Live clock, sticky top. It owns the first grid row, whose `auto`
       minimum is exactly the clock's height — no magic band value. -->
  <div class="pointer-events-none sticky top-0 z-[1] flex w-full flex-col items-center pb-4 pt-8">
    <div class="text-[3.5rem] leading-none font-light tracking-[-0.02em] text-[var(--hv-login-clock)] tabular-nums">{clock}</div>
    <div class="mt-1 text-xs uppercase tracking-[0.12em] text-[var(--hv-login-muted)]">{dateLabel}</div>
  </div>

  <!-- Centered card, second grid row.
       Row 1 and row 3 are both `1fr`, so they split the leftover space evenly
       and the 520px block ends up exactly in the middle of the viewport — on
       both axes, at any resolution or device.
       Row 1 is floored at the clock's own height and row 3 at 3rem: as soon as
       the card would collide with the clock the split collapses, the card
       starts right under the clock and the page scrolls from the card's top
       instead of clipping it. -->
  <div class="flex justify-center row-start-2 w-full">
    <div class="relative w-full max-w-[520px]">
      <div data-testid="setup-card" class="overflow-hidden rounded-2xl border border-[var(--hv-login-card-border)] bg-white shadow-[var(--hv-login-card-shadow)]">
        <!-- Card header: brand + step dots + progress -->
        <div class="border-b border-[var(--hv-login-hairline)] px-8 pt-8 pb-5 text-center @max-[480px]:px-6 @max-[480px]:pt-7">
          <div class="mx-auto mb-4 grid h-12 w-12 place-items-center rounded-2xl bg-[image:var(--hv-login-logo-grad)] shadow-[var(--hv-login-logo-shadow)]">
            <svg viewBox="0 0 24 24" width="28" height="28" fill="none" stroke="white" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M3 10.5 12 3l9 7.5"/><path d="M5 9.5V21h14V9.5"/><circle cx="12" cy="14" r="2.4"/><path d="M12 16.4V19"/></svg>
          </div>
          <div class="text-base font-semibold tracking-wide text-[var(--hv-wizard-text-title)]">HomeVault</div>
          <div class="mt-0.5 text-xs uppercase tracking-wider text-[var(--hv-login-muted)]">{t('setup.assistant_subtitle')}</div>

          <!-- Step dots -->
          <div data-testid="setup-stepper" class="mt-4 flex items-center justify-center gap-2">
            {#each stepLabels as label, i}
              <div class="flex items-center gap-2">
                <div class="flex flex-col items-center gap-1">
                  <div
                    data-testid="setup-stepper-dot"
                    class:done={i < currentStep}
                    class:current={i === currentStep}
                    class={cn(
                      'grid h-7 w-7 place-items-center rounded-full transition-all duration-300',
                      i < currentStep
                        ? 'bg-[var(--hv-login-accent)] text-white'
                        : i === currentStep
                          ? 'bg-[image:var(--hv-login-logo-grad)] text-white shadow-[var(--hv-wizard-step-glow)]'
                          : 'border-[1.5px] border-[var(--hv-login-field-border)] bg-[var(--hv-wizard-track)] text-[var(--hv-login-muted)]',
                    )}
                  >
                    {#if i < currentStep}<IconCheck size={13} stroke={3} />{:else}<span class="text-xs font-semibold">{i + 1}</span>{/if}
                  </div>
                  <span
                    data-testid="setup-stepper-label"
                    class:active={i === currentStep}
                    class={cn(
                      'hidden text-xs @min-[640px]:block',
                      i === currentStep
                        ? 'font-semibold text-[var(--hv-login-accent)]'
                        : i < currentStep
                          ? 'text-[var(--hv-login-accent-soft-border)]'
                          : 'text-[var(--hv-login-muted)]',
                    )}
                  >{label}</span>
                </div>
                {#if i < stepLabels.length - 1}
                  <div
                    data-testid="setup-stepper-line"
                    class:done={i < currentStep}
                    class={cn('mb-4 h-px w-8', i < currentStep ? 'bg-[var(--hv-login-accent-soft-border)]' : 'bg-[var(--hv-login-field-border)]')}
                  ></div>
                {/if}
              </div>
            {/each}
          </div>

          <!-- Progress bar -->
          <div class="mt-3 h-1 overflow-hidden rounded-full bg-[var(--hv-wizard-track)]">
            <div class="h-full rounded-full bg-[image:var(--hv-login-btn-grad)] transition-all duration-500" style="width: {((currentStep + 1) / totalSteps) * 100}%"></div>
          </div>
        </div>

        <div class="px-8 py-7 @max-[480px]:px-6 @max-[480px]:py-5">
          {#if resumedNotice && currentStep > 0}
            <div class="mb-5" data-testid="setup-resumed">
              <Alert variant="info"><p class="m-0">{t('setup.resumed')}</p></Alert>
            </div>
          {/if}

          {#if insecureConnection}
            <div class="mb-5" data-testid="setup-insecure-banner">
              <Alert variant="warning">
                <p class="m-0 text-sm font-semibold">{t('setup.insecure.title')}</p>
                <p class="m-0 mt-1 text-sm">{t('setup.insecure.body')}</p>
              </Alert>
            </div>
          {/if}

          {#if capabilitiesLoaded && !sudoAvailable}
            <div class="mb-5">
              <Alert variant="warning">
                <p class="hv-prereq-title m-0 text-sm font-semibold">{t('setup.prereq.title')}</p>
                <ul class="hv-prereq-list m-0 mt-1 pl-4"><li>{t('setup.prereq.sudo')}</li></ul>
                <p class="m-0 mt-1">{t('setup.prereq.note')}</p>
              </Alert>
            </div>
          {/if}

          <!-- ── Step 1: Welcome ── -->
          {#if currentStep === 0}
            <div data-step="0" class="text-center">
              <p class="mb-2 text-xs font-semibold uppercase tracking-widest text-[var(--hv-login-accent)]">{t('setup.stepper.welcome')}</p>
              <h1 class="hv-welcome-title m-0 text-3xl font-bold leading-tight text-[var(--hv-wizard-text-heading)]!">{t('setup.welcome.title')}</h1>
              <p class="hv-welcome-text m-0 mt-3 text-sm leading-relaxed text-[var(--hv-wizard-text-body)]">{t('setup.welcome.message1')}</p>
              <p class="hv-welcome-text m-0 mt-1 text-sm leading-relaxed text-[var(--hv-wizard-text-body)]">{t('setup.welcome.message2')}</p>
              <div class="mt-8 grid gap-4 text-left">
                {#each welcomeFeatures as feature}
                  <div class="hv-feature flex items-center gap-4 rounded-xl border border-[var(--hv-login-field-border)] bg-white p-4">
                    <div class="hv-welcome-icon grid h-10 w-10 shrink-0 place-items-center rounded-xl" style="background: color-mix(in oklch, {feature.color} 8%, white); color: {feature.color}">
                      <svelte:component this={feature.icon} size={20} stroke={1.8} />
                    </div>
                    <div class="min-w-0 flex-1">
                      <p class="hv-feature-title m-0 text-sm font-semibold leading-snug text-[var(--hv-wizard-text-title)]">{feature.title}</p>
                      <p class="hv-feature-desc m-0 mt-1 text-xs leading-relaxed text-[var(--hv-wizard-text-body)]">{feature.desc}</p>
                    </div>
                  </div>
                {/each}
              </div>
            </div>
          {/if}



          <!-- ── Step 2: Account ── -->
          {#if currentStep === 1}
            <div data-step="1">
              <p class="mb-2 text-xs font-semibold uppercase tracking-widest text-[var(--hv-login-accent)]">{t('setup.step')} 2 {t('setup.of')} {totalSteps}</p>
              <h1 data-testid="setup-step-title" class="m-0 text-3xl font-bold text-[var(--hv-wizard-text-heading)]!">{t('setup.step1.title')}</h1>
              <p data-testid="setup-step-desc" class="m-0 mt-2 text-sm text-[var(--hv-wizard-text-body)]">{t('setup.step1.description')}</p>

              <div class="mt-6 flex flex-col items-center gap-3" data-testid="avatar-picker">
                <div class="relative h-24 w-24 overflow-hidden rounded-2xl border-2 border-[var(--hv-login-field-border)] shadow-[var(--hv-shadow-md)]">
                  {#if avatarPreview}
                    <img data-testid="setup-avatar-preview" class="h-full w-full object-cover" src={avatarPreview} alt={t('setup.avatar.preview_alt')} width="96" height="96" />
                  {:else}
                    <div data-testid="setup-avatar-placeholder" class="grid h-full w-full place-items-center bg-[var(--hv-login-field-bg)] text-[var(--hv-wizard-text-body)]" aria-hidden="true"><IconUser size={36} stroke={2} /></div>
                  {/if}
                  {#if avatarLoading}
                    <div class="absolute inset-0 grid place-items-center rounded-2xl bg-black/30 text-white"><IconLoader size={20} stroke={2} /></div>
                  {/if}
                </div>
                <Button variant="secondary" data-testid="setup-avatar-regen" onclick={regenerateAvatar} disabled={avatarLoading} class="h-9! border-[var(--hv-login-accent-soft-border)]! text-[var(--hv-login-accent)]! hover:bg-[var(--hv-login-accent-soft)]!"><IconRefresh size={16} stroke={2} /> {t('setup.avatar.regenerate')}</Button>
              </div>

              <div class="mt-6">
                <label for="username" class="mb-1.5 block text-sm font-medium text-[var(--hv-wizard-text-label)]">{t('setup.username.label')} <span class="text-[var(--hv-wizard-error)]">*</span></label>
                <div class={cn(fieldWrapperClass, username && !usernameFormatValid ? 'border-[var(--hv-wizard-error)]!' : usernameFormatValid ? 'border-[var(--hv-login-accent)]!' : '')}>
                  <IconUser size={19} stroke={1.7} class="shrink-0 text-[var(--hv-login-muted)]" />
                  <input id="username" type="text" class={textInputClass} bind:value={username} oninput={() => { usernameAvailable = null; }} onblur={() => { validateUsername(); suggestAvatarFromUsername(); if (usernameFormatValid) checkUsernameAvailability(); }} placeholder={t('setup.username.placeholder')} autocomplete="username" />
                </div>
                {#if usernameError}<p class="hv-field-error m-0 mt-1 text-xs text-[var(--hv-wizard-error)]">{usernameError}</p>{/if}
                <p class="m-0 mt-1 text-xs text-[var(--hv-login-muted)]">{t('setup.username.hint')}</p>
                <!-- Reserved slot: the checking/available row must not shift
                  the footer while it resolves, or a Next click dispatched
                  mid-resolve lands on empty space and the step never
                  advances (deterministic on slow backends, invisible warm). -->
                <div class="mt-1 min-h-4" aria-live="polite">
                  {#if checkingUsername}
                    <p class="hv-username-status m-0 flex items-center gap-1.5 text-xs text-[var(--hv-login-muted)]">
                      <span class="inline-block h-3 w-3 animate-spin rounded-full border-2 border-[var(--hv-login-accent)] border-t-transparent"></span>
                      {t('setup.verification.checking')}
                    </p>
                  {:else if usernameAvailable === true}
                    <p class="hv-username-status m-0 flex items-center gap-1.5 text-xs font-medium text-[var(--hv-wizard-success-strong)]">
                      <svg viewBox="0 0 14 14" class="h-3.5 w-3.5 shrink-0" fill="none"><circle cx="7" cy="7" r="7" fill="var(--hv-wizard-success)"/><path d="M3.5 7l2.5 2.5 4.5-5" stroke="white" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>
                      {t('setup.verification.available')}
                    </p>
                  {/if}
                </div>
              </div>

              <div class="mt-4">
                <label for="fullname" class="mb-1.5 block text-sm font-medium text-[var(--hv-wizard-text-label)]">{t('setup.fullname.label')} <span class="font-normal text-[var(--hv-login-muted)]">({t('setup.optional')})</span></label>
                <div class={fieldWrapperClass}>
                  <IconUserCheck size={19} stroke={1.7} class="shrink-0 text-[var(--hv-login-muted)]" />
                  <input id="fullname" type="text" class={textInputClass} bind:value={fullname} placeholder={t('setup.fullname.placeholder')} autocomplete="name" />
                </div>
              </div>
            </div>
          {/if}

          <!-- ── Step 3: Password ── -->
          {#if currentStep === 2}
            <div data-step="2">
              <p class="mb-2 text-xs font-semibold uppercase tracking-widest text-[var(--hv-login-accent)]">{t('setup.step')} 3 {t('setup.of')} {totalSteps}</p>
              <h1 data-testid="setup-step-title" class="m-0 text-3xl font-bold text-[var(--hv-wizard-text-heading)]!">{t('setup.step2.title')}</h1>
              <p data-testid="setup-step-desc" class="m-0 mt-2 text-sm text-[var(--hv-wizard-text-body)]">{t('setup.step2.description')}</p>

              <div class="mt-6">
                <label for="password" class="mb-1.5 block text-sm font-medium text-[var(--hv-wizard-text-label)]">{t('setup.password.label')} <span class="text-[var(--hv-wizard-error)]">*</span></label>
                <div class={cn(fieldWrapperClass, passwordError && 'border-[var(--hv-wizard-error)]!')}>
                  <IconLock size={19} stroke={1.7} class="shrink-0 text-[var(--hv-login-muted)]" />
                  <input id="password" type={showPassword ? 'text' : 'password'} class={textInputClass} bind:value={password} onblur={validatePassword} placeholder={t('setup.password.placeholder')} autocomplete="new-password" />
                  <button type="button" class="grid h-8 w-8 flex-none cursor-pointer place-items-center rounded-full border-none bg-transparent text-[var(--hv-login-muted)] hover:text-[var(--hv-wizard-text-title)] hv-ic" aria-label={showPassword ? t('login.hide_password') : t('login.show_password')} onclick={() => showPassword = !showPassword}>
                    {#if showPassword}<IconEyeOff size={20} stroke={1.7}/>{:else}<IconEye size={20} stroke={1.7}/>{/if}
                  </button>
                </div>
                {#if passwordError}<p class="hv-field-error m-0 mt-1 text-xs text-[var(--hv-wizard-error)]">{passwordError}</p>{/if}

                {#if password.length > 0}
                  <div class="hv-password-strength mt-2.5 space-y-2">
                    <div class="flex gap-1">
                      {#each passwordChecks as check}
                        <div class={cn('h-1.5 flex-1 rounded-full transition-all duration-300', check.met ? 'bg-[var(--hv-wizard-success)]' : 'bg-[var(--hv-login-field-border)]')}></div>
                      {/each}
                    </div>
                    <p class="hv-strength-label m-0 text-xs font-medium" style="color: {strengthColor}">{strengthLabel}</p>
                    <ul class="hv-req-list m-0 mt-1 grid list-none grid-cols-2 @max-[560px]:grid-cols-1 gap-x-4 gap-y-1 p-0">
                      {#each passwordChecks as check}
                        <li class:hv-req-met={check.met} class:hv-req-unmet={!check.met} class={cn('flex items-center gap-1.5 text-xs', check.met ? 'text-[var(--hv-wizard-success-strong)]' : 'text-[var(--hv-login-muted)]')}>
                          {#if check.met}
                            <svg viewBox="0 0 12 12" class="h-3 w-3 shrink-0" fill="none"><circle cx="6" cy="6" r="6" fill="var(--hv-wizard-success)"/><path d="M3 6l2 2 4-4" stroke="white" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>
                          {:else}
                            <svg viewBox="0 0 12 12" class="h-3 w-3 shrink-0" fill="none"><circle cx="6" cy="6" r="5.5" stroke="var(--hv-login-faint)" stroke-width="1"/></svg>
                          {/if}
                          <span>{check.label}</span>
                        </li>
                      {/each}
                    </ul>
                  </div>
                {/if}
              </div>

              <div class="mt-4">
                <label for="confirm-password" class="mb-1.5 block text-sm font-medium text-[var(--hv-wizard-text-label)]">{t('setup.confirm_password.label')} <span class="text-[var(--hv-wizard-error)]">*</span></label>
                <div class={cn(fieldWrapperClass, confirmPassword && confirmPassword !== password ? 'border-[var(--hv-wizard-error)]!' : confirmPassword && confirmPassword === password ? 'border-[var(--hv-wizard-success)]!' : '')}>
                  <IconLock size={19} stroke={1.7} class="shrink-0 text-[var(--hv-login-muted)]" />
                  <input id="confirm-password" type={showConfirmPassword ? 'text' : 'password'} class={textInputClass} bind:value={confirmPassword} onblur={validateConfirmPassword} placeholder={t('setup.confirm_password.placeholder')} autocomplete="new-password" />
                  <button type="button" class="grid h-8 w-8 flex-none cursor-pointer place-items-center rounded-full border-none bg-transparent text-[var(--hv-login-muted)] hover:text-[var(--hv-wizard-text-title)] hv-ic" aria-label={showConfirmPassword ? t('login.hide_password') : t('login.show_password')} onclick={() => showConfirmPassword = !showConfirmPassword}>
                    {#if showConfirmPassword}<IconEyeOff size={20} stroke={1.7}/>{:else}<IconEye size={20} stroke={1.7}/>{/if}
                  </button>
                </div>
                {#if confirmPasswordError}<p class="hv-field-error m-0 mt-1 text-xs text-[var(--hv-wizard-error)]">{confirmPasswordError}</p>{/if}
                {#if confirmPassword && confirmPassword !== password}<p class="m-0 mt-1 text-xs text-[var(--hv-wizard-error)]">{t('setup.validation.password.mismatch')}</p>{/if}
              </div>

              <!-- Optional two-factor (TOTP) enrollment: the toggle reveals the
                   QR code. The secret is generated client-side; the 6-digit code
                   is validated server-side before the account is created. -->
              <div class="mt-6 rounded-2xl border border-[var(--hv-login-field-border)] bg-white p-5">
                <div class="flex items-center justify-between gap-3">
                  <div class="min-w-0">
                    <p class="hv-2fa-optional-title m-0 text-sm font-semibold text-[var(--hv-wizard-text-title)]">{t('setup.2fa.optional_title')}</p>
                    <p class="m-0 mt-0.5 text-xs leading-relaxed text-[var(--hv-wizard-text-body)]">{t('setup.2fa.optional_desc')}</p>
                  </div>
                  <Switch checked={totpEnabled} onchange={onTOTPToggle} ariaLabel={t('setup.2fa.optional_title')} data-testid="setup-totp-toggle" />
                </div>

                {#if totpEnabled}
                  <div class="hv-2fa-enroll mt-4 border-t border-[var(--hv-login-hairline)] pt-4">
                    {#if totpGenerating}
                      <div class="flex items-center gap-2 text-sm text-[var(--hv-wizard-text-body)]"><Spinner size="sm" /><p class="m-0">{t('setup.2fa.generating')}</p></div>
                    {:else}
                      <div class="flex flex-col items-center gap-2">
                        {#if totpQrDataUrl}
                          <img data-testid="setup-totp-qr" src={totpQrDataUrl} alt={t('setup.2fa.qr_alt')} class="h-44 w-44 rounded-md border border-[var(--hv-login-field-border)]" width="180" height="180" />
                        {/if}
                        <code data-testid="setup-totp-secret" class="hv-2fa-secret-value block w-full break-all rounded-md border border-[var(--hv-login-field-border)] bg-[var(--hv-login-field-bg)] px-3 py-2 text-center font-[family-name:var(--hv-font-mono)] text-sm text-[var(--hv-wizard-text-soft)] select-none">{totpSecret}</code>
                      </div>
                      <p class="m-0 mt-3 text-xs leading-relaxed text-[var(--hv-wizard-text-body)]">{t('setup.2fa.scan_instruction')}</p>

                      <div class="mt-3">
                        <label for="setup-totp-code" class="mb-1.5 block text-xs font-semibold text-[var(--hv-wizard-text-label)]">{t('setup.2fa.code')}</label>
                        <div class={cn(fieldWrapperClass, totpError && 'border-[var(--hv-wizard-error)]!')}>
                          <IconShieldCheckered size={19} stroke={1.7} class="shrink-0 text-[var(--hv-login-muted)]" />
                          <input id="setup-totp-code" data-testid="setup-totp-code" type="text" inputmode="numeric" maxlength="6" class={textInputClass} bind:value={totpCode} oninput={() => { totpVerified = false; totpError = ''; }} placeholder="000000" autocomplete="one-time-code" disabled={totpVerifying} />
                        </div>
                        {#if totpError}<p class="hv-field-error hv-2fa-verify-error m-0 mt-1 text-xs text-[var(--hv-wizard-error)]">{totpError}</p>{/if}
                        {#if totpVerified}
                          <p data-testid="setup-totp-verified" class="m-0 mt-1.5 flex items-center gap-1.5 text-xs font-medium text-[var(--hv-wizard-success-strong)]">
                            <svg viewBox="0 0 14 14" class="h-3.5 w-3.5 shrink-0" fill="none"><circle cx="7" cy="7" r="7" fill="var(--hv-wizard-success)"/><path d="M3.5 7l2.5 2.5 4.5-5" stroke="white" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>
                            {t('setup.2fa.verified')}
                          </p>
                        {/if}
                        <div class="mt-2">
                          <Button variant="primary" data-testid="setup-totp-verify" class={ctaClass} fullWidth onclick={verifyTotpCode} disabled={totpVerifying || totpCode.length !== 6}>
                            {#if totpVerifying}<IconLoader size={20} stroke={2} />{/if}
                            {t('setup.2fa.verify')}
                          </Button>
                        </div>
                      </div>

                      <div data-testid="setup-totp-apps" class="hv-2fa-apps-note mt-4 flex items-start gap-2.5 rounded-md bg-[var(--hv-login-accent-soft)] p-3">
                        <IconShield size={18} stroke={1.8} class="mt-0.5 shrink-0 text-[var(--hv-login-accent-deep)]" />
                        <div class="min-w-0">
                          <p class="m-0 text-xs leading-relaxed text-[var(--hv-wizard-text-body)]">{t('setup.2fa.compatible')}</p>
                          <p class="m-0 mt-1 text-xs leading-relaxed text-[var(--hv-wizard-text-body)]">{t('setup.2fa.platforms')}</p>
                        </div>
                      </div>
                    {/if}
                  </div>
                {/if}
              </div>
            </div>
          {/if}

          <!-- ── Step 4: Verify & create (then recovery + 2FA) ── -->
          {#if currentStep === 3}
            <div data-step="3">
              <p class="mb-2 text-xs font-semibold uppercase tracking-widest text-[var(--hv-login-accent)]">{t('setup.step')} 4 {t('setup.of')} {totalSteps}</p>
              <h1 data-testid="setup-step-title" class="m-0 text-3xl font-bold text-[var(--hv-wizard-text-heading)]!">{t('setup.step3.title')}</h1>
              <p data-testid="setup-step-desc" class="m-0 mt-2 text-sm text-[var(--hv-wizard-text-body)]">{t('setup.step3.description')}</p>

              <!-- Summary card -->
              <div class="mt-6 overflow-hidden rounded-2xl border border-[var(--hv-login-field-border)] bg-white">
                <div class="flex items-center gap-4 border-b border-[var(--hv-login-hairline)] px-6 py-5">
                  <div class="h-16 w-16 shrink-0 overflow-hidden rounded-2xl border-2 border-[var(--hv-login-field-border)]">
                    {#if avatarPreview}
                      <img class="h-full w-full object-cover" src={avatarPreview} alt={t('setup.avatar.preview_alt')} width="64" height="64" />
                    {:else}
                      <div class="grid h-full w-full place-items-center bg-[var(--hv-login-field-bg)] text-[var(--hv-wizard-text-body)]" aria-hidden="true"><IconUser size={28} stroke={2} /></div>
                    {/if}
                  </div>
                  <div class="min-w-0">
                    <p class="hv-summary-value m-0 text-lg font-bold text-[var(--hv-wizard-text-heading)]!">{username || '—'}</p>
                    {#if fullname}<p class="hv-summary-item m-0 mt-0.5 truncate text-sm text-[var(--hv-wizard-text-body)]">{fullname}</p>{/if}
                    <span class="mt-1.5 inline-flex items-center gap-1 rounded-full bg-[var(--hv-login-accent-soft)] px-2 py-0.5 text-xs font-medium text-[var(--hv-login-accent-deep)]">
                      <svg viewBox="0 0 12 12" class="h-3 w-3" fill="currentColor"><path d="M6 1l1.5 3 3.5.5-2.5 2.5.6 3.5L6 9l-3.1 1.5.6-3.5L1 4.5l3.5-.5z"/></svg>
                      {t('setup.verification.admin_badge')}
                    </span>
                  </div>
                </div>
                <div class="divide-y divide-[var(--hv-wizard-divide)]">
                  <div class="hv-summary-item flex items-center justify-between gap-3 px-6 py-3">
                    <span class="text-sm text-[var(--hv-wizard-text-body)]">{t('setup.verification.availability')}</span>
                    <span class="hv-summary-value">
                      {#if checkingUsername || (isVerifying && usernameAvailable === null)}
                        <span class="flex items-center gap-1.5 text-sm text-[var(--hv-login-muted)]"><span class="inline-block h-3.5 w-3.5 animate-spin rounded-full border-2 border-[var(--hv-login-accent)] border-t-transparent"></span>{t('setup.verification.checking')}</span>
                      {:else if usernameAvailable === true}
                        <span class="flex items-center gap-1.5 text-sm font-medium text-[var(--hv-wizard-success-strong)]">
                          <svg viewBox="0 0 14 14" class="h-3.5 w-3.5" fill="none"><circle cx="7" cy="7" r="7" fill="var(--hv-wizard-success)"/><path d="M3.5 7l2.5 2.5 4.5-5" stroke="white" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>
                          {t('setup.verification.available')}
                        </span>
                      {:else}
                        <span class="flex items-center gap-1.5 text-sm font-medium text-[var(--hv-wizard-error)]">
                          <svg viewBox="0 0 14 14" class="h-3.5 w-3.5" fill="none"><circle cx="7" cy="7" r="7" fill="var(--hv-wizard-error)"/><path d="M4.5 4.5l5 5M9.5 4.5l-5 5" stroke="white" stroke-width="1.5" stroke-linecap="round"/></svg>
                          {t('setup.verification.unavailable')}
                        </span>
                      {/if}
                    </span>
                  </div>
                  <div class="hv-summary-item flex items-center justify-between gap-3 px-6 py-3">
                    <span class="text-sm text-[var(--hv-wizard-text-body)]">{t('setup.password.label')}</span>
                    <span class="hv-summary-value text-sm font-medium text-[var(--hv-wizard-text-title)]">••••••••</span>
                  </div>
                </div>
              </div>

              <!-- Verification result / recovery / 2FA -->
              {#if isVerifying}
                <div class="hv-loading-row mt-4 flex items-center gap-2 text-sm text-[var(--hv-wizard-text-body)]"><Spinner size="sm" /><p class="m-0">{t('setup.verification.checking')}</p></div>
              {:else if showRecovery}
                <div class="mt-5">
                  {#if recoveryBusy}
                    <div class="hv-loading-row flex items-center gap-2 text-sm text-[var(--hv-wizard-text-body)]"><Spinner size="sm" /><p class="m-0">{t('setup.verification.checking')}</p></div>
                  {:else if recoveryError}
                    <Alert variant="error" title={t('setup.recovery_title')}>
                      <p class="m-0">{recoveryError}</p>
                      <p class="m-0 mt-1">{t('setup.recovery_required')}</p>
                    </Alert>
                    <Button variant="primary" class={cn(ctaClass, 'mt-3')} fullWidth onclick={startRecoveryCode} data-testid="setup-recovery-retry">{t('setup.recovery_retry')}</Button>
                  {:else}
                    <div class="hv-recovery-box rounded-2xl border-2 border-[var(--hv-wizard-recovery-border)] bg-[var(--hv-wizard-recovery-bg)] p-5">
                      <div class="flex items-start gap-2">
                        <IconAlertTriangle size={20} stroke={2} class="mt-0.5 shrink-0 text-[var(--hv-wizard-recovery-desc)]" />
                        <div>
                          <p class="m-0 text-sm font-semibold text-[var(--hv-wizard-recovery-title)]">{t('setup.recovery_title')}</p>
                          <p class="m-0 mt-0.5 text-xs leading-relaxed text-[var(--hv-wizard-recovery-desc)]">{t('setup.recovery_intro')}</p>
                          <p class="m-0 mt-0.5 text-xs leading-relaxed text-[var(--hv-wizard-recovery-desc)]">{t('setup.recovery_instruction')}</p>
                        </div>
                      </div>
                      <div class="mt-3 select-none rounded-md border border-[var(--hv-wizard-recovery-border)] bg-[var(--hv-wizard-recovery-code-bg)] px-4 py-3 text-center font-[family-name:var(--hv-font-mono)] text-base font-semibold tracking-[0.15em] text-[var(--hv-wizard-recovery-code-text)]" role="group" aria-label={t('setup.recovery_title')} oncopy={(e) => e.preventDefault()} oncontextmenu={(e) => e.preventDefault()}>
                        {recoveryCode}
                      </div>
                      <p class="m-0 mt-2 text-xs leading-relaxed text-[var(--hv-wizard-recovery-desc)]">{t('setup.recovery_warning')}</p>
                      <label class="mt-3 flex cursor-pointer items-center gap-2.5 text-xs font-medium text-[var(--hv-wizard-recovery-title)]">
                        <input type="checkbox" class="h-4 w-4 accent-[var(--hv-warning)]" bind:checked={recoveryAcknowledged} />
                        <span>{t('setup.recovery_ack')}</span>
                      </label>
                      <Button variant="primary" class={cn(ctaClass, 'mt-4')} fullWidth onclick={continueFromRecovery} disabled={!recoveryAcknowledged}>{t('setup.recovery_continue')}</Button>
                    </div>
                  {/if}
                </div>
              {:else if showTwoFactorSummary}
                <div class="hv-2fa-box mt-5 rounded-2xl border border-[var(--hv-login-field-border)] bg-white p-5">
                  {#if twoFactorUnavailable}
                    <Alert variant="warning" title={t('setup.2fa.unavailable_title')}><p class="m-0">{t('setup.2fa.unavailable_message')}</p></Alert>
                    <Button variant="primary" class={cn(ctaClass, 'mt-3')} fullWidth onclick={finishSetup}>{t('setup.2fa.finish')}</Button>
                  {:else}
                    <Alert variant="success" title={t('setup.2fa.enabled')}><p class="m-0">{t('setup.2fa.enabled_message')}</p></Alert>
                    {#if twoFactorBackupCodes.length > 0}
                      <div class="mt-3">
                        <p class="m-0 mb-1 text-sm font-semibold text-[var(--hv-wizard-text-title)]">{t('setup.2fa.backup_title')}</p>
                        <ul class="hv-2fa-backup-list m-0 grid list-none grid-cols-2 @max-[560px]:grid-cols-1 gap-1.5 p-0 select-none">
                          {#each twoFactorBackupCodes as code}<li class="rounded-md border border-[var(--hv-login-field-border)] bg-white px-2 py-1 text-center font-[family-name:var(--hv-font-mono)] text-xs text-[var(--hv-wizard-text-label)]">{code}</li>{/each}
                        </ul>
                        <p class="m-0 mt-1 text-xs text-[var(--hv-login-muted)]">{t('setup.2fa.backup_note')}</p>
                      </div>
                    {/if}
                    <Button variant="primary" class={cn(ctaClass, 'mt-3')} fullWidth onclick={finishSetup}>{t('setup.2fa.finish')}</Button>
                  {/if}
                </div>
              {:else if verificationResult.valid}
                <div class="mt-4"><Alert variant="success" title={t('setup.verification.success')}><p class="m-0">{t('setup.verification.success_message')}</p></Alert></div>
              {:else if verificationResult.errors.length > 0}
                <div class="mt-4"><Alert variant="error" title={t('setup.verification.error')}>
                  {#each verificationResult.errors as error}<p class="m-0 mt-1">• {error}</p>{/each}
                </Alert></div>
              {:else}
                <div class="mt-4"><Alert variant="info"><p class="m-0">{t('setup.verification.message')}</p></Alert></div>
              {/if}

              {#if verificationResult.warnings.length > 0 && !showRecovery && !showTwoFactorSummary}
                <div class="mt-3"><Alert variant="warning">{#each verificationResult.warnings as warning}<p class="m-0 mt-1">• {warning}</p>{/each}</Alert></div>
              {/if}
            </div>
          {/if}
        </div>

        <!-- Footer nav -->
        <div data-testid="setup-footer" class="flex items-center justify-between gap-2.5 border-t border-[var(--hv-login-hairline)] px-8 py-4 @max-[480px]:px-6">
          <Button variant="secondary" class={ghostPrevClass} onclick={prevStep} disabled={currentStep === 0 || setupComplete || isLoading}>
            <IconChevronLeft size={20} stroke={2} />
            {t('setup.previous')}
          </Button>
          {#if currentStep < totalSteps - 1}
            <Button variant="primary" class={ctaClass} onclick={nextStep} disabled={!canProceed || isVerifying}>
              {t('setup.next')}
              <IconChevronRight size={20} stroke={2} />
            </Button>
          {:else if !showTwoFactorSummary && !showRecovery}
            <Button variant="primary" class={createClass} onclick={handleSubmit} disabled={isLoading || isVerifying || !verificationResult.valid}>
              {#if isLoading}<IconLoader size={20} stroke={2} /> {t('setup.creating')}{:else}<IconCheck size={20} stroke={2} /> {t('setup.create_account')}{/if}
            </Button>
          {/if}
        </div>
      </div>
    </div>
  </div>
</div>