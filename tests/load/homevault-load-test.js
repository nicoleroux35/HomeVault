import http from 'k6/http';
import { check, sleep, group } from 'k6';
import { Rate, Trend, Counter } from 'k6/metrics';
import { htmlReport } from 'https://raw.githubusercontent.com/benc-uk/k6-reporter/main/dist/bundle.js';
import { textSummary } from 'https://jslib.k6.io/k6-summary/0.0.1/index.js';

// Custom metrics
const errorRate = new Rate('errors');
const loginDuration = new Trend('login_duration');
const apiCallDuration = new Trend('api_call_duration');
const failedRequests = new Counter('failed_requests');

// Test configuration
export const options = {
  stages: [
    { duration: '30s', target: 10 }, // Ramp-up to 10 users
    { duration: '1m', target: 50 },  // Ramp-up to 50 users
    { duration: '2m', target: 100 }, // Plateau at 100 users
    { duration: '1m', target: 50 },  // Ramp-down to 50
    { duration: '30s', target: 0 },  // Ramp-down to 0
  ],
  thresholds: {
    http_req_duration: ['p(95)<500', 'p(99)<1000'], // 95% under 500ms, 99% under 1s
    http_req_failed: ['rate<0.05'], // Less than 5% errors
    errors: ['rate<0.1'], // Less than 10% error rate
    login_duration: ['p(95)<1000'], // Login under 1s for 95%
    api_call_duration: ['p(95)<300'], // API calls under 300ms for 95%
  },
  ext: {
    loadimpact: {
      projectID: 3588801,
      name: 'HomeVault Load Test',
    },
  },
};

// Environment variables
const BASE_URL = __ENV.BASE_URL || 'http://localhost:8080';
const ADMIN_USER = __ENV.ADMIN_USER || 'e2eadmin';
const ADMIN_PASS = __ENV.ADMIN_PASS || 'E2eSecret123';

// Test data
const wallpapers = Array.from({ length: 44 }, (_, i) => `hv-${String(i + 1).padStart(2, '0')}`);
const colors = ['#2E72C9', '#167A4D', '#96601A', '#C0392B', '#6A45D6'];

export function setup() {
  console.log(`Starting load test against ${BASE_URL}`);

  // Verify server is up
  const res = http.get(`${BASE_URL}/api/health`);
  check(res, {
    'server is up': (r) => r.status === 200,
  });

  return { baseUrl: BASE_URL };
}

export default function (data) {
  const baseUrl = data.baseUrl;
  let token = null;

  // Group 1: Authentication
  group('Authentication', function () {
    const loginStart = Date.now();

    const loginPayload = JSON.stringify({
      username: ADMIN_USER,
      password: ADMIN_PASS,
      remember: false,
    });

    const loginRes = http.post(`${baseUrl}/api/auth/login`, loginPayload, {
      headers: { 'Content-Type': 'application/json' },
    });

    const loginSuccess = check(loginRes, {
      'login successful': (r) => r.status === 200,
      'received token': (r) => {
        try {
          const body = JSON.parse(r.body);
          return body.success && body.data && body.data.token;
        } catch {
          return false;
        }
      },
    });

    if (loginSuccess) {
      const body = JSON.parse(loginRes.body);
      token = body.data.token;
      loginDuration.add(Date.now() - loginStart);
    } else {
      errorRate.add(1);
      failedRequests.add(1);
      console.error(`Login failed: ${loginRes.status} - ${loginRes.body}`);
      return; // Skip rest of test if login fails
    }

    sleep(0.5);
  });

  if (!token) return;

  const headers = {
    'X-Session-Token': token,
    'Content-Type': 'application/json',
  };

  // Group 2: Read operations (most common)
  group('Read Operations', function () {
    const endpoints = [
      '/api/auth/me',
      '/api/settings/appearance',
      '/api/health',
      '/api/dashboard/storage',
    ];

    endpoints.forEach((endpoint) => {
      const start = Date.now();
      const res = http.get(`${baseUrl}${endpoint}`, { headers });

      const success = check(res, {
        [`${endpoint} - status 200`]: (r) => r.status === 200,
        [`${endpoint} - response time OK`]: (r) => r.timings.duration < 500,
      });

      if (!success) {
        errorRate.add(1);
        failedRequests.add(1);
      } else {
        apiCallDuration.add(Date.now() - start);
      }

      sleep(0.3);
    });
  });

  // Group 3: Write operations (less frequent)
  if (Math.random() < 0.3) { // 30% of users perform writes
    group('Write Operations', function () {
      const wallpaper = wallpapers[Math.floor(Math.random() * wallpapers.length)];
      const color = colors[Math.floor(Math.random() * colors.length)];

      const payload = JSON.stringify({
        wallpaper,
        accent_color: color,
      });

      const start = Date.now();
      const res = http.put(`${baseUrl}/api/settings/appearance`, payload, { headers });

      const success = check(res, {
        'settings update - status 200': (r) => r.status === 200,
        'settings update - response time OK': (r) => r.timings.duration < 1000,
      });

      if (!success) {
        errorRate.add(1);
        failedRequests.add(1);
      } else {
        apiCallDuration.add(Date.now() - start);
      }

      sleep(0.5);
    });
  }

  // Group 4: Mixed workload simulation
  group('Mixed Workload', function () {
    const scenarios = [
      () => http.get(`${baseUrl}/api/auth/me`, { headers }),
      () => http.get(`${baseUrl}/api/health`, { headers }),
      () => http.get(`${baseUrl}/api/settings/appearance`, { headers }),
    ];

    // Random scenario
    const scenario = scenarios[Math.floor(Math.random() * scenarios.length)];
    const start = Date.now();
    const res = scenario();

    const success = check(res, {
      'mixed workload - status OK': (r) => r.status >= 200 && r.status < 400,
    });

    if (!success) {
      errorRate.add(1);
      failedRequests.add(1);
    } else {
      apiCallDuration.add(Date.now() - start);
    }

    sleep(1);
  });

  // Group 5: Logout (cleanup)
  if (Math.random() < 0.1) { // 10% logout
    group('Logout', function () {
      const res = http.post(`${baseUrl}/api/auth/logout`, null, { headers });

      check(res, {
        'logout successful': (r) => r.status === 200,
      });
    });
  }

  sleep(1);
}

export function handleSummary(data) {
  return {
    'summary.html': htmlReport(data),
    stdout: textSummary(data, { indent: ' ', enableColors: true }),
    'summary.json': JSON.stringify(data),
  };
}

export function teardown(data) {
  console.log('Load test completed');
  console.log(`Test ran against: ${data.baseUrl}`);
}
