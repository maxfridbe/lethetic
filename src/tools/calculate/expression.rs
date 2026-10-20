//! Pure floating-point calculator compatibility on top of evalexpr's parser.
use evalexpr::{
    Context, ContextWithMutableFunctions, ContextWithMutableVariables, Function, HashMapContext,
    Node, Operator, Value,
};

pub(super) fn evaluate(expression: &str) -> Result<f64, String> {
    let mut tree = evalexpr::build_operator_tree(&without_unary_plus(expression))
        .map_err(|error| error.to_string())?;
    // Validate arity before rotating powers; malformed input must be an error,
    // never a panic while rearranging an incomplete expression tree.
    if tree.iter().any(|node| match node.operator() {
        Operator::Exp => node.children().len() != 2,
        Operator::Neg => node.children().len() != 1,
        _ => false,
    }) {
        return Err("Incomplete mathematical expression".into());
    }
    for operator in tree.iter_operators_mut() {
        match operator {
            // Keep floating-point division even when both source literals are integers.
            Operator::Const {
                value: Value::Int(value),
            } => {
                *operator = Operator::Const {
                    value: Value::Float(*value as f64),
                };
            }
            Operator::RootNode
            | Operator::Add
            | Operator::Sub
            | Operator::Neg
            | Operator::Mul
            | Operator::Div
            | Operator::Mod
            | Operator::Exp
            | Operator::Tuple
            | Operator::Const {
                value: Value::Float(_),
            }
            | Operator::VariableIdentifierRead { .. }
            | Operator::FunctionIdentifier { .. } => {}
            _ => return Err("Only mathematical expressions are supported".into()),
        }
    }
    right_associate_powers(&mut tree);
    tree.eval_number_with_context(&context().map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())
}

// The previous calculator accepts unary +. Remove only plus signs where an
// operand is expected; keep binary addition and scientific-notation signs.
fn without_unary_plus(expression: &str) -> String {
    let mut operand_expected = true;
    expression
        .chars()
        .filter(|&ch| {
            if ch.is_whitespace() {
                return true;
            }
            if ch == '+' && operand_expected {
                return false;
            }
            operand_expected = matches!(ch, '(' | ',' | '+' | '-' | '*' | '/' | '%' | '^');
            true
        })
        .collect()
}

// evalexpr associates powers left-to-right; the calculator has always used
// mathematical right associativity. Parenthesized expressions have RootNode
// wrappers, so this rotation preserves explicitly grouped powers.
fn right_associate_powers(tree: &mut Node) {
    while matches!(tree.operator(), Operator::Exp)
        && tree
            .children()
            .first()
            .is_some_and(|left| matches!(left.operator(), Operator::Exp))
    {
        let right = tree.children_mut().pop().expect("binary power");
        let mut left = tree.children_mut().pop().expect("binary power");
        let middle = left.children_mut().pop().expect("binary power");
        let first = left.children_mut().pop().expect("binary power");
        left.children_mut().extend([middle, right]);
        tree.children_mut().extend([first, left]);
    }
    // In an unparenthesized exponent chain, - binds outside the next power:
    // 2^-3^2 means 2^(-(3^2)), not 2^((-3)^2).
    if matches!(tree.operator(), Operator::Exp)
        && tree
            .children()
            .first()
            .is_some_and(|left| matches!(left.operator(), Operator::Neg))
    {
        let right = tree.children_mut().pop().expect("binary power");
        let mut negative = tree.children_mut().pop().expect("binary power");
        let base = negative.children_mut().pop().expect("unary negation");
        *negative.operator_mut() = Operator::Exp;
        negative.children_mut().extend([base, right]);
        *tree.operator_mut() = Operator::Neg;
        tree.children_mut().push(negative);
    }
    for child in tree.children_mut() {
        right_associate_powers(child);
    }
}

type UnaryFunction = fn(f64) -> f64;

