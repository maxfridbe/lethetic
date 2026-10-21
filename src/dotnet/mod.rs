//! Native C# cells. Each ToolRuntime owns a child process; it never shares CLI state.
use crate::tools::ToolExecution;
use std::{path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};
use tokio_util::sync::CancellationToken;

const PROJECT: &str = include_str!("../../runtime/dotnet/Lethetic.Dotnet.csproj");
const HOST: &str = include_str!("../../runtime/dotnet/Program.cs");
const MAX_CODE: usize = 1024 * 1024;
const MAX_REPLY: u64 = 16 * 1024 * 1024;

#[derive(Clone, Default)]
pub struct DotnetRunspace {
    worker: Arc<Mutex<Option<Worker>>>,
}

impl DotnetRunspace {
    pub async fn reset(&self) {
        if let Some(mut worker) = self.worker.lock().await.take() {
            if let Some(pid) = worker.child.id() {
                crate::platform::terminate_process_group(pid, true);
            }
            let _ = worker.child.kill().await;
        }
    }

    pub async fn execute(
        &self,
        code: &str,
        cwd: &str,
        cancellation: CancellationToken,
    ) -> ToolExecution {
        if code.len() > MAX_CODE {
            return ToolExecution::error(
                "ERROR: C# cell exceeds the 1 MiB source limit; save large programs to files.",
                cwd,
            );
        }
        let mut slot = tokio::select! {
            _ = cancellation.cancelled() => return ToolExecution::error("[Operation Cancelled by User]", cwd),
            slot = self.worker.lock() => slot,
        };
        // Move ownership into the future so dropping/cancelling a call kills its worker.
        let existing = slot.take();
        let run = async {
            let mut worker = match existing {
                Some(worker) => worker,
                None => Worker::start(cwd).await?,
            };
            let reply = worker.call(code, cwd).await?;
            Ok::<_, String>((worker, reply))
        };
        let completed = tokio::select! {
            _ = cancellation.cancelled() => Err("[Operation Cancelled by User] .NET runspace reset; files remain.".to_string()),
            result = tokio::time::timeout(Duration::from_secs(120), run) => result.unwrap_or_else(|_| Err("ERROR: .NET cell exceeded 120 seconds; runspace reset; files remain.".to_string())),
        };
        match completed {
            Ok((worker, reply)) => {
                *slot = Some(worker);
                let mut output = reply.stdout;
                if !reply.stderr.is_empty() {
                    output.push_str(&format!("\nstderr:\n{}", reply.stderr));
                }
                if let Some(value) = reply.value.filter(|v| !v.is_empty()) {
                    output.push_str(&format!("\n{value}"));
                }
                for diagnostic in reply.diagnostics {
                    output.push_str(&format!("\n{diagnostic}"));
                }
                if let Some(error) = reply.error {
                    output.push_str(&format!("\nERROR: {error}"));
                }
                if output.trim().is_empty() {
                    output = "(no output)".to_string();
                }
                if reply.success {
                    ToolExecution::success(output, reply.cwd)
                } else {
                    ToolExecution::error(output, reply.cwd)
                }
            }
            Err(error) => ToolExecution::error(
                format!("{error}\n.NET variables were reset; workspace files remain."),
                cwd,
            ),
        }
    }
}

struct BuildDirectory(PathBuf);
impl BuildDirectory {
    fn create() -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!("lethetic-dotnet-{}", uuid::Uuid::new_v4()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&path)
            .map_err(|e| format!("Cannot create .NET host directory: {e}"))?;
        Ok(Self(path))
    }
}
impl Drop for BuildDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    _build: BuildDirectory,
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(pid) = self.child.id() {
            crate::platform::terminate_process_group(pid, true);
        }
    }
}

impl Worker {
    async fn start(cwd: &str) -> Result<Self, String> {
        let build = BuildDirectory::create()?;
        let executable = std::env::var_os("LETHETIC_DOTNET").unwrap_or_else(|| "dotnet".into());
        let configure = || {
            let mut command = Command::new(&executable);
            command
                .current_dir(&build.0)
                .kill_on_drop(true)
                .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
                .env("DOTNET_NOLOGO", "1")
                .stdin(Stdio::null());
            command
        };
        let version = configure().arg("--version").output().await.map_err(|e| format!("Cannot start .NET SDK: {e}. Install SDK 8+ or set LETHETIC_DOTNET to its executable."))?;
        let version = String::from_utf8_lossy(&version.stdout);
        let major = version
            .trim()
            .split('.')
            .next()
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|v| *v >= 8)
            .ok_or("Native .NET mode requires SDK 8 or newer (not only the runtime).")?;
        tokio::fs::write(
            build.0.join("Lethetic.Dotnet.csproj"),
            PROJECT.replace("__SDK_MAJOR__", &major.to_string()),
        )
        .await
        .map_err(|e| e.to_string())?;
        tokio::fs::write(build.0.join("Program.cs"), HOST)
            .await
            .map_err(|e| e.to_string())?;
        tokio::fs::write(
            build.0.join("NuGet.Config"),
            "<configuration><packageSources><clear /></packageSources></configuration>",
        )
        .await
        .map_err(|e| e.to_string())?;
        let output = configure()
            .args([
                "build",
                "Lethetic.Dotnet.csproj",
                "--nologo",
                "-v:q",
                "-o",
                "out",
                "-p:UseSharedCompilation=false",
                "-p:ImportDirectoryBuildProps=false",
                "-p:ImportDirectoryBuildTargets=false",
            ])
            .output()
            .await
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(format!(
                "Cannot compile native .NET host:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let mut command = configure();
        command
            .arg(build.0.join("out/Lethetic.Dotnet.dll"))
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|e| format!("Cannot launch .NET host: {e}"))?;
        let stdin = child.stdin.take().ok_or("Missing .NET stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("Missing .NET stdout")?);
        Ok(Self {
            child,
            stdin,
            stdout,
            _build: build,
        })
    }

    async fn call(&mut self, code: &str, cwd: &str) -> Result<Reply, String> {
        let mut request = serde_json::to_vec(&serde_json::json!({"code":code,"cwd":cwd}))
            .map_err(|e| e.to_string())?;
        request.push(b'\n');
        self.stdin
            .write_all(&request)
            .await
            .map_err(|e| format!(".NET host stopped: {e}"))?;
        self.stdin.flush().await.map_err(|e| e.to_string())?;
        let mut line = Vec::new();
        (&mut self.stdout)
            .take(MAX_REPLY + 1)
            .read_until(b'\n', &mut line)
            .await
            .map_err(|e| e.to_string())?;
        if line.len() as u64 > MAX_REPLY || line.last() != Some(&b'\n') {
            return Err(".NET host stopped or exceeded the reply limit".to_string());
        }
        serde_json::from_slice(&line).map_err(|e| format!("Invalid .NET host reply: {e}"))
    }
}

#[derive(serde::Deserialize)]
struct Reply {
    success: bool,
    stdout: String,
    stderr: String,
    value: Option<String>,
    diagnostics: Vec<String>,
    error: Option<String>,
    cwd: String,
}
