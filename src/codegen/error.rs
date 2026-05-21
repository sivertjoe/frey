use inkwell::builder::BuilderError;

#[derive(Debug)]
pub enum Error {
    Builder(BuilderError),
    IrWrite(String),
}

impl From<BuilderError> for Error {
    fn from(e: BuilderError) -> Self {
        Error::Builder(e)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Builder(e) => write!(f, "codegen error: {e}"),
            Error::IrWrite(msg) => write!(f, "failed to write IR: {msg}"),
        }
    }
}

impl std::error::Error for Error {}
