-- Migration: 047_user_lockout_expiry.sql
-- Temporary vs. permanent account locks.
--
-- `users.locked` used to be a single boolean set by the failed-login counter and
-- never cleared, so five wrong passwords bricked the account for good (there is
-- no support desk on a NAS, and the only admin may be the locked account).
--
-- `locked_until` now carries the expiry of an automatic lock:
--   * NULL + locked = 1  -> a deliberate, permanent lock (admin action, via
--                           PUT /users/{username}/lock); only an admin lifts it.
--   * set  + locked = 1  -> an automatic lock; it lifts itself once the
--                           timestamp has passed, and a successful login clears
--                           it too.
ALTER TABLE users ADD COLUMN locked_until TEXT;

-- Repair accounts bricked by the old counter. An admin-initiated lock leaves no
-- failed attempts behind, while a counter lock always has them — so only rows
-- with at least 5 recorded failures are treated as automatic and given an
-- expiry (5 minutes, the lockout window). Accounts an admin locked stay
-- permanent, as intended.
UPDATE users
   SET locked_until = strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+5 minutes')
 WHERE locked = 1
   AND username IN (
        SELECT username
          FROM login_attempts
         WHERE success = 0
      GROUP BY username
        HAVING COUNT(*) >= 5
   );
