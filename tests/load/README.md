# Load Testing with k6

This directory contains distributed load tests for HomeVault using [k6](https://k6.io/).

## Prerequisites

```bash
# Install k6
# macOS
brew install k6

# Linux
sudo gpg -k
sudo gpg --no-default-keyring --keyring /usr/share/keyrings/k6-archive-keyring.gpg --keyserver hkp://keyserver.ubuntu.com:80 --recv-keys C5AD17C747E3415A3642D57D77C6C491D6AC1D69
echo "deb [signed-by=/usr/share/keyrings/k6-archive-keyring.gpg] https://dl.k6.io/deb stable main" | sudo tee /etc/apt/sources.list.d/k6.list
sudo apt-get update
sudo apt-get install k6

# Windows
choco install k6
```

## Running Tests

### 1. Basic Load Test
```bash
# Run against local instance
k6 run tests/load/homevault-load-test.js

# Run against specific URL
k6 run --env BASE_URL=http://192.168.1.100:8080 tests/load/homevault-load-test.js

# With custom credentials
k6 run --env ADMIN_USER=admin --env ADMIN_PASS=MyPass123 tests/load/homevault-load-test.js
```

### 2. Spike Test (sudden traffic burst)
```bash
k6 run tests/load/spike-test.js
```

### 3. Stress Test (find breaking point)
```bash
k6 run tests/load/stress-test.js
```

### 4. Distributed Cloud Testing (k6 Cloud)
```bash
# Requires k6 cloud account
k6 cloud tests/load/homevault-load-test.js
```

## Test Scenarios

### Load Test (`homevault-load-test.js`)
- **Duration**: 5 minutes
- **Users**: Ramps from 0 → 100 → 0
- **Scenarios**:
  - Authentication (login/logout)
  - Read operations (GET endpoints)
  - Write operations (PUT settings)
  - Mixed workload
- **Thresholds**:
  - 95% of requests < 500ms
  - 99% of requests < 1s
  - Error rate < 5%

### Spike Test (`spike-test.js`)
- **Duration**: 30 seconds
- **Pattern**: 5 → 500 → 5 users
- **Purpose**: Test system behavior under sudden traffic spikes
- **Acceptable**: 15% error rate during spike

### Stress Test (`stress-test.js`)
- **Duration**: 10 minutes
- **Users**: Gradually increases to 400
- **Purpose**: Find the breaking point
- **Expected**: System degrades gracefully (503 instead of crash)

## Interpreting Results

### Key Metrics
- `http_req_duration`: Response time (p95, p99)
- `http_req_failed`: Failed request rate
- `iterations`: Completed test iterations
- `vus`: Virtual users (concurrent)

### Good Results
```
✓ http_req_duration..............: avg=250ms p95=450ms p99=800ms
✓ http_req_failed................: 2.5%
✓ iterations......................: 15000
```

### Warning Signs
```
✗ http_req_duration..............: avg=1.2s p95=3s p99=8s
✗ http_req_failed................: 15%
✗ errors.........................: 20%
```

## CI Integration

Add to `.github/workflows/load-test.yml`:
```yaml
- name: Run load test
  run: |
    k6 run --out json=results.json tests/load/homevault-load-test.js
    
- name: Check thresholds
  run: |
    if grep -q "threshold.*failed" results.json; then
      echo "Load test thresholds failed"
      exit 1
    fi
```

## Output Files

After running tests:
- `summary.html` - Visual HTML report
- `summary.json` - JSON results for parsing
- Console output - Real-time metrics

## Custom Scenarios

Create new test files for specific scenarios:
```javascript
export const options = {
  scenarios: {
    dashboard_heavy: {
      executor: 'constant-vus',
      vus: 50,
      duration: '2m',
      exec: 'dashboardScenario',
    },
    api_spike: {
      executor: 'ramping-arrival-rate',
      startRate: 10,
      timeUnit: '1s',
      preAllocatedVUs: 100,
      stages: [
        { target: 200, duration: '30s' },
      ],
    },
  },
};
```

## Troubleshooting

### High error rate
- Check server logs
- Verify database connections
- Check system resources (CPU, RAM, disk I/O)

### Slow response times
- Enable database query logging
- Check for N+1 queries
- Review middleware performance

### Connection errors
- Increase file descriptors: `ulimit -n 65536`
- Check firewall rules
- Verify network bandwidth
