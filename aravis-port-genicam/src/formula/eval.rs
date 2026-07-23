use crate::error::FormulaError;

/// A formula-evaluator value: either an integer or a floating-point number. Mixed-type binary
/// operations promote to `Float` unless both operands are `Int`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
}

impl Value {
    pub fn as_i64(self) -> i64 {
        match self {
            Value::Int(v) => v,
            Value::Float(f) => f as i64,
        }
    }

    pub fn as_f64(self) -> f64 {
        match self {
            Value::Int(v) => v as f64,
            Value::Float(f) => f,
        }
    }

    pub fn is_truthy(self) -> bool {
        match self {
            Value::Int(v) => v != 0,
            Value::Float(f) => f != 0.0,
        }
    }

    fn both_int(a: Value, b: Value) -> Option<(i64, i64)> {
        match (a, b) {
            (Value::Int(x), Value::Int(y)) => Some((x, y)),
            _ => None,
        }
    }
}

pub fn add(a: Value, b: Value) -> Value {
    match Value::both_int(a, b) {
        Some((x, y)) => Value::Int(x.wrapping_add(y)),
        None => Value::Float(a.as_f64() + b.as_f64()),
    }
}

pub fn sub(a: Value, b: Value) -> Value {
    match Value::both_int(a, b) {
        Some((x, y)) => Value::Int(x.wrapping_sub(y)),
        None => Value::Float(a.as_f64() - b.as_f64()),
    }
}

pub fn mul(a: Value, b: Value) -> Value {
    match Value::both_int(a, b) {
        Some((x, y)) => Value::Int(x.wrapping_mul(y)),
        None => Value::Float(a.as_f64() * b.as_f64()),
    }
}

pub fn div(a: Value, b: Value) -> Result<Value, FormulaError> {
    match Value::both_int(a, b) {
        Some((_, 0)) => Err(FormulaError::DivisionByZero),
        Some((x, y)) => Ok(Value::Int(x.wrapping_div(y))),
        None => {
            let denom = b.as_f64();
            if denom == 0.0 {
                Err(FormulaError::DivisionByZero)
            } else {
                Ok(Value::Float(a.as_f64() / denom))
            }
        }
    }
}

pub fn rem(a: Value, b: Value) -> Result<Value, FormulaError> {
    match Value::both_int(a, b) {
        Some((_, 0)) => Err(FormulaError::DivisionByZero),
        Some((x, y)) => Ok(Value::Int(x.wrapping_rem(y))),
        None => {
            let denom = b.as_f64();
            if denom == 0.0 {
                Err(FormulaError::DivisionByZero)
            } else {
                Ok(Value::Float(a.as_f64() % denom))
            }
        }
    }
}

pub fn pow(a: Value, b: Value) -> Value {
    Value::Float(a.as_f64().powf(b.as_f64()))
}

pub fn compare(a: Value, b: Value) -> std::cmp::Ordering {
    match Value::both_int(a, b) {
        Some((x, y)) => x.cmp(&y),
        None => a.as_f64().partial_cmp(&b.as_f64()).unwrap_or(std::cmp::Ordering::Equal),
    }
}

pub fn bool_value(b: bool) -> Value {
    Value::Int(if b { 1 } else { 0 })
}
