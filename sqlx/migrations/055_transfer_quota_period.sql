-- Migration: 055_transfer_quota_period.sql
-- ROADMAP 23.2 — the transfer counters were plain lifetime accumulators, so
-- "100 GB/month" really meant "100 GB ever": once the cap was reached the
-- subject stayed blocked until an admin edited the row by hand.
--
-- `period` names the window the two counters are counted in:
--   'lifetime' (default, and what every existing row gets — no behaviour
--               change without an explicit edit), 'day', 'week' or 'month'.
-- `period_started_at` is the RFC3339 instant the current window opened. The
-- window is a *calendar* window in the appliance timezone (midnight, Monday
-- 00:00, the 1st at 00:00), so "per month" resets on the 1st and not 30 days
-- after the first transfer.
--
-- NULL `period_started_at` means "no window opened yet" (either a lifetime
-- row, or a row written before this migration). The enforcement path and the
-- reset loop both open it on first sight.
--
-- Validated on write (`transfer_quota::Period::parse`) and re-checked at run
-- time, so a hand-written row cannot make the reset loop do anything but
-- leave the row alone.

ALTER TABLE transfer_quotas ADD COLUMN period TEXT NOT NULL DEFAULT 'lifetime';
ALTER TABLE transfer_quotas ADD COLUMN period_started_at TEXT;
