use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Multipart, Path, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Semaphore;
use zip::write::SimpleFileOptions;

use crate::{
    core::security::generate_short_id,
    error::{AppError, AppResult},
    state::AppState,
};

const MODRINTH_UA: &str = "Sayraa-srx/mc-fixlag-autobuild/2.0.0 (sayraa.srx@gmail.com)";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/file/upload", post(upload_file_ep))
        .route("/api/v1/file/upload", post(upload_file_ep))
        .route("/v1/file/download/{file_id}", get(download_file_ep))
        .route("/api/v1/file/download/{file_id}", get(download_file_ep))
        .route("/d/{file_id}", get(download_file_ep))
        .route("/api/d/{file_id}", get(download_file_ep))
        .route("/v1/mc/auto-build", post(mc_auto_build_ep))
        .route("/api/v1/mc/auto-build", post(mc_auto_build_ep))
        .route("/mc/auto-build", post(mc_auto_build_ep))
        .route("/auto-build", post(mc_auto_build_ep))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileRecord {
    #[serde(rename = "originalName")]
    original_name: String,
    filename: String,
    path: String,
    #[serde(rename = "uploadedAt")]
    uploaded_at: String,
    #[serde(rename = "totalMods", skip_serializing_if = "Option::is_none")]
    total_mods: Option<usize>,
}

async fn read_file_db(state: &AppState) -> HashMap<String, FileRecord> {
    let path = state.config.data_dir().join("file_database.json");
    if !path.exists() {
        return HashMap::new();
    }
    tokio::fs::read_to_string(&path)
        .await
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

async fn write_file_db(state: &AppState, db: &HashMap<String, FileRecord>) -> AppResult<()> {
    let dir = state.config.data_dir();
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join("file_database.json");
    let tmp = dir.join("file_database.json.tmp");
    let content = serde_json::to_string_pretty(db)?;
    tokio::fs::write(&tmp, content).await?;
    tokio::fs::rename(&tmp, &path).await?;
    Ok(())
}

fn sanitize_filename(raw: &str) -> String {
    let name = PathBuf::from(raw)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("upload.bin")
        .to_string();
    name.replace(['/', '\\'], "_")
}

async fn upload_file_ep(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> AppResult<Json<Value>> {
    let mut uploaded: Option<(String, Vec<u8>)> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::bad_request(format!("Lỗi đọc multipart: {}", e)))?
    {
        if let Some(fname) = field.file_name().map(sanitize_filename) {
            let bytes = field
                .bytes()
                .await
                .map_err(|e| AppError::bad_request(format!("Lỗi đọc nội dung file: {}", e)))?;
            uploaded = Some((fname, bytes.to_vec()));
            break;
        }
    }

    let Some((filename, content)) = uploaded else {
        return Err(AppError::bad_request("Vui lòng chọn file để upload!"));
    };

    let upload_dir = state.config.uploads_dir();
    tokio::fs::create_dir_all(&upload_dir).await?;
    let dest_path = upload_dir.join(&filename);
    tokio::fs::write(&dest_path, content).await?;

    let file_id = generate_short_id(8, false);
    {
        let _guard = state.file_db_lock.write().await;
        let mut db = read_file_db(&state).await;
        db.insert(
            file_id.clone(),
            FileRecord {
                original_name: filename.clone(),
                filename: filename.clone(),
                path: dest_path.to_string_lossy().to_string(),
                uploaded_at: Utc::now().to_rfc3339(),
                total_mods: None,
            },
        );
        write_file_db(&state, &db).await?;
    }

    Ok(Json(json!({
        "success": true,
        "message": "Upload file thành công!",
        "fileId": file_id,
        "originalName": filename,
        "downloadUrl": format!("{}/d/{}", state.config.base_url, file_id),
        "directUrl": format!("{}/v1/file/download/{}", state.config.base_url, file_id),
    })))
}

async fn download_file_ep(
    State(state): State<AppState>,
    Path(file_id): Path<String>,
) -> Response {
    let record = {
        let _guard = state.file_db_lock.read().await;
        let db = read_file_db(&state).await;
        db.get(&file_id).cloned()
    };

    let Some(info) = record else {
        return (
            StatusCode::NOT_FOUND,
            Html("<h1>File không tồn tại hoặc đã bị xóa!</h1>"),
        )
            .into_response();
    };

    let candidates = [
        state.config.uploads_dir().join(&info.filename),
        PathBuf::from(&info.path),
    ];

    let Some(actual_path) = candidates.into_iter().find(|p| p.exists()) else {
        return (
            StatusCode::NOT_FOUND,
            Html("<h1>File vật lý không tìm thấy trên server!</h1>"),
        )
            .into_response();
    };

    let Ok(bytes) = tokio::fs::read(&actual_path).await else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html("<h1>Không thể đọc tệp tin!</h1>"),
        )
            .into_response();
    };

    let disposition = format!("attachment; filename=\"{}\"", info.original_name);
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        bytes,
    )
        .into_response()
}

