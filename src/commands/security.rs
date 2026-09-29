use crate::error::PdfError;
use crate::output::CommandResult;
use lopdf::{Document, EncryptionState, EncryptionVersion, Object, Permissions, StringFormat};
use std::path::Path;

/// 确保文档 trailer 中存在 /ID（加密算法依赖它）。
fn ensure_file_id(doc: &mut Document) {
    if doc.trailer.get(b"ID").is_ok() {
        return;
    }
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let src = nanos.to_le_bytes();
    let mut id = [0u8; 16];
    for (i, b) in id.iter_mut().enumerate() {
        *b = src[i % src.len()] ^ (i as u8).wrapping_mul(31);
    }
    let obj = Object::String(id.to_vec(), StringFormat::Literal);
    doc.trailer.set("ID", Object::Array(vec![obj.clone(), obj]));
}

pub fn encrypt(
    input: &str,
    user_password: Option<&str>,
    owner_password: Option<&str>,
    output: &str,
    dry_run: bool,
) -> Result<CommandResult, PdfError> {
    if !Path::new(input).exists() {
        return Err(PdfError::FileNotFound {
            path: input.to_string(),
        });
    }
    if user_password.is_none() && owner_password.is_none() {
        return Err(PdfError::MissingArg {
            arg: "password".into(),
            example: "pdf-cli encrypt --input doc.pdf --user-password secret --output locked.pdf"
                .into(),
        });
    }

    if dry_run {
        return Ok(CommandResult::ok(
            "encrypt",
            Some(format!("[dry-run] Would encrypt {} → {}", input, output)),
            serde_json::json!({"dry_run": true, "input": input, "output": output}),
        ));
    }

    let mut doc = Document::load(input).map_err(|e| PdfError::ParseFailed {
        reason: e.to_string(),
    })?;

    ensure_file_id(&mut doc);

    let upwd = user_password.unwrap_or("");
    let opwd = owner_password.unwrap_or(upwd);

    let permissions = Permissions::PRINTABLE
        | Permissions::COPYABLE
        | Permissions::COPYABLE_FOR_ACCESSIBILITY
        | Permissions::PRINTABLE_IN_HIGH_QUALITY;

    let requested_version = EncryptionVersion::V2 {
        document: &doc,
        owner_password: opwd,
        user_password: upwd,
        key_length: 128,
        permissions,
    };

    let state = EncryptionState::try_from(requested_version).map_err(|e| PdfError::Operation {
        reason: format!("{:?}", e),
    })?;

    doc.encrypt(&state).map_err(|e| PdfError::Operation {
        reason: format!("{:?}", e),
    })?;

    doc.save(output).map_err(|e| PdfError::Operation {
        reason: e.to_string(),
    })?;

    Ok(CommandResult::ok(
        "encrypt",
        Some(format!("Encrypted → {}", output)),
        serde_json::json!({"output": output, "user_password_set": user_password.is_some()}),
    ))
}

pub fn decrypt(
    input: &str,
    password: &str,
    output: &str,
    dry_run: bool,
) -> Result<CommandResult, PdfError> {
    if !Path::new(input).exists() {
        return Err(PdfError::FileNotFound {
            path: input.to_string(),
        });
    }

    if dry_run {
        return Ok(CommandResult::ok(
            "decrypt",
            Some(format!("[dry-run] Would decrypt {} → {}", input, output)),
            serde_json::json!({"dry_run": true, "input": input, "output": output}),
        ));
    }

    let mut doc = Document::load(input).map_err(|e| PdfError::ParseFailed {
        reason: e.to_string(),
    })?;

    if doc.is_encrypted() {
        doc.decrypt(password)
            .map_err(|_| PdfError::PasswordRequired)?;
    }

    doc.save(output).map_err(|e| PdfError::Operation {
        reason: e.to_string(),
    })?;

    Ok(CommandResult::ok(
        "decrypt",
        Some(format!("Decrypted → {}", output)),
        serde_json::json!({"output": output}),
    ))
}
