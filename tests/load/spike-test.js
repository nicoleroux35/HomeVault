import http from 'k6/http';
import { check, sleep } from 'k6';
import { Rate } from 'k6/metrics';

const errorRate = new Rate('errors');

// Spike test: sudden burst of traffic
export const options = {
  stages: [
    { duration: '10s', target: 5 },   // Normal load
    { duration: '10s', target: 500 }, // Spike!
    { duration: '10s', target: 5 },   // Recovery
  ],
  thresholds: {
    http_req_failed: ['rate<0.15'], // Allow 15% errors during spike
    errors: ['rate<0.20'],
  },
};

const BASE_URL = __ENV.BASE_URL || 'http://localhost:8080';

export default function () {
  const res = http.get(`${BASE_URL}/api/health`);

  const success = check(res, {
    'status is 200': (r) => r.status === 200,
  });

  if (!success) {
    errorRate.add(1);
  }

  sleep(0.1);
}
