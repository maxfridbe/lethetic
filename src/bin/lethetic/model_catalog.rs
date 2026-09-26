//! "Scan for more" in the model picker: fetch a connection's catalog,
//! record list prices, and save chosen models to the picker.

use std::time::Duration;

use lethetic::app::App;
use lethetic::client::StreamEvent;
use lethetic::config::Config;
use lethetic::saved_models::{SavedModels, add_to_allowlist};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const SCAN_TIMEOUT: Duration = Duration::from_secs(20);

pub(crate) fn scan(
    config: &Config,
    client: &reqwest::Client,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    cancellation: CancellationToken,
    connection_id: String,
) -> Option<tokio::task::JoinHandle<()>> {
    let server = config
        .model_servers
        .iter()
        .find(|server| server.connection_id() == connection_id)?
        .clone();
    let client = client.clone();
    let tx = tx.clone();
    Some(tokio::spawn(async move {
        let fetch = lethetic::client::get_available_models(
            &client,
            server.kind,
            &server.url,
            server.api_key.as_deref(),
        );
        let result = tokio::select! {
            _ = cancellation.cancelled() => return,
            result = tokio::time::timeout(SCAN_TIMEOUT, fetch) => result
                .unwrap_or_else(|_| Err(format!("scan timed out after {}s", SCAN_TIMEOUT.as_secs()))),
        };
        let _ = tx.send(StreamEvent::ModelCatalogReady {
            connection_id,
            result,
        });
    }))
}

fn catalog_source_url(server_url: &str, model: &str) -> String {
    if server_url.contains("openrouter.ai") {
        format!("https://openrouter.ai/{model}")
    } else {
        server_url.to_string()
    }
}

/// Show the catalog and refresh stored prices for every model this
/// connection already lists, so estimates stay current.
pub(crate) fn ready(
    app: &mut App,
    config: &mut Config,
    connection_id: String,
    result: Result<Vec<lethetic::transport::ModelInfo>, String>,
) {
    if let (Ok(models), Some(server)) = (
        &result,
        config
            .model_servers
            .iter()
            .find(|server| server.connection_id() == connection_id),
    ) {
        let mut listed = server.models.clone();
        add_to_allowlist(&mut listed, &server.model, &[]);
        let mut saved = SavedModels::load();
        let mut changed = false;
        for model in models.iter().filter(|model| listed.contains(&model.id)) {
            if let Some(price) = &model.pricing {
                let url = catalog_source_url(&server.url, &model.id);
                changed |= saved.record_price(&connection_id, &model.id, price, &url);
            }
        }
        if changed && saved.save().is_ok() {
            refresh_saved(app, config, &saved);
        }
    }
    if let Some(catalog) = app.model_catalog.as_mut()
        && catalog.connection_id == connection_id
    {
        catalog.set_models(result);
    }
    app.should_redraw = true;
}

/// Add `model_id` to the connection's picker list and store its price.
pub(crate) fn save(app: &mut App, config: &mut Config, connection_id: &str, model_id: &str) {
    let mut saved = SavedModels::load();
    let added = saved.add(connection_id, model_id);
    match saved.save() {
        Ok(()) => {
            refresh_saved(app, config, &saved);
            app.stop_reason = if added {
                format!("Added {model_id} to the model picker")
            } else {
                format!("{model_id} is already in the model picker")
            };
        }
        Err(error) => app.stop_reason = format!("⚠ Could not save model: {error}"),
    }
    app.should_redraw = true;
}

/// Re-apply saved models and prices to the live config, keeping the active
/// selection and its pricing in sync.
fn refresh_saved(app: &mut App, config: &mut Config, saved: &SavedModels) {
    for server in &mut config.model_servers {
        server.model_pricing.clear();
    }
    saved.apply(config);
    if let Some(connection_id) = config.active_connection_id().map(str::to_string) {
        let model = config.model.clone();
        let mut candidate = config.clone();
        if candidate.activate_model(&connection_id, &model).is_ok() {
            config.pricing = candidate.pricing;
        }
    }
    app.config.model_servers = config.model_servers.clone();
    app.config.pricing = config.pricing.clone();
}
