// Copyright (c) 2026 YXWJ.
// All rights reserved.
//
// File: sandbox_protocol.rs
// Description: Versioned, bounded protocol for isolated Windows workers.
// A protocol request never authorizes arbitrary host commands by itself.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
pub const MAX_REQUEST_BYTES: usize = 4096;
pub const MAX_RECEIPT_BYTES: usize = 4096;
pub const MAX_INPUT_BYTES: usize = 2 * 1024 * 1024;
pub const RECEIPT_NAME: &str = "worker-receipt.json";

/// One approved, non-shell operation. Additional operations require their
/// own explicit policy and isolation verification.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    HashInput,
}

/// An untrusted job request decoded within a freshly created AppContainer.
/// A stable schema and exact input allowlist prevent inadvertent broadening
/// when the future command dispatcher receives remote requests.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRequest {
    pub version: u32,
    pub job_id: String,
    pub operation: Operation,
    pub input: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerReceipt {
    pub version: u32,
    pub job_id: String,
    pub input: String,
    pub input_bytes: usize,
    pub sha256: String,
}

fn valid_job_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// Only relative files under the per-job input tree are accepted.
/// Reject Windows drive and alternate data-stream syntax independently of
/// the host OS to avoid cross-platform path interpretation differences.
fn valid_input_path(value: &str) -> bool {
    if value.len() > 256 || value.contains(['\\', ':', '\0']) {
        return false;
    }
    let Some(tail) = value.strip_prefix("inputs/") else {
        return false;
    };
    let segments = tail.split('/').collect::<Vec<_>>();
    !segments.is_empty()
        && segments.len() <= 8
        && segments.iter().all(|segment| {
            !segment.is_empty()
                && segment.len() <= 64
                && !segment.starts_with('.')
                && segment.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric()
                        || matches!(byte, b'_' | b'-' | b'.')
                })
        })
}

fn parse_bounded<T: for<'de> Deserialize<'de>>(bytes: &[u8], limit: usize) -> Result<T> {
    if bytes.is_empty() || bytes.len() > limit {
        bail!("Sandbox protocol document is empty or exceeds {limit} bytes");
    }
    serde_json::from_slice(bytes).context("Invalid sandbox protocol JSON")
}

impl WorkerRequest {
    pub fn new(job_id: &str, input: &str) -> Result<Self> {
        let result = Self {
            version: VERSION,
            job_id: job_id.to_owned(),
            operation: Operation::HashInput,
            input: input.to_owned(),
        };
        result.validate()?;
        Ok(result)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let value: Self = parse_bounded(bytes, MAX_REQUEST_BYTES)?;
        value.validate()?;
        Ok(value)
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() > MAX_REQUEST_BYTES {
            bail!("Sandbox request exceeds protocol byte limit");
        }
        Ok(bytes)
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != VERSION {
            bail!("Unsupported sandbox protocol version {}", self.version);
        }
        if !valid_job_id(&self.job_id) {
            bail!("Sandbox job ID must contain 1..64 ASCII letters, digits, '-' or '_'");
        }
        if !valid_input_path(&self.input) {
            bail!("Sandbox input must be a safe relative file under inputs/");
        }
        Ok(())
    }
}

impl WorkerReceipt {
    pub fn for_input(request: &WorkerRequest, bytes: &[u8]) -> Result<Self> {
        request.validate()?;
        if bytes.len() > MAX_INPUT_BYTES {
            bail!("Sandbox input is larger than the approved file limit");
        }
        Ok(Self {
            version: VERSION,
            job_id: request.job_id.clone(),
            input: request.input.clone(),
            input_bytes: bytes.len(),
            sha256: crate::util::digest(bytes),
        })
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let value: Self = parse_bounded(bytes, MAX_RECEIPT_BYTES)?;
        if value.version != VERSION
            || !valid_job_id(&value.job_id)
            || !valid_input_path(&value.input)
            || value.input_bytes > MAX_INPUT_BYTES
            || value.sha256.len() != 64
            || !value.sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            bail!("Sandbox receipt contains an invalid version or untrusted fields");
        }
        Ok(value)
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let bytes = serde_json::to_vec(self)?;
        let _ = Self::decode(&bytes)?;
        Ok(bytes)
    }

    /// The broker must supply its OWN securely opened input bytes. A receipt
    /// cannot establish its own authenticity merely by presenting a digest.
    pub fn verify_against(&self, request: &WorkerRequest, trusted_bytes: &[u8]) -> Result<()> {
        request.validate()?;
        if self.version != VERSION
            || self.job_id != request.job_id
            || self.input != request.input
            || self.input_bytes != trusted_bytes.len()
            || self.sha256 != crate::util::digest(trusted_bytes)
        {
            bail!("Sandbox receipt did not match the independently verified request and input");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip_accepts_only_staged_inputs() {
        let request = WorkerRequest::new("job_a-12", "inputs/src/data.txt").unwrap();
        assert_eq!(WorkerRequest::decode(&request.encode().unwrap()).unwrap(), request);
        for input in [
            "../secret", ".env", "inputs/.env", "inputs/../secrets",
            "inputs//bad", "inputs/file:stream", "inputs\\secret.txt",
            r"C:\Users\Public\secret", "inputs/file/./source",
            "inputs/trailing/", "inputs/file?",
        ] {
            assert!(WorkerRequest::new("job_a-12", input).is_err(), "{input}");
        }
    }

    #[test]
    fn unrecognized_or_oversized_protocol_is_rejected() {
        let valid = WorkerRequest::new("job_1", "inputs/file.txt").unwrap();
        let mut unknown: serde_json::Value =
            serde_json::from_slice(&valid.encode().unwrap()).unwrap();
        unknown["shell"] = serde_json::json!("powershell");
        assert!(WorkerRequest::decode(&serde_json::to_vec(&unknown).unwrap()).is_err());
        unknown["version"] = serde_json::json!(2);
        assert!(WorkerRequest::decode(&serde_json::to_vec(&unknown).unwrap()).is_err());
        assert!(WorkerRequest::decode(&vec![b'x'; MAX_REQUEST_BYTES + 1]).is_err());
        assert!(WorkerRequest::decode(b"").is_err());
        assert!(WorkerRequest::decode(
            br#"{"version":1,"version":1,"job_id":"a","operation":"hash_input","input":"inputs/file"}"#
        ).is_err());
        for id in ["", "bad/id", "job with spaces", "😀"] {
            assert!(WorkerRequest::new(id, "inputs/file").is_err());
        }
    }

    #[test]
    fn receipt_is_verified_using_independent_input_bytes() {
        let request = WorkerRequest::new("job_2", "inputs/code.rs").unwrap();
        let original = b"fn main() {}\n";
        let receipt = WorkerReceipt::for_input(&request, original).unwrap();
        let decoded = WorkerReceipt::decode(&receipt.encode().unwrap()).unwrap();
        decoded.verify_against(&request, original).unwrap();
        assert!(decoded.verify_against(&request, b"modified file").is_err());
        let other = WorkerRequest::new("different", "inputs/code.rs").unwrap();
        assert!(decoded.verify_against(&other, original).is_err());
        let mut invalid = serde_json::to_value(&receipt).unwrap();
        invalid["sha256"] = serde_json::json!("not-a-valid-sha");
        assert!(WorkerReceipt::decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
        invalid["sha256"] = serde_json::json!(crate::util::digest(original));
        invalid["unexpected"] = serde_json::json!(true);
        assert!(WorkerReceipt::decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
}
