/**
 * HomeVault Service Worker
 * Enables offline functionality and PWA support
 */

const CACHE_NAME = 'homevault-v1';
const STATIC_CACHE = 'homevault-static-v1';
const DYNAMIC_CACHE = 'homevault-dynamic-v1';

// Assets to cache immediately on install
const PRECACHE_ASSETS = [
  '/',
  '/index.html',
  '/manifest.json',
  '/favicon.svg'
];

// Install event - precache static assets
self.addEventListener('install', (event) => {
  event.waitUntil(
    caches.open(STATIC_CACHE).then((cache) => {
      return cache.addAll(PRECACHE_ASSETS);
    })
  );
  // Activate immediately
  self.skipWaiting();
});

// Activate event - clean up old caches
self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches.keys().then((cacheNames) => {
      return Promise.all(
        cacheNames
          .filter((name) => name !== STATIC_CACHE && name !== DYNAMIC_CACHE)
          .map((name) => caches.delete(name))
      );
    })
  );
  // Take control of all clients immediately
  self.clients.claim();
});

// Fetch event - serve from cache, fallback to network
self.addEventListener('fetch', (event) => {
  const { request } = event;
  const url = new URL(request.url);

  // Skip non-GET requests
  if (request.method !== 'GET') return;

  // Skip external requests (API calls to backend)
  if (url.origin !== location.origin) return;

  // Skip API requests to backend
  if (url.pathname.startsWith('/api')) return;

  // Skip WebSocket connections
  if (request.url.includes('/ws')) return;

  event.respondWith(
    caches.match(request).then((cachedResponse) => {
      if (cachedResponse) {
        // Return cached version and update cache in background
        event.waitUntil(
          fetch(request).then((networkResponse) => {
            if (networkResponse.ok) {
              caches.open(DYNAMIC_CACHE).then((cache) => {
                cache.put(request, networkResponse.clone());
              });
            }
          }).catch(() => {
            // Network failed, but we have cache
          })
        );
        return cachedResponse;
      }

      // Not in cache, try network
      return fetch(request).then((networkResponse) => {
        // Cache successful responses
        if (networkResponse.ok && !request.url.includes('chrome-extension')) {
          const responseToCache = networkResponse.clone();
          caches.open(DYNAMIC_CACHE).then((cache) => {
            cache.put(request, responseToCache);
          });
        }
        return networkResponse;
      }).catch(() => {
        // Offline fallback for HTML requests
        if (request.headers.get('accept')?.includes('text/html')) {
          return caches.match('/index.html');
        }
        // Return offline indicator for other requests
        return new Response('Offline', { status: 503 });
      });
    })
  );
});

// Handle push notifications (future)
self.addEventListener('push', (event) => {
  if (event.data) {
    const data = event.data.json();
    const options = {
      body: data.body || 'New notification',
      icon: '/favicon.svg',
      badge: '/favicon.svg',
      vibrate: [100, 50, 100],
      data: data.data || {}
    };
    event.waitUntil(
      self.registration.showNotification(data.title || 'HomeVault', options)
    );
  }
});

// Handle notification clicks
self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  event.waitUntil(
    clients.openWindow(event.notification.data?.url || '/')
  );
});
