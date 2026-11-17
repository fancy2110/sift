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

/// Detect the real format from the file's first bytes, not its name.
fn sniff_image(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Some("image/png");
    }
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if head.len() > 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if head.starts_with(b"BM") {
        return Some("image/bmp");
    }
    None
}

/// Text-like content: mostly printable UTF-8 without NUL bytes.
fn looks_like_text(bytes: &[u8]) -> bool {
    if bytes.contains(&0) {
        return false;
    }
    let sample = &bytes[..bytes.len().min(4096)];
    if sample.is_empty() {
        return true;
    }
    let control = sample
        .iter()
        .filter(|byte| **byte < 0x09 || (**byte > 0x0D && **byte < 0x20))
        .count();
    if control * 20 > sample.len() {
        return false;
    }
    std::str::from_utf8(sample).is_ok()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_text_without_an_extension() {
        let path = std::env::temp_dir().join(format!("sift-preview-{}-noext", std::process::id()));
        std::fs::write(&path, b"#!/bin/sh\necho hello\n").unwrap();
        let dto = preview_file(path.to_str().unwrap()).unwrap();
        assert_eq!(dto.kind, "text");
        assert!(dto.text.unwrap().contains("echo hello"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn detects_png_by_magic_not_extension() {
        let path = std::env::temp_dir().join(format!("sift-preview-{}-img.bin", std::process::id()));
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.extend_from_slice(&[0u8; 32]);
        std::fs::write(&path, &bytes).unwrap();
        let dto = preview_file(path.to_str().unwrap()).unwrap();
        assert_eq!(dto.kind, "image");
        assert!(dto.data_url.unwrap().starts_with("data:image/png;base64,"));
        std::fs::remove_file(&path).ok();
    }
}

#[tauri::command]
pub async fn file_preview(
    _services: State<'_, AppServices>,
    path: String,
) -> Result<FilePreviewDto, String> {
    tauri::async_runtime::spawn_blocking(move || preview_file(&path))
        .await
        .map_err(|error| error.to_string())?
}

fn preview_file(path: &str) -> Result<FilePreviewDto, String> {
    let file = Path::new(&path);
    if !Path::new(&path).is_absolute() {
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
        path: path.to_string(),
        size,
        modified_ms,
        text: None,
        data_url: None,
        truncated: false,
    };

    // Read the head once: it identifies images and text, whatever the name.
    let mut head = Vec::new();
    if let Ok(mut reader) = std::fs::File::open(file) {
        use std::io::Read;
        let mut buffer = [0u8; 8192];
        if let Ok(read) = reader.read(&mut buffer) {
            head.extend_from_slice(&buffer[..read]);
        }
    }
    if size == 0 {
        dto.kind = "text".to_string();
        dto.text = Some(String::new());
        return Ok(dto);
    }

    let image = sniff_image(&head).or_else(|| mime_for(file));
    if let Some(mime) = image {
        if size > MAX_IMAGE_BYTES {
            return Ok(dto);
        }
        if head.len() >= 8 || mime_for(file).is_some() {
            let bytes = std::fs::read(file).map_err(|_| "err.previewRead".to_string())?;
            dto.kind = "image".to_string();
            dto.data_url = Some(format!("data:{mime};base64,{}", base64(&bytes)));
            return Ok(dto);
        }
    }

    let text_sized = size <= 8 * 1024 * 1024;
    let text_named = is_text_extension(file);
    if text_sized && (text_named || looks_like_text(&head)) {
        let bytes = std::fs::read(file).map_err(|_| "err.previewRead".to_string())?;
        let truncated = bytes.len() > MAX_TEXT_BYTES;
        let slice = &bytes[..bytes.len().min(MAX_TEXT_BYTES)];
        if looks_like_text(slice) || text_named {
            dto.kind = "text".to_string();
            dto.text = Some(String::from_utf8_lossy(slice).into_owned());
            dto.truncated = truncated;
        }
    }
    Ok(dto)
}
