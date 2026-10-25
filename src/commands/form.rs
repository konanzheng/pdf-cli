//! Implements `pdf-cli form`: AcroForm field listing, value assignment, and flattening.
//!
//! All form handling goes through Pdfium, because only Pdfium knows how the widget
//! annotations are laid out page by page and how the "on"/"off" appearance states of
//! buttons are spelled. Field names therefore come from the raw widget annotations
//! (`/T` on the annotation itself); a field that has no `/T` entry is reported with an
//! empty name and cannot be addressed by `set`.

use std::collections::HashMap;

use pdfium_render::prelude::*;
use serde_json::{json, Value};

use crate::error::PdfError;
use crate::io;
use crate::output::CommandResult;

/// CLI arguments for `pdf-cli form`, mirroring the clap definition in `main.rs`.
pub struct FormArgs<'a> {
    pub input: &'a str,
    pub action: &'a str,
    pub fields: Option<&'a str>,
    pub values: Option<&'a str>,
    pub output: Option<&'a str>,
    pub password: Option<&'a str>,
    pub pdfium_path: Option<&'a str>,
}

/// One logical form field, flattened out of Pdfium's per-widget annotation view.
struct FieldInfo {
    name: String,
    kind: &'static str,
    value: Option<String>,
    required: bool,
    page: u16,
    bbox: [f64; 4],
}

/// Runs the `form` command.
///
/// Three actions are supported:
///
/// * `list` – enumerates every AcroForm field in the document and reports its name, type,
///   current value, required flag, 1-based page number, and widget bounding box. A document
///   without any form fields is *not* an error: it produces `has_form: false` plus an empty
///   `fields` array. `--fields` restricts the listing to the comma-separated names given.
/// * `set` – assigns new values from `--values "name=value;other=value"` (split on `;`, then on
///   the first `=` only, so values may contain `=`). Text fields take the value verbatim
///   (surrounding whitespace preserved), an empty value clears the field, and checkbox fields
///   accept `true/false/1/0/yes/no/on/off` case-insensitively (an empty value unchecks). `--output`
///   is required; the result is written there and every requested assignment is re-read from
///   the saved document before it is reported as changed, so a value that Pdfium refused to
///   store shows up under `mismatches` instead of being claimed as done.
/// * `flatten` – bakes each widget's appearance into the page content so the fields are no
///   longer interactive, writes `--output` (required), and removes the leftover `/AcroForm`
///   dictionary shell so the result carries no form structure at all. Remaining field count is
///   verified after writing and reported in `fields_remaining`.
///
/// Caveat shared by `list` and `set`: field names are read from the page widget annotations,
/// not from the document-level `/AcroForm` tree. Groups such as checkboxes or radio buttons
/// with several widgets collapse into a single reported field keeping the first widget's page
/// and bounding box. Fields without a `/T` name appear with an empty name, and Pdfium's
/// read-only radio/combo/list widgets expose no value setter through `pdfium-render`, so `set`
/// on those types fails with an explanatory error rather than pretending to have written a value.
pub fn run(args: FormArgs<'_>, dry_run: bool) -> Result<CommandResult, PdfError> {
    match args.action.trim() {
        "list" => run_list(&args),
        "set" => run_set(&args, dry_run),
        "flatten" => run_flatten(&args, dry_run),
        other => Err(PdfError::Operation {
            reason: format!(
                "Unknown form action \"{}\". Valid actions: list, set, flatten",
                other
            ),
        }),
    }
}

