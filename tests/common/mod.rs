
use lethetic::app::App;
use lethetic::config::Config;

pub fn setup_mock_app() -> App {
    let config = Config {
        server_url: "http://localhost:11434".to_string(),
        model: "gemma4:latest".to_string(),
        context_size: 2048,
        tool_wrapper: None,
    };
    App::new(&config)
}
