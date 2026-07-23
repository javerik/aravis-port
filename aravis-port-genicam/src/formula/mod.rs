//! Hand-written recursive-descent formula evaluator for GenICam `<Formula>`/`<Value>` expressions.
//!
//! This deliberately deviates from the C reference's tokenize-then-shunting-yard-to-RPN
//! approach: the grammar is fixed and non-extensible, so a precedence-climbing recursive-descent
//! parser is no harder to get right, produces an `Expr` AST that naturally supports named
//! sub-expression inlining, and is easy to unit-test level-by-level.

mod eval;
mod lexer;
mod parser;

use std::collections::HashMap;

pub use eval::Value;
pub use parser::{BinOp, Expr, Formula, Func, UnOp};

use crate::error::FormulaError;

pub fn eval_expr(expr: &Expr, vars: &HashMap<String, Value>) -> Result<Value, FormulaError> {
    match expr {
        Expr::Const(v) => Ok(*v),
        Expr::Var(name) => vars
            .get(name)
            .copied()
            .ok_or_else(|| FormulaError::UnknownVariable(name.clone())),
        Expr::Unary(op, e) => {
            let v = eval_expr(e, vars)?;
            Ok(match op {
                UnOp::Plus => v,
                UnOp::Neg => match v {
                    Value::Int(x) => Value::Int(x.wrapping_neg()),
                    Value::Float(f) => Value::Float(-f),
                },
                UnOp::BitNot => Value::Int(!v.as_i64()),
            })
        }
        Expr::Binary(op, l, r) => {
            let a = eval_expr(l, vars)?;
            let b = eval_expr(r, vars)?;
            eval_binary(*op, a, b)
        }
        Expr::Ternary(c, t, f) => {
            if eval_expr(c, vars)?.is_truthy() {
                eval_expr(t, vars)
            } else {
                eval_expr(f, vars)
            }
        }
        Expr::Call(func, args) => {
            let values: Vec<Value> = args.iter().map(|a| eval_expr(a, vars)).collect::<Result<_, _>>()?;
            eval_call(*func, &values)
        }
    }
}

fn eval_binary(op: BinOp, a: Value, b: Value) -> Result<Value, FormulaError> {
    use std::cmp::Ordering;
    Ok(match op {
        BinOp::Add => eval::add(a, b),
        BinOp::Sub => eval::sub(a, b),
        BinOp::Mul => eval::mul(a, b),
        BinOp::Div => eval::div(a, b)?,
        BinOp::Mod => eval::rem(a, b)?,
        BinOp::Pow => eval::pow(a, b),
        BinOp::Shl => Value::Int(a.as_i64().wrapping_shl(b.as_i64() as u32)),
        BinOp::Shr => Value::Int(a.as_i64().wrapping_shr(b.as_i64() as u32)),
        BinOp::BitAnd => Value::Int(a.as_i64() & b.as_i64()),
        BinOp::BitOr => Value::Int(a.as_i64() | b.as_i64()),
        BinOp::BitXor => Value::Int(a.as_i64() ^ b.as_i64()),
        BinOp::And => eval::bool_value(a.is_truthy() && b.is_truthy()),
        BinOp::Or => eval::bool_value(a.is_truthy() || b.is_truthy()),
        BinOp::Eq => eval::bool_value(eval::compare(a, b) == Ordering::Equal),
        BinOp::Ne => eval::bool_value(eval::compare(a, b) != Ordering::Equal),
        BinOp::Le => eval::bool_value(eval::compare(a, b) != Ordering::Greater),
        BinOp::Ge => eval::bool_value(eval::compare(a, b) != Ordering::Less),
        BinOp::Lt => eval::bool_value(eval::compare(a, b) == Ordering::Less),
        BinOp::Gt => eval::bool_value(eval::compare(a, b) == Ordering::Greater),
    })
}

