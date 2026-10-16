//! Dataset catalog and raster-file boundary endpoints.

use crate::{err, valid_id, ApiError, AppState, Shared};
use axum::extract::{Path as UrlPath, State};
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tower::ServiceExt;

#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: Value,
    #[serde(default)]
    group: String,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    license: License,
    #[serde(default)]
    coverage: Coverage,
    #[serde(default)]
    crs: String,
    #[serde(default)]
    geometry_type: String,
    #[serde(default)]
    format_normalized: String,
    #[serde(default)]
    files: Vec<ManifestFile>,
    #[serde(default)]
    notes: String,
}

#[derive(Debug, Default, Deserialize)]
struct License {
    #[serde(default)]
    name: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    attribution: String,
    #[serde(default)]
    redistribution: String,
}

#[derive(Debug, Default, Deserialize)]
struct Coverage {
    #[serde(default)]
    name: String,
    #[serde(default)]
    bbox: Value,
}

#[derive(Debug, Deserialize)]
struct ManifestFile {
    #[serde(default)]
    path: String,
    #[serde(default)]
    size_bytes: Value,
    #[serde(default)]
    rows: Value,
}

fn manifest_paths(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|extension| extension == "json")
                && path
                    .file_name()
                    .is_some_and(|name| name != "example.json" && !name.to_string_lossy().starts_with('_'))
        })
        .collect();
    paths.sort();
    paths
}

fn read_manifest(path: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&text).map_err(|error| error.to_string())
}

fn repository_file(repo_root: &Path, relative: &str) -> Option<PathBuf> {
    if relative.is_empty() {
        return None;
    }
    let root = repo_root.canonicalize().ok()?;
    let candidate = root.join(relative).canonicalize().ok()?;
    candidate.starts_with(&root).then_some(candidate)
}

fn quality(repo_root: &Path, id: &str) -> Value {
    let path = repo_root.join("data").join("quality").join(format!("{id}.json"));
    let Ok(text) = std::fs::read_to_string(path) else {
        return Value::Null;
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Value::Null;
    };
    value
        .get("files")
        .and_then(Value::as_array)
        .and_then(|files| files.first())
        .cloned()
        .unwrap_or(Value::Null)
}

fn raster_files(manifest: &Manifest, repo_root: &Path) -> Value {
    if manifest.group != "raster" {
        return Value::Null;
    }
    let files: Vec<_> = manifest
        .files
        .iter()
        .filter_map(|file| {
            let path = repository_file(repo_root, &file.path)?;
            let name = path.file_name()?.to_string_lossy().into_owned();
            Some(json!({
                "name": name,
                "url": format!("/api/rasters/{}/{}", manifest.id, name),
                "size_bytes": std::fs::metadata(path).ok()?.len(),
            }))
        })
        .collect();
    Value::Array(files)
}

fn dataset(st: &AppState, manifest: Manifest) -> Value {
    let present = manifest
        .files
        .iter()
        .all(|file| repository_file(&st.repo_root, &file.path).is_some_and(|path| path.is_file()));
    let source_ids: Vec<_> = manifest
        .files
        .iter()
        .filter_map(|file| Path::new(&file.path).file_stem())
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|id| st.source_path(id).is_some())
        .collect();

    json!({
        "id": manifest.id,
        "title": manifest.title,
        "group": manifest.group,
        "provider": manifest.provider,
        "license": {
            "name": manifest.license.name,
            "url": manifest.license.url,
            "attribution": manifest.license.attribution,
            "redistribution": manifest.license.redistribution,
        },
        "coverage": {"name": manifest.coverage.name, "bbox": manifest.coverage.bbox},
        "crs": manifest.crs,
        "geometry_type": manifest.geometry_type,
        "format_normalized": manifest.format_normalized,
        "source_ids": source_ids,
        "present": present,
        "files": manifest.files.iter().map(|file| json!({
            "path": file.path,
            "size_bytes": file.size_bytes,
            "rows": file.rows,
        })).collect::<Vec<_>>(),
        "raster": raster_files(&manifest, &st.repo_root),
        "quality": quality(&st.repo_root, &manifest.id),
        "notes": manifest.notes,
    })
}

fn catalog_value(st: &AppState) -> Value {
    let mut datasets = Vec::new();
    let mut attributions = BTreeSet::new();
    let mut errors = Vec::new();

    for path in manifest_paths(&st.manifests_dir) {
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        match read_manifest(&path) {
            Ok(manifest) => {
                let value = dataset(st, manifest);
                if value["present"] == true {
                    if let Some(attribution) = value["license"]["attribution"].as_str().filter(|value| !value.is_empty()) {
                        attributions.insert(attribution.to_string());
                    }
                }
                datasets.push(value);
            }
            Err(error) => errors.push(json!({"manifest": name, "error": error})),
        }
    }

    json!({
        "datasets": datasets,
        "attributions": attributions.into_iter().collect::<Vec<_>>(),
        "errors": errors,
    })
}

pub async fn catalog(State(st): State<Shared>) -> Json<Value> {
    let state = st.clone();
    let value = tokio::task::spawn_blocking(move || catalog_value(&state)).await.unwrap_or_else(|error| {
        json!({"datasets": [], "attributions": [], "errors": [{"manifest": "", "error": error.to_string()}]})
    });
    Json(value)
}

fn safe_component(value: &str) -> bool {
    !value.is_empty() && value != ".." && !value.contains("..") && !value.contains('/') && !value.contains('\\')
}

fn listed_raster(st: &AppState, id: &str, name: &str) -> Option<PathBuf> {
    for manifest_path in manifest_paths(&st.manifests_dir) {
        let Ok(manifest) = read_manifest(&manifest_path) else {
            continue;
        };
        if manifest.id != id || manifest.group != "raster" {
            continue;
        }
        for file in manifest.files {
            let Some(path) = repository_file(&st.repo_root, &file.path) else {
                continue;
            };
            if path.file_name().is_some_and(|file_name| file_name == name) && path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

pub async fn raster(
    State(st): State<Shared>,
    UrlPath((id, name)): UrlPath<(String, String)>,
    req: Request<axum::body::Body>,
) -> Result<Response, ApiError> {
    if !valid_id(&id) || !safe_component(&name) {
        return Err(err(StatusCode::BAD_REQUEST, "bad_id", "invalid raster id or file name"));
    }
    let path = listed_raster(&st, &id, &name).ok_or_else(|| err(StatusCode::NOT_FOUND, "no_such_raster", name))?;
    let response = tower_http::services::ServeFile::new(path)
        .oneshot(req)
        .await
        .map_err(|error| err(StatusCode::INTERNAL_SERVER_ERROR, "file", error.to_string()))?;
    let mut response = response.map(axum::body::Body::new).into_response();
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("image/tiff"));
    Ok(response)
}
