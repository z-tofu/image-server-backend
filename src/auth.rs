use crate::AppState;
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::Row;
use tokio::task;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct RegisterResponse {
    pub token: String,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub is_admin: bool,
    pub is_approved: bool,
}

pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

pub async fn register_user(
    State(state): State<AppState>,
    Json(payload): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>, (StatusCode, String)> {

    let password_hash = task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(payload.password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
    })
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Task panicked".into()))?
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Hash failed".into()))?;

    let plaintext_token = Uuid::new_v4().to_string();
    let token_hash = hash_token(&plaintext_token);

    let mut tx = state.db.begin().await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "DB Error".into()))?;

    let user_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&mut *tx)
        .await
        .unwrap_or(0);

    let (is_admin, is_approved) = if user_count == 0 { (1, 1) } else { (0, 0) };

    let result = sqlx::query(
        "INSERT INTO users (username, password_hash, token_hash, is_approved, is_admin) VALUES (?, ?, ?, ?, ?)"
    )
        .bind(&payload.username)
        .bind(&password_hash)
        .bind(&token_hash)
        .bind(is_approved)
        .bind(is_admin)
        .execute(&mut *tx)
        .await;

    match result {
        Ok(_) => {
            tx.commit().await.map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "DB Error".into()))?;
            Ok(Json(RegisterResponse { token: plaintext_token }))
        },
        Err(_) => Err((StatusCode::BAD_REQUEST, "Username taken".into())),
    }
}

pub async fn login_user(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, (StatusCode, String)> {

    let user_record = sqlx::query("SELECT id, password_hash, is_admin, is_approved FROM users WHERE username = ?")
        .bind(&payload.username)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into()))?;

    let is_valid = match user_record.as_ref() {
        Some(row) => {
            let stored_hash: String = row.get("password_hash");
            let password_to_verify = payload.password.clone();

            task::spawn_blocking(move || {
                let parsed_hash = PasswordHash::new(&stored_hash).unwrap();
                Argon2::default().verify_password(password_to_verify.as_bytes(), &parsed_hash).is_ok()
            }).await.unwrap_or(false)
        }
        None => {
            let dummy_password = payload.password.clone();
            task::spawn_blocking(move || {
                let salt = SaltString::generate(&mut OsRng);
                let _ = Argon2::default().hash_password(dummy_password.as_bytes(), &salt);
            }).await.ok();
            false
        }
    };

    if !is_valid {
        return Err((StatusCode::UNAUTHORIZED, "Invalid username or password".into()));
    }

    let row = user_record.unwrap();
    let user_id: i64 = row.get("id");

    let new_plaintext_token = Uuid::new_v4().to_string();
    let new_token_hash = hash_token(&new_plaintext_token);

    sqlx::query("UPDATE users SET token_hash = ? WHERE id = ?")
        .bind(&new_token_hash)
        .bind(user_id)
        .execute(&state.db)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Failed to update token".into()))?;

    Ok(Json(LoginResponse {
        token: new_plaintext_token,
        is_admin: row.get::<i64, _>("is_admin") == 1,
        is_approved: row.get::<i64, _>("is_approved") == 1,
    }))
}