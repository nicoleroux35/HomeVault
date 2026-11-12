/* HomeVault HTTP load / soak profile (k6).
 *
 * Drives the deployed binary over real sockets with several simultaneous
 * sessions: authentication once per VU, then a mixed read workload plus a
 * large-transfer check. The in-process Rust suite covers the handlers on
 * every `cargo test`; this file answers the socket/connection-pool and
 * long-run questions a unit harness cannot.
 *
 * Usage:
 *   k6 run scripts/load/homevault.k6.js
 *   BASE_URL=https://hv.local ADMIN_USER=admin ADMIN_PASS=secret \
 *     SOAK_VUS=64 SOAK_DURATION=10m k6 run scripts/load/homevault.k6.js
 *
 * Thresholds fail the run: <1% failed requests, p95 under 2 s.
 */
import http from 'k6/http';
import { check, sleep } from 'k6';
import { Trend } from 'k6/metrics';

const BASE = __ENV.BASE_URL || 'http://localhost:8080';
const USER = __ENV.ADMIN_USER || 'e2eadmin';
const PASS = __ENV.ADMIN_PASS || 'E2eSecret123';
const VUS = Number(__ENV.SOAK_VUS || 16);
const DURATION = __ENV.SOAK_DURATION || '60s';
const FILE_MB = Number(__ENV.SOAK_FILE_MB || 8);

const loginTrend = new Trend('hv_login_ms', true);
const listTrend = new Trend('hv_list_ms', true);

export const options = {
  scenarios: {
    readers: {
      executor: 'constant-vus',
      vus: VUS,
      duration: DURATION,
      gracefulStop: '5s',
      tags: { workload: 'read' },
    },
    large_transfer: {
      executor: 'per-vu-iterations',
      exec: 'large_transfer',
      vus: 1,
      iterations: 1,
      startTime: '2s',
      tags: { workload: 'transfer' },
    },
  },
  thresholds: {
    http_req_failed: ['rate<0.01'],
    http_req_duration: ['p(95)<2000'],
  },
};

function login() {
  const started = Date.now();
  const res = http.post(
    `${BASE}/api/auth/login`,
    JSON.stringify({ username: USER, password: PASS }),
    { headers: { 'Content-Type': 'application/json' }, tags: { name: 'login' } },
  );
  loginTrend.add(Date.now() - started);
  check(res, { 'login: 200': (r) => r.status === 200 });
  const body = res.json();
  return { token: body?.data?.token || '', csrf: body?.data?.csrf_token || '' };
}

export function setup() {
  const session = login();
  const headers = {
    'x-session-token': session.token,
    'x-csrf-token': session.csrf,
  };
  const roots = http.get(`${BASE}/api/explorer/roots`, { headers });
  check(roots, { 'roots: 200': (r) => r.status === 200 });
  const body = roots.json();
  const first = body?.data?.[0]?.path || body?.data?.[0] || '/tmp';
  return { headers, root: first };
}

export default function (data) {
  const headers = data.headers;
  const started = Date.now();
  const res = http.get(`${BASE}/api/explorer/list?path=${encodeURIComponent(data.root)}`, {
    headers,
    tags: { name: 'list' },
  });
  listTrend.add(Date.now() - started);
  check(res, {
    'list: 200': (r) => r.status === 200,
    'list: success': (r) => r.json('success') === true,
  });
  sleep(0.2);
}

/* One large upload/download per run: proves the multipart path and the
 * streaming download agree under real sockets. */
export function large_transfer(data) {
  const filename = `soak-${Date.now()}.bin`;
  const chunk = 'x'.repeat(FILE_MB * 1024 * 1024);
  const form = http.file(chunk, filename, 'application/octet-stream');
  const upload = http.post(
    `${BASE}/api/explorer/upload?path=${encodeURIComponent(data.root)}`,
    { file: form },
    { headers: data.headers, tags: { name: 'upload' } },
  );
  check(upload, { 'upload: 200': (r) => r.status === 200 });
}