/// `--action list`: report every AcroForm field found on the document's pages.
fn run_list(args: &FormArgs<'_>) -> Result<CommandResult, PdfError> {
    let pdfium = io::bind_pdfium(args.pdfium_path)?;
    let bytes = io::read_bytes(args.input)?;
    let doc = open_document(&pdfium, &bytes, args.password)?;

    let mut all_fields = collect_fields(&doc)?;

    if let Some(filter) = args.fields {
        let wanted: Vec<&str> = filter
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .collect();

        if !wanted.is_empty() {
            all_fields.retain(|field| wanted.iter().any(|name| *name == field.name));
        }
    }

    let has_form = !all_fields.is_empty();

    let fields: Vec<Value> = all_fields
        .iter()
        .map(|field| {
            json!({
                "name": field.name,
                "type": field.kind,
                "value": field.value,
                "required": field.required,
                "page": field.page,
                "bbox": {
                    "x": field.bbox[0],
                    "y": field.bbox[1],
                    "width": field.bbox[2],
                    "height": field.bbox[3],
                },
            })
        })
        .collect();

    let summary = if has_form {
        format!("{} form field(s) in {}", all_fields.len(), args.input)
    } else {
        format!("No form fields found in {}", args.input)
    };

    Ok(CommandResult::ok(
        "form",
        Some(summary),
        json!({
            "input": args.input,
            "action": "list",
            "has_form": has_form,
            "field_count": all_fields.len(),
            "fields": fields,
        }),
    ))
}

/// `--action set`: write new field values and save the result to `--output`.
fn run_set(args: &FormArgs<'_>, dry_run: bool) -> Result<CommandResult, PdfError> {
    let output = args.output.ok_or_else(|| PdfError::MissingArg {
        arg: "output".to_string(),
        example: "pdf-cli form --input form.pdf --action set --values \"name=Ada;agree=true\" --output out.pdf"
            .to_string(),
    })?;

    let raw_values = args.values.ok_or_else(|| PdfError::MissingArg {
        arg: "values".to_string(),
        example: "pdf-cli form --input form.pdf --action set --values \"name=Ada;agree=true\" --output out.pdf"
            .to_string(),
    })?;

    let assignments = parse_assignments(raw_values)?;

    let pdfium = io::bind_pdfium(args.pdfium_path)?;
    let bytes = io::read_bytes(args.input)?;
    let doc = open_document(&pdfium, &bytes, args.password)?;

    let before = collect_fields(&doc)?;
    if before.is_empty() {
        return Err(PdfError::Operation {
            reason: format!("{} contains no AcroForm fields to set", args.input),
        });
    }

    // Validate every name up front so a typo never leaves us with a half-written document.
    for (name, _) in &assignments {
        if !before.iter().any(|field| &field.name == name) {
            return Err(PdfError::Operation {
                reason: format!(
                    "Unknown form field \"{}\". Available fields: {}",
                    name,
                    available_names(&before)
                ),
            });
        }
    }

    if dry_run {
        let planned: Vec<Value> = assignments
            .iter()
            .map(|(name, value)| {
                json!({
                    "name": name,
                    "requested": value,
                    "type": before
                        .iter()
                        .find(|field| &field.name == name)
                        .map(|field| field.kind)
                        .unwrap_or("unknown"),
                })
            })
            .collect();

        return Ok(CommandResult::ok(
            "form",
            Some(format!(
                "[dry-run] Would set {} form field(s) and write {output}",
                assignments.len()
            )),
            json!({
                "input": args.input,
                "action": "set",
                "output": output,
                "fields_changed": [],
                "planned": planned,
                "dry_run": true,
            }),
        ));
    }

    let mut changed: Vec<String> = Vec::new();
    let mut mismatches: Vec<Value> = Vec::new();

    for (name, raw_value) in &assignments {
        let Some(info) = before.iter().find(|field| &field.name == name) else {
            continue;
        };

        let expected = apply_assignment(&doc, info, raw_value)?;

        // Re-read the field from the document rather than trusting the write.
        let after = collect_fields(&doc)?;
        let actual = after
            .iter()
            .find(|field| &field.name == name)
            .and_then(|field| field.value.clone());

        if values_match(&expected, actual.as_deref()) {
            changed.push(name.clone());
        } else {
            mismatches.push(json!({
                "name": name,
                "requested": raw_value,
                "expected": expected,
                "actual": actual,
            }));
        }
    }

    if changed.is_empty() {
        return Err(PdfError::Operation {
            reason: format!(
                "No form field value could be changed in {}: {}",
                args.input,
                serde_json::to_string(&mismatches).unwrap_or_else(|_| "[]".to_string())
            ),
        });
    }

    doc.save_to_file(output)
        .map_err(|error| PdfError::Operation {
            reason: format!("Failed to save {output}: {error}"),
        })?;

    let summary = if mismatches.is_empty() {
        format!("Updated {} form field(s) → {}", changed.len(), output)
    } else {
        format!(
            "Updated {} form field(s) → {} ({} could not be written)",
            changed.len(),
            output,
            mismatches.len()
        )
    };

    Ok(CommandResult::ok(
        "form",
        Some(summary),
        json!({
            "input": args.input,
            "action": "set",
            "output": output,
            "fields_changed": changed,
            "mismatches": mismatches,
            "dry_run": false,
        }),
    ))
}

