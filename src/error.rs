use thiserror::Error;

#[derive(Error, Debug)]
pub enum PdfError {
    #[error("File not found: {path}\n  → Check the path and try again")]
    FileNotFound { path: String },

    #[error("Unsupported format: {format}\n  → Supported: jpg, png, bmp, webp, svg, md, txt")]
    UnsupportedFormat { format: String },

    #[error(
        "PDF parsing failed: {reason}\n  → The file may be corrupted. Try: pdf-cli info <file>"
    )]
    ParseFailed { reason: String },

    #[error("Encrypted PDF requires password\n  → Use: --password <pwd>")]
    PasswordRequired,

    #[error("Operation failed: {reason}")]
    Operation { reason: String },

    #[error("Invalid page range: {range}\n  → Example: --pages 1-3,5,7-9")]
    InvalidPageRange { range: String },

    #[error("Missing required argument: {arg}\n  → Example: {example}")]
    MissingArg { arg: String, example: String },

    #[error("Browser not found: {reason}\n  → Install Microsoft Edge, or pass --edge-path \"C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe\"")]
    BrowserNotFound { reason: String },
}

impl PdfError {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "error": true,
            "code": self.code(),
            "message": self.to_string(),
        })
    }

    fn code(&self) -> &str {
        match self {
            PdfError::FileNotFound { .. } => "FILE_NOT_FOUND",
            PdfError::UnsupportedFormat { .. } => "UNSUPPORTED_FORMAT",
            PdfError::ParseFailed { .. } => "PARSE_FAILED",
            PdfError::PasswordRequired => "PASSWORD_REQUIRED",
            PdfError::Operation { .. } => "OPERATION_FAILED",
            PdfError::InvalidPageRange { .. } => "INVALID_PAGE_RANGE",
            PdfError::MissingArg { .. } => "MISSING_ARG",
            PdfError::BrowserNotFound { .. } => "BROWSER_NOT_FOUND",
        }
    }
}
