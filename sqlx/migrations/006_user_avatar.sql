-- Migration: 006_user_avatar.sql
-- Add avatar_seed column to users table for DiceBear Miniavs avatar generation

ALTER TABLE users ADD COLUMN avatar_seed TEXT NOT NULL DEFAULT '';