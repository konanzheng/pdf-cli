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

    #[error("Incorrect password for: {path}\n  → Verify the password, or use decrypt --password <pwd> first")]
    WrongPassword { path: String },

    #[error("Operation failed: {reason}")]
    Operation { reason: String },

    #[error("Invalid page range: {range}\n  → Example: --pages 1-3,5,7-9")]
    InvalidPageRange { range: String },

    #[error("Invalid page range \"{spec}\": {reason}\n  → Example: --pages 1-3,5,7-9 (pages are 1-based)")]
    InvalidPageRangeReason { spec: String, reason: String },

    #[error("Missing required argument: {arg}\n  → Example: {example}")]
    MissingArg { arg: String, example: String },

    #[error("Browser not found: {reason}\n  → Install Microsoft Edge, or pass --edge-path \"C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe\"")]
    BrowserNotFound { reason: String },

    #[error("pdfium library not found: {reason}\n  → Download pdfium from https://github.com/bblanchon/pdfium-binaries/releases and pass --pdfium-path <path to pdfium.dll>")]
    PdfiumNotFound { reason: String },
}

impl PdfError {
    /// Machine-readable error object. `message` keeps the human text (including the
    /// `→ hint` suffix) and `hint` exposes the same hint as a separate field so an
    /// agent can branch on it without string surgery.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "error": true,
            "code": self.code(),
            "message": self.to_string(),
            "hint": self.hint(),
        })
    }

    pub fn code(&self) -> &str {
        match self {
            PdfError::FileNotFound { .. } => "FILE_NOT_FOUND",
            PdfError::UnsupportedFormat { .. } => "UNSUPPORTED_FORMAT",
            PdfError::ParseFailed { .. } => "PARSE_FAILED",
            PdfError::PasswordRequired => "PASSWORD_REQUIRED",
            PdfError::WrongPassword { .. } => "WRONG_PASSWORD",
            PdfError::Operation { .. } => "OPERATION_FAILED",
            PdfError::InvalidPageRange { .. } => "INVALID_PAGE_RANGE",
            PdfError::InvalidPageRangeReason { .. } => "INVALID_PAGE_RANGE",
            PdfError::MissingArg { .. } => "MISSING_ARG",
            PdfError::BrowserNotFound { .. } => "BROWSER_NOT_FOUND",
            PdfError::PdfiumNotFound { .. } => "PDFIUM_NOT_FOUND",
        }
    }

    /// A single-line, actionable next step for the caller (empty when none).
    pub fn hint(&self) -> String {
        match self {
            PdfError::FileNotFound { path } => {
                format!("Verify the path exists: {path}")
            }
            PdfError::UnsupportedFormat { .. } => {
                "Supported input formats: jpg, png, bmp, webp, svg, md, txt".into()
            }
            PdfError::ParseFailed { .. } => {
                "Run `pdf-cli info --input <file>` to inspect the document; the file may be corrupt".into()
            }
            PdfError::PasswordRequired => "Re-run with --password <pwd>".into(),
            PdfError::WrongPassword { .. } => {
                "The password is incorrect; try decrypt with the owner password first".into()
            }
            PdfError::Operation { .. } => String::new(),
            PdfError::InvalidPageRange { .. } | PdfError::InvalidPageRangeReason { .. } => {
                "Pages are 1-based; use forms like 1-3,5,7-9 or 3- for to-end".into()
            }
            PdfError::MissingArg { example, .. } => example.clone(),
            PdfError::BrowserNotFound { .. } => {
                "Install Microsoft Edge or pass --edge-path to msedge.exe".into()
            }
            PdfError::PdfiumNotFound { .. } => {
                "Pass --pdfium-path <path to pdfium.dll>, or place pdfium.dll next to pdf-cli.exe".into()
            }
        }
    }
}
