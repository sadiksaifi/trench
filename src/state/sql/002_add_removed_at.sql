-- Migration 002: Add removed_at column to worktrees table.
-- Tracks when a worktree was removed (soft-delete for audit trail).

ALTER TABLE worktrees ADD COLUMN removed_at INTEGER;
