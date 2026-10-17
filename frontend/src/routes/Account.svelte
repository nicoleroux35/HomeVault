<!-- ========================================================================
     File          : Account.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     The My Account app: self-service profile edit, avatar regeneration,
     password change, 2FA enrollment/management, active-session list with
     revocation and personal login history.

     Dependencies:
       - svelte (>= 5), theme + app/ composites, $lib/services/api

     Usage:
     Opened as a desktop window app.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t, translate, languageStore } from '$lib/stores/i18n';
  import { authStore } from '$lib/stores/auth';
  import UserAvatar from '$lib/components/theme/UserAvatar.svelte';
  import { Button } from '$lib/components/theme';
  import { PageLayout, SectionCard, ConfirmDialog } from '$lib/components/app';
  import { createConfirmState } from '$lib/utils/confirmDialog.svelte';
  import { cn } from '$lib/utils.js';
  import type { User as UserType } from '$lib/types/api';
  import { IconUser as UserIcon, IconKey, IconShieldCheckered, IconShieldOff, IconRefresh, IconDeviceDesktop, IconDeviceMobile, IconLogout, IconHistory } from '@tabler/icons-svelte';

  const LOCALE_MAP: Record<string, string> = { en: 'en-US', fr: 'fr-FR', de: 'de-DE', es: 'es-ES' };

  let user: UserType | null = null;
  let loading = true;

  let profileForm = { fullname: '', email: '' };
  let profileSubmitting = false;
  let profileError = '';
  let profileSuccess = '';

  let passwordForm = { currentPassword: '', newPassword: '', confirmPassword: '' };
  let passwordSubmitting = false;
  let passwordErrors: Record<string, string> = {};

  // Avatar regeneration
  let avatarRegenerating = false;

  // 2FA enrollment flow
  let enrolling = false;
  let twoFaBusy = false;
  let secret = '';
  let otpauthUrl = '';
  let code = '';
  let enrollPassword = '';
  let twoFaError = '';

  // Backup codes viewer
  let backupCodes: string[] = [];
  let showBackupCodes = false;

  // 2FA disable flow
  let showDisableForm = false;
  let disablePassword = '';
  let disableBusy = false;
  let disableError = '';

  // Shared control shape for this screen's native inputs (appliance tokens
  // only — the per-app CSS vocabulary is gone).
  const inp =
    'w-full box-border h-[46px] px-3 border border-[var(--hv-shell-border-strong)] rounded-lg ' +
    'bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-strong)] text-c-md outline-none ' +
    'transition-colors placeholder:text-[var(--hv-shell-text-3)] focus:border-[var(--hv-primary)] ' +
    'disabled:text-[var(--hv-shell-text-2)] disabled:opacity-80 disabled:cursor-not-allowed';

  onMount(async () => {
    await load();
    loadSessions();
    loadHistory();
  });

  async function load(silent = false) {
    if (!silent) loading = true;
    const res = await api.getCurrentUser();
    if (res.success && res.data) {
      user = res.data;
      profileForm = { fullname: user!.fullname || '', email: user!.email || '' };
    }
    if (!silent) loading = false;
    return res;
  }

  function formatDate(s: string): string {
    if (!s) return '—';
    return new Date(s).toLocaleDateString(LOCALE_MAP[$languageStore] || 'en-US', { day: 'numeric', month: 'short', year: 'numeric' });
  }

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

  async function regenerateAvatar() {
    if (!user) return;
    avatarRegenerating = true;
    const seed = generateSeed();
    const res = await api.updateMyAvatar(seed);
    if (res.success && res.data) {
      user.avatar_seed = res.data.seed || seed;
      user.avatar_url = res.data.avatar_url || user.avatar_url;
      authStore.set({
        user: { ...user, avatar_seed: user.avatar_seed, avatar_url: user.avatar_url },
        isAuthenticated: true,
        isLoading: false,
      });
    }
    avatarRegenerating = false;
  }

  async function saveProfile() {
    if (!user) return;
    profileError = '';
    profileSuccess = '';
    profileSubmitting = true;
    const res = await api.updateUser(user.username, {
      fullname: profileForm.fullname,
      email: profileForm.email,
    });
    profileSubmitting = false;
    if (res.success) {
      profileSuccess = translate('users.profile_saved');
      setTimeout(() => { profileSuccess = ''; }, 4000);
      await load(true);
      authStore.set({
        user: { ...user, fullname: profileForm.fullname, email: profileForm.email },
        isAuthenticated: true,
        isLoading: false,
      });
    } else {
      profileError = res.message || res.error || translate('common.error');
    }
  }

  async function changePassword() {
    if (!user) return;
    passwordErrors = {};
    if (!passwordForm.currentPassword) {
      passwordErrors = { current: translate('account.current_password_required') };
      return;
    }
    if (!passwordForm.newPassword || passwordForm.newPassword.length < 12) {
      passwordErrors = { new: translate('users.password_min') };
      return;
    }
    if (passwordForm.newPassword !== passwordForm.confirmPassword) {
      passwordErrors = { confirm: translate('users.passwords_mismatch') };
      return;
    }
    passwordSubmitting = true;
    const res = await api.updateUserPassword(user.username, passwordForm.newPassword, passwordForm.currentPassword);
    passwordSubmitting = false;
    if (res.success) {
      passwordForm = { currentPassword: '', newPassword: '', confirmPassword: '' };
      await authStore.logout();
      window.history.pushState({}, '', '/login');
      window.dispatchEvent(new PopStateEvent('popstate'));
    } else {
      passwordErrors = { current: res.message || res.error || translate('common.error') };
    }
  }

  async function startEnroll2FA() {
    if (!user || user.two_fa_enabled) return;
    twoFaError = '';
    code = '';
    enrollPassword = '';
    twoFaBusy = true;
    const res = await api.setup2FASecret();
    twoFaBusy = false;
    if (res.success && res.data) {
      secret = res.data.secret;
      otpauthUrl = res.data.qrcode_url;
      enrolling = true;
    } else {
      twoFaError = res.message || translate('account.2fa_generate_failed');
    }
  }

  function cancelEnroll2FA() {
    enrolling = false;
    twoFaBusy = false;
    secret = '';
    otpauthUrl = '';
    code = '';
    enrollPassword = '';
    twoFaError = '';
  }

  async function confirmEnroll2FA() {
    twoFaError = '';
    if (code.length !== 6) {
      twoFaError = translate('account.2fa_invalid_code');
      return;
    }
    if (!enrollPassword) {
      twoFaError = translate('account.2fa_password_required');
      return;
    }
    twoFaBusy = true;
    const res = await api.verify2FASecret(code, enrollPassword);
    twoFaBusy = false;
    if (res.success) {
      backupCodes = res.data?.backup_codes || [];
      showBackupCodes = true;
      enrolling = false;
      await load(true);
    } else {
      twoFaError = res.message || translate('account.2fa_verify_failed');
    }
  }

  function closeBackupCodes() {
    showBackupCodes = false;
    backupCodes = [];
  }

  async function viewBackupCodes() {
    const res = await api.getMy2FABackupCodes();
    if (res.success && res.data) {
      backupCodes = res.data.codes || [];
      showBackupCodes = true;
    } else {
      twoFaError = res.message || translate('account.2fa_backup_failed');
    }
  }

  function startDisable2FA() {
    showDisableForm = true;
    disablePassword = '';
    disableError = '';
  }

  function cancelDisable2FA() {
    showDisableForm = false;
    disableBusy = false;
    disablePassword = '';
    disableError = '';
  }

  async function confirmDisable2FA() {
    disableError = '';
    if (!disablePassword) {
      disableError = translate('account.2fa_password_required');
      return;
    }
    disableBusy = true;
    const res = await api.disableMy2FA(disablePassword);
    disableBusy = false;
    if (res.success) {
      showDisableForm = false;
      disablePassword = '';
      backupCodes = [];
      showBackupCodes = false;
      await load(true);
    } else {
      disableError = res.message || translate('account.2fa_disable_failed');
    }
  }

  // Active sessions + recent sign-in history (self-service session management)
  interface SessionInfo {
    token: string;
    ip_address: string;
    created_at: string;
    expires_at: string;
    is_current: boolean;
  }
  interface LoginAttemptInfo {
    username: string;
    ip_address: string;
    success: boolean;
    timestamp: string;
    user_agent?: string | null;
  }
  const dlg = createConfirmState();

  let sessions: SessionInfo[] = [];
  let sessionsLoading = false;
  let sessionsBusy = false;
  let sessionsError = '';
  let sessionsNotice = '';

  let history: LoginAttemptInfo[] = [];
  let historyLoading = false;
  let historyError = '';

  async function loadSessions() {
    sessionsLoading = true;
    sessionsError = '';
    sessionsNotice = '';
    const res = await api.getMySessions();
    sessionsLoading = false;
    if (res.success && Array.isArray(res.data?.sessions)) {
      sessions = res.data.sessions;
    } else {
      sessionsError = res.message || res.error || translate('account.session_action_failed');
    }
  }

  async function loadHistory() {
    if (!user) return;
    historyLoading = true;
    historyError = '';
    const res = await api.getUserLoginHistory(user.username);
    historyLoading = false;
    if (res.success && Array.isArray(res.data?.attempts)) {
      history = res.data.attempts;
    } else {
      historyError = res.message || res.error || translate('account.session_action_failed');
    }
  }

  async function revokeSession(token: string) {
    sessionsError = '';
    sessionsNotice = '';
    sessionsBusy = true;
    const res = await api.revokeSessions([token]);
    sessionsBusy = false;
    if (res.success) {
      sessions = sessions.filter(s => s.token !== token);
      sessionsNotice = translate('account.revoke_success');
    } else {
      sessionsError = res.message || res.error || translate('account.session_action_failed');
    }
  }

  async function revokeAllOtherSessions() {
    if (!(await dlg.ask(translate('account.revoke_all_others_confirm')))) return;
    sessionsError = '';
    sessionsNotice = '';
    sessionsBusy = true;
    const res = await api.revokeAllSessions();
    sessionsBusy = false;
    if (res.success) {
      sessions = sessions.filter(s => s.is_current);
      sessionsNotice = translate('account.revoke_all_success');
    } else {
      sessionsError = res.message || res.error || translate('account.session_action_failed');
    }
  }

  function formatDateTime(s: string): string {
    if (!s) return '—';
    return new Date(s).toLocaleString(LOCALE_MAP[$languageStore] || 'en-US', {
      day: 'numeric',
      month: 'short',
      year: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  }

  // Each SectionCard renders its own icon + label header via `titleSnippet`.
</script>

<PageLayout
  title={$t('account.heading')}
  data-testid="account-page"
>
  {#snippet icon()}
    <UserIcon stroke={1.7} />
  {/snippet}

  {#if loading}
    <div data-testid="account-state" class="flex-1 min-h-0 flex flex-col items-center justify-center gap-3 text-[var(--hv-shell-text)] text-c-base">
      <div class="w-[30px] h-[30px] rounded-full border-[3px] border-[var(--hv-shell-border-strong)] border-t-[var(--hv-primary)] animate-spin"></div>
      <p class="m-0">{$t('account.loading')}</p>
    </div>
  {:else if user}
    <div data-testid="account-body" class="@container">
      <div class="mx-auto w-full max-w-[980px] grid grid-cols-2 gap-4 items-start py-1 pb-10 @max-[880px]:grid-cols-1">
        <!-- Identity -->
        <header data-testid="account-identity" class="col-span-2 flex items-center gap-4 flex-wrap pb-4 border-b border-[var(--hv-shell-border)] @max-[560px]:gap-3">
          <div data-testid="account-avatar" class="relative flex-none leading-none">
            <UserAvatar username={user.username} seed={user.avatar_seed ?? null} size={64} />
            <button
              data-testid="account-avatar-regen"
              class="absolute -right-1.5 -bottom-1.5 w-[30px] h-[30px] grid place-items-center p-0 border-2 border-[var(--hv-shell-glass)] rounded-full bg-[var(--hv-app-accent-grad)] text-white cursor-pointer transition-transform hover:scale-105 active:scale-95 disabled:opacity-50 disabled:cursor-default"
              onclick={regenerateAvatar}
              disabled={avatarRegenerating}
              aria-label={$t('chat.regenerate')}
            >
              <span class:animate-spin={avatarRegenerating}><IconRefresh size={15} stroke={2} /></span>
            </button>
          </div>
          <div class="flex-1 min-w-0">
            <h2 data-testid="account-identity-name" class="m-0 text-c-2xl font-semibold tracking-tight text-[var(--hv-shell-text-strong)] truncate">{user.fullname || user.username}</h2>
            {#if user.email}<p data-testid="account-identity-sub" class="mt-0.5 font-[family-name:var(--hv-font-mono)] text-c-label text-[var(--hv-shell-text)] truncate">@{user.username} · {user.email}</p>{:else if user.username}<p data-testid="account-identity-sub" class="mt-0.5 font-[family-name:var(--hv-font-mono)] text-c-label text-[var(--hv-shell-text)] truncate">@{user.username}</p>{/if}
            <p data-testid="account-identity-meta" class="mt-1.5 text-xs text-[var(--hv-shell-text-3)]">
              {$t('account.member_since', { date: formatDate(user.created_at ?? '') })}
            </p>
          </div>
          <span
            data-testid="account-tag"
            class:on={user.two_fa_enabled}
            class={cn(
              'flex-none inline-flex items-center gap-1.5 h-7 px-3 rounded-full text-c-xs font-bold tracking-wide border transition-colors @max-[560px]:order-3',
              user.two_fa_enabled
                ? 'bg-[var(--hv-primary-light)] border-[var(--hv-primary)]/40 text-[var(--hv-primary)]'
                : 'bg-[var(--hv-shell-fill)] border-[var(--hv-shell-border)] text-[var(--hv-shell-text-2)]',
            )}
          >
            {#if user.two_fa_enabled}<IconShieldCheckered size={14} stroke={2} />{:else}<IconShieldOff size={14} stroke={2} />{/if}
            {user.two_fa_enabled ? $t('account.2fa_enabled_badge') : $t('account.2fa_disabled_badge')}
          </span>
        </header>

        <!-- Profile -->
        <SectionCard data-testid="account-card" class="w-full">
          {#snippet titleSnippet()}
            <div data-testid="account-card-title" class="flex items-center gap-2 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">
              <UserIcon size={17} stroke={2} class="text-[var(--hv-primary)]" /> {$t('account.section_profile')}
            </div>
          {/snippet}

          {#if profileSuccess}<div data-testid="account-ok" class="mb-3 px-3 py-2.5 rounded-lg bg-[var(--hv-success-bg)] border border-[var(--hv-success)]/40 text-[var(--hv-success-text)] text-c-label">{profileSuccess}</div>{/if}
          {#if profileError}<div data-testid="account-err" class="block mb-3 px-3 py-2.5 rounded-lg bg-[var(--hv-error-bg)] border border-[var(--hv-error)]/40 text-[var(--hv-error-text)] text-c-label">{profileError}</div>{/if}

          <div class="flex flex-col gap-1.5 mb-3">
            <label for="account-username-display" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('users.username_label')}</label>
            <input id="account-username-display" class={cn(inp, 'font-[family-name:var(--hv-font-mono)] text-c-base')} value={user.username} disabled />
          </div>
          <div class="grid grid-cols-2 gap-3 @max-[560px]:grid-cols-1">
            <div class="flex flex-col gap-1.5">
              <label for="a-fullname" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('users.fullname_label')}</label>
              <input id="a-fullname" class={inp} bind:value={profileForm.fullname} />
            </div>
            <div class="flex flex-col gap-1.5">
              <label for="a-email" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('users.email_label')}</label>
              <input id="a-email" class={inp} type="email" autocomplete="email" bind:value={profileForm.email} />
            </div>
          </div>
          <div class="flex flex-wrap gap-2.5 mt-4">
            <Button variant="primary" onclick={saveProfile} disabled={profileSubmitting}>
              {$t('account.save_profile')}
            </Button>
          </div>
        </SectionCard>

        <!-- Password -->
        <SectionCard data-testid="account-card" class="w-full">
          {#snippet titleSnippet()}
            <div data-testid="account-card-title" class="flex items-center gap-2 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">
              <IconKey size={17} stroke={2} class="text-[var(--hv-primary)]" /> {$t('account.section_password')}
            </div>
          {/snippet}

          <div class="flex flex-col gap-1.5 mb-3">
            <label for="a-current-pw" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('account.current_password_label')}</label>
            <input id="a-current-pw" class={cn(inp, passwordErrors.current && 'border-[var(--hv-error)]')} type="password" autocomplete="current-password" bind:value={passwordForm.currentPassword} />
            {#if passwordErrors.current}<span data-testid="account-err" class="text-c-label text-[var(--hv-error-text)]">{passwordErrors.current}</span>{/if}
          </div>
          <div class="grid grid-cols-2 gap-3 @max-[560px]:grid-cols-1">
            <div class="flex flex-col gap-1.5">
              <label for="a-new-pw" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('users.new_password_label')}</label>
              <input id="a-new-pw" class={cn(inp, passwordErrors.new && 'border-[var(--hv-error)]')} type="password" autocomplete="new-password" bind:value={passwordForm.newPassword} placeholder={$t('users.password_min')} />
              {#if passwordErrors.new}<span data-testid="account-err" class="text-c-label text-[var(--hv-error-text)]">{passwordErrors.new}</span>{/if}
            </div>
            <div class="flex flex-col gap-1.5">
              <label for="a-confirm-pw" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('users.confirm_label')}</label>
              <input id="a-confirm-pw" class={cn(inp, passwordErrors.confirm && 'border-[var(--hv-error)]')} type="password" autocomplete="new-password" bind:value={passwordForm.confirmPassword} placeholder={$t('users.repeat_password')} />
              {#if passwordErrors.confirm}<span data-testid="account-err" class="text-c-label text-[var(--hv-error-text)]">{passwordErrors.confirm}</span>{/if}
            </div>
          </div>
          <div class="flex flex-wrap gap-2.5 mt-4">
            <Button variant="primary" onclick={changePassword} disabled={passwordSubmitting}>
              {$t('account.update_password_btn')}
            </Button>
          </div>
        </SectionCard>

        <!-- Two-factor authentication -->
        <SectionCard data-testid="account-card" class="col-span-2 w-full @max-[880px]:col-span-1">
          {#snippet titleSnippet()}
            <div data-testid="account-card-title" class="flex items-center gap-2 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">
              {#if user?.two_fa_enabled}<IconShieldCheckered size={17} stroke={2} class="text-[var(--hv-primary)]" />{:else}<IconShieldOff size={17} stroke={2} class="text-[var(--hv-primary)]" />{/if}
              {$t('account.section_security')}
            </div>
          {/snippet}

          {#if showBackupCodes}
            {#if backupCodes.length > 0}
              <p class="mb-2.5 text-c-base font-semibold text-[var(--hv-shell-text-strong)]">{$t('account.2fa_backup_title')}</p>
              <ul data-testid="account-backup-list" class="list-none m-0 mb-3 p-0 grid grid-cols-2 gap-2">
                {#each backupCodes as bc}
                  <li class="font-[family-name:var(--hv-font-mono)] text-c-base text-center py-2 px-2.5 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]">{bc}</li>
                {/each}
              </ul>
              <p class="my-0 mb-3 text-c-label leading-relaxed text-[var(--hv-shell-text-2)]">{$t('account.2fa_backup_note')}</p>
              <div class="flex flex-wrap gap-2.5 mt-4">
                <Button variant="ghost" onclick={closeBackupCodes}>{$t('account.done_btn')}</Button>
              </div>
            {:else}
              <p class="my-0 mb-3 text-c-label leading-relaxed text-[var(--hv-shell-text-2)]">{$t('account.2fa_no_codes')}</p>
              <div class="flex flex-wrap gap-2.5 mt-4">
                <Button variant="ghost" onclick={closeBackupCodes}>{$t('account.done_btn')}</Button>
              </div>
            {/if}
          {:else if enrolling}
            <p class="my-0 mb-3 text-c-label leading-relaxed text-[var(--hv-shell-text-2)]">{$t('account.2fa_intro')}</p>
            <div class="flex flex-col gap-1.5 mb-3">
              <span class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('account.2fa_secret_label')}</span>
              <code data-testid="account-secret" class="block font-[family-name:var(--hv-font-mono)] text-c-base text-[var(--hv-shell-text-strong)] bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] px-3 py-2.5 rounded-lg break-all">{secret}</code>
            </div>
            {#if otpauthUrl}
              <div class="flex flex-col gap-1.5 mb-3">
                <span class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('account.2fa_link_label')}</span>
                <a href={otpauthUrl} data-testid="account-secret" class="block font-[family-name:var(--hv-font-mono)] text-c-xs text-[var(--hv-primary)] no-underline hover:underline bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] px-3 py-2.5 rounded-lg break-all" target="_blank" rel="noreferrer">{otpauthUrl}</a>
              </div>
            {/if}
            <div class="grid grid-cols-2 gap-3 @max-[560px]:grid-cols-1">
              <div class="flex flex-col gap-1.5">
                <label for="a-2fa-code" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('account.2fa_code_label')}</label>
                <input id="a-2fa-code" class={cn(inp, 'font-[family-name:var(--hv-font-mono)] text-c-xl tracking-[0.32em] text-center')} inputmode="numeric" maxlength="6" autocomplete="one-time-code" placeholder="000000" bind:value={code} />
              </div>
              <div class="flex flex-col gap-1.5">
                <label for="a-2fa-pw" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('account.2fa_password_label')}</label>
                <input id="a-2fa-pw" class={inp} type="password" autocomplete="current-password" bind:value={enrollPassword} />
              </div>
            </div>
            {#if twoFaError}<span data-testid="account-err" class="text-c-label text-[var(--hv-error-text)]">{twoFaError}</span>{/if}
            <div class="flex flex-wrap gap-2.5 mt-4">
              <Button variant="ghost" onclick={cancelEnroll2FA} disabled={twoFaBusy}>{$t('common.cancel')}</Button>
              <Button variant="primary" onclick={confirmEnroll2FA} disabled={twoFaBusy}>{$t('account.2fa_verify_btn')}</Button>
            </div>
          {:else if showDisableForm}
            <p class="my-0 mb-3 text-c-label leading-relaxed text-[var(--hv-shell-text-2)]">{$t('account.2fa_disable_desc')}</p>
            <div class="flex flex-col gap-1.5 mb-3">
              <label for="a-2fa-disable-pw" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('account.current_password_label')}</label>
              <input id="a-2fa-disable-pw" class={cn(inp, disableError && 'border-[var(--hv-error)]')} type="password" autocomplete="current-password" bind:value={disablePassword} />
              {#if disableError}<span data-testid="account-err" class="text-c-label text-[var(--hv-error-text)]">{disableError}</span>{/if}
            </div>
            <div class="flex flex-wrap gap-2.5 mt-4">
              <Button variant="ghost" onclick={cancelDisable2FA} disabled={disableBusy}>{$t('common.cancel')}</Button>
              <Button variant="danger" onclick={confirmDisable2FA} disabled={disableBusy}>{$t('account.2fa_disable_confirm_btn')}</Button>
            </div>
          {:else}
            <div data-testid="account-toggle-row" class="flex items-center gap-3 px-3 py-3 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]">
              <span data-testid="account-status-ico" class:on={user.two_fa_enabled} class={cn('flex-none grid place-items-center w-11 h-11 rounded-lg transition-colors', user.two_fa_enabled ? 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]' : 'bg-[var(--hv-shell-border)] text-[var(--hv-shell-text-2)]')}>
                {#if user.two_fa_enabled}<IconShieldCheckered size={20} stroke={1.9} />{:else}<IconShieldOff size={20} stroke={1.9} />{/if}
              </span>
              <div class="min-w-0">
                <strong class="block text-sm font-semibold text-[var(--hv-shell-text-strong)]">{user.two_fa_enabled ? $t('account.2fa_enabled_badge') : $t('account.2fa_disabled_badge')}</strong>
                <small class="text-xs text-[var(--hv-shell-text)] leading-relaxed">{user.two_fa_enabled ? $t('account.2fa_enabled_desc') : $t('account.2fa_disabled_desc')}</small>
              </div>
            </div>
            <div class="flex flex-wrap gap-2.5 mt-4">
              {#if user.two_fa_enabled}
                <Button variant="ghost" onclick={viewBackupCodes}>{$t('account.view_backup_codes')}</Button>
                <Button variant="danger" onclick={startDisable2FA}>{$t('account.disable_2fa_btn')}</Button>
              {:else}
                <Button variant="primary" onclick={startEnroll2FA} disabled={twoFaBusy}>{$t('account.enable_2fa_btn')}</Button>
              {/if}
            </div>
          {/if}
        </SectionCard>

        <!-- Active sessions -->
        <SectionCard data-testid="account-card" class="col-span-2 w-full @max-[880px]:col-span-1">
          {#snippet titleSnippet()}
            <div data-testid="account-card-title" class="flex items-center gap-2 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">
              <IconDeviceDesktop size={17} stroke={2} class="text-[var(--hv-primary)]" /> {$t('account.section_sessions')}
            </div>
          {/snippet}

          {#if sessionsNotice}<div data-testid="account-ok" class="mb-3 px-3 py-2.5 rounded-lg bg-[var(--hv-success-bg)] border border-[var(--hv-success)]/40 text-[var(--hv-success-text)] text-c-label">{sessionsNotice}</div>{/if}
          {#if sessionsError}<div data-testid="account-err" class="block mb-3 px-3 py-2.5 rounded-lg bg-[var(--hv-error-bg)] border border-[var(--hv-error)]/40 text-[var(--hv-error-text)] text-c-label">{sessionsError}</div>{/if}

          <p class="m-0 mb-3 text-c-label leading-relaxed text-[var(--hv-shell-text-2)]">{$t('account.sessions_desc')}</p>

          {#if sessionsLoading}
            <p data-testid="sessions-loading" class="m-0 text-c-label text-[var(--hv-shell-text-3)]">{$t('common.loading')}</p>
          {:else if sessions.length === 0}
            <p data-testid="sessions-empty" class="m-0 text-c-label text-[var(--hv-shell-text-3)]">{$t('account.sessions_empty')}</p>
          {:else}
            <ul data-testid="sessions-list" class="list-none m-0 p-0 flex flex-col gap-2">
              {#each sessions as s (s.token)}
                <li data-testid="session-row" class="flex items-center gap-3 px-3 py-2.5 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]">
                  <span class="flex-none grid place-items-center w-11 h-11 rounded-lg bg-[var(--hv-shell-border)] text-[var(--hv-shell-text-2)]">
                    {#if s.is_current}<IconDeviceMobile size={20} stroke={1.9} />{:else}<IconDeviceDesktop size={20} stroke={1.9} />{/if}
                  </span>
                  <div class="flex-1 min-w-0">
                    <div class="flex items-center gap-2 flex-wrap">
                      <strong class="font-[family-name:var(--hv-font-mono)] text-sm text-[var(--hv-shell-text-strong)]">{s.ip_address}</strong>
                      {#if s.is_current}<span data-testid="session-current" class="flex-none inline-flex items-center h-7 px-2 rounded-full text-c-xs font-bold bg-[var(--hv-primary-light)] text-[var(--hv-primary)]">{$t('account.this_device')}</span>{/if}
                    </div>
                    <small class="block text-xs text-[var(--hv-shell-text)] mt-0.5">
                      {$t('account.signed_in')}: {formatDateTime(s.created_at)} — {$t('account.expires')}: {formatDateTime(s.expires_at)}
                    </small>
                  </div>
                  {#if s.is_current}
                    <span class="flex-none text-c-xs text-[var(--hv-shell-text-3)] px-2">{$t('account.revoke_current_blocked')}</span>
                  {:else}
                    <button
                      data-testid="session-revoke"
                      class="hv-ic flex-none grid place-items-center w-[30px] h-[30px] p-0 rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer transition-colors hover:text-[var(--hv-error-text)] focus-visible:outline-none focus-visible:shadow-[var(--hv-focus-ring)] disabled:opacity-50 disabled:cursor-default"
                      onclick={() => revokeSession(s.token)}
                      disabled={sessionsBusy}
                      aria-label={$t('account.revoke_device')}
                    >
                      <IconLogout size={16} stroke={2} />
                    </button>
                  {/if}
                </li>
              {/each}
            </ul>
          {/if}

          <div class="flex flex-wrap gap-2.5 mt-4">
            <Button variant="ghost" onclick={loadSessions} disabled={sessionsLoading}>{$t('common.refresh')}</Button>
            <Button variant="danger" onclick={revokeAllOtherSessions} disabled={sessionsBusy || sessions.length <= 1}>
              {$t('account.revoke_all_others')}
            </Button>
          </div>
        </SectionCard>

        <!-- Recent sign-ins -->
        <SectionCard data-testid="account-card" class="col-span-2 w-full @max-[880px]:col-span-1">
          {#snippet titleSnippet()}
            <div data-testid="account-card-title" class="flex items-center gap-2 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">
              <IconHistory size={17} stroke={2} class="text-[var(--hv-primary)]" /> {$t('account.section_history')}
            </div>
          {/snippet}

          {#if historyError}<div data-testid="account-err" class="block mb-3 px-3 py-2.5 rounded-lg bg-[var(--hv-error-bg)] border border-[var(--hv-error)]/40 text-[var(--hv-error-text)] text-c-label">{historyError}</div>{/if}

          {#if historyLoading}
            <p data-testid="history-loading" class="m-0 text-c-label text-[var(--hv-shell-text-3)]">{$t('common.loading')}</p>
          {:else if history.length === 0}
            <p data-testid="history-empty" class="m-0 text-c-label text-[var(--hv-shell-text-3)]">{$t('account.history_empty')}</p>
          {:else}
            <ul data-testid="history-list" class="list-none m-0 p-0 flex flex-col gap-2">
              {#each history as h (h.timestamp + h.ip_address)}
                <li data-testid="history-row" class="flex items-center gap-3 px-3 py-2.5 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]">
                  <span class={cn('flex-none w-2.5 h-2.5 rounded-full', h.success ? 'bg-[var(--hv-success)]' : 'bg-[var(--hv-error)]')}></span>
                  <div class="flex-1 min-w-0">
                    <div class="flex items-center gap-2 flex-wrap">
                      <strong class="font-[family-name:var(--hv-font-mono)] text-sm text-[var(--hv-shell-text-strong)]">{h.ip_address}</strong>
                      <span class={cn('flex-none inline-flex items-center h-7 px-2 rounded-full text-c-xs font-bold', h.success ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success-text)]' : 'bg-[var(--hv-error-bg)] text-[var(--hv-error-text)]')}>{h.success ? $t('account.history_success') : $t('account.history_failed')}</span>
                    </div>
                    <small class="block text-xs text-[var(--hv-shell-text)] mt-0.5">
                      {formatDateTime(h.timestamp)}
                      {#if h.user_agent}<span class="text-[var(--hv-shell-text-3)]"> · {h.user_agent}</span>{/if}
                    </small>
                  </div>
                </li>
              {/each}
            </ul>
          {/if}
        </SectionCard>
      </div>
    </div>
  {/if}
  <ConfirmDialog
    {dlg}
    onconfirm={() => dlg.settle(true)}
    oncancel={() => dlg.settle(false)}
  />
</PageLayout>