/// `--action flatten`: bake widget appearances into the page content and drop the form tree.
fn run_flatten(args: &FormArgs<'_>, dry_run: bool) -> Result<CommandResult, PdfError> {
    let output = args.output.ok_or_else(|| PdfError::MissingArg {
        arg: "output".to_string(),
        example: "pdf-cli form --input form.pdf --action flatten --output flat.pdf".to_string(),
    })?;

    let pdfium = io::bind_pdfium(args.pdfium_path)?;
    let bytes = io::read_bytes(args.input)?;

    let field_names: Vec<String> = {
        let doc = open_document(&pdfium, &bytes, args.password)?;
        let before = collect_fields(&doc)?;
        let names: Vec<String> = before.iter().map(|field| field.name.clone()).collect();

        if dry_run {
            return Ok(CommandResult::ok(
                "form",
                Some(format!(
                    "[dry-run] Would flatten {} form field(s) and write {output}",
                    names.len()
                )),
                json!({
                    "input": args.input,
                    "action": "flatten",
                    "output": output,
                    "fields_changed": names,
                    "dry_run": true,
                }),
            ));
        }

        for page_index in 0..doc.pages().len() {
            let mut page = doc
                .pages()
                .get(page_index)
                .map_err(|error| PdfError::Operation {
                    reason: format!("Could not read page {}: {}", page_index + 1, error),
                })?;

            page.flatten().map_err(|error| PdfError::Operation {
                reason: format!(
                    "Could not flatten form fields on page {}: {}",
                    page_index + 1,
                    error
                ),
            })?;
        }

        doc.save_to_file(output)
            .map_err(|error| PdfError::Operation {
                reason: format!("Failed to save {output}: {error}"),
            })?;

        names
    };

    // Pdfium removes the widgets from the pages, but the document-level /AcroForm shell can
    // survive; strip it so the output carries no form structure at all.
    let acro_form_stripped = strip_acro_form(output)?;
    let remaining = count_fields_in_file(&pdfium, output, args.password);

    let summary = match remaining {
        Some(0) => format!(
            "Flattened {} form field(s) → {} (no interactive fields remain)",
            field_names.len(),
            output
        ),
        Some(left) => format!(
            "Flattened {} form field(s) → {} (warning: {left} field(s) still reported)",
            field_names.len(),
            output
        ),
        None => format!("Flattened {} form field(s) → {}", field_names.len(), output),
    };

    Ok(CommandResult::ok(
        "form",
        Some(summary),
        json!({
            "input": args.input,
            "action": "flatten",
            "output": output,
            "fields_changed": field_names,
            "acro_form_removed": acro_form_stripped,
            "fields_remaining": remaining,
            "dry_run": false,
        }),
    ))
}

