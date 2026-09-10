use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// What Claude has to do to actually read an attached file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttachmentKind {
    /// An image — Claude's Read tool renders these directly
    Image,
    /// A PDF — Read handles these natively (page ranges and all)
    Pdf,
    /// Plain text, markdown, CSV, JSON, source code — read as-is
    Text,
    /// A binary document format Read can't open, which we converted to a
    /// plain-text sidecar file for it
    Converted,
    /// Something we can't make readable
    Unsupported,
}

/// An attachment resolved to a path Claude can actually read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    /// The file the user picked
    pub original_path: String,
    /// The path to reference in the prompt — same as `original_path` unless
    /// the file had to be converted
    pub read_path: String,
    /// Base file name, for display
    pub file_name: String,
    pub kind: AttachmentKind,
    /// Size of the original file in bytes
    pub size_bytes: u64,
    /// Human-readable explanation when a file was converted or can't be used
    pub note: Option<String>,
}

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico"];

/// Formats Read can't open, but that `textutil` (macOS) can flatten to text
const CONVERTIBLE_EXTENSIONS: &[&str] = &["doc", "docx", "rtf", "odt", "rtfd", "wordml", "webarchive"];

/// Extensions we treat as directly readable text. Anything without an
/// extension, or with an unknown one, is sniffed for binary content instead —
/// source files come in too many flavours to enumerate.
const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "text", "md", "markdown", "mdx", "csv", "tsv", "json", "jsonl", "yaml", "yml", "toml",
    "ini", "cfg", "conf", "env", "xml", "html", "htm", "css", "scss", "less", "log", "sql", "sh",
    "bash", "zsh", "fish", "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "rb", "go", "java",
    "kt", "kts", "swift", "c", "h", "cc", "cpp", "hpp", "cs", "php", "pl", "lua", "r", "dart",
    "vue", "svelte", "graphql", "gql", "proto", "patch", "diff", "gitignore", "dockerfile",
];

fn extension_of(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
}

/// Heuristic used for files whose extension we don't recognise: if the first
/// chunk contains a NUL byte or fails UTF-8, treat it as binary.
fn looks_like_text(path: &Path) -> bool {
    use std::io::Read;

    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };

    let mut buffer = [0u8; 8192];
    let read = match file.read(&mut buffer) {
        Ok(n) => n,
        Err(_) => return false,
    };
    let chunk = &buffer[..read];

    if chunk.contains(&0) {
        return false;
    }

    // A truncated multi-byte character at the boundary is fine; anything else
    // invalid means this isn't text.
    match std::str::from_utf8(chunk) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none(),
    }
}

/// Directory holding text conversions of binary documents
fn conversions_dir() -> PathBuf {
    std::env::temp_dir().join("opcode-attachments")
}

/// Convert a document Read can't open into a plain-text file it can.
///
/// Uses macOS's built-in `textutil`; there's no equivalent bundled on other
/// platforms, so this reports that the format isn't supported there rather
/// than pretending to handle it.
#[cfg(target_os = "macos")]
fn convert_to_text(path: &Path) -> Result<PathBuf, String> {
    let out_dir = conversions_dir();
    fs::create_dir_all(&out_dir).map_err(|e| format!("Failed to create conversion dir: {}", e))?;

    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("attachment");
    // Include the original extension in the name so a.docx and a.doc don't collide
    let original_ext = extension_of(path).unwrap_or_default();
    let out_path = out_dir.join(format!("{}-{}.txt", stem, original_ext));

    let output = std::process::Command::new("textutil")
        .arg("-convert")
        .arg("txt")
        .arg("-output")
        .arg(&out_path)
        .arg(path)
        .output()
        .map_err(|e| format!("Failed to run textutil: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "textutil could not convert this file: {}",
            stderr.trim()
        ));
    }

    if !out_path.is_file() {
        return Err("textutil reported success but produced no output file".to_string());
    }

    Ok(out_path)
}

#[cfg(not(target_os = "macos"))]
fn convert_to_text(_path: &Path) -> Result<PathBuf, String> {
    Err("Converting Word documents is only supported on macOS. Export the file to PDF, TXT or Markdown and attach that instead.".to_string())
}

