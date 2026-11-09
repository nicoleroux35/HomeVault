/* =============================================================================
 * File          : downloader.ts
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Types of the Downloader's RSS subscriptions and automatic extraction:
 *   `/api/downloader/feeds*`, `/api/downloader/settings`, `/api/downloader/extract`.
 *
 * Dependencies:
 *   - none
 *
 * Usage:
 *   Imported by $lib/services/api and the Downloader route.
 * =============================================================================
 */

/** One subscribed RSS/Atom feed, as stored on the server. */
export interface DownloaderRssFeed {
  id: string;
  name: string;
  url: string;
  enabled: boolean;
  /** Queue new items automatically when the feed is refreshed. */
  auto_download: boolean;
  last_checked_at: string | null;
  /** The last fetch failure, so the row can explain itself. */
  last_error: string | null;
  /** Identities already queued (the server caps this list). */
  seen: string[];
}

/** One download candidate found in a feed. */
export interface DownloaderRssItem {
  title: string;
  link: string;
  /** What the Downloader would fetch (enclosure, else the item link). */
  download_url: string;
  published: string | null;
  size: number | null;
  guid: string;
}

/** An item plus whether it has already been queued for this feed. */
export interface DownloaderFeedItem extends DownloaderRssItem {
  seen: boolean;
}

/** `POST /api/downloader/feeds/{id}/refresh`. */
export interface DownloaderFeedRefresh {
  feed_id: string;
  items: DownloaderFeedItem[];
  /** How many new items were sent to aria2 (0 when only listing). */
  queued: number;
  /** Per-item queueing failures, named by the item's label. */
  errors: string[];
}
