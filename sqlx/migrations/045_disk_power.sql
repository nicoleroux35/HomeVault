-- Migration: 045_disk_power.sql
-- Per-disk power settings (see docs/guides/power.md).
--
-- One row per disk the admin configured for spindown: hdparm APM level
-- (-B, 1..=255) and idle timeout in minutes (mapped to hdparm's coded -S
-- values by src/power.rs). Settings are re-applied at server startup so a
-- reboot restores the intended behaviour without a systemd unit of our own.
--
-- `device` is the bare kernel block-device name (sda, nvme0n1) — validated
-- by power::is_valid_block_device_name before it is ever spliced into a
-- /dev/ path for a sudo-wrapped hdparm call.

CREATE TABLE IF NOT EXISTS disk_power_settings (
    device            TEXT PRIMARY KEY,
    apm_level         INTEGER CHECK (apm_level IS NULL OR (apm_level BETWEEN 1 AND 255)),
    spindown_minutes  INTEGER CHECK (spindown_minutes IS NULL OR (spindown_minutes BETWEEN 0 AND 330)),
    updated_at        TEXT NOT NULL
);
