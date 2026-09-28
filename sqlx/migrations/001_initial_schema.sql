-- Migration: 001_initial_schema.sql
-- Create users table with role_id (updated for new schema)
CREATE TABLE IF NOT EXISTS users (
    username TEXT PRIMARY KEY,
    password_hash TEXT NOT NULL,
    fullname TEXT NOT NULL,
    email TEXT NOT NULL,
    role_id INTEGER NOT NULL DEFAULT 1,
    locked INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    FOREIGN KEY (role_id) REFERENCES roles(id),
    CHECK (role_id IN (1, 2))
);

-- Create roles table first
CREATE TABLE IF NOT EXISTS roles (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    CHECK (id IN (1, 2))
);

-- Insert default roles
INSERT OR IGNORE INTO roles (id, name) VALUES (1, 'user'), (2, 'admin');

-- Create sessions table
CREATE TABLE IF NOT EXISTS sessions (
    token TEXT PRIMARY KEY,
    username TEXT NOT NULL,
    ip_address TEXT NOT NULL,
    csrf_token TEXT NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    is_admin INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (username) REFERENCES users(username) ON DELETE CASCADE
);

-- Create widgets table
CREATE TABLE IF NOT EXISTS widgets (
    id TEXT PRIMARY KEY,
    wtype TEXT NOT NULL,
    title TEXT NOT NULL,
    size TEXT NOT NULL,
    row_pos INTEGER NOT NULL DEFAULT 1,
    col_pos INTEGER NOT NULL DEFAULT 1,
    row_span INTEGER NOT NULL DEFAULT 1,
    col_span INTEGER NOT NULL DEFAULT 1,
    username TEXT,
    FOREIGN KEY (username) REFERENCES users(username) ON DELETE SET NULL
);

-- Create indexes
CREATE INDEX IF NOT EXISTS idx_sessions_username ON sessions(username);
CREATE INDEX IF NOT EXISTS idx_sessions_expires_at ON sessions(expires_at);
CREATE INDEX IF NOT EXISTS idx_widgets_username ON widgets(username);
