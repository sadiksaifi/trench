-- Migration 001: Initial schema
-- Creates all 6 core tables for trench state management.

CREATE TABLE repos (
    id          INTEGER PRIMARY KEY,
    name        TEXT    NOT NULL,
    path        TEXT    NOT NULL UNIQUE,
    default_base TEXT,
    created_at  INTEGER NOT NULL
);

CREATE TABLE worktrees (
    id            INTEGER PRIMARY KEY,
    repo_id       INTEGER NOT NULL REFERENCES repos(id),
    name          TEXT    NOT NULL,
    branch        TEXT    NOT NULL,
    path          TEXT    NOT NULL UNIQUE,
    base_branch   TEXT,
    managed       INTEGER NOT NULL DEFAULT 1,
    adopted_at    INTEGER,
    last_accessed INTEGER,
    created_at    INTEGER NOT NULL
);

CREATE TABLE events (
    id           INTEGER PRIMARY KEY,
    worktree_id  INTEGER REFERENCES worktrees(id),
    repo_id      INTEGER NOT NULL REFERENCES repos(id),
    event_type   TEXT    NOT NULL,
    payload      TEXT,
    created_at   INTEGER NOT NULL
);

CREATE TABLE logs (
    id          INTEGER PRIMARY KEY,
    event_id    INTEGER NOT NULL REFERENCES events(id),
    stream      TEXT    NOT NULL,
    line        TEXT    NOT NULL,
    line_number INTEGER NOT NULL,
    created_at  INTEGER NOT NULL
);

CREATE TABLE tags (
    id          INTEGER PRIMARY KEY,
    worktree_id INTEGER NOT NULL REFERENCES worktrees(id),
    name        TEXT    NOT NULL,
    created_at  INTEGER NOT NULL,
    UNIQUE(worktree_id, name)
);

CREATE TABLE session (
    key        TEXT    PRIMARY KEY,
    value      TEXT    NOT NULL,
    updated_at INTEGER NOT NULL
);
