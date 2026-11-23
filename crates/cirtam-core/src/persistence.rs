use crate::circuit::Circuit;
use serde::{Deserialize, Serialize};

pub const CIRTAM_EXTENSION: &str = "crtm";
pub const CIRTAM_I_EXTENSION: &str = "crtmi";

#[derive(Serialize, Deserialize)]
struct Document {
    format: String,
    version: u32,
    circuit: Circuit,
}

pub fn encode(circuit: &Circuit) -> Result<String, String> {
    circuit.validate()?;
    serde_json::to_string_pretty(&Document {
        format: "cirtam".into(),
        version: if circuit.connections.is_some() { 2 } else { 1 },
        circuit: circuit.clone(),
    })
    .map_err(|error| error.to_string())
}

pub fn decode(text: &str) -> Result<Circuit, String> {
    let header: serde_json::Value =
        serde_json::from_str(text).map_err(|error| error.to_string())?;
    if !matches!(
        (header["format"].as_str(), header["version"].as_u64()),
        (Some("cirtam" | "cirmat"), Some(1)) | (Some("cirtam"), Some(2))
    ) {
        return Err("Unsupported circuit file format or version.".into());
    }
    let document: Document = serde_json::from_value(header).map_err(|error| error.to_string())?;
    if document.version == 2 && document.circuit.connections.is_none() {
        return Err("Version 2 requires explicit connection records.".into());
    }
    document.circuit.validate_geometry()?;
    Ok(document.circuit)
}
