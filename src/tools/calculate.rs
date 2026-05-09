use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "calculate".to_string(),
            description: "Perform a mathematical calculation. Supports +, -, *, /, ^, sqrt(), sin(), cos(), tan(), asin(), acos(), atan(), atan2(), exp(), ln(), log(), abs(), floor(), ceil(), round(), pi, e. Examples: 'sin(pi/2)', 'sqrt(2)', '2^10', 'log(100, 10)'.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "expression": {
                        "type": "string",
                        "description": "The math expression to evaluate, e.g. 'sin(pi/2)', 'sqrt(2)', '2^10 + 1'"
                    },
                    "description": {
                        "type": "string",
                        "description": "Short description of the action"
                    },
                    "tool_call_id": {
                        "type": "string",
                        "description": "A unique, descriptive string identifier for this call (e.g., 'read_main_rs', 'check_folders'). Do not use simple numbers."
                    }
                },
                "required": ["expression", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} {}", icons::CALC, desc);
    }
    let expr = arguments["expression"].as_str().unwrap_or("");
    format!("{} Calculating: `{}`", icons::CALC, expr)
}

pub async fn execute(expression: &str) -> String {
    match meval::eval_str(expression) {
        Ok(result) => {
            // Show integer form when the result is a whole number
            if result.fract() == 0.0 && result.abs() < 1e15 {
                format!("{}", result as i64)
            } else {
                format!("{}", result)
            }
        }
        Err(e) => format!("ERROR: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_basic_arithmetic() {
        assert_eq!(execute("2 + 2").await, "4");
        assert_eq!(execute("10 / 4").await, "2.5");
        assert_eq!(execute("3 * 7").await, "21");
    }

    #[tokio::test]
    async fn test_trig() {
        let r = execute("sin(pi/2)").await;
        assert_eq!(r, "1");
        let r = execute("cos(0)").await;
        assert_eq!(r, "1");
    }

    #[tokio::test]
    async fn test_sqrt_pow() {
        assert_eq!(execute("sqrt(9)").await, "3");
        assert_eq!(execute("2^10").await, "1024");
    }

    #[tokio::test]
    async fn test_constants() {
        let r = execute("e^1").await;
        // e ≈ 2.718...
        let v: f64 = r.parse().unwrap();
        assert!((v - std::f64::consts::E).abs() < 1e-10);
    }
}