/// Resolve one file into something Claude can read, converting it if needed.
///
/// This does not copy or move the user's file — the prompt just references it
/// by path (the only exception being converted documents, which get a
/// text sidecar in a temp directory).
#[tauri::command]
pub async fn prepare_attachment(path: String) -> Result<Attachment, String> {
    debug!("Preparing attachment: {}", path);

    let file_path = PathBuf::from(&path);

    if !file_path.exists() {
        return Err(format!("File does not exist: {}", path));
    }
    if file_path.is_dir() {
        return Err(format!(
            "{} is a directory — attach a file, or reference the directory with @ instead",
            path
        ));
    }

    let metadata = fs::metadata(&file_path).map_err(|e| format!("Failed to read file: {}", e))?;
    let size_bytes = metadata.len();

    let file_name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("attachment")
        .to_string();

    let ext = extension_of(&file_path).unwrap_or_default();

    let mut attachment = Attachment {
        original_path: path.clone(),
        read_path: path.clone(),
        file_name,
        kind: AttachmentKind::Unsupported,
        size_bytes,
        note: None,
    };

    if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        attachment.kind = AttachmentKind::Image;
    } else if ext == "pdf" {
        attachment.kind = AttachmentKind::Pdf;
    } else if TEXT_EXTENSIONS.contains(&ext.as_str()) {
        attachment.kind = AttachmentKind::Text;
    } else if CONVERTIBLE_EXTENSIONS.contains(&ext.as_str()) {
        match convert_to_text(&file_path) {
            Ok(converted) => {
                info!("Converted {} to text at {:?}", path, converted);
                attachment.read_path = converted.to_string_lossy().to_string();
                attachment.kind = AttachmentKind::Converted;
                attachment.note = Some(format!(
                    "{} can't be read directly, so it was converted to plain text (formatting and images are dropped)",
                    ext.to_uppercase()
                ));
            }
            Err(e) => {
                warn!("Failed to convert {}: {}", path, e);
                attachment.kind = AttachmentKind::Unsupported;
                attachment.note = Some(e);
            }
        }
    } else if looks_like_text(&file_path) {
        // Unknown extension but the bytes look like text — read it as text
        attachment.kind = AttachmentKind::Text;
    } else {
        attachment.kind = AttachmentKind::Unsupported;
        attachment.note = Some(format!(
            "{} files can't be read. Export to PDF, TXT, CSV or Markdown and attach that instead.",
            if ext.is_empty() {
                "Binary".to_string()
            } else {
                ext.to_uppercase()
            }
        ));
    }

    Ok(attachment)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str, contents: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "opcode-attachment-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, contents).unwrap();
        path
    }

    #[tokio::test]
    async fn classifies_images_pdfs_and_text() {
        let png = temp_file("shot.png", b"\x89PNG\r\n\x1a\n");
        let att = prepare_attachment(png.to_string_lossy().to_string())
            .await
            .unwrap();
        assert_eq!(att.kind, AttachmentKind::Image);
        // Images are referenced in place, not copied
        assert_eq!(att.read_path, att.original_path);
        assert_eq!(att.file_name, "shot.png");

        let pdf = temp_file("report.pdf", b"%PDF-1.7");
        let att = prepare_attachment(pdf.to_string_lossy().to_string())
            .await
            .unwrap();
        assert_eq!(att.kind, AttachmentKind::Pdf);

        let md = temp_file("notes.md", b"# Notes");
        let att = prepare_attachment(md.to_string_lossy().to_string())
            .await
            .unwrap();
        assert_eq!(att.kind, AttachmentKind::Text);
    }

    #[tokio::test]
    async fn sniffs_unknown_extensions() {
        let texty = temp_file("data.weirdext", b"col_a,col_b\n1,2\n");
        let att = prepare_attachment(texty.to_string_lossy().to_string())
            .await
            .unwrap();
        assert_eq!(att.kind, AttachmentKind::Text);

        let binary = temp_file("blob.weirdext", &[0x00, 0x01, 0x02, 0xff, 0xfe]);
        let att = prepare_attachment(binary.to_string_lossy().to_string())
            .await
            .unwrap();
        assert_eq!(att.kind, AttachmentKind::Unsupported);
        assert!(att.note.is_some(), "unsupported files should explain why");
    }

    #[tokio::test]
    async fn unsupported_binary_formats_explain_themselves() {
        // A real .xlsx is a zip; there's no bundled converter for it
        let xlsx = temp_file("sheet.xlsx", b"PK\x03\x04\x00\x00binary");
        let att = prepare_attachment(xlsx.to_string_lossy().to_string())
            .await
            .unwrap();
        assert_eq!(att.kind, AttachmentKind::Unsupported);
        let note = att.note.unwrap();
        assert!(note.contains("XLSX"), "note should name the format: {note}");
    }

    #[tokio::test]
    async fn rejects_directories_and_missing_files() {
        let dir = std::env::temp_dir().join("opcode-attachment-test-dir");
        fs::create_dir_all(&dir).unwrap();
        let err = prepare_attachment(dir.to_string_lossy().to_string())
            .await
            .unwrap_err();
        assert!(err.contains("directory"), "got: {err}");

        let err = prepare_attachment("/definitely/not/here.txt".to_string())
            .await
            .unwrap_err();
        assert!(err.contains("does not exist"), "got: {err}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn text_sniffing_handles_utf8_and_binary() {
        let utf8 = temp_file("u.bin", "héllo — em dash".as_bytes());
        assert!(looks_like_text(&utf8));

        let with_nul = temp_file("n.bin", b"text\x00more");
        assert!(!looks_like_text(&with_nul));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn converts_rtf_to_readable_text() {
        // RTF is a textutil-supported format we can write by hand
        let rtf = temp_file(
            "memo.rtf",
            br"{\rtf1\ansi\deff0 {\fonttbl {\f0 Helvetica;}}\f0\fs24 Quarterly revenue was up.}",
        );

        let att = prepare_attachment(rtf.to_string_lossy().to_string())
            .await
            .unwrap();

        assert_eq!(att.kind, AttachmentKind::Converted);
        assert_ne!(att.read_path, att.original_path);

        let converted = fs::read_to_string(&att.read_path).unwrap();
        assert!(
            converted.contains("Quarterly revenue"),
            "converted text should hold the document's words, got: {converted}"
        );
    }
}
