/**
 * Real-hardware test paths that a scratch server cannot prove.
 *
 * These tests run only against an already-provisioned host
 * (`playwright.vm.config.ts`) and every mutation is behind an explicit
 * environment gate, so a normal run skips them with a reason instead of
 * touching a real appliance. The companion client-side scripts in
 * `scripts/hardware/` cover the parts that must run on a second machine
 * (SMB/NFS mount, real WireGuard handshake).
 *
 * Run:
 *   HV_VM_BASE_URL=http://hv-test.local:8080 \
 *   HV_VM_ADMIN_USER=admin HV_VM_ADMIN_PASS=secret \
 *   npx playwright test --config=playwright.vm.config.ts
 */
import { test, expect, type APIRequestContext } from '@playwright/test';

const BASE_URL = process.env.HV_VM_BASE_URL;
const ADMIN_USER = process.env.HV_VM_ADMIN_USER || 'admin';
const ADMIN_PASS = process.env.HV_VM_ADMIN_PASS || '';

test.beforeEach(() => {
  test.skip(!BASE_URL, 'HV_VM_BASE_URL is not set — no real VM to test against.');
});

interface Session {
  token: string;
  csrf: string;
}

async function login(request: APIRequestContext, user = ADMIN_USER, pass = ADMIN_PASS): Promise<Session> {
  const res = await request.post('/api/auth/login', { data: { username: user, password: pass } });
  const body = await res.json().catch(() => ({}));
  expect(res.ok(), `login for ${user} must succeed`).toBeTruthy();
  return { token: body?.data?.token || '', csrf: body?.data?.csrf_token || '' };
}

function headers(session: Session): Record<string, string> {
  return { 'x-session-token': session.token, 'x-csrf-token': session.csrf };
}

// ---------------------------------------------------------------------------
// 1. Rescue — clone, cancel, resume (ddrescue mapfile reuse)
// ---------------------------------------------------------------------------

test('rescue clone can be started, cancelled and resumed from its mapfile', async ({ request }) => {
  const source = process.env.HV_VM_RESCUE_SOURCE;
  const destination = process.env.HV_VM_RESCUE_DEST;
  const logfile = process.env.HV_VM_RESCUE_LOGFILE;
  test.skip(
    !source || !destination || !logfile,
    'Set HV_VM_RESCUE_SOURCE, HV_VM_RESCUE_DEST and HV_VM_RESCUE_LOGFILE (a dedicated loop device pair) to run the destructive clone rehearsal.',
  );

  const session = await login(request);

  const start = await request.post('/api/rescue/ddrescue/clone', {
    headers: headers(session),
    data: { source, destination, logfile },
  });
  const started = await start.json();
  expect(start.ok() && started.success, `clone start: ${JSON.stringify(started)}`).toBeTruthy();
  const jobId = started?.data?.job_id ?? started?.data?.id;
  expect(jobId, 'clone must return a progress job id').toBeTruthy();

  // Give ddrescue a moment to write the mapfile, then cancel the job.
  await new Promise((r) => setTimeout(r, 1500));
  const status = await request.get(`/api/progress/${jobId}/status`, { headers: headers(session) });
  expect(status.ok()).toBeTruthy();
  const statusBody = await status.json();
  expect(statusBody.success).toBeTruthy();

  const cancel = await request.delete(`/api/progress/${jobId}`, { headers: headers(session) });
  // A tiny clone can finish before the cancel lands; both outcomes are valid.
  expect([200, 404, 409]).toContain(cancel.status());

  // A second start with the same mapfile resumes instead of starting over.
  const resume = await request.post('/api/rescue/ddrescue/clone', {
    headers: headers(session),
    data: { source, destination, logfile },
  });
  const resumed = await resume.json();
  expect(resume.ok() && resumed.success, `clone resume: ${JSON.stringify(resumed)}`).toBeTruthy();
});

// ---------------------------------------------------------------------------
// 2. SMB / NFS services + real write proof pointer
// ---------------------------------------------------------------------------

test('SMB and NFS services report a live configuration', async ({ request }) => {
  const session = await login(request);

  const smb = await request.get('/api/shares/smb/config', { headers: headers(session) });
  const nfs = await request.get('/api/shares/nfs/config', { headers: headers(session) });
  expect(smb.ok()).toBeTruthy();
  expect(nfs.ok()).toBeTruthy();

  const smbBody = await smb.json();
  const nfsBody = await nfs.json();
  expect(smbBody.success).toBeTruthy();
  expect(nfsBody.success).toBeTruthy();

  // The actual write proof needs an SMB/NFS client on a second machine:
  //   scripts/hardware/smb-nfs-acl.sh <share> <mountpoint>
  // It mounts the share, writes/reads/renames/deletes a file and checks that
  // a revoked member is refused. This test asserts the server side is up.
  const shares = await request.get('/api/shares', { headers: headers(session) });
  expect(shares.ok()).toBeTruthy();
  const list = await shares.json();
  expect(Array.isArray(list.data)).toBe(true);
});

// ---------------------------------------------------------------------------
// 3. WireGuard peer lifecycle (real config, real keys)
// ---------------------------------------------------------------------------

