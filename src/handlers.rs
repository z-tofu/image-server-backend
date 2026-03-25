use crate::{AppState, auth::hash_token};
use axum::{
    extract::{Multipart, Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use serde::Serialize;
use sqlx::Row;
use tokio::fs;
use uuid::Uuid;

async fn authenticate_user(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<sqlx::sqlite::SqliteRow, (StatusCode, String)> {
    let auth_token = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or((StatusCode::UNAUTHORIZED, "Missing Token".to_string()))?;

    let token_hash = hash_token(auth_token);

    let row = sqlx::query("SELECT id, is_approved, is_admin FROM users WHERE token_hash = ?")
        .bind(token_hash)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into()))?
        .ok_or((StatusCode::UNAUTHORIZED, "Invalid Token".to_string()))?;

    Ok(row)
}

pub async fn upload_image(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<String, (StatusCode, String)> {
    let user_row = authenticate_user(&state, &headers).await?;
    let user_id: i64 = user_row.get("id");

    if user_row.get::<i64, _>("is_approved") == 0 {
        return Err((StatusCode::FORBIDDEN, "Account pending Admin approval".into()));
    }

    while let Some(field) = multipart.next_field().await.map_err(|_| (StatusCode::BAD_REQUEST, "Malformed multipart".into()))? {
        let data = field.bytes().await.map_err(|_| (StatusCode::BAD_REQUEST, "Failed to read data".into()))?;


        let kind = infer::get(&data).ok_or((StatusCode::UNSUPPORTED_MEDIA_TYPE, "Unknown file type".into()))?;

        if !kind.mime_type().starts_with("image/") {
            return Err((StatusCode::BAD_REQUEST, "Only valid image files are allowed".into()));
        }

        let ext = kind.extension();
        let filename = format!("{}.{}", Uuid::new_v4(), ext);
        let path = format!("uploads/{}", filename);

        fs::write(&path, &data).await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Failed to save file".into()))?;

        let db_path = format!("/images/{}", filename);
        let inserted_id = sqlx::query("INSERT INTO images (user_id, file_path) VALUES (?, ?)")
            .bind(user_id)
            .bind(&db_path)
            .execute(&state.db)
            .await
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into()))?
            .last_insert_rowid();

        let db_clone = state.db.clone();
        let path_clone = path.clone();
        let client_clone = state.http_client.clone();
        tokio::spawn(async move {
            if let Err(e) = auto_tag_image(inserted_id, path_clone, db_clone, client_clone).await {
                eprintln!("Auto-tagging failed for image {}: {}", inserted_id, e);
            }
        });

        return Ok(db_path);
    }

    Err((StatusCode::BAD_REQUEST, "No file found in request".into()))
}

#[derive(Serialize)]
pub struct ImageRecord {
    pub id: i64,
    pub file_path: String,
    pub is_favorited: bool,
    pub tags: Vec<String>,
}

#[derive(Deserialize)]
pub struct SearchParams {
    pub tag: Option<String>,
}

#[derive(Deserialize)]
pub struct TagPayload {
    pub name: String,
}


pub fn parse_image_records(rows: Vec<sqlx::sqlite::SqliteRow>) -> Vec<ImageRecord> {
    rows.into_iter()
        .map(|r| {
            let tags_str: String = r.get("tags");
            let tags = if tags_str.is_empty() {
                vec![]
            } else {
                tags_str.split(',').map(String::from).collect()
            };

            ImageRecord {
                id: r.get("id"),
                file_path: r.get("file_path"),
                is_favorited: r.try_get::<bool, _>("is_favorited")
                    .unwrap_or_else(|_| r.try_get::<i64, _>("is_favorited").unwrap_or(0) != 0),
                tags,
            }
        })
        .collect()
}

pub async fn list_user_images(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<ImageRecord>>, (StatusCode, String)> {
    let user_row = authenticate_user(&state, &headers).await?;
    let user_id: i64 = user_row.get("id");

    if user_row.get::<i64, _>("is_approved") == 0 {
        return Err((StatusCode::FORBIDDEN, "Account pending approval".into()));
    }

    let tag_filter = params.tag.unwrap_or_default();

    let rows = sqlx::query(
        "SELECT i.id, i.file_path, (f.image_id IS NOT NULL) AS is_favorited, \
         IFNULL(GROUP_CONCAT(t.name), '') AS tags \
         FROM images i \
         LEFT JOIN favorites f ON i.id = f.image_id AND f.user_id = ? \
         LEFT JOIN image_tags it ON i.id = it.image_id \
         LEFT JOIN tags t ON it.tag_id = t.id \
         WHERE i.user_id = ? AND (? = '' OR i.id IN (SELECT image_id FROM image_tags JOIN tags ON image_tags.tag_id = tags.id WHERE tags.name = ?)) \
         GROUP BY i.id \
         ORDER BY i.id DESC"
    )
        .bind(user_id)
        .bind(user_id)
        .bind(&tag_filter)
        .bind(&tag_filter)
        .fetch_all(&state.db)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into()))?;

    Ok(Json(parse_image_records(rows)))
}