/// Applies a single `name=value` assignment and returns the value the field should now hold.
fn apply_assignment(
    doc: &PdfDocument<'_>,
    info: &FieldInfo,
    raw_value: &str,
) -> Result<String, PdfError> {
    match info.kind {
        "text" => {
            let applied = write_field(doc, &info.name, |field| {
                let Some(text) = field.as_text_field_mut() else {
                    return Ok(false);
                };

                text.set_value(raw_value).map_err(|error| PdfError::Operation {
                    reason: format!(
                        "Could not set text field \"{}\": {}",
                        info.name, error
                    ),
                })?;

                Ok(true)
            })?;

            if !applied {
                return Err(PdfError::Operation {
                    reason: format!(
                        "Field \"{}\" is reported as a text field but has no writable text widget",
                        info.name
                    ),
                });
            }

            Ok(raw_value.to_string())
        }
        "checkbox" => {
            // An empty value clears the box, which is the `name=` convention.
            let desired = if raw_value.trim().is_empty() {
                false
            } else {
                match parse_bool(raw_value) {
                    Some(value) => value,
                    None => {
                        return Err(PdfError::Operation {
                            reason: format!(
                                "Invalid value \"{}\" for checkbox \"{}\". Accepted values: true, false, 1, 0, yes, no, on, off",
                                raw_value, info.name
                            ),
                        });
                    }
                }
            };

            let applied = write_field(doc, &info.name, |field| {
                let Some(checkbox) = field.as_checkbox_field_mut() else {
                    return Ok(false);
                };

                checkbox
                    .set_checked(desired)
                    .map_err(|error| PdfError::Operation {
                        reason: format!(
                            "Could not set checkbox \"{}\": {}",
                            info.name, error
                        ),
                    })?;

                Ok(true)
            })?;

            if !applied {
                return Err(PdfError::Operation {
                    reason: format!(
                        "Field \"{}\" is reported as a checkbox but has no writable widget",
                        info.name
                    ),
                });
            }

            Ok(if desired { "true" } else { "false" }.to_string())
        }
        "radio" => Err(PdfError::Operation {
            reason: format!(
                "Radio button group \"{}\" cannot be set: pdfium-render exposes no value setter for radio widgets, so the change would not stick",
                info.name
            ),
        }),
        "combo" => Err(PdfError::Operation {
            reason: format!(
                "Combo box \"{}\" cannot be set: pdfium-render exposes no value setter for choice fields",
                info.name
            ),
        }),
        "list" => Err(PdfError::Operation {
            reason: format!(
                "List box \"{}\" cannot be set: pdfium-render exposes no value setter for choice fields",
                info.name
            ),
        }),
        "button" => Err(PdfError::Operation {
            reason: format!(
                "Push button \"{}\" has no value to set; it only triggers an action",
                info.name
            ),
        }),
        "signature" => Err(PdfError::Operation {
            reason: format!(
                "Signature field \"{}\" cannot be filled through this command",
                info.name
            ),
        }),
        _ => Err(PdfError::Operation {
            reason: format!(
                "Field \"{}\" has an unsupported field type and cannot be set",
                info.name
            ),
        }),
    }
}

