//! `AppleScript` dispatcher.
//!
//! All interaction with MoneyMoney goes through [`OsascriptRunner::run`],
//! which spawns `osascript -e <script>` and returns the raw stdout bytes.
//! [`run_plist`] is the typed helper: it calls the runner and decodes the
//! bytes as a plist into any `Deserialize` type.
//!
//! Errors are classified from the `osascript` stderr output into
//! [`MoneyMoneyError`] variants so callers can react to lock state / missing
//! app / protocol errors without string-matching themselves.

use std::future::Future;
#[cfg(target_os = "macos")]
use std::process::Stdio;
#[cfg(target_os = "macos")]
use std::time::Duration;

use serde::de::DeserializeOwned;
#[cfg(target_os = "macos")]
use tokio::io::{AsyncRead, AsyncReadExt};
#[cfg(target_os = "macos")]
use tokio::process::Command;
#[cfg(target_os = "macos")]
use tokio::time::timeout;
#[cfg(target_os = "macos")]
use tracing::debug;

use crate::moneymoney::MoneyMoneyError;

#[cfg(target_os = "macos")]
const OSASCRIPT_PATH: &str = "/usr/bin/osascript";
#[cfg(target_os = "macos")]
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(120);
#[cfg(target_os = "macos")]
const MAX_STDOUT_BYTES: usize = 64 * 1024 * 1024;
#[cfg(target_os = "macos")]
const MAX_STDERR_BYTES: usize = 1024 * 1024;

/// Encode arbitrary text as an AppleScript string expression.
///
/// Quotes and ASCII control characters become explicit character expressions;
/// backslashes are escaped inside quoted chunks. No caller may interpolate a
/// raw string into a script.
#[must_use]
pub fn string_expression(value: &str) -> String {
    fn flush(parts: &mut Vec<String>, chunk: &mut String) {
        if !chunk.is_empty() {
            parts.push(format!("\"{chunk}\""));
            chunk.clear();
        }
    }

    if value.is_empty() {
        return "\"\"".to_owned();
    }

    let mut parts = Vec::new();
    let mut chunk = String::new();
    for ch in value.chars() {
        match ch {
            '\\' => chunk.push_str("\\\\"),
            '"' => {
                flush(&mut parts, &mut chunk);
                parts.push("(ASCII character 34)".to_owned());
            }
            ch if ch.is_ascii_control() => {
                flush(&mut parts, &mut chunk);
                parts.push(format!("(ASCII character {})", u32::from(ch)));
            }
            ch => chunk.push(ch),
        }
    }
    flush(&mut parts, &mut chunk);
    parts.join(" & ")
}

#[cfg(target_os = "macos")]
struct CapturedOutput {
    bytes: Vec<u8>,
    exceeded: bool,
}

#[cfg(target_os = "macos")]
async fn capture_bounded<R: AsyncRead + Unpin>(
    reader: R,
    limit: usize,
) -> std::io::Result<CapturedOutput> {
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    let mut limited = reader.take(limit.saturating_add(1) as u64);
    limited.read_to_end(&mut bytes).await?;
    let exceeded = bytes.len() > limit;
    bytes.truncate(limit);
    Ok(CapturedOutput { bytes, exceeded })
}

#[cfg(test)]
pub mod mock;

/// Trait wrapping the one-shot `osascript -e <script>` invocation.
///
/// Split out so tests can substitute a mock runner that returns canned plist
/// bytes without spawning any subprocess.
pub trait OsascriptRunner: Send + Sync {
    /// Run a single AppleScript snippet and return its stdout bytes.
    fn run(&self, script: &str) -> impl Future<Output = Result<Vec<u8>, MoneyMoneyError>> + Send;
}

/// Default runner that shells out to `osascript` via `tokio::process`.
#[derive(Debug, Default, Clone, Copy)]
pub struct TokioOsascriptRunner;

