<!-- ========================================================================
     File          : Login.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 12/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     The login screen: credentials + optional 2FA step, remember-me,
     recovery-code entry point, and admin-only enforcement (a valid
     non-admin login is rejected with an explanatory error).

     Dependencies:
       - svelte (>= 5), stores (auth, appGate, i18n), theme primitives

     Usage:
     Rendered by App.svelte when the app gate is on 'login'; also the
     redirect target of the app-proxy auth entry.
     ======================================================================== -->
<script lang="ts">
  import { onMount, onDestroy, tick } from 'svelte';
  import { api } from '$lib/services/api';
  import { authStore } from '$lib/stores/auth';
  import { appGate } from '$lib/stores/appGate';
  import { translations, languageStore } from '$lib/stores/i18n';
  import { IconAlertTriangle, IconArrowLeft, IconEye, IconEyeOff, IconLock, IconLogin, IconRefresh, IconShieldCheckered, IconUser, IconWifiOff } from '@tabler/icons-svelte';
  import { Modal, Button } from '$lib/components/theme';
  import { Switch } from '$lib/components/ui/switch/index.js';
  import { cn } from '$lib/utils.js';

  let username = '';
  let password = '';
  let showPassword = false;
  let rememberMe = false;
  let totpCode = '';
  let codeDigits: string[] = ['', '', '', '', '', ''];
  let codeInputs: HTMLInputElement[] = [];
  let isLoading = false;
  let requires2fa = false;
  // Set by the app gate (`App.svelte`): the server is unreachable, so no setup
  // status could be read (drives the connection-error screen).
  export let connectionError = false;
  // Set by the app gate on the hardware-change recovery route: an admin already
  // exists but the recorded fingerprint changed, so signing in once confirms
  // the new hardware (step-up authentication) instead of dead-ending on the
  // Setup Wizard.
  export let recoveryMode = false;
  // Real server version + TLS indicator for the card footer. The version comes
  // from the public `/api/settings/public` endpoint (no auth needed on the
  // login screen); TLS is detected by the transport (HTTPS = built-in TLS or a
  // TLS-terminating reverse proxy).
  let serverVersion = '';
  $: tlsActive = typeof window !== 'undefined' && window.location.protocol === 'https:';

  // DSM-style gateway motion: the card shakes on a failed attempt and shrinks
  // + fades out while a successful login unlocks toward the desktop.
  let shaking = false;
  let unlocking = false;

  // TOTP countdown ring (RFC 6238 30 s step — mirrors the 2FA step).
  const TOTP_R = 12;
  const TOTP_CIRC = 2 * Math.PI * TOTP_R;
  let totpProgress = 1;
  let totpRemaining = 30;
  let totpPulse = false;

  let showErrorModal = false;
  let errorModalMessage = '';
  // Inline field error (the DSM mock shows it under the fields) — shown
  // alongside the modal so the E2E error flow stays intact.
  let inlineError = '';

  $: t = (key: string) =>
    (translations as any)?.[$languageStore]?.[key] || (translations as any)?.en?.[key] || key;

  $: totpCode = codeDigits.join('');

  $: stepLabel = unlocking
    ? t('login.authenticated')
    : showForgotPassword
      ? t('login.forgot_heading')
      : requires2fa
        ? t('login.2fa_heading')
        : t('login.secure_signin');

  $: timerColorClass =
    totpRemaining <= 5
      ? 'stroke-[var(--hv-login-timer-danger)] text-[var(--hv-login-timer-danger)]'
      : totpRemaining <= 10
        ? 'stroke-[var(--hv-login-timer-warn)] text-[var(--hv-login-timer-warn)]'
        : 'stroke-[var(--hv-login-timer-ok)] text-[var(--hv-login-timer-ok)]';

  // base.css ships unlayered global `input { width:100%; border; background;
  // padding; … }` rules that would otherwise beat Tailwind's layered
  // utilities, so every form field carries the trailing `!` important
  // modifier. The DSM field look lives on the wrapper; the inner input is
  // transparent and borderless, the focus ring lives on the wrapper.
  const textInputClass =
    'min-w-0 flex-1 p-0! border-none! bg-transparent! text-[var(--hv-shell-text-strong)] caret-[var(--hv-login-accent)] outline-none placeholder:text-[var(--hv-login-muted)]! focus:shadow-none! disabled:bg-transparent!';
  const fieldWrapperClass =
    'flex h-11 items-center gap-2 rounded-lg border border-[var(--hv-login-field-border)] bg-[var(--hv-login-field-bg)] px-3 text-sm focus-within:border-[var(--hv-login-accent)] focus-within:shadow-[0_0_0_3px_var(--hv-login-accent-ring)]';
  const ctaClass =
    'bg-[image:var(--hv-login-btn-grad)]! rounded-lg! shadow-[var(--hv-login-btn-shadow)] hover:brightness-105 active:scale-[0.98] disabled:opacity-60 disabled:shadow-none';

  let clock = '';
  let dateLabel = '';
  let clockTimer: ReturnType<typeof setInterval>;

  function updateClock() {
    const now = new Date();
    clock = now.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false });
    dateLabel = now.toLocaleDateString([], { weekday: 'long', day: 'numeric', month: 'long', year: 'numeric' });
    const s = now.getSeconds() % 30;
    totpProgress = 1 - s / 30;
    totpRemaining = Math.round(totpProgress * 30);
    totpPulse = s === 0;
  }

  function shake() {
    shaking = true;
    setTimeout(() => (shaking = false), 500);
  }

  function gotoDesktop() {
    // Let the gate take over — it re-renders the desktop and syncs the URL.
    appGate.enterApp();
    window.history.pushState({}, '', '/desktop');
    window.dispatchEvent(new PopStateEvent('popstate'));
  }

  // The connection-error screen's retry: re-run the whole gate (setup status
  // + session), not just this component's view of it.
  function retryConnection() {
    appGate.refresh();
  }

  onMount(() => {
    api.getPublicSettings().then((res) => {
      if (res.success && res.data?.version) serverVersion = res.data.version;
    }).catch(() => {});
    updateClock();
    clockTimer = setInterval(updateClock, 1000);
  });

  onDestroy(() => {
    if (clockTimer) clearInterval(clockTimer);
  });

  $: if (requires2fa && codeInputs[0]) {
    tick().then(() => codeInputs[0]?.focus());
  }

  function togglePasswordVisibility() {
    showPassword = !showPassword;
  }

  function showLoginError(key: string) {
    const message = t(key);
    errorModalMessage = message;
    inlineError = message;
    showErrorModal = true;
    shake();
  }

  function closeErrorModal() {
    showErrorModal = false;
  }

  function handleCodeInput(index: number, event: Event) {
    const input = event.target as HTMLInputElement;
    const value = input.value;
    if (!/^\d*$/.test(value)) { input.value = ''; codeDigits[index] = ''; return; }
    if (value.length > 1) input.value = value.slice(-1);
    codeDigits[index] = input.value;
    inlineError = '';
    if (input.value && index < 5) codeInputs[index + 1]?.focus();
    if (codeDigits.every(d => d !== '')) setTimeout(() => handleLogin(), 300);
  }

  function handleCodeKeydown(index: number, event: KeyboardEvent) {
    if (event.key === 'Backspace') {
      if (!codeDigits[index] && index > 0) {
        event.preventDefault();
        codeDigits[index - 1] = '';
        codeInputs[index - 1].value = '';
        codeInputs[index - 1]?.focus();
      }
    }
  }

  function handleCodePaste(index: number, event: ClipboardEvent) {
    event.preventDefault();
    const pasteData = event.clipboardData?.getData('text').replace(/\D/g, '') || '';
    pasteData.split('').forEach((char, i) => {
      if (index + i < 6) { codeDigits[index + i] = char; codeInputs[index + i].value = char; }
    });
    const lastIndex = Math.min(index + pasteData.length - 1, 5);
    codeInputs[lastIndex]?.focus();
  }

  async function handleLogin() {
    if (!username && !password) { showLoginError('login.validation.required'); return; }
    if (!username) { showLoginError('login.validation.username_required'); return; }
    if (!password) { showLoginError('login.validation.password_required'); return; }
    if (requires2fa) {
      if (!totpCode) { showLoginError('login.validation.totp_required'); return; }
      if (totpCode.length !== 6 || !/^\d{6}$/.test(totpCode)) { showLoginError('login.validation.totp_invalid'); return; }
    }

    isLoading = true;
    try {
      const result = await authStore.login(username, password, totpCode || undefined, rememberMe);
      if (result.success) {
        if (!result.isAdmin) { isLoading = false; showLoginError('login.error.not_admin'); return; }
        // Hardware-change recovery: the admin's sign-in doubles as the
        // confirmation step, so re-baseline the fingerprint before entering.
        if (recoveryMode) {
          const rebaseline = await api.rebaselineSetup();
          if (!rebaseline.success) {
            isLoading = false;
            showLoginError('login.hardware_recovery.failed');
            return;
          }
        }
        unlocking = true;
        setTimeout(gotoDesktop, 700);
      } else if (result.requires2fa) {
        requires2fa = true;
        inlineError = '';
      } else {
        const errorMsg = result.error || '';
        if (errorMsg.includes('credentials') || errorMsg.includes('invalid') || errorMsg.includes('password')) {
          showLoginError('login.error.invalid_credentials');
        } else if (errorMsg.includes('locked') || errorMsg.includes('disabled')) {
          showLoginError('login.error.account_locked');
        } else if (errorMsg.toLowerCase().includes('too many')) {
          showLoginError('login.error.rate_limited');
        } else if (errorMsg.includes('totp') || errorMsg.includes('2fa') || errorMsg.includes('code')) {
          if (requires2fa) { codeDigits = ['', '', '', '', '', '']; codeInputs.forEach(inp => inp.value = ''); codeInputs[0]?.focus(); }
          showLoginError('login.error.invalid_totp');
        } else if (errorMsg.includes('expired') || errorMsg.includes('session')) {
          showLoginError('login.error.session_expired');
        } else if (errorMsg.includes('network') || errorMsg.includes('fetch')) {
          showLoginError('login.error.network');
        } else if (errorMsg.includes('timeout')) {
          showLoginError('login.error.timeout');
        } else if (errorMsg.includes('500') || errorMsg.includes('server')) {
          showLoginError('login.error.server');
        } else {
          showLoginError('login.error');
        }
      }
    } catch (error: any) {
      if (error.message?.includes('fetch') || error.message?.includes('network')) {
        showLoginError('login.error.network');
      } else if (error.message?.includes('timeout')) {
        showLoginError('login.error.timeout');
      } else {
        showLoginError('login.error');
      }
    } finally {
      isLoading = false;
    }
  }

  function handleBackTo2FA() {
    requires2fa = false;
    codeDigits = ['', '', '', '', '', ''];
    totpCode = '';
    inlineError = '';
  }

  // Email-free "forgot password": the user re-types the numeric recovery code
  // they wrote down during setup, then chooses a new password in a popup.
  let showForgotPassword = false;
  let recoveryUsername = '';
  let recoveryCodeInput = '';
  let isVerifyingRecovery = false;
  let recoveryVerifyError = '';

  let showNewPasswordModal = false;
  let newPassword = '';
  let newPasswordConfirm = '';
  let showNewPasswordText = false;
  let isResettingRecovery = false;
  let recoveryResetError = '';
  let recoveryResetSuccess = false;

  function openForgotPassword() {
    showForgotPassword = true;
    recoveryUsername = username;
    recoveryCodeInput = '';
    recoveryVerifyError = '';
    recoveryResetSuccess = false;
    inlineError = '';
  }

  function backToLoginFromForgot() {
    showForgotPassword = false;
    showNewPasswordModal = false;
    recoveryResetSuccess = false;
    recoveryVerifyError = '';
    recoveryResetError = '';
  }

  // Recovery codes are 32 digits shown as 8 groups of 4. Keep only digits and
  // re-insert a space every 4 so the user never has to type the separators.
  function formatRecoveryCode(raw: string): string {
    const digits = raw.replace(/\D/g, '').slice(0, 32);
    return digits.replace(/(.{4})(?=.)/g, '$1 ');
  }

  function handleRecoveryCodeInput(e: Event) {
    const el = e.currentTarget as HTMLInputElement;
    const caret = el.selectionStart ?? el.value.length;
    const digitsBeforeCaret = el.value.slice(0, caret).replace(/\D/g, '').length;
    const formatted = formatRecoveryCode(el.value);
    el.value = formatted;
    recoveryCodeInput = formatted;
    // Restore the caret after the same number of digits.
    let pos = 0;
    let seen = 0;
    while (pos < formatted.length && seen < digitsBeforeCaret) {
      if (/\d/.test(formatted[pos])) seen++;
      pos++;
    }
    if (formatted[pos] === ' ') pos++;
    el.setSelectionRange(pos, pos);
  }

  async function handleRecoveryVerify() {
    recoveryVerifyError = '';
    if (!recoveryUsername.trim() || !recoveryCodeInput.trim()) {
      recoveryVerifyError = t('login.recovery_error_invalid');
      return;
    }
    isVerifyingRecovery = true;
    try {
      const res = await api.verifyRecoveryCode(recoveryUsername.trim(), recoveryCodeInput.trim());
      if (res.success && res.data?.valid) {
        newPassword = '';
        newPasswordConfirm = '';
        recoveryResetError = '';
        showNewPasswordModal = true;
      } else {
        const msg = (res.message || (res as any).error || '').toLowerCase();
        recoveryVerifyError = msg.includes('too many')
          ? t('login.recovery_error_ratelimited')
          : t('login.recovery_error_invalid');
      }
    } catch {
      recoveryVerifyError = t('login.error.network');
    } finally {
      isVerifyingRecovery = false;
    }
  }

  function closeNewPasswordModal() {
    showNewPasswordModal = false;
  }

  async function handleRecoveryReset() {
    recoveryResetError = '';
    if (!newPassword || newPassword !== newPasswordConfirm) {
      recoveryResetError = t('login.reset_mismatch');
      return;
    }
    isResettingRecovery = true;
    try {
      const result = await api.resetPasswordWithRecovery(
        recoveryUsername.trim(),
        recoveryCodeInput.trim(),
        newPassword,
      );
      if (result.success) {
        showNewPasswordModal = false;
        recoveryResetSuccess = true;
      } else {
        const msg = (result.message || (result as any).error || '').toLowerCase();
        if (msg.includes('too many')) recoveryResetError = t('login.recovery_error_ratelimited');
        else if (msg.includes('recently') || msg.includes('reused')) recoveryResetError = t('login.reset_error_reused');
        else if (msg.includes('security requirements') || msg.includes('at least 8')) recoveryResetError = t('login.reset_error_weak');
        else if (msg.includes('incorrect') || msg.includes('recovery')) recoveryResetError = t('login.recovery_error_invalid');
        else recoveryResetError = result.message || t('login.reset_error');
      }
    } catch {
      recoveryResetError = t('login.error.network');
    } finally {
      isResettingRecovery = false;
    }
  }

