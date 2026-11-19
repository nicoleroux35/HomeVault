import http from 'k6/http';
import { check, sleep } from 'k6';

// Stress test: gradually increase load beyond capacity
export const options = {
  stages: [
    { duration: '1m', target: 50 },
    { duration: '2m', target: 100 },
    { duration: '2m', target: 200 },
    { duration: '2m', target: 300 },
    { duration: '2m', target: 400 },
    { duration: '1m', target: 0 },
  ],
  thresholds: {
    http_req_duration: ['p(95)<5000'], // During stress, allow 5s
  },
};

const BASE_URL = __ENV.BASE_URL || 'http://localhost:8080';

export default function () {
  const res = http.get(`${BASE_URL}/api/health`);

  check(res, {
    'status is 200 or 503': (r) => r.status === 200 || r.status === 503,
    'response received': (r) => r.body.length > 0,
  });

  sleep(1);
}
