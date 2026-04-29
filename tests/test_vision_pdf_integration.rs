use lethetic::tools::get_pdf_text;
use lethetic::tools::process_image;
use lethetic::tools::process_pdf_image;
use lethetic::config::Config;
use reqwest::Client;
use std::fs;
use tempfile::tempdir;

#[tokio::test]
async fn test_get_pdf_text_file_not_found() {
    let (tx, _) = tokio::sync::mpsc::unbounded_channel();
    let res = get_pdf_text::execute("non_existent.pdf", ".", &tx).await;
    assert!(res.contains("ERROR: PDF file not found"));
}

#[tokio::test]
async fn test_process_image_file_not_found() {
    let client = Client::new();
    let config = Config {
        server_url: "http://localhost:11434".to_string(),
        model: "gemma2".to_string(),
        context_size: 2048,
        tool_wrapper: None,
    };
    let (tx, _) = tokio::sync::mpsc::unbounded_channel();
    let res = process_image::execute("test", "non_existent.png", None, ".", &client, &config, &tx).await;
    assert!(res.contains("ERROR: Image file not found"));
}

#[tokio::test]
async fn test_process_pdf_image_invalid_page() {
    let client = Client::new();
    let config = Config {
        server_url: "http://localhost:11434".to_string(),
        model: "gemma2".to_string(),
        context_size: 2048,
        tool_wrapper: None,
    };
    let (tx, _) = tokio::sync::mpsc::unbounded_channel();
    let res = process_pdf_image::execute("test", "non_existent.pdf", 1, None, ".", &client, &config, &tx).await;
    assert!(res.contains("ERROR: PDF file not found"));
}

#[tokio::test]
#[ignore]
async fn test_live_vision_screenshot() {
    let config_content = fs::read_to_string("config.yml").expect("Could not read config.yml");
    let config: Config = serde_yaml::from_str(&config_content).expect("Failed to parse config");
    let client = Client::new();
    let (tx, _) = tokio::sync::mpsc::unbounded_channel();
    
    // Using the existing screenshot in res/
    let timeout = std::time::Duration::from_secs(600);
    let res = tokio::time::timeout(timeout, process_image::execute(
        "What is the contents of this image?", 
        "res/Screenshot.webp", 
        Some(1024), 
        ".", 
        &client, 
        &config,
        &tx
    )).await.expect("Test timed out after 600s");
    
    println!("Vision Response: '{}'", res);
    assert!(!res.contains("ERROR"), "Vision processing should not return an error");
    if res.trim().is_empty() {
        println!("WARNING: Vision response was empty. This might indicate the model couldn't 'see' the image tokens or is under-reacting.");
    }
}

#[tokio::test]
#[ignore]
async fn test_live_pdf_processing() {
    let config_content = fs::read_to_string("config.yml").expect("Could not read config.yml");
    let config: Config = serde_yaml::from_str(&config_content).expect("Failed to parse config");
    let client = Client::new();
    let (tx, _) = tokio::sync::mpsc::unbounded_channel();

    // 1. Create a dummy PDF with text
    let pdf_path = "test_sample.pdf";
    let mut builder = pdf_oxide::writer::DocumentBuilder::new();
    builder.page(pdf_oxide::writer::PageSize::Letter)
        .text("Gemma 4 Vision Test")
        .text("This is a sample PDF generated for integration testing lethetic vision tools.");
    builder.save(pdf_path).expect("Failed to save test PDF");

    // 2. Test text extraction
    let extracted = get_pdf_text::execute(pdf_path, ".", &tx).await;
    println!("Extracted PDF Text:\n{}", extracted);
    assert!(extracted.contains("Gemma 4 Vision Test"), "Text extraction should find the title");

    // 3. Test vision rendering and analysis
    let timeout = std::time::Duration::from_secs(600);
    let vision_res = tokio::time::timeout(timeout, process_pdf_image::execute(
        "What is the title of this PDF page?", 
        pdf_path, 
        1, 
        Some(512), 
        ".", 
        &client, 
        &config,
        &tx
    )).await.expect("Test timed out after 600s");
    
    println!("PDF Vision Response: {}", vision_res);
    assert!(!vision_res.contains("ERROR"), "PDF Vision processing failed");
    
    let _ = fs::remove_file(pdf_path);
}

#[tokio::test]
#[ignore]
async fn test_live_vision_bike() {
    let config_content = fs::read_to_string("config.yml").expect("Could not read config.yml");
    let config: Config = serde_yaml::from_str(&config_content).expect("Failed to parse config");
    let client = Client::new();
    let (tx, _) = tokio::sync::mpsc::unbounded_channel();
    
    let timeout = std::time::Duration::from_secs(600);
    let res = tokio::time::timeout(timeout, process_image::execute(
        "what kind of image this is", 
        "/var/home/maxfridbe/Downloads/bike.jpg", 
        Some(1024), 
        ".", 
        &client, 
        &config,
        &tx
    )).await.expect("Test timed out after 600s");
    
    println!("Bike Image Response: {}", res);
    assert!(!res.contains("ERROR"), "Vision processing failed for bike image");
}

#[tokio::test]
#[ignore]
async fn test_live_vision_sequential() {
    let config_content = fs::read_to_string("config.yml").expect("Could not read config.yml");
    let config: Config = serde_yaml::from_str(&config_content).expect("Failed to parse config");
    let client = Client::new();
    let (tx, _) = tokio::sync::mpsc::unbounded_channel();
    
    let timeout = std::time::Duration::from_secs(600);
    
    println!("--- Starting Sequential Vision Test (Query 1) ---");
    let res1 = tokio::time::timeout(timeout, process_image::execute(
        "what kind of image this is", 
        "/var/home/maxfridbe/Downloads/bike.jpg", 
        Some(1024), 
        ".", 
        &client, 
        &config,
        &tx
    )).await.expect("Test 1 timed out");
    println!("Query 1 Response: {}", res1);
    assert!(!res1.contains("ERROR"), "Sequential query 1 failed");

    println!("--- Starting Sequential Vision Test (Query 2) ---");
    let res2 = tokio::time::timeout(timeout, process_image::execute(
        "What is the contents of this image?", 
        "res/Screenshot.webp", 
        Some(1024), 
        ".", 
        &client, 
        &config,
        &tx
    )).await.expect("Test 2 timed out");
    println!("Query 2 Response: {}", res2);
    assert!(!res2.contains("ERROR"), "Sequential query 2 failed");
}

#[test]
fn test_tool_registration() {
    let tools = lethetic::tools::get_all_tools();
    assert!(tools.iter().any(|t| t.function.name == "process_image"));
    assert!(tools.iter().any(|t| t.function.name == "process_pdf_image"));
    assert!(tools.iter().any(|t| t.function.name == "get_pdf_text"));
}
