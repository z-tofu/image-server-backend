# Image Hosting API (Rust Backend)

A lightweight image hosting backend built with Rust, [Axum](https://github.com/tokio-rs/axum), and SQLite. 

This document details the **Rust components** of the project, including the database schema, architecture overview, and API reference.

---

## Architecture Overview

| File | Purpose |
|------|---------|
| `src/main.rs` | Server setup, routing, CORS, and app state (`reqwest` client, `sqlx` pool) |
| `src/db.rs` | SQLite pool initialisation and schema migration |
| `src/auth.rs` | Registration, login handlers, and password hashing |
| `src/handlers.rs`| Image upload, automated AI tagging with Ollama, listing, deletion, tags, favorites, admin actions, and Discord webhooks |

---

## Database Schema

**`users`**
| Column | Type | Notes |
|--------|------|-------|
| `id` | INTEGER | Primary key |
| `username` | TEXT | Unique |
| `password_hash` | TEXT | Argon2 hash |
| `token_hash` | TEXT | SHA-256 hash of the bearer token |
| `is_approved` | INTEGER | `0` = pending, `1` = approved |
| `is_admin` | INTEGER | `0` = regular user, `1` = admin |

**`images`**
| Column | Type | Notes |
|--------|------|-------|
| `id` | INTEGER | Primary key |
| `user_id` | INTEGER | Foreign key → `users.id` |
| `file_path` | TEXT | Path in the form `/images/<uuid>.<ext>` |

**`favorites`**
| Column | Type | Notes |
|--------|------|-------|
| `user_id` | INTEGER | Foreign key → `users.id` (Composite PK) |
| `image_id` | INTEGER | Foreign key → `images.id` (Composite PK) |

**`tags`**
| Column | Type | Notes |
|--------|------|-------|
| `id` | INTEGER | Primary key |
| `name` | TEXT | Unique |

**`image_tags`**
| Column | Type | Notes |
|--------|------|-------|
| `image_id` | INTEGER | Foreign key → `images.id` (Composite PK) |
| `tag_id` | INTEGER | Foreign key → `tags.id` (Composite PK) |

---

## Authentication

Protected endpoints require a `Bearer` token in the `Authorization` header, obtained from `/register` or `/login`.

```
Authorization: Bearer <token>
```

Tokens are **rotated on every login/logout**. The server stores only a SHA-256 hash of the token. The **first registered user** is automatically granted admin and approval status. All subsequent users must be approved by an admin before they can upload images.

---

## API Reference

### Authentication Endpoints

#### POST /register
Create a new user account.
- **Request Body:** `{"username": "alice", "password": "xyz"}`
- **Success:** `200 OK` `{"token": "..."}`

#### POST /login
Authenticate and receive a fresh token. Invalidates previous token.
- **Request Body:** `{"username": "alice", "password": "xyz"}`
- **Success:** `200 OK` `{"token": "...", "is_admin": false, "is_approved": true}`

#### POST /logout
Clears the token for the authenticated user.
- **Headers:** `Authorization: Bearer <token>`
- **Success:** `200 OK` (plain text)

---

### Image Management

#### POST /upload
Upload an image. On upload, a background task automatically connects to a local Ollama instance (`http://127.0.0.1:11434/api/generate`) to generate descriptive tags using the `llava` model. Requires an approved account.
- **Headers:** `Authorization: Bearer <token>`, `Content-Type: multipart/form-data`
- **Body:** Form data containing a single file field.
- **Success:** `200 OK` with the `/images/<file>` path

#### POST /delete/:image_id
Delete an uploaded image.
- **Headers:** `Authorization: Bearer <token>`
- **Success:** `200 OK` (plain text)

#### GET /my_images
List all images uploaded by the authenticated user.
- **Headers:** `Authorization: Bearer <token>`
- **Query Params:** `?tag=optional_tag_filter`
- **Success:** `200 OK` returning JSON array of image records.

#### GET /all_images
List every image in the system. No authentication required.
- **Query Params:** `?tag=optional_tag_filter`
- **Success:** `200 OK` returning JSON array of image records.

---

### Tags and Favorites

#### GET /tags
List all system tags.
- **Success:** `200 OK` JSON array of strings.

#### POST /api/images/:image_id/tags
Manually add a tag to an image.
- **Headers:** `Authorization: Bearer <token>`
- **Body:** `{"name": "mytag"}`
- **Success:** `200 OK` (plain text)

#### DELETE /api/images/:image_id/tags/:tag_name
Remove a tag from an image.
- **Headers:** `Authorization: Bearer <token>`
- **Success:** `200 OK` (plain text)

#### GET /favorites
List the authenticated user's favorite images.
- **Headers:** `Authorization: Bearer <token>`
- **Success:** `200 OK` JSON array of image records.

#### POST /favorite/:image_id
Toggle favorite status for an image.
- **Headers:** `Authorization: Bearer <token>`
- **Success:** `200 OK` (returns "Added to favorites" or "Removed from favorites")

---

### Admin Actions

#### GET /admin/users
List pending users. Admin only.
- **Headers:** `Authorization: Bearer <token>`
- **Success:** `200 OK` JSON array of user records.

#### POST /admin/approve/:user_id
Approve a pending user. Admin only.
- **Headers:** `Authorization: Bearer <token>`
- **Success:** `200 OK`

---

### Discord Integration

#### POST /api/discord_webhook
Post an image to Discord via bot API (Requires `BOT_TOKEN` in `.env`). Authenticated endpoint.
- **Headers:** `Authorization: Bearer <token>`
- **Body:** `{"channel_id": "123...", "image_path": "/images/abc.jpg"}`
- **Success:** `200 OK`

#### GET /send
Send an image to Discord using query parameters.
- **Query Params:** `?channel=123...&path=/images/abc.jpg`
- **Success:** `200 OK`