#[derive(Deserialize)]
struct MCAutoBuildRequest {
    version: String,
    #[serde(default = "default_loader")]
    loader: String,
    mods: HashMap<String, String>,
}

fn default_loader() -> String {
    "fabric".to_string()
}

#[derive(Clone)]
struct MatchedMod {
    name: String,
    filename: String,
    url: String,
    version_number: String,
}

fn version_prefix(version: &str) -> String {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() >= 2 {
        format!("{}.{}.", parts[0], parts[1])
    } else {
        version.to_string()
    }
}

async fn match_mod_version(
    client: reqwest::Client,
    sem: Arc<Semaphore>,
    mod_name: String,
    mod_id: String,
    version: String,
    loader: String,
) -> Result<MatchedMod, Value> {
    let _permit = sem.acquire().await.map_err(|e| {
        json!({ "name": mod_name, "reason": e.to_string() })
    })?;

    let prefix = version_prefix(&version);
    let mut target_loaders = vec![loader.to_lowercase()];
    if loader.eq_ignore_ascii_case("fabric") {
        target_loaders.push("quilt".to_string());
    }

    let url = format!("https://api.modrinth.com/v2/project/{}/version", mod_id);
    let res = client
        .get(&url)
        .header("User-Agent", MODRINTH_UA)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| json!({ "name": mod_name, "reason": e.to_string() }))?;

    if !res.status().is_success() {
        return Err(json!({
            "name": mod_name,
            "reason": format!("Modrinth HTTP {}", res.status().as_u16()),
        }));
    }

    let versions: Vec<Value> = res
        .json()
        .await
        .map_err(|e| json!({ "name": mod_name, "reason": e.to_string() }))?;

    let mut matched: Option<&Value> = None;

    // 1. Exact version match
    for v in &versions {
        let loaders_ok = v
            .get("loaders")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter().any(|l| {
                    l.as_str()
                        .map(|s| target_loaders.contains(&s.to_lowercase()))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        if !loaders_ok {
            continue;
        }
        let exact = v
            .get("game_versions")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().any(|gv| gv.as_str() == Some(version.as_str())))
            .unwrap_or(false);
        if exact {
            if v.get("version_type").and_then(Value::as_str) == Some("release") {
                matched = Some(v);
                break;
            }
            if matched.is_none() {
                matched = Some(v);
            }
        }
    }

    // 2. Smart minor series fallback (e.g. 1.21.x)
    if matched.is_none() {
        for v in &versions {
            let loaders_ok = v
                .get("loaders")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter().any(|l| {
                        l.as_str()
                            .map(|s| target_loaders.contains(&s.to_lowercase()))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false);
            if !loaders_ok {
                continue;
            }
            let series = v
                .get("game_versions")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter().any(|gv| {
                        gv.as_str()
                            .map(|s| s.starts_with(&prefix))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false);
            if series {
                if v.get("version_type").and_then(Value::as_str) == Some("release") {
                    matched = Some(v);
                    break;
                }
                if matched.is_none() {
                    matched = Some(v);
                }
            }
        }
    }

    if let Some(v) = matched {
        if let Some(files) = v.get("files").and_then(Value::as_array) {
            if let Some(primary) = files
                .iter()
                .find(|f| f.get("primary").and_then(Value::as_bool) == Some(true))
                .or_else(|| files.first())
            {
                let filename = primary
                    .get("filename")
                    .and_then(Value::as_str)
                    .unwrap_or("mod.jar")
                    .to_string();
                let dl_url = primary
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let ver_num = v
                    .get("version_number")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if !dl_url.is_empty() {
                    return Ok(MatchedMod {
                        name: mod_name,
                        filename,
                        url: dl_url,
                        version_number: ver_num,
                    });
                }
            }
        }
    }

    Err(json!({
        "name": mod_name,
        "reason": format!("No compatible version found for {} or {}*", version, prefix),
    }))
}

async fn mc_auto_build_ep(
    State(state): State<AppState>,
    Json(payload): Json<MCAutoBuildRequest>,
) -> AppResult<Response> {
    let version = payload.version.trim().to_string();
    let loader = payload.loader.trim().to_string();

    if version.is_empty() || loader.is_empty() || payload.mods.is_empty() {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "Missing or misformatted data fields (version, loader, mods{})."
            })),
        )
            .into_response());
    }

    let mut loader_chars = loader.chars();
    let loader_cap = match loader_chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + loader_chars.as_str(),
        None => loader.clone(),
    };
    let zip_file_name = format!("[{}] {} - Sayraa.zip", version, loader_cap);
    let upload_dir = state.config.uploads_dir();
    tokio::fs::create_dir_all(&upload_dir).await?;
    let zip_file_path = upload_dir.join(&zip_file_name);

    // Step 1: Concurrently resolve mod versions on Modrinth
    let meta_sem = Arc::new(Semaphore::new(12));
    let match_futs = payload.mods.iter().map(|(name, id)| {
        match_mod_version(
            state.http.clone(),
            meta_sem.clone(),
            name.clone(),
            id.clone(),
            version.clone(),
            loader.clone(),
        )
    });
    let match_results = join_all(match_futs).await;

    let mut valid_files = Vec::new();
    let mut missing_mods = Vec::new();
    for res in match_results {
        match res {
            Ok(m) => valid_files.push(m),
            Err(miss) => missing_mods.push(miss),
        }
    }

    if valid_files.is_empty() {
        return Ok((
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": "No mod files compatible with the selected version and loader were found!",
                "missingMods": missing_mods,
            })),
        )
            .into_response());
    }

    // Step 2: Concurrently download mod jars
    let dl_sem = Arc::new(Semaphore::new(10));
    let dl_futs = valid_files.iter().map(|m| {
        let client = state.http.clone();
        let sem = dl_sem.clone();
        let m = m.clone();
        async move {
            let _permit = sem.acquire().await.ok()?;
            let res = client
                .get(&m.url)
                .header("User-Agent", MODRINTH_UA)
                .timeout(Duration::from_secs(60))
                .send()
                .await
                .ok()?;
            if !res.status().is_success() {
                return None;
            }
            let bytes = res.bytes().await.ok()?;
            Some((m.filename, bytes.to_vec()))
        }
    });
    let downloaded: Vec<(String, Vec<u8>)> = join_all(dl_futs).await.into_iter().flatten().collect();

    // Step 3: Build ZIP archive on blocking threadpool
    let zip_path_clone = zip_file_path.clone();
    let packed_count = tokio::task::spawn_blocking(move || -> Result<usize, String> {
        let file = std::fs::File::create(&zip_path_clone).map_err(|e| e.to_string())?;
        let mut zip_writer = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut count = 0usize;
        for (fname, data) in downloaded {
            zip_writer.start_file(&fname, options).map_err(|e| e.to_string())?;
            zip_writer.write_all(&data).map_err(|e| e.to_string())?;
            count += 1;
        }
        zip_writer.finish().map_err(|e| e.to_string())?;
        Ok(count)
    })
    .await
    .map_err(|e| AppError::internal(e.to_string()))?
    .map_err(AppError::internal)?;

    let file_id = generate_short_id(8, false);
    {
        let _guard = state.file_db_lock.write().await;
        let mut db = read_file_db(&state).await;
        db.insert(
            file_id.clone(),
            FileRecord {
                original_name: zip_file_name.clone(),
                filename: zip_file_name.clone(),
                path: zip_file_path.to_string_lossy().to_string(),
                uploaded_at: Utc::now().to_rfc3339(),
                total_mods: Some(packed_count),
            },
        );
        write_file_db(&state, &db).await?;
    }

    let packed_mods: Vec<Value> = valid_files
        .iter()
        .map(|f| {
            json!({
                "name": f.name,
                "filename": f.filename,
                "version": f.version_number,
            })
        })
        .collect();

    Ok(Json(json!({
        "success": true,
        "message": format!("Automatically build mod collection [{}] successfully!", version),
        "fileId": file_id,
        "zipName": zip_file_name,
        "downloadUrl": format!("{}/d/{}", state.config.base_url, file_id),
        "directUrl": format!("{}/v1/file/download/{}", state.config.base_url, file_id),
        "totalModsPacked": packed_count,
        "totalModsRequested": payload.mods.len(),
        "packedMods": packed_mods,
        "missingMods": missing_mods,
    }))
    .into_response())
}