fn eval_call(func: Func, args: &[Value]) -> Result<Value, FormulaError> {
    Ok(match func {
        Func::Sgn => match args[0] {
            Value::Int(x) => Value::Int(x.signum()),
            Value::Float(f) => Value::Float(if f > 0.0 {
                1.0
            } else if f < 0.0 {
                -1.0
            } else {
                0.0
            }),
        },
        Func::Neg => match args[0] {
            Value::Int(x) => Value::Int(x.wrapping_neg()),
            Value::Float(f) => Value::Float(-f),
        },
        Func::Abs => match args[0] {
            Value::Int(x) => Value::Int(x.wrapping_abs()),
            Value::Float(f) => Value::Float(f.abs()),
        },
        Func::Sin => Value::Float(args[0].as_f64().sin()),
        Func::Cos => Value::Float(args[0].as_f64().cos()),
        Func::Atan => Value::Float(args[0].as_f64().atan()),
        Func::Tan => Value::Float(args[0].as_f64().tan()),
        Func::Exp => Value::Float(args[0].as_f64().exp()),
        Func::Ln => Value::Float(args[0].as_f64().ln()),
        Func::Lg => Value::Float(args[0].as_f64().log10()),
        Func::Sqrt => Value::Float(args[0].as_f64().sqrt()),
        Func::Asin => Value::Float(args[0].as_f64().asin()),
        Func::Acos => Value::Float(args[0].as_f64().acos()),
        Func::Floor => Value::Float(args[0].as_f64().floor()),
        Func::Ceil => Value::Float(args[0].as_f64().ceil()),
        Func::Trunc => {
            let x = args[0].as_f64();
            Value::Float(if x > 0.0 { x.floor() } else { x.ceil() })
        }
        Func::Round => {
            let x = args[0].as_f64();
            if args.len() == 2 {
                let precision = args[1].as_i64();
                let scale = 10f64.powi(precision as i32);
                Value::Float((x * scale).round() / scale)
            } else {
                Value::Float(x.round())
            }
        }
    })
}

impl Formula {
    pub fn eval(&self, vars: &HashMap<String, Value>) -> Result<Value, FormulaError> {
        eval_expr(&self.expr, vars)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval_str(text: &str, vars: &[(&str, Value)]) -> Value {
        let formula = Formula::parse(text, &[], &[]).unwrap();
        let map: HashMap<String, Value> = vars.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        formula.eval(&map).unwrap()
    }

    #[test]
    fn division_by_zero_is_an_error_not_a_panic() {
        let formula = Formula::parse("1 / X", &[], &[]).unwrap();
        let mut vars = HashMap::new();
        vars.insert("X".to_string(), Value::Int(0));
        assert_eq!(formula.eval(&vars), Err(FormulaError::DivisionByZero));
    }

    #[test]
    fn integer_division_overflow_does_not_panic() {
        // i64::MIN / -1 overflows in two's complement; wrapping_div must be used, not `/`.
        let formula = Formula::parse("X / Y", &[], &[]).unwrap();
        let mut vars = HashMap::new();
        vars.insert("X".to_string(), Value::Int(i64::MIN));
        vars.insert("Y".to_string(), Value::Int(-1));
        // Should not panic; wrapping semantics produce i64::MIN again.
        assert_eq!(formula.eval(&vars).unwrap(), Value::Int(i64::MIN));
    }

    #[test]
    fn bitwise_and_shift_operators() {
        assert_eq!(eval_str("6 & 3", &[]), Value::Int(2));
        assert_eq!(eval_str("6 | 1", &[]), Value::Int(7));
        assert_eq!(eval_str("5 ^ 1", &[]), Value::Int(4));
        assert_eq!(eval_str("1 << 4", &[]), Value::Int(16));
        assert_eq!(eval_str("16 >> 2", &[]), Value::Int(4));
        assert_eq!(eval_str("~0", &[]), Value::Int(-1));
    }

    #[test]
    fn mixed_int_float_promotes_to_float() {
        assert_eq!(eval_str("1 + 0.5", &[]), Value::Float(1.5));
    }

    #[test]
    fn live_camera_model_detection_formula() {
        // Verbatim shape of the live C5-2040-GigE camera's CMVSeriesModelType formula.
        let formula = Formula::parse(
            "X=648 ? CMV300 : (X=4096 ? CMV12000 : (X=3360 ? CMV8000 : CMV2000))",
            &[
                ("CMV300".to_string(), "1".to_string()),
                ("CMV12000".to_string(), "2".to_string()),
                ("CMV2000".to_string(), "3".to_string()),
                ("CMV8000".to_string(), "4".to_string()),
            ],
            &[],
        )
        .unwrap();
        let mut vars = HashMap::new();
        vars.insert("X".to_string(), Value::Int(4096));
        assert_eq!(formula.eval(&vars).unwrap(), Value::Int(2));
    }

    #[test]
    fn round_with_precision() {
        assert_eq!(eval_str("ROUND(1.23456, 2)", &[]), Value::Float(1.23));
    }

    #[test]
    fn sgn_and_neg_preserve_operand_type() {
        assert_eq!(eval_str("SGN(-5)", &[]), Value::Int(-1));
        assert_eq!(eval_str("SGN(-5.0)", &[]), Value::Float(-1.0));
        assert_eq!(eval_str("NEG(5)", &[]), Value::Int(-5));
    }
}
