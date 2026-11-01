/* =============================================================================
 * File          : passkey.ts
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The browser half of a WebAuthn ceremony: turn the server's options into a
 *   `navigator.credentials.create()` / `.get()` call and the resulting
 *   credential back into the base64url payload the server verifies. Every
 *   base64url conversion lives here, because a single encoding slip (standard
 *   base64 instead of url-safe, or a padded string) makes the signature fail
 *   with no useful clue.
 *
 * Dependencies:
 *   - $lib/services/api (the begin/finish calls)
 *
 * Usage:
 *   stores/passkey.ts exposes registerPasskey()/signInWithPasskey(), used by
 *   the Account screen's Passkeys card and the Login screen's button.
 * =============================================================================
 */

import { api } from '$lib/services/api';

/** `true` when this browser can run a WebAuthn ceremony at all. */
export function passkeySupported(): boolean {
  return (
    typeof window !== 'undefined' &&
    typeof window.PublicKeyCredential !== 'undefined' &&
    typeof navigator !== 'undefined' &&
    !!navigator.credentials
  );
}

/** Whether the platform authenticator (Touch ID, Windows Hello…) exists. */
export async function platformAuthenticatorAvailable(): Promise<boolean> {
  if (!passkeySupported()) return false;
  try {
    return await window.PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable();
  } catch {
    return false;
  }
}

/**
 * base64url → bytes.
 *
 * A `Uint8Array` is a valid `BufferSource` at runtime, but its TypeScript type
 * is now generic over `ArrayBufferLike`, which `PublicKeyCredentialRequestOptions`
 * refuses — hence the explicit buffer copy in the callers below.
 */
function toBytes(value: string): Uint8Array {
  const padded = value.replace(/-/g, '+').replace(/_/g, '/');
  const binary = atob(padded + '='.repeat((4 - (padded.length % 4)) % 4));
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) out[i] = binary.charCodeAt(i);
  return out;
}

/** A freshly-owned `ArrayBuffer` holding the decoded bytes. */
function bufferOf(value: string): ArrayBuffer {
  return toBytes(value).buffer as ArrayBuffer;
}

function fromBytes(bytes: ArrayBuffer): string {
  const view = new Uint8Array(bytes);
  let binary = '';
  for (let i = 0; i < view.length; i += 1) binary += String.fromCharCode(view[i]);
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

/**
 * Register a new passkey for the signed-in account.
 *
 * Answers the stored credential on success, and a **sentence** on failure —
 * including the case where the browser has no authenticator, which is not an
 * error worth a stack trace.
 */
export async function registerPasskey(
  label: string
): Promise<{ ok: true; credentialId: string } | { ok: false; error: string }> {
  if (!passkeySupported()) {
    return { ok: false, error: 'unsupported' };
  }
  const begin = await api.beginPasskeyRegistration();
  if (!begin.success || !begin.data) {
    return { ok: false, error: begin.message || begin.error || 'begin-failed' };
  }
  const options = begin.data as any;

  let credential: PublicKeyCredential | null = null;
  try {
    credential = (await navigator.credentials.create({
      publicKey: {
        challenge: bufferOf(options.challenge),
        rp: options.rp,
        user: {
          id: bufferOf(options.user.id),
          name: options.user.name,
          displayName: options.user.display_name
        },
        pubKeyCredParams: options.pub_key_cred_params,
        timeout: options.timeout,
        attestation: options.attestation,
        authenticatorSelection: {
          userVerification: options.authenticator_selection?.user_verification ?? 'preferred',
          residentKey: options.authenticator_selection?.resident_key ?? 'preferred'
        },
        excludeCredentials: (options.exclude_credentials ?? []).map((c: any) => ({
          type: c.type,
          id: bufferOf(c.id)
        }))
      }
    })) as PublicKeyCredential | null;
  } catch (e: any) {
    // The user cancelling is the common case and must not look like a bug.
    if (e?.name === 'NotAllowedError') return { ok: false, error: 'cancelled' };
    return { ok: false, error: e?.message || 'ceremony-failed' };
  }
  if (!credential) return { ok: false, error: 'cancelled' };

  const response = credential.response as AuthenticatorAttestationResponse;
  const finish = await api.finishPasskeyRegistration({
    client_data_json: fromBytes(response.clientDataJSON),
    attestation_object: fromBytes(response.attestationObject),
    credential_id: fromBytes(credential.rawId),
    label
  });
  if (!finish.success) {
    return { ok: false, error: finish.message || finish.error || 'finish-failed' };
  }
  return { ok: true, credentialId: fromBytes(credential.rawId) };
}

/**
 * Sign in with a passkey.
 *
 * Answers the session payload the server issued (the same shape the password
 * login returns) so the caller can hand it to the auth store.
 */
export async function signInWithPasskey(
  username: string,
  rememberMe = false
): Promise<{ ok: true; data: any; csrf: string } | { ok: false; error: string }> {
  if (!passkeySupported()) {
    return { ok: false, error: 'unsupported' };
  }
  const begin = await api.beginPasskeyLogin(username);
  if (!begin.success || !begin.data) {
    return { ok: false, error: begin.message || begin.error || 'begin-failed' };
  }
  const options = begin.data as any;

  let credential: PublicKeyCredential | null = null;
  try {
    credential = (await navigator.credentials.get({
      publicKey: {
        challenge: bufferOf(options.challenge),
        rpId: options.rp_id,
        timeout: options.timeout,
        allowCredentials: (options.allow_credentials ?? []).map((c: any) => ({
          type: c.type,
          id: bufferOf(c.id)
        })),
        userVerification: options.user_verification ?? 'preferred'
      }
    })) as PublicKeyCredential | null;
  } catch (e: any) {
    if (e?.name === 'NotAllowedError') return { ok: false, error: 'cancelled' };
    return { ok: false, error: e?.message || 'ceremony-failed' };
  }
  if (!credential) return { ok: false, error: 'cancelled' };

  const response = credential.response as AuthenticatorAssertionResponse;
  const finish = await api.finishPasskeyLogin({
    credential_id: fromBytes(credential.rawId),
    client_data_json: fromBytes(response.clientDataJSON),
    authenticator_data: fromBytes(response.authenticatorData),
    signature: fromBytes(response.signature),
    username,
    remember_me: rememberMe
  });
  if (!finish.success || !finish.data) {
    return { ok: false, error: finish.message || finish.error || 'finish-failed' };
  }
  return { ok: true, data: finish.data, csrf: (finish.data as any).csrf_token ?? '' };
}
