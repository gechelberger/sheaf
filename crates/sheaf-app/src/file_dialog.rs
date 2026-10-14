//! Use the native picker first, with a standalone Linux picker for WSL/minimal desktops.
use std::path::{Path, PathBuf};

use anyhow::Result;
use futures::channel::oneshot;
use gpui::{App, PathPromptOptions};

type Dialog<T> = oneshot::Receiver<Result<Option<T>>>;

/// Dev: `SHEAF_SCRIPT_PICK` answers every picker with that path, so UI
/// scripts (see script.rs) can get past them.
fn scripted() -> Option<PathBuf> {
    std::env::var_os("SHEAF_SCRIPT_PICK").map(PathBuf::from)
}

fn answer<T>(value: T) -> Dialog<T> {
    let (tx, rx) = oneshot::channel();
    let _ = tx.send(Ok(Some(value)));
    rx
}

pub fn open(cx: &App, options: PathPromptOptions) -> Dialog<Vec<PathBuf>> {
    if let Some(p) = scripted() {
        return answer(vec![p]);
    }
    #[cfg(target_os = "linux")]
    let command = {
        let mut command = linux::command();
        if options.directories && !options.files {
            command.arg("--directory");
        } else {
            command.args([
                "--file-filter=PDF files | *.pdf *.PDF",
                "--file-filter=All files | *",
            ]);
        }
        if options.multiple {
            command.args(["--multiple", "--separator=\n"]);
        }
        if let Some(prompt) = &options.prompt {
            command.arg(format!("--title={prompt}"));
        }
        command
    };
    let rx = cx.prompt_for_paths(options);
    #[cfg(target_os = "linux")]
    return linux::with_fallback(cx, rx, command, |paths| paths);
    #[cfg(not(target_os = "linux"))]
    rx
}

pub fn save(cx: &App, directory: &Path, suggested_name: Option<&str>) -> Dialog<PathBuf> {
    if let Some(p) = scripted() {
        return answer(p);
    }
    let rx = cx.prompt_for_new_path(directory, suggested_name);
    #[cfg(target_os = "linux")]
    {
        let mut command = linux::command();
        command.args(["--save", "--confirm-overwrite"]);
        let mut filename = std::ffi::OsString::from("--filename=");
        filename.push(directory.join(suggested_name.unwrap_or("")));
        if suggested_name.is_none() {
            filename.push("/");
        }
        command.arg(filename);
        linux::with_fallback(cx, rx, command, |mut paths| paths.remove(0))
    }
    #[cfg(not(target_os = "linux"))]
    rx
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use anyhow::{Context, bail};
    use std::os::unix::ffi::OsStringExt;
    use std::process::{Command, Output};

    pub fn command() -> Command {
        let mut command = Command::new("zenity");
        command.arg("--file-selection");
        command
    }

    pub fn with_fallback<T: Send + 'static>(
        cx: &App,
        native: Dialog<T>,
        mut command: Command,
        convert: impl FnOnce(Vec<PathBuf>) -> T + Send + 'static,
    ) -> Dialog<T> {
        let multiple = command.get_args().any(|arg| arg == "--multiple");
        let (tx, rx) = oneshot::channel();
        cx.background_executor().spawn(async move {
            let result = match native.await {
                Ok(Err(error)) => {
                    log::warn!("native file dialog failed; trying Zenity: {error:#}");
                    command.output()
                        .context("could not start Zenity; install zenity (e.g. sudo apt install zenity) or configure xdg-desktop-portal")
                        .and_then(|output| parse_output(output, multiple))
                        .map(|paths| paths.map(convert))
                        .with_context(|| format!("native file dialog failed: {error:#}; fallback failed"))
                }
                Ok(result) => result,
                Err(error) => Err(error.into()),
            };
            if let Err(error) = &result {
                log::error!("file dialog: {error:#}");
            }
            tx.send(result).ok();
        }).detach();
        rx
    }

    fn parse_output(output: Output, multiple: bool) -> Result<Option<Vec<PathBuf>>> {
        match output.status.code() {
            Some(1) => return Ok(None), // Zenity's Cancel/close result.
            Some(0) => {}
            _ => bail!(
                "Zenity failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ),
        }
        // Strip only the output terminator; preserve spaces and non-UTF-8 paths.
        let bytes = output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout);
        if bytes.is_empty() {
            bail!("Zenity returned no selected path");
        }
        Ok(Some(
            bytes
                .split(|b| multiple && *b == b'\n')
                .map(|path| PathBuf::from(std::ffi::OsString::from_vec(path.to_vec())))
                .collect(),
        ))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::process::ExitStatusExt;

        fn output(code: i32, stdout: &[u8]) -> Output {
            Output {
                status: std::process::ExitStatus::from_raw(code << 8),
                stdout: stdout.to_vec(),
                stderr: b"picker error".to_vec(),
            }
        }

        #[test]
        fn selection_preserves_spaces_and_non_utf8() {
            let paths = parse_output(output(0, b"/tmp/a b.pdf\n/tmp/\xff.pdf\n"), true)
                .unwrap()
                .unwrap();
            assert_eq!(
                paths,
                vec![
                    PathBuf::from("/tmp/a b.pdf"),
                    PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/\xff.pdf".to_vec()))
                ]
            );
        }

        #[test]
        fn single_selection_preserves_newlines() {
            assert_eq!(
                parse_output(output(0, b"/tmp/a\nb.pdf\n"), false).unwrap(),
                Some(vec![PathBuf::from("/tmp/a\nb.pdf")])
            );
        }

        #[test]
        fn cancellation_and_failure_are_distinct() {
            assert!(parse_output(output(1, b""), false).unwrap().is_none());
            assert!(parse_output(output(2, b""), false).is_err());
            assert!(parse_output(output(0, b""), false).is_err());
        }
    }
}
