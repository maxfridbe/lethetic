mod adapter;
mod broker;
mod network_policy;
mod peer;
mod protocol;
mod relay;

pub use adapter::{
    AdapterConfig, DEFAULT_ADAPTER_PORT, open_broker_readiness_connection, probe_broker,
    run_adapter, run_adapter_with_readiness,
};
pub use broker::{BrokerConfig, run_broker};
pub use network_policy::{is_public_destination, validate_public_hostname};
pub(crate) use peer::validate_unix_socket_path_length;
pub use protocol::{
    BROKER_PROTOCOL_ABI, BROKER_PROTOCOL_VERSION, BrokerRequest, BrokerRequestKind, BrokerResponse,
    ProxyKind,
};

#[cfg(test)]
mod tests;