pub async fn delete_image(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(image_id): Path<i64>,
) -> Result<String, (StatusCode, String)> {
    let user_row = authenticate_user(&state, &headers).await?;
    let user_id: i64 = user_row.get("id");

    let file_path: String = sqlx::query_scalar("SELECT file_path FROM images WHERE id = ? AND user_id = ?")
        .bind(image_id)
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into()))?
        .ok_or((StatusCode::NOT_FOUND, "Image not found or access denied".to_string()))?;

    let disk_path = format!("uploads/{}", file_path.trim_start_matches("/images/"));

    if let Err(e) = fs::remove_file(&disk_path).await {
        eprintln!("Warning: Failed to delete file from disk {}: {}", disk_path, e);
    }

    sqlx::query("DELETE FROM images WHERE id = ?")
        .bind(image_id)
        .execute(&state.db)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into()))?;

    Ok("Image deleted".into())
}

#[derive(Serialize)]
pub struct PendingUser {
    pub id: i64,
    pub username: String,
}

pub async fn list_pending_users(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<PendingUser>>, (StatusCode, String)> {
    let user_row = authenticate_user(&state, &headers).await?;

    if user_row.get::<i64, _>("is_admin") == 0 {
        return Err((StatusCode::FORBIDDEN, "Admins only".into()));
    }

    let rows = sqlx::query("SELECT id, username FROM users WHERE is_approved = 0")
        .fetch_all(&state.db)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into()))?;

    let pending = rows
        .into_iter()
        .map(|row| PendingUser {
            id: row.get("id"),
            username: row.get("username"),
        })
        .collect();

    Ok(Json(pending))
}

pub async fn approve_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(target_user_id): Path<i64>,
) -> Result<String, (StatusCode, String)> {
    let user_row = authenticate_user(&state, &headers).await?;

    if user_row.get::<i64, _>("is_admin") == 0 {
        return Err((StatusCode::FORBIDDEN, "Admins only".into()));
    }

    sqlx::query("UPDATE users SET is_approved = 1 WHERE id = ?")
        .bind(target_user_id)
        .execute(&state.db)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into()))?;

    Ok("User approved".into())
}

#[derive(Deserialize)]
pub struct DiscordWebhookPayload {
    pub channel_id: String,
    pub image_path: String,
}

pub async fn post_image_to_discord(client: &reqwest::Client, channel_id: &str, image_path: &str) -> Result<(), (StatusCode, String)> {
    let bot_token = std::env::var("BOT_TOKEN")
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Server configuration error".into()))?;

    let base_url = std::env::var("BASE_URL")
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Server configuration error".into()))?;

    let url = format!("https://discord.com/api/v10/channels/{}/messages", channel_id);
    let full_image_url = format!("{}{}",base_url, image_path);

    let res = client
        .post(&url)
        .header("Authorization", format!("Bot {}", bot_token))
        .json(&serde_json::json!({
            "content": full_image_url
        }))
        .send()
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Network error reaching Discord".into()))?;

    if res.status().is_success() {
        Ok(())
    } else {
        Err((StatusCode::INTERNAL_SERVER_ERROR, "Discord rejected the message".into()))
    }
}

pub async fn send_to_discord(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<DiscordWebhookPayload>,
) -> Result<StatusCode, (StatusCode, String)> {
    let user_row = authenticate_user(&state, &headers).await?;

    if user_row.get::<i64, _>("is_approved") == 0 {
        return Err((StatusCode::FORBIDDEN, "Account pending approval".into()));
    }

    post_image_to_discord(&state.http_client, &payload.channel_id, &payload.image_path).await?;

    Ok(StatusCode::OK)
}

