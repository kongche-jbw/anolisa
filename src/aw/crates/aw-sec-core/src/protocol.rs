//! Strict native response fields; opaque evidence is consumed but never projected.

use crate::{Error, PROTOCOL_PROFILE};
use serde::{
    de::{self, MapAccess, Visitor},
    Deserialize, Deserializer,
};
use serde_json::Value;
use std::{collections::BTreeMap, fmt};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Response {
    pub ok: bool,
    pub verdict: String,
    pub summary: Summary,
    pub findings: Vec<Finding>,
    #[serde(rename = "elapsed_ms")]
    _elapsed_ms: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Summary {
    pub total: u64,
    #[serde(deserialize_with = "counts")]
    pub by_type: BTreeMap<String, u64>,
    #[serde(deserialize_with = "counts")]
    pub by_category: BTreeMap<String, u64>,
    #[serde(deserialize_with = "counts")]
    pub by_severity: BTreeMap<String, u64>,
    pub source: String,
    pub bytes_scanned: u64,
    pub truncated: bool,
    pub custom_rules: CustomRules,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CustomRules {
    pub status: String,
    pub rule_count: u64,
    runtime_error_count: u64,
    budget_exhausted: bool,
    truncated: bool,
    ruleset_sha256: Option<String>,
    error_code: Option<String>,
}

impl CustomRules {
    /// Admits only complete native default-detector runs and their rule identities.
    pub fn rulesets(&self) -> Result<Vec<String>, Error> {
        if self.status == "invalid"
            || self.runtime_error_count != 0
            || self.budget_exhausted
            || self.truncated
            || self.error_code.is_some()
        {
            return Err(Error::IncompleteCoverage);
        }
        let mut ids = vec![PROTOCOL_PROFILE.to_owned()];
        match self.status.as_str() {
            "absent" if self.rule_count == 0 && self.ruleset_sha256.is_none() => {}
            "loaded" if self.rule_count <= 100 => {
                let digest = self
                    .ruleset_sha256
                    .as_deref()
                    .ok_or(Error::InvalidResponse)?;
                if digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                {
                    return Err(Error::InvalidResponse);
                }
                ids.push(format!("agent-sec.custom/sha256:{digest}"));
            }
            _ => return Err(Error::InvalidResponse),
        }
        Ok(ids)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Finding {
    #[serde(rename = "type")]
    pub kind: String,
    pub category: String,
    pub severity: String,
    pub confidence: f64,
    #[serde(rename = "evidence_redacted")]
    _evidence_redacted: String,
    span: Span,
    #[serde(rename = "metadata")]
    _metadata: BTreeMap<String, Value>,
}

impl Finding {
    /// Checks native rule names and character spans without sanitizing identifiers.
    pub fn validate(&self, chars: u64, include_low: bool) -> Result<(), Error> {
        if self.kind.is_empty()
            || self.kind.len() > 64
            || !self.kind.as_bytes()[0].is_ascii_lowercase()
            || !self
                .kind
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
            || !(0.0..=1.0).contains(&self.confidence)
            || (!include_low && self.confidence < 0.5)
            || self.span.start >= self.span.end
            || self.span.end > chars
        {
            return Err(Error::InvalidResponse);
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Span {
    start: u64,
    end: u64,
}

fn counts<'de, D: Deserializer<'de>>(deserializer: D) -> Result<BTreeMap<String, u64>, D::Error> {
    struct Counts;
    impl<'de> Visitor<'de> for Counts {
        type Value = BTreeMap<String, u64>;
        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("unique finding counters")
        }
        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut counts = BTreeMap::new();
            while let Some((key, count)) = map.next_entry::<String, u64>()? {
                if counts.insert(key, count).is_some() {
                    return Err(de::Error::custom("duplicate counter"));
                }
            }
            Ok(counts)
        }
    }
    deserializer.deserialize_map(Counts)
}
