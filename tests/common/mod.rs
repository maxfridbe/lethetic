
use lethetic::app::App;
use lethetic::config::Config;

pub fn setup_mock_app() -> App {
    let config = Config {
        server_url: "http://brainiac-nvidia:7210/v1/responses".to_string(),
        model: "Gemma-4-26B-TurboQuant-262k".to_string(),
        context_size: 2048,
        tool_wrapper: None,
    };
    App::new(&config)
}