/// Walks every widget annotation in the document, calling `apply` on the matching fields.
///
/// Returns `true` when at least one widget was handed to `apply`.
fn write_field<F>(doc: &PdfDocument<'_>, name: &str, mut apply: F) -> Result<bool, PdfError>
where
    F: FnMut(&mut PdfFormField<'_>) -> Result<bool, PdfError>,
{
    let mut touched = false;

    for page_index in 0..doc.pages().len() {
        let page = doc
            .pages()
            .get(page_index)
            .map_err(|error| PdfError::Operation {
                reason: format!("Could not read page {}: {}", page_index + 1, error),
            })?;

        let annotations = page.annotations();

        for annotation_index in 0..annotations.len() {
            let mut annotation =
                annotations
                    .get(annotation_index)
                    .map_err(|error| PdfError::Operation {
                        reason: format!(
                            "Could not read annotation {} on page {}: {}",
                            annotation_index + 1,
                            page_index + 1,
                            error
                        ),
                    })?;

            let matches = match annotation.as_form_field() {
                Some(field) => field.name().as_deref() == Some(name),
                None => false,
            };

            if !matches {
                continue;
            }

            let Some(field) = annotation.as_form_field_mut() else {
                continue;
            };

            if apply(field)? {
                touched = true;
            }
        }
    }

    Ok(touched)
}

/// Collects every widget-backed form field in the document, collapsing repeated widget names.
fn collect_fields(doc: &PdfDocument<'_>) -> Result<Vec<FieldInfo>, PdfError> {
    let mut fields: Vec<FieldInfo> = Vec::new();
    let mut index_of: HashMap<String, usize> = HashMap::new();

    for page_index in 0..doc.pages().len() {
        let page = doc
            .pages()
            .get(page_index)
            .map_err(|error| PdfError::Operation {
                reason: format!("Could not read page {}: {}", page_index + 1, error),
            })?;

        let annotations = page.annotations();

        for annotation_index in 0..annotations.len() {
            let annotation =
                annotations
                    .get(annotation_index)
                    .map_err(|error| PdfError::Operation {
                        reason: format!(
                            "Could not read annotation {} on page {}: {}",
                            annotation_index + 1,
                            page_index + 1,
                            error
                        ),
                    })?;

            let Some(field) = annotation.as_form_field() else {
                continue;
            };

            let name = field.name().unwrap_or_default();
            let kind = field_kind(field);
            let value = field_value(field);
            let required = field.is_required();

            let bbox = match annotation.bounds() {
                Ok(rect) => [
                    round2(rect.left().value),
                    round2(rect.bottom().value),
                    round2(rect.width().value),
                    round2(rect.height().value),
                ],
                Err(_) => [0.0, 0.0, 0.0, 0.0],
            };

            match index_of.get(&name).copied() {
                Some(existing) => {
                    // Multi-widget groups (checkboxes, radio buttons) share one name; keep the
                    // first widget's position but prefer the state that says "selected".
                    if let Some(previous) = fields.get_mut(existing) {
                        if is_selected(previous.value.as_deref()) && !is_selected(value.as_deref())
                        {
                            continue;
                        }

                        previous.value = value;
                    }
                }
                None => {
                    index_of.insert(name.clone(), fields.len());
                    fields.push(FieldInfo {
                        name,
                        kind,
                        value,
                        required,
                        page: page_index + 1,
                        bbox,
                    });
                }
            }
        }
    }

    Ok(fields)
}

/// Returns `true` when a rendered field value represents a selected button.
fn is_selected(value: Option<&str>) -> bool {
    matches!(value, Some(text) if !text.eq_ignore_ascii_case("false") && !text.is_empty())
}

/// Maps Pdfium's field type onto the stable names used in the JSON output.
fn field_kind(field: &PdfFormField<'_>) -> &'static str {
    match field.field_type() {
        PdfFormFieldType::PushButton => "button",
        PdfFormFieldType::Checkbox => "checkbox",
        PdfFormFieldType::RadioButton => "radio",
        PdfFormFieldType::ComboBox => "combo",
        PdfFormFieldType::ListBox => "list",
        PdfFormFieldType::Signature => "signature",
        PdfFormFieldType::Text => "text",
        _ => "unknown",
    }
}

/// Reads the current value of a field in the same string form the JSON output reports.
fn field_value(field: &PdfFormField<'_>) -> Option<String> {
    match field.field_type() {
        PdfFormFieldType::Text => field.as_text_field().and_then(|text| text.value()),
        PdfFormFieldType::Checkbox => field
            .as_checkbox_field()
            .map(|checkbox| bool_text(matches!(checkbox.is_checked(), Ok(true)))),
        PdfFormFieldType::RadioButton => field.as_radio_button_field().map(|radio| {
            if matches!(radio.is_checked(), Ok(true)) {
                radio.group_value().unwrap_or_else(|| "true".to_string())
            } else {
                "false".to_string()
            }
        }),
        PdfFormFieldType::ComboBox => field.as_combo_box_field().and_then(|combo| combo.value()),
        PdfFormFieldType::ListBox => field
            .as_list_box_field()
            .and_then(|list_box| list_box.value()),
        _ => None,
    }
}

/// Formats a boolean field state.
fn bool_text(value: bool) -> String {
    if value { "true" } else { "false" }.to_string()
}

/// Compares an expected field value against the value read back from the document.
///
/// Pdfium reports a cleared text or choice field as `None` rather than as `Some("")`, so an
/// empty expectation is satisfied by a missing value.
fn values_match(expected: &str, actual: Option<&str>) -> bool {
    match actual {
        Some(value) => value == expected,
        None => expected.is_empty(),
    }
}

/// Parses the boolean spellings accepted for checkbox fields.
fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Rounds a Pdfium coordinate to two decimals for readable JSON.
fn round2(value: f32) -> f64 {
    ((value as f64) * 100.0).round() / 100.0
}

/// Builds a short, comma-separated list of field names for error messages.
fn available_names(fields: &[FieldInfo]) -> String {
    const LIMIT: usize = 20;

    if fields.is_empty() {
        return "(none)".to_string();
    }

    let shown: Vec<&str> = fields
        .iter()
        .take(LIMIT)
        .map(|field| field.name.as_str())
        .collect();

    let mut text = shown.join(", ");

    if fields.len() > LIMIT {
        text.push_str(&format!(", ... ({} more)", fields.len() - LIMIT));
    }

    text
}

/// Splits `--values` into `(name, value)` pairs on `;` and the first `=` of each entry.
fn parse_assignments(raw: &str) -> Result<Vec<(String, String)>, PdfError> {
    let mut assignments: Vec<(String, String)> = Vec::new();

    for entry in raw.split(';') {
        if entry.trim().is_empty() {
            continue;
        }

        let Some((name, value)) = entry.split_once('=') else {
            return Err(PdfError::Operation {
                reason: format!(
                    "Invalid --values entry \"{}\": expected name=value",
                    entry.trim()
                ),
            });
        };

        let name = name.trim();
        if name.is_empty() {
            return Err(PdfError::Operation {
                reason: format!(
                    "Invalid --values entry \"{}\": field name is empty",
                    entry.trim()
                ),
            });
        }

        assignments.push((name.to_string(), value.to_string()));
    }

    if assignments.is_empty() {
        return Err(PdfError::Operation {
            reason: "No field=value pairs were provided in --values".to_string(),
        });
    }

    Ok(assignments)
}

/// Opens a document from an in-memory buffer, mapping Pdfium errors onto CLI errors.
fn open_document<'a>(
    pdfium: &'a Pdfium,
    bytes: &'a [u8],
    password: Option<&str>,
) -> Result<PdfDocument<'a>, PdfError> {
    pdfium
        .load_pdf_from_byte_slice(bytes, password)
        .map_err(map_load_error)
}

/// Translates a Pdfium load failure into the CLI's error vocabulary.
fn map_load_error(error: PdfiumError) -> PdfError {
    match error {
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
            PdfError::PasswordRequired
        }
        other => PdfError::ParseFailed {
            reason: other.to_string(),
        },
    }
}

/// Removes a leftover `/AcroForm` entry from the saved catalog.
///
/// Returns `true` when the entry was present and has been removed.
fn strip_acro_form(path: &str) -> Result<bool, PdfError> {
    let mut doc = io::load_doc_path(path)?;

    let present = doc
        .catalog()
        .map(|catalog| catalog.has(b"AcroForm"))
        .unwrap_or(false);

    if !present {
        return Ok(false);
    }

    doc.catalog_mut()
        .map_err(|error| PdfError::Operation {
            reason: format!("Could not read the document catalog of {path}: {error}"),
        })?
        .remove(b"AcroForm");

    doc.save(path).map_err(|error| PdfError::Operation {
        reason: format!("Failed to finalize {path}: {error}"),
    })?;

    Ok(true)
}

/// Re-opens a written document and counts the form fields that are still interactive.
///
/// Returns `None` when the file cannot be re-read; that is only reported, never fatal.
fn count_fields_in_file(pdfium: &Pdfium, path: &str, password: Option<&str>) -> Option<usize> {
    let bytes = io::read_bytes(path).ok()?;
    let doc = open_document(pdfium, &bytes, password).ok()?;

    collect_fields(&doc).ok().map(|fields| fields.len())
}
