-- Migration: 054_scheduled_task_timezone.sql
-- ROADMAP 12.2 — a cron expression is only meaningful together with the
-- timezone it is evaluated in.
--
-- `timezone` holds an IANA name (`Europe/Paris`). NULL means "follow the
-- appliance timezone" (Settings → General, applied to the host by
-- `timedatectl`), which is also what every existing row gets: before this
-- migration the engine evaluated everything in UTC, and the appliance default
-- is UTC, so no existing schedule changes behaviour without an explicit edit.
--
-- Validated on write (`scheduled_tasks::validate_request`) and again at run
-- time, so a hand-written row cannot make the scheduler panic.

ALTER TABLE scheduled_tasks ADD COLUMN timezone TEXT;
