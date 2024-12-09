use actix_files::Files;
use actix_multipart::Multipart;
use actix_web::{error::ErrorInternalServerError, web, App, Error, HttpResponse, HttpServer};
use chrono::Utc;
use futures_util::stream::StreamExt as _;
use sanitize_filename::sanitize;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

// Configurable admin password
const ADMIN_PASSWORD: &str = "changeme";
const MAIN_PAGE_TITLE: &str = "All Articles";

#[derive(Serialize, Deserialize)]
struct CommentForm {
    comment: String,
}

#[derive(Serialize, Deserialize)]
struct PasswordForm {
    password: String,
}

#[derive(Serialize, Deserialize)]
struct EditForm {
    password: String,
    mode: String, // "check" or "save"
    title: Option<String>,
    body: Option<String>,
}

#[derive(Serialize, FromRow)]
struct DbArticle {
    id: i32,
    title: String,
    body: String,
    bump_time: i64,
}

#[derive(Serialize)]
struct Article {
    id: i32,
    title: String,
    body: String,
    media_paths: Vec<String>,
    bump_time: i64,
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init();
    create_and_set_permissions("uploads")?;

    let database_url = env::var("DATABASE_URL")
        .map_err(|e| {
            log_error(&format!("DATABASE_URL not set: {}", e));
            std::io::Error::new(std::io::ErrorKind::NotFound, "DATABASE_URL not set")
        })?;

    let pool = PgPool::connect(&database_url).await.map_err(|e| {
        log_error(&format!("Failed to connect to Postgres: {}", e));
        std::io::Error::new(std::io::ErrorKind::Other, "DB connection failed")
    })?;

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(pool.clone()))
            .route("/", web::get().to(new_article_form))
            .route("/submit", web::post().to(submit_article))
            .route("/articles", web::get().to(list_articles))
            .route("/articles/{id}", web::get().to(view_article))
            .route("/articles/{id}/comment", web::post().to(submit_comment))
            // Delete routes
            .route("/articles/{id}/delete", web::get().to(delete_article_form))
            .route("/articles/{id}/delete", web::post().to(delete_article))
            .route("/comments/{id}/delete", web::get().to(delete_comment_form))
            .route("/comments/{id}/delete", web::post().to(delete_comment))
            // Edit routes
            .route("/articles/{id}/edit", web::get().to(edit_article_form))
            .route("/articles/{id}/edit", web::post().to(edit_article))
            .service(Files::new("/static", "./static"))
            .service(Files::new("/uploads", "./uploads"))
    })
    .bind("127.0.0.1:8080")?
    .run()
    .await
}

fn create_and_set_permissions(dir: &str) -> std::io::Result<()> {
    if !Path::new(dir).exists() {
        fs::create_dir(dir)?;
    }
    Ok(())
}

// Logs all errors to error.txt
fn log_error(error_message: &str) {
    if let Ok(file) = OpenOptions::new().create(true).append(true).open("error.txt") {
        let mut writer = BufWriter::new(file);
        let _ = writeln!(writer, "ERROR: {}", error_message);
    }
}

async fn new_article_form() -> HttpResponse {
    let html = r#"
    <!DOCTYPE html>
    <html lang="en">
    <head>
        <meta charset="UTF-8">
        <title>Submit a New Article</title>
        <link rel="stylesheet" href="/static/style.css">
    </head>
    <body>
        <div class="post-form-box">
            <h1>Submit a New Article</h1>
            <form action="/submit" method="POST" enctype="multipart/form-data">
                <input type="text" name="title" placeholder="Title" required><br>
                <textarea name="body" rows="10" placeholder="Body" required></textarea><br>
                <input type="file" name="media" accept=".jpg,.jpeg,.png,.gif,.webp,.mp4" required><br><br>
                <label>jpg, png, gif, webp, or MP4</label><br><br>
                <input type="submit" value="Submit Article">
            </form>
        </div>
        <br>
        <div class="center-link"><a href="/articles">View All Articles</a></div>
    </body>
    </html>
    "#;

    HttpResponse::Ok().content_type("text/html").body(html)
}