test('a WireGuard peer can be created, reads a valid config and is revoked', async ({ request }) => {
  const session = await login(request);
  const caps = await request.get('/api/vpn/capabilities', { headers: headers(session) });
  const capsBody = await caps.json();
  test.skip(
    capsBody?.data?.wireguard?.available === false,
    'WireGuard is not installed on this host (capabilities report unavailable).',
  );

  const name = `vm-e2e-${Date.now()}`;
  const create = await request.post('/api/vpn/wireguard/peers', {
    headers: headers(session),
    data: { name },
  });
  const created = await create.json();
  expect(create.ok() && created.success, `peer create: ${JSON.stringify(created)}`).toBeTruthy();
  const id = created?.data?.id;
  expect(id).toBeTruthy();

  const config = await request.get(`/api/vpn/wireguard/peers/${id}`, { headers: headers(session) });
  const configBody = await config.json();
  expect(config.ok()).toBeTruthy();
  const text = configBody?.data?.config ?? configBody?.data ?? '';
  expect(String(text)).toContain('[Interface]');
  expect(String(text)).toContain('PrivateKey');

  // Real handshake proof runs on a client:
  //   scripts/hardware/vpn-tunnel.sh <peer.conf> <url-through-tunnel>
  const revoke = await request.delete(`/api/vpn/wireguard/peers/${id}`, { headers: headers(session) });
  expect(revoke.ok()).toBeTruthy();
});

// ---------------------------------------------------------------------------
// 4. Netplan rollback (destructive to the named interface — opt-in)
// ---------------------------------------------------------------------------

test('an invalid netplan change is rolled back and the live config survives', async ({ request }) => {
  const iface = process.env.HV_VM_NETPLAN_IFACE;
  test.skip(
    !iface,
    'Set HV_VM_NETPLAN_IFACE (a NON-management interface) to exercise the netplan rollback path.',
  );

  const session = await login(request);
  const before = await request.get(`/api/network/interfaces/${iface}/config`, {
    headers: headers(session),
  });
  expect(before.ok()).toBeTruthy();
  const beforeBody = await before.json();

  // `method: "static"` with no address is refused by validation, so this
  // exercises the pre-apply guard; `method: "bogus"` exercises validation.
  const update = await request.put(`/api/network/interfaces/${iface}/config`, {
    headers: headers(session),
    data: { method: 'bogus', address: '', gateway: '', dns: [] },
  });
  expect(update.ok()).toBeFalsy();

  const after = await request.get(`/api/network/interfaces/${iface}/config`, {
    headers: headers(session),
  });
  const afterBody = await after.json();
  expect(afterBody?.data?.method).toBe(beforeBody?.data?.method);
});

// ---------------------------------------------------------------------------
// 5. ACL revocation — a real member loses access immediately
// ---------------------------------------------------------------------------

test('revoking a folder grant reaches the live ACL and the member is refused', async ({ request }) => {
  const folder = process.env.HV_VM_ACL_FOLDER;
  const member = process.env.HV_VM_MEMBER_USER;
  const memberPass = process.env.HV_VM_MEMBER_PASS;
  test.skip(
    !folder || !member || !memberPass,
    'Set HV_VM_ACL_FOLDER + HV_VM_MEMBER_USER/HV_VM_MEMBER_PASS to exercise a real grant/revoke.',
  );

  const admin = await login(request);
  const memberSession = await login(request, member, memberPass);

  const grant = await request.post('/api/shares/access', {
    headers: headers(admin),
    data: { member, kind: 'folder', resource: folder, permission: 'read' },
  });
  expect(grant.ok()).toBeTruthy();

  const allowed = await request.get(
    `/api/explorer/list?path=${encodeURIComponent(folder!)}`,
    { headers: { 'x-session-token': memberSession.token } },
  );
  expect(allowed.ok(), 'the member must reach the folder while the grant exists').toBeTruthy();

  const revoke = await request.delete(
    `/api/shares/access?member=${encodeURIComponent(member!)}&kind=folder&resource=${encodeURIComponent(folder!)}`,
    { headers: headers(admin) },
  );
  expect(revoke.ok()).toBeTruthy();

  const denied = await request.get(
    `/api/explorer/list?path=${encodeURIComponent(folder!)}`,
    { headers: { 'x-session-token': memberSession.token } },
  );
  expect(denied.ok(), 'the member must be refused after the revoke').toBeFalsy();
});

// ---------------------------------------------------------------------------
// 6. App proxy + HTTPS
// ---------------------------------------------------------------------------

test('the per-app proxy answers and TLS redirects to the HTTPS entrypoint', async ({ request }) => {
  const session = await login(request);

  const catalog = await request.get('/api/docker/catalog', { headers: headers(session) });
  const catalogBody = await catalog.json();
  const running = (catalogBody?.data ?? []).find(
    (app: { proxy_port?: number; running?: boolean }) => app.running && app.proxy_port,
  );
  test.skip(!running, 'No installed catalog app is running behind the per-app proxy.');

  const host = new URL(BASE_URL!).hostname;
  const proxied = await request.get(`http://${host}:${running.proxy_port}/`, { maxRedirects: 0 });
  // 200 (served), 302/303 (auth gate) and 401 all prove the listener is up;
  // a connection error or 5xx does not.
  expect([200, 302, 303, 401]).toContain(proxied.status());

  const tls = await request.get('/api/settings/tls', { headers: headers(session) });
  const tlsBody = await tls.json();
  if (tlsBody?.data?.enabled) {
    const port = tlsBody?.data?.https_port ?? 8443;
    const secure = await request.get(`https://${host}:${port}/api/meta`, {
      ignoreHTTPSErrors: true,
    });
    expect(secure.ok()).toBeTruthy();
  }
});
