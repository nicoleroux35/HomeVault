-- Migration: 012_deferred_permission_indexes.sql
-- Index laisses de cote par la migration 005 (qui s'executait avant la
-- migration 008, laquelle cree les tables share_permissions, hv_quotas et
-- hv_groups). Les index deja crees par la migration 008
-- (idx_quota_group, idx_share_perm_lookup, idx_share_perm_grantee,
-- idx_hv_groups_gid) ne sont pas dupliques ici.

-- Index pour les quotas depassant les limites (pour monitoring)
CREATE INDEX IF NOT EXISTS idx_quotas_over_soft ON hv_quotas(soft_limit, current_usage);
CREATE INDEX IF NOT EXISTS idx_quotas_over_hard ON hv_quotas(hard_limit, current_usage);

-- Index pour hv_groups par statut systeme
CREATE INDEX IF NOT EXISTS idx_hv_groups_system ON hv_groups(is_system);
