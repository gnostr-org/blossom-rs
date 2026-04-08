//! Git command wrapper for stateless-rpc operations.
//!
//! Spawns `git upload-pack` and `git receive-pack` as child processes,
//! piping request/response bodies through stdin/stdout.

use std::path::Path;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Wrapper around the git CLI for HTTP smart protocol operations.
pub struct GitCommand<'a> {
    git_path: &'a str,
    repo_path: &'a Path,
}

impl<'a> GitCommand<'a> {
    pub fn new(git_path: &'a str, repo_path: &'a Path) -> Self {
        Self {
            git_path,
            repo_path,
        }
    }

    /// Advertise refs for a service (git-upload-pack or git-receive-pack).
    pub async fn refs(&self, service: &str, v2: bool) -> Result<Vec<u8>, &'static str> {
        let service_cmd = service.strip_prefix("git-").unwrap_or(service);

        let mut cmd = self.build_command(service_cmd, v2);
        cmd.arg("--advertise-refs").arg(".");

        let output = cmd.output().await.map_err(|_| "failed to run git")?;

        if output.status.success() {
            Ok(output.stdout)
        } else {
            tracing::error!(
                stderr = %String::from_utf8_lossy(&output.stderr),
                "git --advertise-refs failed"
            );
            Err("git advertise-refs failed")
        }
    }

    /// Execute git-upload-pack (fetch/clone).
    pub async fn upload_pack(&self, body: &[u8], v2: bool) -> Result<Vec<u8>, &'static str> {
        self.run_stateless_rpc("upload-pack", body, v2).await
    }

    /// Execute git-receive-pack (push).
    pub async fn receive_pack(&self, body: &[u8]) -> Result<Vec<u8>, &'static str> {
        self.run_stateless_rpc("receive-pack", body, false).await
    }

    async fn run_stateless_rpc(
        &self,
        service: &str,
        body: &[u8],
        v2: bool,
    ) -> Result<Vec<u8>, &'static str> {
        let mut cmd = self.build_command(service, v2);
        cmd.arg("--stateless-rpc")
            .arg(".")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn().map_err(|_| "failed to spawn git")?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(body)
                .await
                .map_err(|_| "failed to write to git stdin")?;
            drop(stdin);
        }

        let output = child
            .wait_with_output()
            .await
            .map_err(|_| "failed to read git output")?;

        if output.status.success() {
            Ok(output.stdout)
        } else {
            tracing::error!(
                service = service,
                stderr = %String::from_utf8_lossy(&output.stderr),
                "git stateless-rpc failed"
            );
            Err("git command failed")
        }
    }

    fn build_command(&self, service: &str, v2: bool) -> Command {
        let mut cmd = Command::new(self.git_path);
        cmd.current_dir(self.repo_path);

        // Allow fetching any reachable SHA1
        cmd.arg("-c")
            .arg("uploadpack.allowTipSHA1InWant=true")
            .arg("-c")
            .arg("uploadpack.allowReachableSHA1InWant=true");

        cmd.arg(service);

        if v2 {
            cmd.env("GIT_PROTOCOL", "version=2");
        }

        cmd
    }
}
