#!/usr/bin/env bash
set -e

DB_NAME="chessdb"
DB_USER="chess1"
DB_PASSWORD="Af84aammZxrdafcgexzvufghtelr"
DB_HOST="localhost"

export PGPASSWORD="$DB_PASSWORD"

psql -h "$DB_HOST" -U "$DB_USER" -d "$DB_NAME" <<EOF
-- Drop existing tables if they exist
DROP TABLE IF EXISTS article_media;
DROP TABLE IF EXISTS comments;
DROP TABLE IF EXISTS articles;
DROP TABLE IF EXISTS admins;

-- Create articles table
CREATE TABLE articles (
    id SERIAL PRIMARY KEY,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    bump_time BIGINT NOT NULL
);

-- Create table for associated media
CREATE TABLE article_media (
    id SERIAL PRIMARY KEY,
    article_id INT NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    media_path TEXT NOT NULL
);

-- Create table for comments
CREATE TABLE comments (
    id SERIAL PRIMARY KEY,
    article_id INT NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    comment TEXT NOT NULL
);

-- Create admins table
CREATE TABLE admins (
    username TEXT PRIMARY KEY,
    password_hash TEXT NOT NULL
);

-- Insert a sample admin
INSERT INTO admins (username, password_hash) VALUES ('admin', 'plaintextpassword');

-- Optionally insert a sample article and data
INSERT INTO articles (title, body, bump_time) VALUES ('Sample Article', 'This is a test article body.', EXTRACT(EPOCH FROM now())::BIGINT);
INSERT INTO article_media (article_id, media_path)
    SELECT id, '/uploads/sample_image.jpg' FROM articles WHERE title='Sample Article';
INSERT INTO comments (article_id, comment)
    SELECT id, 'This is a sample comment.' FROM articles WHERE title='Sample Article';
EOF

echo "Database tables created and sample data inserted."