#[cfg(target_os = "macos")]
impl OsascriptRunner for TokioOsascriptRunner {
    async fn run(&self, script: &str) -> Result<Vec<u8>, MoneyMoneyError> {
        debug!(bytes = script.len(), "invoking osascript");
        let mut child = Command::new(OSASCRIPT_PATH)
            .arg("-e")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("osascript stdout pipe unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| std::io::Error::other("osascript stderr pipe unavailable"))?;

        let collected = timeout(SCRIPT_TIMEOUT, async {
            let (stdout, stderr, status) = tokio::join!(
                capture_bounded(stdout, MAX_STDOUT_BYTES),
                capture_bounded(stderr, MAX_STDERR_BYTES),
                child.wait()
            );
            Ok::<_, MoneyMoneyError>((stdout?, stderr?, status?))
        })
        .await;

        let (stdout, stderr, status) = if let Ok(result) = collected {
            result?
        } else {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(MoneyMoneyError::ScriptTimeout {
                seconds: SCRIPT_TIMEOUT.as_secs(),
            });
        };
        if stdout.exceeded {
            return Err(MoneyMoneyError::ScriptOutputTooLarge {
                stream: "stdout",
                limit: MAX_STDOUT_BYTES,
            });
        }
        if stderr.exceeded {
            return Err(MoneyMoneyError::ScriptOutputTooLarge {
                stream: "stderr",
                limit: MAX_STDERR_BYTES,
            });
        }
        if !status.success() {
            let stderr = String::from_utf8_lossy(&stderr.bytes).into_owned();
            return Err(classify_stderr(&stderr));
        }
        Ok(stdout.bytes)
    }
}

#[cfg(not(target_os = "macos"))]
impl OsascriptRunner for TokioOsascriptRunner {
    #[allow(clippy::unused_async_trait_impl)] // the trait method is async
    async fn run(&self, _script: &str) -> Result<Vec<u8>, MoneyMoneyError> {
        Err(MoneyMoneyError::NotSupported)
    }
}

/// Run a script and deserialize its stdout as a plist.
pub async fn run_plist<T, R>(runner: &R, script: &str) -> Result<T, MoneyMoneyError>
where
    T: DeserializeOwned,
    R: OsascriptRunner,
{
    let bytes = runner.run(script).await?;
    Ok(plist::from_bytes(&bytes)?)
}

/// Run a script and return its stdout as a trimmed UTF-8 string.
///
/// Use this for AppleScript calls that don't emit plist (e.g., returning a
/// bare version string or a boolean).
pub async fn run_text<R: OsascriptRunner>(
    runner: &R,
    script: &str,
) -> Result<String, MoneyMoneyError> {
    let bytes = runner.run(script).await?;
    let text = String::from_utf8_lossy(&bytes).trim().to_owned();
    Ok(text)
}

/// Map `osascript` stderr to a [`MoneyMoneyError`] variant.
///
/// Recognized patterns (case-sensitive):
///
/// | Pattern | Mapped to |
/// |---|---|
/// | `Locked database` / `(-2720)` | [`DatabaseLocked`](MoneyMoneyError::DatabaseLocked) |
/// | `isn't running` / `-600` | [`NotRunning`](MoneyMoneyError::NotRunning) |
/// | `Can't get application "MoneyMoney"` / `-10814` | [`NotInstalled`](MoneyMoneyError::NotInstalled) |
/// | anything else | [`ScriptError`](MoneyMoneyError::ScriptError) carrying the stderr text |
#[cfg(any(target_os = "macos", test))]
fn classify_stderr(stderr: &str) -> MoneyMoneyError {
    if stderr.contains("Locked database") || stderr.contains("-2720") {
        MoneyMoneyError::DatabaseLocked
    } else if stderr.contains("isn't running")
        || stderr.contains("is not running")
        || stderr.contains("(-600)")
    {
        MoneyMoneyError::NotRunning
    } else if stderr.contains("-10814") || stderr.contains("Can't get application") {
        MoneyMoneyError::NotInstalled
    } else {
        MoneyMoneyError::ScriptError(stderr.trim().to_owned())
    }
}

#[cfg(test)]
mod string_tests {
    use super::string_expression;

    #[test]
    fn encodes_quotes_controls_and_backslashes() {
        assert_eq!(string_expression("plain"), r#""plain""#);
        assert_eq!(
            string_expression(r#"a"b"#),
            r#""a" & (ASCII character 34) & "b""#
        );
        assert_eq!(
            string_expression("line 1\nline 2"),
            r#""line 1" & (ASCII character 10) & "line 2""#
        );
        assert_eq!(string_expression(r"Food\Coffee"), r#""Food\\Coffee""#);
    }

    #[test]
    fn keeps_injection_text_inside_string_chunks() {
        let encoded = string_expression(r#"" & (do shell script "rm -rf /") & ""#);
        let outside_literals = encoded
            .split('"')
            .enumerate()
            .filter_map(|(index, part)| index.is_multiple_of(2).then_some(part))
            .collect::<String>();
        assert!(!outside_literals.contains("do shell script"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn capture_enforces_byte_limit() -> std::io::Result<()> {
        use tokio::io::{AsyncWriteExt as _, duplex};

        let (mut writer, reader) = duplex(16);
        writer.write_all(b"abcdef").await?;
        writer.shutdown().await?;

        let captured = super::capture_bounded(reader, 5).await?;
        assert_eq!(captured.bytes, b"abcde");
        assert!(captured.exceeded);
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::panic, reason = "tests need to panic on unexpected variants")]
mod tests {
    use super::*;

    #[test]
    fn classify_locked_database() {
        let err = classify_stderr(
            "33:48: execution error: MoneyMoney got an error: Locked database. (-2720)",
        );
        assert!(matches!(err, MoneyMoneyError::DatabaseLocked));
    }

    #[test]
    fn classify_not_running() {
        let err = classify_stderr(
            "0:0: execution error: MoneyMoney got an error: Application isn't running. (-600)",
        );
        assert!(matches!(err, MoneyMoneyError::NotRunning));
    }

    #[test]
    fn classify_fallback() {
        let err = classify_stderr("some unexpected text");
        match err {
            MoneyMoneyError::ScriptError(s) => assert_eq!(s, "some unexpected text"),
            other => panic!("expected ScriptError, got {other:?}"),
        }
    }
}
