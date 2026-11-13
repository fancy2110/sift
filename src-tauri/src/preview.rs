//! File previews for the browser: text and images inline, details otherwise.

use std::path::Path;

use serde::Serialize;
use tauri::State;

use crate::analyze::AppServices;

const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePreviewDto {
    /// `text` | `image` | `unsupported`
    pub kind: String,
    pub name: String,
    pub path: String,
    pub size: u64,
    pub modified_ms: Option<i64>,
    pub text: Option<String>,
    /// Data URL for images (`data:image/png;base64,…`).
    pub data_url: Option<String>,
    /// True when the content was cut to the preview limit.
    pub truncated: bool,
}

fn mime_for(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => return None,
    })
}

fn is_text_extension(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "txt"
            | "md"
            | "markdown"
            | "json"
            | "jsonc"
            | "yaml"
            | "yml"
            | "toml"
            | "ini"
            | "cfg"
            | "conf"
            | "csv"
            | "tsv"
            | "log"
            | "xml"
            | "html"
            | "htm"
            | "css"
            | "scss"
            | "js"
            | "mjs"
            | "ts"
            | "tsx"
            | "jsx"
            | "rs"
            | "py"
            | "go"
            | "java"
            | "c"
            | "h"
            | "cpp"
            | "sh"
            | "zsh"
            | "swift"
            | "sql"
            | "gitignore"
            | "env"
            | "svg"
    )
}

/// Small, dependency-free base64 encoder for image previews.
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) & 63] as char);
        out.push(TABLE[(n >> 12) & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n & 63] as char
        } else {
            '='
        });
    }
    out
}

#[tauri::command]
pub fn file_preview(
    _services: State<'_, AppServices>,
    path: String,
) -> Result<FilePreviewDto, String> {
    let file = Path::new(&path);
    if !path.starts_with('/') {
        return Err("err.previewPath".to_string());
    }
    let meta = std::fs::metadata(file).map_err(|_| "err.previewMissing".to_string())?;
    if !meta.is_file() {
        return Err("err.previewNotFile".to_string());
    }
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|age| age.as_millis() as i64);
    let size = meta.len();
    let mut dto = FilePreviewDto {
        kind: "unsupported".to_string(),
        name,
        path: path.clone(),
        size,
        modified_ms,
        text: None,
        data_url: None,
        truncated: false,
    };

    if let Some(mime) = mime_for(file) {
        if size > MAX_IMAGE_BYTES {
            return Ok(dto);
        }
        let bytes = std::fs::read(file).map_err(|_| "err.previewRead".to_string())?;
        dto.kind = "image".to_string();
        dto.data_url = Some(format!("data:{mime};base64,{}", base64(&bytes)));
        return Ok(dto);
    }

    if is_text_extension(file) && size <= 8 * 1024 * 1024 {
        let bytes = std::fs::read(file).map_err(|_| "err.previewRead".to_string())?;
        let truncated = bytes.len() > MAX_TEXT_BYTES;
        let slice = &bytes[..bytes.len().min(MAX_TEXT_BYTES)];
        if let Ok(text) = std::str::from_utf8(slice) {
            dto.kind = "text".to_string();
            dto.text = Some(text.to_string());
            dto.truncated = truncated;
        }
    }
    Ok(dto)
}