fn context() -> evalexpr::EvalexprResult<HashMapContext> {
    let mut context = HashMapContext::new();
    context.set_builtin_functions_disabled(true)?;
    context.set_value("pi".into(), Value::Float(std::f64::consts::PI))?;
    context.set_value("e".into(), Value::Float(std::f64::consts::E))?;
    let unary: &[(&str, UnaryFunction)] = &[
        ("sqrt", f64::sqrt),
        ("exp", f64::exp),
        ("ln", f64::ln),
        ("abs", f64::abs),
        ("sin", f64::sin),
        ("cos", f64::cos),
        ("tan", f64::tan),
        ("asin", f64::asin),
        ("acos", f64::acos),
        ("atan", f64::atan),
        ("sinh", f64::sinh),
        ("cosh", f64::cosh),
        ("tanh", f64::tanh),
        ("asinh", f64::asinh),
        ("acosh", f64::acosh),
        ("atanh", f64::atanh),
        ("floor", f64::floor),
        ("ceil", f64::ceil),
        ("round", f64::round),
        ("signum", f64::signum),
    ];
    for &(name, function) in unary {
        context.set_function(
            name.into(),
            Function::new(move |value| Ok(Value::Float(function(value.as_number()?)))),
        )?;
    }
    for (name, function) in [
        ("atan2", f64::atan2 as fn(f64, f64) -> f64),
        ("log", f64::log),
    ] {
        context.set_function(
            name.into(),
            Function::new(move |value| {
                let args = value.as_fixed_len_tuple(2)?;
                Ok(Value::Float(function(
                    args[0].as_number()?,
                    args[1].as_number()?,
                )))
            }),
        )?;
    }
    for (name, function) in [("min", f64::min as fn(f64, f64) -> f64), ("max", f64::max)] {
        context.set_function(
            name.into(),
            Function::new(move |value| {
                let args = match value {
                    Value::Tuple(args) => args.as_slice(),
                    _ => std::slice::from_ref(value),
                };
                let mut values = args.iter().map(Value::as_number);
                let Some(first) = values.next() else {
                    return Err(evalexpr::EvalexprError::CustomMessage(
                        "Expected at least one argument".into(),
                    ));
                };
                let result =
                    values.try_fold(first?, |acc, next| next.map(|next| function(acc, next)))?;
                Ok(Value::Float(result))
            }),
        )?;
    }
    Ok(context)
}

#[cfg(test)]
mod tests {
    use super::evaluate;

    #[test]
    fn preserves_calculator_arithmetic_and_functions() {
        for (expression, expected) in [
            ("10 / 4", 2.5),
            ("+2", 2.0),
            ("2 * +3", 6.0),
            ("1 + +2", 3.0),
            ("2^-3^2", 0.001953125),
            ("2^(-3)^2", 512.0),
            ("2^3^2", 512.0),
            ("(2^3)^2", 64.0),
            ("-2^2", -4.0),
            ("(-2)^2", 4.0),
            ("2^-2", 0.25),
            ("1e3 + 2.5e-1", 1000.25),
            ("sin(pi/2)", 1.0),
            ("ln(e)", 1.0),
            ("atan2(0, 1)", 0.0),
            ("log(100, 10)", 2.0),
            ("min(4, -2, 8)", -2.0),
            ("max(4, -2, 8)", 8.0),
            ("max(4)", 4.0),
            ("floor(2.8)+ceil(2.2)", 5.0),
            ("round(-2.5)", -3.0),
            ("sinh(0)+cosh(0)+tanh(0)", 1.0),
            ("asinh(0)+acosh(1)+atanh(0)", 0.0),
            ("signum(-3)", -1.0),
        ] {
            let actual = evaluate(expression).unwrap_or_else(|e| panic!("{expression}: {e}"));
            assert!(
                (actual - expected).abs() < 1e-10,
                "{expression}: {actual} != {expected}"
            );
        }
    }

    #[test]
    fn rejects_malformed_and_non_mathematical_input() {
        for expression in [
            "",
            "2 +",
            "2^3^",
            "2^",
            "-",
            "2^-",
            "sqrt(1,2)",
            "unknown(2)",
            "missing",
            "pi = 2",
            "1; 2",
            "true",
            "2 > 1",
            "min()",
        ] {
            assert!(evaluate(expression).is_err(), "{expression}");
        }
    }
}