async fn submit_article(
    pool: web::Data<PgPool>,
    mut payload: Multipart,
) -> Result<HttpResponse, Error> {
    let mut title = String::new();
    let mut body = String::new();
    let mut media_paths = Vec::new();

    create_and_set_permissions("uploads").map_err(|e| {
        log_error(&format!("Failed to create uploads dir: {}", e));
        ErrorInternalServerError("Failed to setup uploads directory")
    })?;

    while let Some(item) = payload.next().await {
        let mut field = item.map_err(|e| {
            log_error(&format!("Error reading multipart field: {}", e));
            ErrorInternalServerError("Multipart read error")
        })?;

        let cd = match field.content_disposition() {
            Some(cd) => cd,
            None => {
                log_error("Missing content disposition in multipart field");
                return Err(ErrorInternalServerError("Missing content disposition"));
            }
        };

        let field_name = match cd.get_name() {
            Some(n) => n.to_string(),
            None => {
                log_error("Missing field name in content disposition");
                return Err(ErrorInternalServerError("Missing field name"));
            }
        };

        let filename = cd.get_filename().map(|f| f.to_string());

        // Collect field data
        let mut value = Vec::new();
        while let Some(chunk) = field.next().await {
            let chunk = chunk.map_err(|e| {
                log_error(&format!("Error reading chunk: {}", e));
                ErrorInternalServerError("Error reading chunk")
            })?;
            value.extend_from_slice(&chunk);
        }

        if field_name == "title" {
            title = String::from_utf8(value).unwrap_or_default();
        } else if field_name == "body" {
            body = String::from_utf8(value).unwrap_or_default();
        } else if field_name == "media" {
            if let Some(fname) = filename {
                let sanitized_filename = sanitize(&fname);
                let filepath = format!("./uploads/article_{}", sanitized_filename);
                let mut f = File::create(&filepath).map_err(|e| {
                    log_error(&format!("Failed to create file: {}", e));
                    ErrorInternalServerError("Failed to create file")
                })?;

                f.write_all(&value).map_err(|e| {
                    log_error(&format!("Failed to write file: {}", e));
                    ErrorInternalServerError("Failed to write file")
                })?;
                media_paths.push(format!("/uploads/article_{}", sanitized_filename));
            }
        }
    }

    if media_paths.is_empty() {
        return Ok(HttpResponse::BadRequest().body("Media file is required"));
    }

    let bump_time = Utc::now().timestamp();

    let article_id: i32 = sqlx::query_scalar(
        "INSERT INTO articles (title, body, bump_time) VALUES ($1, $2, $3) RETURNING id"
    )
    .bind(&title)
    .bind(&body)
    .bind(bump_time)
    .fetch_one(pool.get_ref())
    .await
    .map_err(|e| {
        log_error(&format!("Failed to store article: {}", e));
        ErrorInternalServerError("Database insert failed")
    })?;

    for path in media_paths {
        sqlx::query("INSERT INTO article_media (article_id, media_path) VALUES ($1, $2)")
            .bind(article_id)
            .bind(path)
            .execute(pool.get_ref())
            .await
            .map_err(|e| {
                log_error(&format!("Failed to store media: {}", e));
                ErrorInternalServerError("Failed to store media")
            })?;
    }

    Ok(HttpResponse::Found()
        .append_header(("Location", "/articles"))
        .finish())
}

