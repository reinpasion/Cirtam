#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Severity {
    Warning,
    Serious,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Location {
    Component(usize),
    Wire(usize),
}

#[derive(Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub location: Option<Location>,
    pub message: String,
}

impl Diagnostic {
    pub fn serious(location: Option<Location>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Serious,
            location,
            message: message.into(),
        }
    }

    pub fn warning(location: Location, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            location: Some(location),
            message: message.into(),
        }
    }
}
