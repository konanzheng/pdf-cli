use serde::Serialize;

#[derive(Serialize)]
pub struct CommandResult {
    pub success: bool,
    pub command: String,
    pub output: Option<String>,
    pub details: serde_json::Value,
}

impl CommandResult {
    pub fn ok(command: &str, output: Option<String>, details: serde_json::Value) -> Self {
        Self {
            success: true,
            command: command.to_string(),
            output,
            details,
        }
    }

    /// Emit this result.
    ///
    /// * `json`       — one pretty-printed JSON object (the default agent mode).
    /// * `json_lines` — one compact JSON object on a single line, so a batch of
    ///                  results can be piped into a line-oriented consumer.
    ///                  Takes precedence over `json`.
    /// * `quiet`      — suppress the human-readable summary. Machine output
    ///                  (JSON) is never suppressed, because suppressing it would
    ///                  leave the caller with no result at all.
    pub fn print(&self, json: bool, quiet: bool, json_lines: bool) {
        if json_lines {
            match serde_json::to_string(self) {
                Ok(s) => println!("{s}"),
                Err(e) => eprintln!("Failed to serialise result: {e}"),
            }
        } else if json {
            match serde_json::to_string_pretty(self) {
                Ok(s) => println!("{s}"),
                Err(e) => eprintln!("Failed to serialise result: {e}"),
            }
        } else if !quiet {
            if let Some(ref out) = self.output {
                println!("{out}");
            }
        }
    }
}