async fn list_articles(pool: web::Data<PgPool>) -> HttpResponse {
    let articles_db = match sqlx::query_as::<_, DbArticle>("SELECT id, title, body, bump_time FROM articles ORDER BY bump_time DESC")
        .fetch_all(pool.get_ref())
        .await {
            Ok(a) => a,
            Err(e) => {
                log_error(&format!("Failed to fetch articles: {}", e));
                return HttpResponse::InternalServerError().body("Failed to load articles");
            }
        };

    let mut articles_html = format!(r#"
    <!DOCTYPE html>
    <html lang="en">
    <head>
        <meta charset="UTF-8">
        <title>{}</title>
        <link rel="stylesheet" href="/static/style.css">
    </head>
    <body>
        <h1>{}</h1>
        <div class="center-link"><a href="/">Submit a New Article</a></div>
    "#, MAIN_PAGE_TITLE, MAIN_PAGE_TITLE);

    for article in &articles_db {
        articles_html.push_str(&format!(
            r#"<div class="article">
                <h2><a href="/articles/{}">{}</a></h2>
                <a href="/articles/{}/delete" class="delete-link">[x]</a>
                <a href="/articles/{}/edit" class="edit-link">[+]</a>
            </div>"#,
            article.id, article.title, article.id, article.id
        ));
    }

    articles_html.push_str("</body></html>");

    HttpResponse::Ok().content_type("text/html").body(articles_html)
}

async fn view_article(pool: web::Data<PgPool>, path: web::Path<i32>) -> HttpResponse {
    let article_id = path.into_inner();

    let article_db = match sqlx::query_as::<_, DbArticle>(
        "SELECT id, title, body, bump_time FROM articles WHERE id = $1",
    )
    .bind(article_id)
    .fetch_one(pool.get_ref())
    .await
    {
        Ok(a) => a,
        Err(_) => {
            log_error("Article not found");
            return HttpResponse::NotFound().body("Article not found");
        }
    };

    let media_paths = sqlx::query!("SELECT media_path FROM article_media WHERE article_id = $1", article_db.id)
        .fetch_all(pool.get_ref())
        .await
        .map(|rows| rows.into_iter().map(|r| r.media_path).collect::<Vec<_>>())
        .unwrap_or_else(|e| {
            log_error(&format!("Failed to fetch article media: {}", e));
            Vec::new()
        });

    let article = Article {
        id: article_db.id,
        title: article_db.title,
        body: article_db.body,
        bump_time: article_db.bump_time,
        media_paths,
    };

    let comments = sqlx::query!("SELECT id, comment FROM comments WHERE article_id = $1", article.id)
        .fetch_all(pool.get_ref())
        .await
        .map(|rows| rows.into_iter().map(|r| (r.id, r.comment)).collect::<Vec<_>>())
        .unwrap_or_else(|e| {
            log_error(&format!("Failed to fetch comments: {}", e));
            Vec::new()
        });

    let mut article_html = String::new();
    article_html.push_str(r#"<!DOCTYPE html><html lang="en"><head><meta charset="UTF-8">"#);
    article_html.push_str(&format!("<title>{}</title>", article.title));
    article_html.push_str(r#"<link rel="stylesheet" href="/static/style.css"></head><body>"#);
    article_html.push_str(r#"<div class="center-link"><a href="/articles">← Back to All Articles</a></div>"#);

    // Article container
    article_html.push_str(r#"<div class="article">"#);
    article_html.push_str(&format!("<h1>{}</h1>", article.title));

    for media in &article.media_paths {
        if media.ends_with(".mp4") {
            article_html.push_str(&format!(
                r#"<video controls class="article-media">
                    <source src="{}" type="video/mp4">
                    Your browser does not support the video tag.
                </video><br>"#,
                media
            ));
        } else {
            article_html.push_str(&format!(
                r#"<img src="{}" alt="Article Image" class="article-media"><br>"#,
                media
            ));
        }
    }

    article_html.push_str(&format!(
        r#"
        <p>{}</p>
        <h3>Leave a Comment</h3>
        <form action="/articles/{}/comment" method="POST">
            <textarea name="comment" rows="4" required></textarea><br>
            <input type="submit" value="Submit Comment">
        </form>
        <h3>Comments</h3>
    "#,
        article.body, article.id
    ));

    // Admin links inside article
    article_html.push_str(&format!(r#"<a href="/articles/{}/delete" class="delete-link">[x]</a>"#, article.id));
    article_html.push_str(&format!(r#"<a href="/articles/{}/edit" class="edit-link">[+]</a>"#, article.id));

    article_html.push_str("</div>"); // end of .article

    for (comment_id, comment) in comments {
        article_html.push_str(&format!(
            r#"<div class="comment"><p>{}</p><a href="/comments/{}/delete" class="delete-link">[x]</a></div>"#,
            comment, comment_id
        ));
    }

    article_html.push_str("</body></html>");

    HttpResponse::Ok().content_type("text/html").body(article_html)
}

async fn submit_comment(
    pool: web::Data<PgPool>,
    path: web::Path<i32>,
    form: web::Form<CommentForm>,
) -> HttpResponse {
    let article_id = path.into_inner();

    if let Err(e) = sqlx::query("INSERT INTO comments (article_id, comment) VALUES ($1, $2)")
        .bind(article_id)
        .bind(&form.comment)
        .execute(pool.get_ref())
        .await
    {
        log_error(&format!("Failed to store comment: {}", e));
        return HttpResponse::InternalServerError().body("Failed to store comment.");
    }

    let new_bump_time = Utc::now().timestamp();
    if let Err(e) = sqlx::query("UPDATE articles SET bump_time = $1 WHERE id = $2")
        .bind(new_bump_time)
        .bind(article_id)
        .execute(pool.get_ref())
        .await
    {
        log_error(&format!("Failed to bump article: {}", e));
        return HttpResponse::InternalServerError().body("Failed to bump article.");
    }

    HttpResponse::Found()
        .append_header(("Location", format!("/articles/{}", article_id)))
        .finish()
}

async fn delete_article_form(path: web::Path<i32>) -> HttpResponse {
    let article_id = path.into_inner();
    let html = format!(
        r#"
        <!DOCTYPE html>
        <html lang="en">
        <head><meta charset="UTF-8"><title>Delete Article</title>
        <link rel="stylesheet" href="/static/style.css"></head>
        <body>
        <div class="post-form-box">
        <h2>Enter Password to Delete Article</h2>
        <form action="/articles/{}/delete" method="POST" enctype="multipart/form-data">
            <input type="password" name="password" placeholder="Password" required>
            <input type="submit" value="Delete Article">
        </form>
        </div>
        </body>
        </html>
        "#,
        article_id
    );
    HttpResponse::Ok().content_type("text/html").body(html)
}

async fn delete_article(pool: web::Data<PgPool>, path: web::Path<i32>, form: web::Form<PasswordForm>) -> HttpResponse {
    let article_id = path.into_inner();
    let password = &form.password;

    if password != ADMIN_PASSWORD {
        log_error("Incorrect password for article deletion");
        return HttpResponse::Unauthorized().body("Incorrect password");
    }

    if let Err(e) = sqlx::query("DELETE FROM articles WHERE id = $1")
        .bind(article_id)
        .execute(pool.get_ref())
        .await
    {
        log_error(&format!("Failed to delete article: {}", e));
        return HttpResponse::InternalServerError().body("Failed to delete article.");
    }

    HttpResponse::Found().append_header(("Location", "/articles")).finish()
}

async fn delete_comment_form(path: web::Path<i32>) -> HttpResponse {
    let comment_id = path.into_inner();
    let html = format!(
        r#"
        <!DOCTYPE html>
        <html lang="en">
        <head><meta charset="UTF-8"><title>Delete Comment</title>
        <link rel="stylesheet" href="/static/style.css"></head>
        <body>
        <div class="post-form-box">
        <h2>Enter Password to Delete Comment</h2>
        <form action="/comments/{}/delete" method="POST" enctype="multipart/form-data">
            <input type="password" name="password" placeholder="Password" required>
            <input type="submit" value="Delete Comment">
        </form>
        </div>
        </body>
        </html>
        "#,
        comment_id
    );
    HttpResponse::Ok().content_type("text/html").body(html)
}

async fn delete_comment(pool: web::Data<PgPool>, path: web::Path<i32>, form: web::Form<PasswordForm>) -> HttpResponse {
    let comment_id = path.into_inner();
    let password = &form.password;

    if password != ADMIN_PASSWORD {
        log_error("Incorrect password for comment deletion");
        return HttpResponse::Unauthorized().body("Incorrect password");
    }

    let article_id: Option<i32> = sqlx::query_scalar("SELECT article_id FROM comments WHERE id = $1")
        .bind(comment_id)
        .fetch_optional(pool.get_ref())
        .await
        .ok()
        .flatten();

    if let Err(e) = sqlx::query("DELETE FROM comments WHERE id = $1")
        .bind(comment_id)
        .execute(pool.get_ref())
        .await
    {
        log_error(&format!("Failed to delete comment: {}", e));
        return HttpResponse::InternalServerError().body("Failed to delete comment.");
    }

    let redirect_location = match article_id {
        Some(a_id) => format!("/articles/{}", a_id),
        None => "/articles".to_string(),
    };

    HttpResponse::Found()
        .append_header(("Location", redirect_location))
        .finish()
}

async fn edit_article_form(path: web::Path<i32>) -> HttpResponse {
    let article_id = path.into_inner();
    // Include enctype here as well to ensure multipart form submission.
    let html = format!(
        r#"
        <!DOCTYPE html>
        <html lang="en">
        <head><meta charset="UTF-8"><title>Edit Article</title>
        <link rel="stylesheet" href="/static/style.css"></head>
        <body>
        <div class="post-form-box">
        <h2>Enter Password to Edit Article</h2>
        <form action="/articles/{}/edit" method="POST" enctype="multipart/form-data">
            <input type="password" name="password" placeholder="Password" required>
            <input type="hidden" name="mode" value="check">
            <input type="submit" value="Continue">
        </form>
        </div>
        </body>
        </html>
        "#,
        article_id
    );
    HttpResponse::Ok().content_type("text/html").body(html)
}

async fn edit_article(
    pool: web::Data<PgPool>,
    path: web::Path<i32>,
    mut payload: Multipart,
) -> Result<HttpResponse, Error> {
    let article_id = path.into_inner();
    let mut password = String::new();
    let mut mode = String::new();
    let mut new_title = String::new();
    let mut new_body = String::new();
    let mut new_media: Option<String> = None; // path to new media

    while let Some(item) = payload.next().await {
        let mut field = item.map_err(|e| {
            log_error(&format!("Error reading edit form field: {}", e));
            ErrorInternalServerError("Multipart read error")
        })?;

        let cd = match field.content_disposition() {
            Some(cd) => cd,
            None => {
                log_error("Missing content disposition in edit article field");
                return Err(ErrorInternalServerError("Missing content disposition"));
            }
        };

        let field_name = match cd.get_name() {
            Some(n) => n.to_string(),
            None => {
                log_error("Missing field name in edit article form");
                return Err(ErrorInternalServerError("Missing field name"));
            }
        };

        let filename = cd.get_filename().map(|f| f.to_string());

        let mut value = Vec::new();
        while let Some(chunk) = field.next().await {
            let chunk = chunk.map_err(|e| {
                log_error(&format!("Error reading chunk in edit form: {}", e));
                ErrorInternalServerError("Error reading chunk")
            })?;
            value.extend_from_slice(&chunk);
        }

        if field_name == "password" {
            password = String::from_utf8(value).unwrap_or_default();
        } else if field_name == "mode" {
            mode = String::from_utf8(value).unwrap_or_default();
        } else if field_name == "title" {
            new_title = String::from_utf8(value).unwrap_or_default();
        } else if field_name == "body" {
            new_body = String::from_utf8(value).unwrap_or_default();
        } else if field_name == "media" && !value.is_empty() {
            if let Some(fname) = filename {
                let sanitized_filename = sanitize(&fname);
                let filepath = format!("./uploads/article_{}", sanitized_filename);
                let mut f = File::create(&filepath).map_err(|e| {
                    log_error(&format!("Failed to create file in edit: {}", e));
                    ErrorInternalServerError("Failed to create file")
                })?;
                f.write_all(&value).map_err(|e| {
                    log_error(&format!("Failed to write file in edit: {}", e));
                    ErrorInternalServerError("Failed to write file")
                })?;
                new_media = Some(format!("/uploads/article_{}", sanitized_filename));
            }
        }
    }

    if password != ADMIN_PASSWORD {
        log_error("Incorrect password for article editing");
        return Ok(HttpResponse::Unauthorized().body("Incorrect password"));
    }

    if mode == "check" {
        // Show edit form with current article data
        let article = sqlx::query_as::<_, DbArticle>(
            "SELECT id, title, body, bump_time FROM articles WHERE id = $1",
        )
        .bind(article_id)
        .fetch_one(pool.get_ref())
        .await
        .map_err(|e| {
            log_error(&format!("Failed to fetch article for editing: {}", e));
            ErrorInternalServerError("Failed to fetch article")
        })?;

        let media_path: Option<String> = sqlx::query_scalar::<_, String>(
            "SELECT media_path FROM article_media WHERE article_id = $1 LIMIT 1",
        )
        .bind(article_id)
        .fetch_optional(pool.get_ref())
        .await
        .map_err(|e| {
            log_error(&format!("Failed to fetch media for editing: {}", e));
            ErrorInternalServerError("Failed to fetch media")
        })?;

        let current_media = media_path.unwrap_or_default();

        let html = format!(
            r#"
            <!DOCTYPE html>
            <html lang="en">
            <head><meta charset="UTF-8"><title>Edit Article</title>
            <link rel="stylesheet" href="/static/style.css"></head>
            <body>
            <div class="post-form-box">
            <h2>Edit Article</h2>
            <form action="/articles/{}/edit" method="POST" enctype="multipart/form-data">
                <input type="hidden" name="password" value="{}">
                <input type="hidden" name="mode" value="save">
                <input type="text" name="title" value="{}" required><br>
                <textarea name="body" rows="10" required>{}</textarea><br>
                Current Media: <br>
                <img src="{}" alt="Article Image" style="max-width:200px;"><br><br>
                Replace Media (optional): <br>
                <input type="file" name="media" accept=".jpg,.jpeg,.png,.gif,.webp,.mp4"><br><br>
                <input type="submit" value="Save Changes">
            </form>
            </div>
            </body>
            </html>
            "#,
            article_id,
            password,
            article.title,
            article.body,
            current_media
        );

        return Ok(HttpResponse::Ok().content_type("text/html").body(html));
    } else if mode == "save" {
        // Update article
        if new_title.is_empty() || new_body.is_empty() {
            log_error("Edit article failed: title/body empty");
            return Ok(HttpResponse::BadRequest().body("Title and body are required"));
        }

        sqlx::query("UPDATE articles SET title = $1, body = $2, bump_time = $3 WHERE id = $4")
            .bind(new_title)
            .bind(new_body)
            .bind(Utc::now().timestamp())
            .bind(article_id)
            .execute(pool.get_ref())
            .await
            .map_err(|e| {
                log_error(&format!("Failed to update article: {}", e));
                ErrorInternalServerError("Failed to update article")
            })?;

        if let Some(new_path) = new_media {
            sqlx::query("DELETE FROM article_media WHERE article_id = $1")
                .bind(article_id)
                .execute(pool.get_ref())
                .await
                .map_err(|e| {
                    log_error(&format!("Failed to delete old media: {}", e));
                    ErrorInternalServerError("Failed to delete old media")
                })?;

            sqlx::query("INSERT INTO article_media (article_id, media_path) VALUES ($1, $2)")
                .bind(article_id)
                .bind(new_path)
                .execute(pool.get_ref())
                .await
                .map_err(|e| {
                    log_error(&format!("Failed to store new media: {}", e));
                    ErrorInternalServerError("Failed to store new media")
                })?;
        }

        return Ok(HttpResponse::Found()
            .append_header(("Location", format!("/articles/{}", article_id)))
            .finish());
    }

    log_error("Invalid mode for edit article");
    Ok(HttpResponse::BadRequest().body("Invalid mode"))
}