pub async fn logout_user(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<String, (StatusCode, String)> {
    let user_row = authenticate_user(&state, &headers).await?;
    let user_id: i64 = user_row.get("id");

    sqlx::query("UPDATE users SET token_hash = '' WHERE id = ?")
        .bind(user_id)
        .execute(&state.db)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into()))?;

    Ok("Logged out".into())
}

#[derive(Deserialize)]
pub struct SendImageParams {
    pub channel: String,
    pub path: String,
}


pub async fn send_image(
    State(state): State<AppState>,
    Query(params): Query<SendImageParams>,
) -> Result<StatusCode, (StatusCode, String)> {
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM images WHERE file_path = ?")
        .bind(&params.path)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into()))?;

    if exists.is_none() {
        return Err((StatusCode::NOT_FOUND, "Image not found".into()));
    }
    post_image_to_discord(&state.http_client, &params.channel, &params.path).await?;

    Ok(StatusCode::OK)
}

pub async fn list_all_images(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<ImageRecord>>, (StatusCode, String)> {

    let mut user_id = None;
    if let Ok(user_row) = authenticate_user(&state, &headers).await {
        user_id = Some(user_row.get::<i64, _>("id"));
    }

    let tag_filter = params.tag.unwrap_or_default();

    let rows = match user_id {
        Some(uid) => {
            sqlx::query(
                "SELECT i.id, i.file_path, (f.image_id IS NOT NULL) AS is_favorited, \
                 IFNULL(GROUP_CONCAT(t.name), '') AS tags \
                 FROM images i \
                 LEFT JOIN favorites f ON i.id = f.image_id AND f.user_id = ? \
                 LEFT JOIN image_tags it ON i.id = it.image_id \
                 LEFT JOIN tags t ON it.tag_id = t.id \
                 WHERE (? = '' OR i.id IN (SELECT image_id FROM image_tags JOIN tags ON image_tags.tag_id = tags.id WHERE tags.name = ?)) \
                 GROUP BY i.id \
                 ORDER BY i.id DESC"
            )
            .bind(uid)
            .bind(&tag_filter)
            .bind(&tag_filter)
            .fetch_all(&state.db)
            .await
        },
        None => {
            sqlx::query(
                "SELECT i.id, i.file_path, 0 AS is_favorited, \
                 IFNULL(GROUP_CONCAT(t.name), '') AS tags \
                 FROM images i \
                 LEFT JOIN image_tags it ON i.id = it.image_id \
                 LEFT JOIN tags t ON it.tag_id = t.id \
                 WHERE (? = '' OR i.id IN (SELECT image_id FROM image_tags JOIN tags ON image_tags.tag_id = tags.id WHERE tags.name = ?)) \
                 GROUP BY i.id \
                 ORDER BY i.id DESC"
            )
            .bind(&tag_filter)
            .bind(&tag_filter)
            .fetch_all(&state.db)
            .await
        }
    }.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into()))?;

    Ok(Json(parse_image_records(rows)))
}

pub async fn toggle_favorite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(image_id): Path<i64>,
) -> Result<String, (StatusCode, String)> {
    let user_row = authenticate_user(&state, &headers).await?;
    let user_id: i64 = user_row.get("id");

    let result = sqlx::query("DELETE FROM favorites WHERE user_id = ? AND image_id = ?")
        .bind(user_id)
        .bind(image_id)
        .execute(&state.db)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into()))?;

    if result.rows_affected() > 0 {
        Ok("Removed from favorites".into())
    } else {
        sqlx::query("INSERT INTO favorites (user_id, image_id) VALUES (?, ?)")
            .bind(user_id)
            .bind(image_id)
            .execute(&state.db)
            .await
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into()))?;
        Ok("Added to favorites".into())
    }
}

pub async fn list_favorite_images(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ImageRecord>>, (StatusCode, String)> {
    let user_row = authenticate_user(&state, &headers).await?;
    let user_id: i64 = user_row.get("id");

    let rows = sqlx::query(
        "SELECT i.id, i.file_path, 1 AS is_favorited, IFNULL(GROUP_CONCAT(t.name), '') AS tags \
         FROM images i \
         JOIN favorites f ON i.id = f.image_id \
         LEFT JOIN image_tags it ON i.id = it.image_id \
         LEFT JOIN tags t ON it.tag_id = t.id \
         WHERE f.user_id = ? \
         GROUP BY i.id \
         ORDER BY i.id DESC"
    )
        .bind(user_id)
        .fetch_all(&state.db)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into()))?;

    Ok(Json(parse_image_records(rows)))
}

