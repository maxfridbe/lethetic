
use lethetic::app::App;
use lethetic::config::Config;

pub fn setup_mock_app() -> App {
    let config = Config {
        server_url: "http://brainiac-nvidia:7210/v1/responses".to_string(),
        model: "Gemma-4-26B-TurboQuant-262k".to_string(),
        context_size: 2048,
        tool_wrapper: None,
        tool_profile: Default::default(),
        python_runtime: Default::default(),
        active_server: None,
        connection_kind: Default::default(),
        api_key: None,
        estimate_cost: None,
        pricing: None,
        input_cost_per_1m: None,
        output_cost_per_1m: None,
        enable_image_processing_tool: false,
        background_tasks: Default::default(),
        tool_calls: Default::default(),
        provider_retries: None,
        theme: None,
        model_servers: Vec::new(),
        thinking: None,
        extra_body: None,
        context_mode: None,
    };
    App::new(&config)
}
