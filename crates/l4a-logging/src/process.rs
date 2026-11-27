//! Audited external process execution.
//!
//! ```ignore
//! let audit = CommandAudit::start("filter.deploy.git", cmd.as_std());
//! let result = cmd.output().await;
//! audit.finish_output(&result);
//! ```
//!
//! Records the full (redacted) command line, working directory, exit code, duration and a tail
//! of stderr under the `l4a::process` target. Environment variable values are never logged.

use crate::redact::{error_chain, format_command_line, truncate};
use std::process::{ExitStatus, Output};
use std::time::Instant;

/// Log target used for all external command records.
pub const PROCESS_TARGET: &str = "l4a::process";

/// Maximum number of characters of stdout/stderr kept in the completion record.
pub const OUTPUT_TAIL_CHARS: usize = 2000;

/// Describes a command as a redacted single-line string.
pub fn describe_command(cmd: &std::process::Command) -> String {
    let program = cmd.get_program().to_string_lossy();
    let args: Vec<String> = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    format_command_line(&program, args)
}

/// Returns the last `max_chars` characters of process output, trimmed.
pub fn output_tail(bytes: &[u8], max_chars: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim();
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let tail: String = text.chars().skip(count - max_chars).collect();
    format!("…{tail}")
}

/// Tracks a single external command from start to completion.
#[must_use = "call one of the finish_* methods to record the command result"]
pub struct CommandAudit {
    purpose: String,
    command: String,
    started: Instant,
    quiet: bool,
}

/// Emits at DEBUG for quiet audits, otherwise at the given level.
macro_rules! audit_event {
    ($self:ident, $level:ident, $($rest:tt)+) => {
        if $self.quiet {
            tracing::debug!(target: PROCESS_TARGET, $($rest)+)
        } else {
            tracing::$level!(target: PROCESS_TARGET, $($rest)+)
        }
    };
}

impl CommandAudit {
    /// Logs the command line (INFO) and starts the timer.
    pub fn start(purpose: impl Into<String>, cmd: &std::process::Command) -> Self {
        let purpose = purpose.into();
        let command = describe_command(cmd);
        let cwd = cmd
            .get_current_dir()
            .map(|dir| dir.display().to_string())
            .unwrap_or_default();
        let env_keys: Vec<String> = cmd
            .get_envs()
            .map(|(key, _)| key.to_string_lossy().into_owned())
            .collect();
        tracing::info!(
            target: PROCESS_TARGET,
            purpose = %purpose,
            command = %command,
            cwd = %cwd,
            env_keys = %env_keys.join(","),
            "Running external command"
        );
        Self {
            purpose,
            command,
            started: Instant::now(),
            quiet: false,
        }
    }

    /// Same as [`CommandAudit::start`] but every record is logged at DEBUG, for frequent
    /// environment probes (`git --version`, Python discovery) that would otherwise flood logs.
    pub fn start_quiet(purpose: impl Into<String>, cmd: &std::process::Command) -> Self {
        let purpose = purpose.into();
        let command = describe_command(cmd);
        tracing::debug!(target: PROCESS_TARGET, purpose = %purpose, command = %command, "Running external command");
        Self {
            purpose,
            command,
            started: Instant::now(),
            quiet: true,
        }
    }

    fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// Records the result of `Command::output()`.
    pub fn finish_output(self, result: &std::io::Result<Output>) {
        match result {
            Ok(output) => self.record_output(output),
            Err(err) => self.finish_error(err),
        }
    }

    /// Records a captured process output.
    pub fn record_output(self, output: &Output) {
        let elapsed_ms = self.elapsed_ms();
        let code = output.status.code();
        if output.status.success() {
            audit_event!(
                self, info,
                purpose = %self.purpose,
                command = %self.command,
                exit_code = code,
                elapsed_ms,
                stdout_bytes = output.stdout.len(),
                stderr_bytes = output.stderr.len(),
                "External command succeeded"
            );
            if !output.stderr.is_empty() {
                tracing::debug!(
                    target: PROCESS_TARGET,
                    purpose = %self.purpose,
                    stderr = %output_tail(&output.stderr, OUTPUT_TAIL_CHARS),
                    "External command stderr"
                );
            }
        } else {
            audit_event!(
                self, warn,
                purpose = %self.purpose,
                command = %self.command,
                exit_code = code,
                elapsed_ms,
                stderr = %output_tail(&output.stderr, OUTPUT_TAIL_CHARS),
                stdout = %output_tail(&output.stdout, OUTPUT_TAIL_CHARS / 4),
                "External command failed"
            );
        }
    }

    /// Records the result of `Command::status()`.
    pub fn finish_status(self, result: &std::io::Result<ExitStatus>) {
        let elapsed_ms = self.elapsed_ms();
        match result {
            Ok(status) if status.success() => audit_event!(
                self, info,
                purpose = %self.purpose,
                command = %self.command,
                exit_code = status.code(),
                elapsed_ms,
                "External command succeeded"
            ),
            Ok(status) => audit_event!(
                self, warn,
                purpose = %self.purpose,
                command = %self.command,
                exit_code = status.code(),
                elapsed_ms,
                "External command failed"
            ),
            Err(err) => self.finish_error(err),
        }
    }

    /// Records that a long-running child process was spawned (it keeps running afterwards).
    pub fn finish_spawned(self, pid: Option<u32>) {
        audit_event!(
            self, info,
            purpose = %self.purpose,
            command = %self.command,
            pid,
            elapsed_ms = self.elapsed_ms(),
            "External process spawned"
        );
    }

    /// Records a spawn/wait failure, timeout or cancellation.
    pub fn finish_error(self, err: &(dyn std::error::Error + 'static)) {
        audit_event!(
            self, warn,
            purpose = %self.purpose,
            command = %self.command,
            elapsed_ms = self.elapsed_ms(),
            error = %truncate(&error_chain(err), OUTPUT_TAIL_CHARS),
            "External command did not complete"
        );
    }

    /// Records a failure described only by a message (e.g. timeout).
    pub fn finish_message(self, message: &str) {
        audit_event!(
            self, warn,
            purpose = %self.purpose,
            command = %self.command,
            elapsed_ms = self.elapsed_ms(),
            error = %message,
            "External command did not complete"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_command_with_redaction() {
        let mut cmd = std::process::Command::new("python");
        cmd.args([
            "-m",
            "pip",
            "install",
            "--index-url",
            "https://u:p@pypi.example/simple",
        ]);
        cmd.env("L4A_RUNNER_TOKEN", "super-secret");
        let text = describe_command(&cmd);
        assert_eq!(
            text,
            "python -m pip install --index-url https://***@pypi.example/simple"
        );
        assert!(!text.contains("super-secret"));
    }

    #[test]
    fn output_tail_keeps_the_end() {
        assert_eq!(output_tail(b"  short \n", 10), "short");
        assert_eq!(output_tail(b"abcdefghij", 3), "…hij");
    }
}