pub async fn list_tags(
    State(state): State<AppState>,
) -> Result<Json<Vec<String>>, (StatusCode, String)> {
    let rows = sqlx::query("SELECT name FROM tags ORDER BY name ASC")
        .fetch_all(&state.db)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into()))?;

    let tags = rows.into_iter().map(|r| r.get("name")).collect();
    Ok(Json(tags))
}

pub async fn add_tag(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(image_id): Path<i64>,
    Json(payload): Json<TagPayload>,
) -> Result<String, (StatusCode, String)> {
    let user_row = authenticate_user(&state, &headers).await?;
    let user_id: i64 = user_row.get("id");

    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM images WHERE id = ? AND user_id = ?")
        .bind(image_id).bind(user_id).fetch_optional(&state.db).await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into()))?;
    if exists.is_none() && user_row.get::<i64, _>("is_admin") == 0 {
        return Err((StatusCode::FORBIDDEN, "Not your image".into()));
    }

    let tag_name = payload.name.trim().to_lowercase();
    if tag_name.is_empty() || tag_name.contains(' ') { return Err((StatusCode::BAD_REQUEST, "Invalid tag".into())); }

    sqlx::query("INSERT OR IGNORE INTO tags (name) VALUES (?)")
        .bind(&tag_name).execute(&state.db).await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into()))?;

    let tag_id: i64 = sqlx::query_scalar("SELECT id FROM tags WHERE name = ?")
        .bind(&tag_name).fetch_one(&state.db).await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into()))?;

    sqlx::query("INSERT OR IGNORE INTO image_tags (image_id, tag_id) VALUES (?, ?)")
        .bind(image_id).bind(tag_id).execute(&state.db).await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into()))?;

    Ok("Tag added".into())
}

pub async fn remove_tag(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((image_id, tag_name)): Path<(i64, String)>,
) -> Result<String, (StatusCode, String)> {
    let user_row = authenticate_user(&state, &headers).await?;
    let user_id: i64 = user_row.get("id");

    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM images WHERE id = ? AND user_id = ?")
        .bind(image_id).bind(user_id).fetch_optional(&state.db).await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into()))?;
    if exists.is_none() && user_row.get::<i64, _>("is_admin") == 0 {
        return Err((StatusCode::FORBIDDEN, "Not your image".into()));
    }

    sqlx::query("DELETE FROM image_tags WHERE image_id = ? AND tag_id = (SELECT id FROM tags WHERE name = ?)")
        .bind(image_id).bind(&tag_name).execute(&state.db).await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into()))?;

    Ok("Tag removed".into())
}

use base64::{engine::general_purpose, Engine as _};

async fn auto_tag_image(image_id: i64, file_path: String, db: sqlx::SqlitePool, client: reqwest::Client) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let bytes = tokio::fs::read(&file_path).await?;
    let base64_image = general_purpose::STANDARD.encode(&bytes);

    let payload = serde_json::json!({
        "model": "llava",
        "prompt": "Analyze this image and return a comma-separated list of 3-5 very short, single-word descriptive tags. No uppercase letters. For example: cat, animal, pet, cute",
        "stream": false,
        "images": [base64_image]
    });

    let res = client.post("http:127.0.0.1:11434/api/generate")
        .json(&payload)
        .send()
        .await?;

    if !res.status().is_success() {
        let status = res.status();
        let error_body = res.text().await.unwrap_or_else(|_| "Unknown error body".to_string());
        return Err(format!("Ollama API error {}: {}", status, error_body).into());
    }

    let json: serde_json::Value = res.json().await?;
    let response_text = json["response"].as_str().unwrap_or("");
    
    let tags: Vec<&str> = response_text.split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && !s.contains(" ") && s.len() < 20)
        .take(5)
        .collect();

    for tag in tags {
        let tag_name = tag.to_lowercase();
        sqlx::query("INSERT OR IGNORE INTO tags (name) VALUES (?)")
            .bind(&tag_name).execute(&db).await?;
            
        let tag_id: i64 = sqlx::query_scalar("SELECT id FROM tags WHERE name = ?")
            .bind(&tag_name).fetch_one(&db).await?;
            
        sqlx::query("INSERT OR IGNORE INTO image_tags (image_id, tag_id) VALUES (?, ?)")
            .bind(image_id).bind(tag_id).execute(&db).await?;
    }

    Ok(())
}