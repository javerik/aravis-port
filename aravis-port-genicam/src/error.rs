/// Errors from XML parsing, tree construction, or feature access.
#[derive(Debug, thiserror::Error)]
pub enum GenIcamError {
    #[error("xml parse error: {0}")]
    Xml(String),

    #[error("feature '{0}' not found")]
    NotFound(String),

    #[error("feature '{name}': expected {expected}, found {found}")]
    TypeMismatch {
        name: String,
        expected: &'static str,
        found: &'static str,
    },

    #[error("feature '{0}' is not writable")]
    NotWritable(String),

    #[error("enum entry '{entry}' not found on '{name}'")]
    UnknownEnumEntry { name: String, entry: String },

    #[error("register i/o error: {0}")]
    Io(String),

    #[error("formula error: {0}")]
    Formula(#[from] FormulaError),

    #[error("dangling node reference: {0}")]
    DanglingReference(String),
}

pub type Result<T> = std::result::Result<T, GenIcamError>;

/// Errors from formula tokenization, parsing, or evaluation.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FormulaError {
    #[error("unexpected character '{0}' at position {1}")]
    UnexpectedChar(char, usize),

    #[error("unexpected end of expression")]
    UnexpectedEnd,

    #[error("expected {expected}, found '{found}'")]
    Expected {
        expected: &'static str,
        found: String,
    },

    #[error("unknown variable '{0}'")]
    UnknownVariable(String),

    #[error("unknown function '{0}'")]
    UnknownFunction(String),

    #[error("wrong number of arguments to '{name}': expected {expected}, got {got}")]
    WrongArgCount {
        name: String,
        expected: &'static str,
        got: usize,
    },

    #[error("division by zero")]
    DivisionByZero,

    #[error("recursive sub-expression reference: '{0}'")]
    RecursiveSubExpression(String),

    #[error("unknown sub-expression or constant '{0}'")]
    UnknownSubExpressionOrConstant(String),

    #[error("trailing input after expression: '{0}'")]
    TrailingInput(String),
}
