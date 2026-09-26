use super::*;

impl App {
    #[cfg(target_os = "linux")]
    pub async fn bind_managed_python_session(
        &mut self,
        workspace: crate::python::runtime_store::WorkspaceIdentity,
        runtime_id: Option<String>,
    ) -> Result<(), String> {
        use crate::config::PythonWorkspaceExposure;

        workspace.verify_current()?;
        let shared_workspace = if self.config.python_invocation.workspace_exposure
            == PythonWorkspaceExposure::SharedLaunchCwd
        {
            let current = crate::python::runtime_store::WorkspaceIdentity::capture(
                self.tool_runtime.workspace_root(),
            )?;
            match &self.shared_python_workspace {
                Some(stored) => {
                    let expected = stored.to_runtime_identity()?;
                    if current != expected {
                        return Err(format!(
                            "Retained session workspace mismatch: session is bound to {}; Lethetic was launched from {}. Resume from the original directory or create a new session.",
                            expected.canonical_path.display(),
                            current.canonical_path.display(),
                        ));
                    }
                }
                None if self.python_runtime_id.is_some() => {
                    return Err(
                        "existing managed Python runtime cannot be silently converted to shared launch-cwd mode; create a new session"
                            .to_string(),
                    );
                }
                None => {}
            }
            Some(current)
        } else {
            if let Some(stored) = &self.shared_python_workspace {
                return Err(format!(
                    "this retained session is bound to shared cwd {}; resume it with the same literal Python flag from that directory",
                    stored.canonical_path.display(),
                ));
            }
            None
        };
        self.tool_runtime
            .bind_managed_session_with_shared(
                self.session_id.clone(),
                runtime_id.clone(),
                workspace.clone(),
                shared_workspace.clone(),
            )
            .await?;
        self.managed_python_workspace =
            Some(SessionWorkspaceBinding::from_runtime_identity(&workspace));
        self.shared_python_workspace = shared_workspace
            .as_ref()
            .map(SessionWorkspaceBinding::from_runtime_identity);
        self.python_runtime_id = runtime_id;
        self.needs_save = true;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub async fn restore_managed_python_session(&self) -> Result<(), String> {
        self.restore_managed_python_session_with_cancellation(
            tokio_util::sync::CancellationToken::new(),
        )
        .await
    }

    #[cfg(target_os = "linux")]
    pub async fn restore_managed_python_session_with_cancellation(
        &self,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<(), String> {
        use crate::config::PythonWorkspaceExposure;

        if cancellation.is_cancelled() {
            return Err("Python session restoration was cancelled".to_string());
        }
        let workspace = self
            .managed_python_workspace
            .as_ref()
            .ok_or_else(|| "session has no managed Python workspace binding".to_string())?
            .to_runtime_identity()?;
        workspace.verify_current()?;
        let shared_workspace = match (
            self.config.python_invocation.workspace_exposure,
            self.shared_python_workspace.as_ref(),
        ) {
            (PythonWorkspaceExposure::SharedLaunchCwd, Some(stored)) => {
                let expected = stored.to_runtime_identity()?;
                let current = crate::python::runtime_store::WorkspaceIdentity::capture(
                    self.tool_runtime.workspace_root(),
                )?;
                if current != expected {
                    return Err(format!(
                        "Retained session workspace mismatch: session is bound to {}; Lethetic was launched from {}. Resume from the original directory or create a new session.",
                        expected.canonical_path.display(),
                        current.canonical_path.display(),
                    ));
                }
                Some(expected)
            }
            (PythonWorkspaceExposure::SharedLaunchCwd, None) => {
                return Err(
                    "managed retained session cannot be resumed in shared launch-cwd mode"
                        .to_string(),
                );
            }
            (PythonWorkspaceExposure::Policy, Some(stored)) => {
                return Err(format!(
                    "this retained session is bound to shared cwd {}; resume it with the same literal Python flag from that directory",
                    stored.canonical_path.display(),
                ));
            }
            (PythonWorkspaceExposure::Policy, None) => None,
        };
        self.tool_runtime
            .bind_managed_session_with_shared(
                self.session_id.clone(),
                self.python_runtime_id.clone(),
                workspace,
                shared_workspace,
            )
            .await?;
        if cancellation.is_cancelled() {
            Err("Python session restoration was cancelled".to_string())
        } else {
            Ok(())
        }
    }

    #[cfg(target_os = "linux")]
    pub async fn record_python_runtime_id(&mut self, runtime_id: String) -> Result<(), String> {
        self.tool_runtime
            .update_bound_runtime_id(runtime_id.clone())
            .await?;
        self.python_runtime_id = Some(runtime_id);
        self.needs_save = true;
        Ok(())
    }
}

impl App {
    #[cfg(target_os = "linux")]
    pub async fn ensure_nonlocal_python_session(&mut self) -> Result<(), String> {
        self.ensure_nonlocal_python_session_with_cancellation(
            tokio_util::sync::CancellationToken::new(),
        )
        .await
    }

    #[cfg(target_os = "linux")]
    pub async fn ensure_nonlocal_python_session_with_cancellation(
        &mut self,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<(), String> {
        if cancellation.is_cancelled() {
            return Err("Python session startup was cancelled".to_string());
        }
        if !crate::config::is_exact_retained_nonlocal_python_policy(
            self.config.tool_profile,
            &self.config.python_runtime,
        ) {
            return Ok(());
        }
        let session_path = self
            .current_session_dir
            .as_deref()
            .ok_or_else(|| "Nonlocal Python requires an active saved chat session".to_string())?;
        let session_binding = self
            .session_directory_binding
            .as_ref()
            .ok_or_else(|| "Nonlocal Python session has no directory identity".to_string())?;
        self.session_lease
            .as_ref()
            .ok_or_else(|| "Nonlocal Python session has no advisory lease".to_string())?
            .verify(
                std::path::Path::new(session_path),
                &self.session_id,
                session_binding,
            )?;
        self.save_session_checked()?;
        let session_id = self.session_id.clone();
        let workspace_store = crate::python::runtime_store::ManagedWorkspaceStore::open()?;
        let workspace = match &self.managed_python_workspace {
            Some(stored) => {
                let expected = stored.to_runtime_identity()?;
                let current = workspace_store.load(&session_id)?;
                if current != expected {
                    return Err(
                        "managed Python workspace no longer matches the chat session binding"
                            .to_string(),
                    );
                }
                current
            }
            None => workspace_store.load_or_create(&session_id)?,
        };
        let runtime_id = match &self.python_runtime_id {
            Some(runtime_id) => runtime_id.clone(),
            None => crate::python::runtime_store::RuntimeStore::open()?.generate_runtime_id()?,
        };
        let launch_cwd = if self.config.python_invocation.workspace_exposure
            == crate::config::PythonWorkspaceExposure::SharedLaunchCwd
        {
            self.tool_runtime.workspace_root().to_path_buf()
        } else {
            workspace.canonical_path.clone()
        };
        if cancellation.is_cancelled() {
            return Err("Python session startup was cancelled".to_string());
        }
        self.bind_managed_python_session(workspace, Some(runtime_id))
            .await?;
        if cancellation.is_cancelled() {
            return Err("Python session startup was cancelled".to_string());
        }
        self.save_session_checked()?;
        self.tool_runtime
            .ensure_ready_with_cancellation(&self.config, &launch_cwd, cancellation.clone())
            .await?;
        while let Some(notice) = self.tool_runtime.take_python_runtime_notice() {
            let message = notice.render();
            self.tool_output_preview = message.clone();
            self.add_segment(format!("\n{message}\n"), BlockType::Text);
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub async fn delete_python_runtime_transaction(&mut self) -> Result<bool, String> {
        self.delete_python_runtime_transaction_with_cancellation(
            tokio_util::sync::CancellationToken::new(),
        )
        .await
    }

    #[cfg(target_os = "linux")]
    pub async fn delete_python_runtime_transaction_with_cancellation(
        &mut self,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<bool, String> {
        if cancellation.is_cancelled() {
            return Err("Python runtime deletion was cancelled".to_string());
        }
        if !self.is_fully_idle() {
            return Err("wait for the active turn before deleting Python packages".to_string());
        }
        self.save_session_checked()?;
        self.tool_runtime.unbind_session_checked().await?;
        if cancellation.is_cancelled() {
            return Err("Python runtime deletion was cancelled after safe detach".to_string());
        }
        let Some(runtime_id) = self.python_runtime_id.clone() else {
            return Ok(false);
        };
        crate::python::retained_runtime::delete_retained_runtime_with_cancellation(
            &runtime_id,
            &self.session_id,
            cancellation.clone(),
        )
        .await?;
        self.python_runtime_id = None;
        self.needs_save = true;
        self.save_session_checked()?;
        Ok(true)
    }
}
