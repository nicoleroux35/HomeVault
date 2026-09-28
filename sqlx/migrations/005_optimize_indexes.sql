-- Migration: 005_optimize_indexes.sql
-- Optimisation des index pour améliorer les performances des requêtes

-- Index pour les requêtes de recherche par email
CREATE INDEX IF NOT EXISTS idx_users_email ON users(email);

-- Index pour les requêtes de recherche par role
CREATE INDEX IF NOT EXISTS idx_users_role_id ON users(role_id);

-- Index pour les widgets triés par position
CREATE INDEX IF NOT EXISTS idx_widgets_position ON widgets(row_pos, col_pos);

-- Index pour user_2fa enabled
CREATE INDEX IF NOT EXISTS idx_user_2fa_enabled ON user_2fa(enabled);

-- NOTE: indexes on share_permissions, hv_quotas and hv_groups live in
-- migration 012_deferred_permission_indexes.sql — those tables are only
-- created by migration 008, which runs after this one.
