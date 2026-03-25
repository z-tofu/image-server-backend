use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};

pub async fn init_db() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect("sqlite:data.db?mode=rwc")
        .await
        .expect("Failed to create database connection pool");

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT UNIQUE NOT NULL,
            password_hash TEXT NOT NULL,
            token_hash TEXT UNIQUE NOT NULL,
            is_approved INTEGER DEFAULT 0,
            is_admin INTEGER DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS images (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL,
            file_path TEXT NOT NULL,
            FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS favorites (
            user_id INTEGER NOT NULL,
            image_id INTEGER NOT NULL,
            PRIMARY KEY (user_id, image_id),
            FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
            FOREIGN KEY (image_id) REFERENCES images(id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS tags (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT UNIQUE NOT NULL
        );
        CREATE TABLE IF NOT EXISTS image_tags (
            image_id INTEGER NOT NULL,
            tag_id INTEGER NOT NULL,
            PRIMARY KEY (image_id, tag_id),
            FOREIGN KEY (image_id) REFERENCES images(id) ON DELETE CASCADE,
            FOREIGN KEY (tag_id) REFERENCES tags(id) ON DELETE CASCADE
        );
        "#,
    )
        .execute(&pool)
        .await
        .expect("Failed to create tables");

    pool
}