</script>

<div class="hv-login-page relative min-h-screen w-full overflow-y-auto bg-[image:var(--hv-login-bg-gradient)]" data-testid="login-page">
  <div class="pointer-events-none fixed inset-0 overflow-hidden" aria-hidden="true">
      <div class="absolute top-1/4 left-1/3 h-96 w-96 rounded-full bg-[image:var(--hv-login-orb-indigo)] opacity-40 blur-[70px]"></div>
      <div class="absolute right-1/3 bottom-1/4 h-80 w-80 rounded-full bg-[image:var(--hv-login-orb-pink)] opacity-30 blur-[60px]"></div>
    </div>

    {#if !connectionError}
      <div class="pointer-events-none fixed top-10 left-1/2 z-[1] -translate-x-1/2 text-center max-[480px]:top-6">
        <div class="text-[4.5rem] leading-none font-light tracking-[-0.02em] text-[var(--hv-login-clock)] tabular-nums max-[480px]:text-[3rem]">{clock}</div>
        <div class="mt-1 text-sm uppercase tracking-[0.12em] text-[var(--hv-login-muted)]">{dateLabel}</div>
      </div>

      <div class="relative z-[1] flex min-h-screen items-center justify-center px-4 pt-[140px] pb-10 max-[480px]:pt-[116px] max-[480px]:pb-8">
        <div
          class={cn(
            'w-[var(--hv-login-card-width)] max-w-full overflow-hidden rounded-2xl border border-[var(--hv-login-card-border)] bg-white shadow-[var(--hv-login-card-shadow)] transition-all duration-500 ease-out',
            shaking && 'animate-[hv-shake_0.4s_ease]',
            unlocking && 'scale-95 opacity-0',
          )}
          data-testid="login-container"
        >
          {#if showForgotPassword}
            <div class="border-b border-[var(--hv-login-hairline)] px-8 pt-8 pb-6 text-center max-[480px]:px-6 max-[480px]:pt-7 max-[480px]:pb-5">
              <div class="mx-auto mb-4 grid h-14 w-14 place-items-center rounded-2xl bg-[image:var(--hv-login-logo-grad)] shadow-[var(--hv-login-logo-shadow)]" data-testid="login-icon-box">
                <svg viewBox="0 0 24 24" width="30" height="30" fill="none" stroke="white" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M3 10.5 12 3l9 7.5"/><path d="M5 9.5V21h14V9.5"/><circle cx="12" cy="14" r="2.4"/><path d="M12 16.4V19"/></svg>
              </div>
              <h1 class="m-0! text-base! font-semibold tracking-wide text-[var(--hv-shell-text-strong)]" data-testid="login-brand-title">HomeVault</h1>
              <div class="mt-0.5 text-xs uppercase tracking-wider text-[var(--hv-login-muted)]" data-testid="login-secure-signin">{stepLabel}</div>
            </div>

            <div class="flex flex-col gap-4 px-8 py-6 max-[480px]:px-6 max-[480px]:py-5">
              {#if recoveryResetSuccess}
                <p class="m-0 text-center text-sm text-[var(--hv-shell-text-2)]" data-testid="login-recovery-success">{t('login.recovery_success')}</p>
                <Button variant="primary" size="lg" fullWidth type="button" onclick={backToLoginFromForgot}>
                  <IconLogin size={18} stroke={2} />
                  <span>{t('login.sign_in')}</span>
                </Button>
              {:else}
                <p class="m-0 text-[13px] leading-relaxed text-[var(--hv-shell-text-2)]">{t('login.recovery_description')}</p>
                <form onsubmit={(e)=>{e.preventDefault();handleRecoveryVerify();}}>
                  <div class="flex flex-col gap-1.5">
                    <label for="recovery-username" class="text-xs font-medium uppercase tracking-[0.1em] text-[var(--hv-login-muted)]">{t('login.username_label')}</label>
                    <div class={fieldWrapperClass}>
                      <IconUser size={19} stroke={1.7} class="shrink-0 text-[var(--hv-login-muted)]" />
                      <input id="recovery-username" type="text" class={textInputClass} bind:value={recoveryUsername} placeholder={t('login.username_placeholder')} disabled={isVerifyingRecovery} required autocomplete="username" />
                    </div>
                  </div>
                  <div class="mt-4 flex flex-col gap-1.5">
                    <label for="recovery-code" class="text-xs font-medium uppercase tracking-[0.1em] text-[var(--hv-login-muted)]">{t('login.recovery_code_label')}</label>
                    <div class={fieldWrapperClass}>
                      <IconShieldCheckered size={19} stroke={1.7} class="shrink-0 text-[var(--hv-login-muted)]" />
                      <input id="recovery-code" type="text" class={textInputClass} value={recoveryCodeInput} oninput={handleRecoveryCodeInput} placeholder={t('login.recovery_code_placeholder')} disabled={isVerifyingRecovery} inputmode="numeric" maxlength="39" autocomplete="off" required />
                    </div>
                  </div>
                  {#if recoveryVerifyError}<p class="m-0 mt-2 text-[13px] text-[var(--hv-error-text)]" role="alert" data-testid="login-reset-error">{recoveryVerifyError}</p>{/if}
                  <Button variant="primary" class={ctaClass} size="lg" fullWidth type="submit" disabled={isVerifyingRecovery} data-testid="login-recovery-submit">
                    {#if isVerifyingRecovery}<div class="hv-spinner"></div><span>{t('login.signing_in')}</span>{:else}<span>{t('login.recovery_submit')}</span>{/if}
                  </Button>
                  <Button variant="ghost" fullWidth type="button" onclick={backToLoginFromForgot} disabled={isVerifyingRecovery} data-testid="login-back">
                    <IconArrowLeft size={16} stroke={2} />{t('login.back')}
                  </Button>
                </form>
              {/if}
            </div>
          {:else if !requires2fa}
            <div class="border-b border-[var(--hv-login-hairline)] px-8 pt-8 pb-6 text-center max-[480px]:px-6 max-[480px]:pt-7 max-[480px]:pb-5">
              <div class="mx-auto mb-4 grid h-14 w-14 place-items-center rounded-2xl bg-[image:var(--hv-login-logo-grad)] shadow-[var(--hv-login-logo-shadow)]" data-testid="login-icon-box">
                <svg viewBox="0 0 24 24" width="30" height="30" fill="none" stroke="white" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M3 10.5 12 3l9 7.5"/><path d="M5 9.5V21h14V9.5"/><circle cx="12" cy="14" r="2.4"/><path d="M12 16.4V19"/></svg>
              </div>
              <h1 class="m-0! text-base! font-semibold tracking-wide text-[var(--hv-shell-text-strong)]" data-testid="login-brand-title">HomeVault</h1>
              <div class="mt-0.5 text-xs uppercase tracking-wider text-[var(--hv-login-muted)]" data-testid="login-secure-signin">{stepLabel}</div>
            </div>

            <form onsubmit={(e)=>{e.preventDefault();handleLogin();}}>
              <div class="flex flex-col gap-4 px-8 py-6 max-[480px]:px-6 max-[480px]:py-5">
                {#if recoveryMode}
                  <div class="flex items-start gap-2 rounded-lg border border-[var(--hv-login-timer-warn)] bg-[var(--hv-login-accent-soft)] px-3 py-2.5" role="status" data-testid="login-hardware-recovery">
                    <IconAlertTriangle size={18} stroke={2} class="mt-0.5 shrink-0 text-[var(--hv-login-timer-warn)]" />
                    <p class="m-0 text-[13px] leading-relaxed text-[var(--hv-shell-text-2)]">{t('login.hardware_recovery.notice')}</p>
                  </div>
                {/if}
                <div class="flex flex-col gap-1.5">
                  <label for="username" class="text-xs font-medium uppercase tracking-[0.1em] text-[var(--hv-login-muted)]">{t('login.username_label')}</label>
                  <div class={fieldWrapperClass}>
                    <IconUser size={19} stroke={1.7} class="shrink-0 text-[var(--hv-login-muted)]" />
                    <input id="username" type="text" class={textInputClass} bind:value={username} oninput={() => inlineError = ''} placeholder={t('login.username_placeholder')} disabled={isLoading} required autocomplete="username" />
                  </div>
                </div>
                <div class="flex flex-col gap-1.5">
                  <label for="password" class="text-xs font-medium uppercase tracking-[0.1em] text-[var(--hv-login-muted)]">{t('login.password_label')}</label>
                  <div class={fieldWrapperClass}>
                    <IconLock size={19} stroke={1.7} class="shrink-0 text-[var(--hv-login-muted)]" />
                    <input id="password" type={showPassword ? 'text' : 'password'} class={textInputClass} bind:value={password} oninput={() => inlineError = ''} placeholder={t('login.password_placeholder')} disabled={isLoading} required autocomplete="current-password" />
                    <button type="button" data-testid="login-password-toggle" class="grid h-8 w-8 flex-none cursor-pointer place-items-center rounded-full border-none bg-transparent text-[var(--hv-login-muted)] hover:text-[var(--hv-shell-text-strong)] disabled:opacity-60 hv-ic" aria-label={showPassword ? t('login.hide_password') : t('login.show_password')} onclick={togglePasswordVisibility} disabled={isLoading}>
                      {#if showPassword}<IconEyeOff size={20} stroke={1.7}/>{:else}<IconEye size={20} stroke={1.7}/>{/if}
                    </button>
                  </div>
                </div>
                {#if inlineError}
                  <div class="flex items-center gap-2 text-xs text-[var(--hv-login-error)]">
                    <svg viewBox="0 0 16 16" fill="currentColor" class="h-3.5 w-3.5 shrink-0"><circle cx="8" cy="8" r="7" opacity=".15"/><path d="M8 4v5M8 11v1" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>
                    {inlineError}
                  </div>
                {/if}
                <div class="flex items-center justify-between gap-2">
                  <div data-testid="login-remember" class="flex items-center gap-2.5">
                    <Switch bind:checked={rememberMe} disabled={isLoading} aria-label={t('login.remember_me')} />
                    <span class="text-sm text-[var(--hv-shell-text-2)]">{t('login.remember_me')}</span>
                  </div>
                  <button type="button" data-testid="login-forgot-link" class="cursor-pointer border-none bg-transparent p-0 text-[13px] font-medium text-[var(--hv-login-accent-deep)] hover:underline disabled:opacity-60" onclick={openForgotPassword} disabled={isLoading}>
                    {t('login.forgot_password_link')}
                  </button>
                </div>
                <Button variant="primary" class={ctaClass} size="lg" fullWidth type="submit" disabled={isLoading}>
                  {#if isLoading}<div class="hv-spinner"></div><span>{t('login.signing_in')}</span>{:else}<span>{t('login.sign_in')}</span>{/if}
                </Button>
              </div>
            </form>
          {:else}
            <div class="border-b border-[var(--hv-login-hairline)] px-8 pt-8 pb-6 text-center max-[480px]:px-6 max-[480px]:pt-7 max-[480px]:pb-5">
              <div class="mx-auto mb-4 grid h-14 w-14 place-items-center rounded-2xl bg-[image:var(--hv-login-logo-grad)] shadow-[var(--hv-login-logo-shadow)]" data-testid="login-icon-box">
                <svg viewBox="0 0 24 24" width="30" height="30" fill="none" stroke="white" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M3 10.5 12 3l9 7.5"/><path d="M5 9.5V21h14V9.5"/><circle cx="12" cy="14" r="2.4"/><path d="M12 16.4V19"/></svg>
              </div>
              <h1 class="m-0! text-base! font-semibold tracking-wide text-[var(--hv-shell-text-strong)]" data-testid="login-brand-title">HomeVault</h1>
              <div class="mt-0.5 text-xs uppercase tracking-wider text-[var(--hv-login-muted)]" data-testid="login-secure-signin">{stepLabel}</div>
            </div>

            <form onsubmit={(e)=>{e.preventDefault();handleLogin();}}>
              <div class="flex flex-col gap-5 px-8 py-6 max-[480px]:px-6 max-[480px]:py-5">
                <div class="mb-1 flex items-center justify-center gap-2" data-testid="login-step-progress" aria-hidden="true">
                  <div class="grid h-6 w-6 place-items-center rounded-full border border-[var(--hv-login-accent-soft-border)] bg-[var(--hv-login-accent-soft)] text-[var(--hv-login-accent)]">
                    <svg viewBox="0 0 12 12" width="12" height="12" fill="none"><path d="M2 6l3 3 5-5" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>
                  </div>
                  <div class="h-px w-8 bg-[var(--hv-login-step-line)]"></div>
                  <div class="grid h-6 w-6 place-items-center rounded-full bg-[image:var(--hv-login-logo-grad)] text-xs font-semibold text-white">2</div>
                </div>

                <h2 class="m-0 text-center text-sm! font-medium text-[var(--hv-login-clock)]!" data-testid="login-step-title">{t('login.totp_prompt')}</h2>
                <p class="m-0 mt-1 text-center text-[13px] text-[var(--hv-login-muted)]" data-testid="login-step-description">{t('login.totp_prompt_hint')}</p>

                <div class="flex items-center justify-center gap-2.5 max-[480px]:gap-2">
                  {#each codeDigits as digit, index}
                    <input
                      bind:this={codeInputs[index]}
                      type="text"
                      inputmode="numeric"
                      maxlength="1"
                      pattern="[0-9]*"
                      data-testid="login-code-input"
                      aria-label={`${t('login.totp_prompt')} ${index + 1} / ${codeDigits.length}`}
                      class={cn(
                        'h-[52px] w-11! max-[480px]:h-12 max-[480px]:w-10! text-center text-xl! font-semibold text-[var(--hv-shell-text-strong)] outline-none transition-all tabular-nums caret-transparent',
                        'rounded-xl! p-0!',
                        'focus:border-[var(--hv-login-accent)]! focus:shadow-[0_0_0_3px_var(--hv-login-accent-ring)]! disabled:opacity-60 disabled:bg-[var(--hv-login-field-bg)]!',
                        digit !== ''
                          ? 'border-[1.5px]! border-[var(--hv-login-accent)]! bg-[var(--hv-login-accent-soft)]! shadow-[0_0_0_3px_var(--hv-login-accent-ring)]!'
                          : 'border border-[var(--hv-login-muted)]! bg-[var(--hv-login-field-bg)]!',
                      )}
                      value={digit}
                      oninput={(e) => handleCodeInput(index, e)}
                      onkeydown={(e) => handleCodeKeydown(index, e)}
                      onpaste={(e) => handleCodePaste(index, e)}
                      disabled={isLoading}
                      autocomplete="one-time-code"
                    />
                  {/each}
                </div>

                {#if inlineError}
                  <div class="flex items-center justify-center gap-2 text-xs text-[var(--hv-login-error)]">
                    <svg viewBox="0 0 16 16" fill="currentColor" class="h-3.5 w-3.5 shrink-0"><circle cx="8" cy="8" r="7" opacity=".15"/><path d="M8 4v5M8 11v1" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>
                    {inlineError}
                  </div>
                {/if}

                <div class={cn('flex items-center justify-center gap-2', totpPulse && 'animate-pulse')} data-testid="login-totp-timer">
                  <svg width="32" height="32" viewBox="0 0 32 32" class="-rotate-90">
                    <circle cx="16" cy="16" r={TOTP_R} class="fill-none stroke-[var(--hv-login-timer-track)] stroke-[2.5]"></circle>
                    <circle cx="16" cy="16" r={TOTP_R} class={cn('fill-none stroke-[2.5] stroke-linecap-round transition-[stroke-dashoffset] duration-[900ms] ease-linear', timerColorClass)} stroke-dasharray={TOTP_CIRC} stroke-dashoffset={TOTP_CIRC * (1 - totpProgress)}></circle>
                  </svg>
                  <span class={cn('text-[13px] tabular-nums', timerColorClass)}>{String(totpRemaining).padStart(2, '0')}s</span>
                </div>

                <Button variant="primary" class={ctaClass} size="lg" fullWidth type="submit" disabled={isLoading}>
                  {#if isLoading}<div class="hv-spinner"></div><span>{t('login.verifying')}</span>{:else}<span>{t('login.verify')}</span>{/if}
                </Button>
                <button type="button" class="mx-auto cursor-pointer border-none bg-transparent text-xs text-[var(--hv-login-muted)] transition-colors hover:text-[var(--hv-shell-text-2)] disabled:opacity-60" data-testid="login-back" onclick={handleBackTo2FA} disabled={isLoading}>
                  ← {t('login.back')}
                </button>
              </div>
            </form>
          {/if}

          <div class="px-8 pb-6 text-center max-[480px]:px-6 max-[480px]:pb-5">
            <span class="text-xs text-[var(--hv-login-muted)]" data-testid="login-brand-subtitle">HomeVault{serverVersion ? ` v${serverVersion}` : ''} · {t('login.secure_signin')}{tlsActive ? ' · TLS 1.3' : ''}</span>
          </div>
        </div>
      </div>
    {:else}
      <div class="flex min-h-screen items-center justify-center px-4 py-6">
        {#if connectionError}
          <div data-testid="login-error-state" class="flex flex-col items-center gap-3 px-6 py-10 text-center text-[var(--hv-shell-text-2)]">
            <IconWifiOff size={32} stroke={2} class="text-[var(--hv-login-muted)]" />
            <p class="m-0 text-sm">{t('login.connection_error')}</p>
            <Button variant="primary" class="hv-error-retry" onclick={retryConnection}><IconRefresh size={16} stroke={2} /> {t('login.retry')}</Button>
          </div>
        {:else}
          <div class="hv-spinner hv-login-spinner h-8 w-8 border-4" data-testid="login-spinner"></div>
        {/if}
      </div>
    {/if}
</div>

<Modal open={showErrorModal} title={t('login.error_title')} size="sm" onclose={closeErrorModal}>
  {#snippet icon()}<IconAlertTriangle size={22} stroke={2} />{/snippet}
  {#snippet children()}<p class="m-0 text-sm text-[var(--hv-text-secondary)]" data-testid="login-error-msg">{errorModalMessage}</p>{/snippet}
  {#snippet footer()}<Button variant="primary" onclick={closeErrorModal} data-testid="login-error-dismiss">{t('login.error_dismiss')}</Button>{/snippet}
</Modal>

<Modal open={showNewPasswordModal} title={t('login.recovery_modal_title')} size="sm" onclose={closeNewPasswordModal}>
  {#snippet icon()}<IconLock size={22} stroke={2} />{/snippet}
  {#snippet children()}
    <form onsubmit={(e)=>{e.preventDefault();handleRecoveryReset();}}>
      <div class="mb-4">
        <label for="recovery-new-pw" class="block mb-1.5 text-c-label font-semibold text-[var(--hv-text-primary)]">{t('login.reset_new_password_label')}</label>
        <div class="flex items-center gap-2 h-12 px-3 rounded-xl border border-[var(--hv-shell-border-strong)] bg-[var(--hv-shell-border-soft)] focus-within:border-[var(--hv-primary)]">
          <IconLock size={19} stroke={1.7} class="text-[var(--hv-text-tertiary)]" />
          <input id="recovery-new-pw" type={showNewPasswordText ? 'text' : 'password'} class="flex-1 min-w-0 border-none bg-transparent text-sm text-[var(--hv-text-primary)] outline-none" bind:value={newPassword} placeholder={t('login.reset_new_password_placeholder')} disabled={isResettingRecovery} required autocomplete="new-password" />
          <button type="button" class="grid place-items-center w-8 h-8 flex-none rounded-full border-none bg-transparent text-[var(--hv-text-tertiary)] cursor-pointer hover:text-[var(--hv-text-primary)] hv-ic" aria-label={showNewPasswordText ? t('login.hide_password') : t('login.show_password')} onclick={() => showNewPasswordText = !showNewPasswordText} disabled={isResettingRecovery}>
            {#if showNewPasswordText}<IconEyeOff size={20} stroke={1.7}/>{:else}<IconEye size={20} stroke={1.7}/>{/if}
          </button>
        </div>
      </div>
      <div class="mb-4">
        <label for="recovery-confirm-pw" class="block mb-1.5 text-c-label font-semibold text-[var(--hv-text-primary)]">{t('login.reset_confirm_password_label')}</label>
        <div class="flex items-center gap-2 h-12 px-3 rounded-xl border border-[var(--hv-shell-border-strong)] bg-[var(--hv-shell-border-soft)] focus-within:border-[var(--hv-primary)]">
          <IconLock size={19} stroke={1.7} class="text-[var(--hv-text-tertiary)]" />
          <input id="recovery-confirm-pw" type={showNewPasswordText ? 'text' : 'password'} class="flex-1 min-w-0 border-none bg-transparent text-sm text-[var(--hv-text-primary)] outline-none" bind:value={newPasswordConfirm} placeholder={t('login.reset_confirm_password_placeholder')} disabled={isResettingRecovery} required autocomplete="new-password" />
        </div>
      </div>
      {#if recoveryResetError}<p class="m-0 mb-2 text-xs text-[var(--hv-error-text)]" role="alert" data-testid="login-reset-error">{recoveryResetError}</p>{/if}
      <Button variant="primary" size="lg" fullWidth type="submit" disabled={isResettingRecovery} data-testid="login-recovery-submit">
        {#if isResettingRecovery}<div class="hv-spinner"></div><span>{t('login.signing_in')}</span>{:else}<span>{t('login.recovery_modal_submit')}</span>{/if}
      </Button>
    </form>
  {/snippet}
</Modal>