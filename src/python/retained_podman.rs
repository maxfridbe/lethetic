mod attestation;
mod cleanup_queue;
mod command;
mod model;
mod spec;
mod transient;
mod workspace;

pub use command::{podman_selinux_enabled, prepare_retained_podman, resolve_local_runtime_image};
pub use model::{
    ABI_LABEL, CONTAINER_BROKER_SOCKET, CONTAINER_CAPABILITY_PATH, CONTAINER_ENTRYPOINT,
    CONTAINER_HOST_BRIDGE, CONTAINER_WORKSPACE, ContainerProcessIdentity, FINGERPRINT_LABEL,
    LAYOUT_LABEL, LEGACY_CONTAINER_BROKER_SOCKET, MANAGED_LABEL, PodmanCommandSpec,
    RUNTIME_ID_LABEL, RUNTIME_IMAGE_LABEL, RUNTIME_IMAGE_LABEL_VALUE, ResolvedRuntimeImage,
    RetainedPodmanConfig, SESSION_ID_LABEL, TRANSIENT_FINGERPRINT_LABEL, TRANSIENT_LABEL,
    TransientPodmanConfig, TransientPodmanMount,
};
pub use spec::validate_container_id;

pub(crate) use attestation::attest_retained_container_with_unbound_labels;
pub(crate) use attestation::{
    attest_frozen_schema_v2_container, attest_new_retained_container,
    attest_new_transient_container, attest_retained_container, verify_exact_container_stopped,
};
pub(crate) use cleanup_queue::{
    clear_transient_cleanup_record, persist_transient_cleanup_record,
    retry_pending_transient_cleanups,
};
pub(crate) use command::{
    create_retained_container, exact_container_presence, podman_command_cancellation_requested,
    resolve_exact_container_name, resolve_local_image_exact, resolve_retained_podman, run_remove,
    run_stop, with_podman_command_cancellation,
};
pub(crate) use model::ContainerPresence;
#[allow(unused_imports)]
pub(crate) use spec::{
    build_create_spec, build_force_remove_spec, build_remove_spec, build_stop_spec,
};
pub(crate) use spec::{build_start_attach_spec, build_transient_create_spec};
#[allow(unused_imports)]
pub(crate) use transient::cleanup_failed_transient_create;
pub(crate) use transient::{
    cleanup_interrupted_transient_create, cleanup_transient_container, create_transient_container,
};
pub(crate) use workspace::validate_external_workspace;

#[cfg(test)]
mod tests;
