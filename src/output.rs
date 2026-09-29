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

    pub fn print(&self, json: bool) {
        if json {
            println!("{}", serde_json::to_string_pretty(self).unwrap());
        } else {
            if let Some(ref out) = self.output {
                println!("{}", out);
            }
        }
    }
}
