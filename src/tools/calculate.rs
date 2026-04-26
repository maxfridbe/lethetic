use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use super::llm_tokens::*;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "calculate".to_string(),
            description: "Perform a mathematical calculation".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "expression": {
                        "type": "string",
                        "description": "The math expression to evaluate, e.g. '2 + 2' or 'sin(pi/2)'"
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

pub fn get_prompt_template() -> String {
    format!("{}declaration:calculate{{description:<|\">Perform a mathematical calculation.<|\">,parameters:{{properties:{{expression:{{description:<|\">The math expression to evaluate, e.g. '2 + 2' or 'sin(pi/2)'<|\">,type:<|\">STRING<|\">}},description:{{description:<|\">Short description of the action<|\">,type:<|\">STRING<|\">}},tool_call_id:{{description:<|\">A unique, descriptive string identifier for this call (e.g., 'read_main_rs', 'check_folders'). Do not use simple numbers.<|\">,type:<|\">STRING<|\">}}}},required:[<|\">expression<|\">,<|\">description<|\">,<|\">tool_call_id<|\">],type:<|\">OBJECT<|\">}}}}{}", TOOL_CALL_OPEN, TOOL_CALL_CLOSE)
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} {}", icons::CALC, desc);
    }
    let expr = arguments["expression"].as_str().unwrap_or("");
    format!("{} Calculating: `{}`", icons::CALC, expr)
}

pub async fn execute(expression: &str) -> String {
    match eval(expression) {
        Ok(res) => res.to_string(),
        Err(e) => format!("ERROR: {}", e),
    }
}

fn eval(expr: &str) -> Result<f64, String> {
    let clean_expr = expr.replace(' ', "");
    if clean_expr.is_empty() { return Err("Empty expression".into()); }
    
    // Simple recursive descent parser for basic math
    let mut pos = 0;
    let chars: Vec<char> = clean_expr.chars().collect();
    
    parse_expression(&chars, &mut pos)
}

fn parse_expression(chars: &[char], pos: &mut usize) -> Result<f64, String> {
    let mut val = parse_term(chars, pos)?;
    while *pos < chars.len() {
        match chars[*pos] {
            '+' => { *pos += 1; val += parse_term(chars, pos)?; }
            '-' => { *pos += 1; val -= parse_term(chars, pos)?; }
            _ => break,
        }
    }
    Ok(val)
}

fn parse_term(chars: &[char], pos: &mut usize) -> Result<f64, String> {
    let mut val = parse_factor(chars, pos)?;
    while *pos < chars.len() {
        match chars[*pos] {
            '*' => { *pos += 1; val *= parse_factor(chars, pos)?; }
            '/' => {
                *pos += 1;
                let divisor = parse_factor(chars, pos)?;
                if divisor == 0.0 { return Err("Division by zero".into()); }
                val /= divisor;
            }
            _ => break,
        }
    }
    Ok(val)
}

fn parse_factor(chars: &[char], pos: &mut usize) -> Result<f64, String> {
    if *pos >= chars.len() { return Err("Unexpected end of expression".into()); }
    
    if chars[*pos] == '(' {
        *pos += 1;
        let val = parse_expression(chars, pos)?;
        if *pos >= chars.len() || chars[*pos] != ')' {
            return Err("Missing closing parenthesis".into());
        }
        *pos += 1;
        Ok(val)
    } else if chars[*pos] == '-' {
        *pos += 1;
        Ok(-parse_factor(chars, pos)?)
    } else {
        let start = *pos;
        while *pos < chars.len() && (chars[*pos].is_digit(10) || chars[*pos] == '.') {
            *pos += 1;
        }
        if start == *pos { return Err(format!("Expected number at position {}", start)); }
        let s: String = chars[start..*pos].iter().collect();
        s.parse::<f64>().map_err(|e| e.to_string())
    }
}
