use std::error::Error;

#[cfg(target_os = "linux")]
use std::{io, path::PathBuf};

#[cfg(target_os = "linux")]
use tokio_util::sync::CancellationToken;

pub(crate) async fn run_if_requested(args: &[String]) -> Result<bool, Box<dyn Error>> {
    #[cfg(not(target_os = "linux"))]
    let _ = args;

    #[cfg(target_os = "linux")]
    if args.get(1).map(String::as_str) == Some("--internal-managed-workspace-cleanup") {
        if args.len() != 9
            || args[2] != lethetic::python::runtime_store::MANAGED_WORKSPACE_CLEANUP_ABI
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid internal managed-workspace cleanup invocation",
            )
            .into());
        }
        let parse_identity = |value: &str, label: &str| {
            value.parse::<u64>().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid managed-workspace {label}"),
                )
            })
        };
        let expected = lethetic::python::runtime_store::WorkspaceIdentity {
            canonical_path: PathBuf::from(&args[5]),
            device: parse_identity(&args[6], "device")?,
            inode: parse_identity(&args[7], "inode")?,
            binding_hash: args[8].clone(),
        };
        lethetic::python::runtime_store::run_internal_managed_workspace_cleanup(
            PathBuf::from(&args[3]),
            &args[4],
            expected,
        )
        .map_err(io::Error::other)?;
        return Ok(true);
    }

    #[cfg(target_os = "linux")]
    if args.get(1).map(String::as_str) == Some("--internal-egress-broker") {
        if args.len() != 13 || args[2] != lethetic::python::egress_broker::BROKER_PROTOCOL_ABI {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid internal egress broker invocation",
            )
            .into());
        }
        lethetic::python::retained_runtime::arm_internal_parent_death_signal()
            .map_err(io::Error::other)?;
        let parse_identity = |value: &str| {
            value.parse::<u32>().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid internal egress broker identity",
                )
            })
        };
        let derive_peer_credentials = args[7] == "auto" && args[8] == "auto";
        if (args[7] == "auto" || args[8] == "auto") && !derive_peer_credentials {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "internal egress broker credentials must both be exact or auto",
            )
            .into());
        }
        let selinux_labels = if args[10] == "none" && args[11] == "none" {
            None
        } else if args[10] == "none" || args[11] == "none" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "internal egress broker SELinux labels must both be exact or none",
            )
            .into());
        } else {
            Some(
                lethetic::python::selinux::SelinuxLabels::new(args[10].clone(), args[11].clone())
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?,
            )
        };
        let allow_label_disabled_peer = match args[12].as_str() {
            "label-disabled" => true,
            "selinux-labeled" => false,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid internal egress broker security profile",
                )
                .into());
            }
        };
        let broker = lethetic::python::egress_broker::BrokerConfig {
            socket_path: PathBuf::from(&args[3]),
            audit_path: PathBuf::from(&args[4]),
            runtime_id: args[5].clone(),
            capability: args[6].clone(),
            expected_peer_uid: if derive_peer_credentials {
                0
            } else {
                parse_identity(&args[7])?
            },
            expected_peer_gid: if derive_peer_credentials {
                0
            } else {
                parse_identity(&args[8])?
            },
            derive_peer_credentials,
            expected_peer_pid: parse_identity(&args[9])?,
            selinux_labels,
            allow_label_disabled_peer,
            max_connections: 32,
        };
        let cancellation = CancellationToken::new();
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
        let broker = lethetic::python::egress_broker::run_broker(broker, cancellation.clone());
        tokio::pin!(broker);
        let result = tokio::select! {
            result = &mut broker => result,
            _ = terminate.recv() => {
                cancellation.cancel();
                broker.await
            }
            _ = interrupt.recv() => {
                cancellation.cancel();
                broker.await
            }
        };
        return result
            .map(|()| true)
            .map_err(|error| io::Error::other(error).into());
    }

    #[cfg(target_os = "linux")]
    if args.get(1).map(String::as_str) == Some("--internal-retained-attach") {
        if args.len() != 5 || args[2] != lethetic::python::retained_runtime::RETAINED_ATTACH_ABI {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid internal retained attach invocation",
            )
            .into());
        }
        lethetic::python::retained_runtime::arm_internal_parent_death_signal()
            .map_err(io::Error::other)?;
        let code = lethetic::python::retained_runtime::run_retained_attach(
            PathBuf::from(&args[4]),
            args[3].clone(),
        )
        .await
        .map_err(io::Error::other)?;
        std::process::exit(code);
    }

    Ok(false)
}
