//! LVS job: the two netlists to compare.
//!
//! A `.lvs` job is a tiny `key: value` file (std-only parser — no deps):
//!
//! ```text
//! layout:    block_layout.spice      # the layout-extracted netlist (side A)
//! schematic: block_schematic.spice   # the reference/schematic netlist (side B)
//! top:       block                   # subckt to compare (optional; else top-level)
//! ```

use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct LvsJob {
    pub layout: Option<String>,     // layout-extracted SPICE netlist (side A)
    pub layout_gds: Option<String>, // OR a GDS to extract natively (needs `rules`)
    pub rules: Option<String>,      // extraction rules for `layout_gds`
    pub schematic: String,
    pub top: Option<String>,
    pub base_dir: String,
}

impl LvsJob {
    pub fn resolve(&self, rel: &str) -> String {
        let p = Path::new(rel);
        if p.is_absolute() || self.base_dir.is_empty() {
            rel.to_string()
        } else {
            Path::new(&self.base_dir).join(rel).to_string_lossy().into_owned()
        }
    }

    pub fn parse(text: &str, base_dir: &str) -> Result<LvsJob, JobError> {
        let mut kv: BTreeMap<String, String> = BTreeMap::new();
        for raw in text.lines() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let (k, v) = line
                .split_once(':')
                .ok_or_else(|| JobError(format!("expected 'key: value', got {line:?}")))?;
            kv.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
        let get = |k: &str| kv.get(k).cloned().ok_or_else(|| JobError(format!("missing key: {k}")));
        let layout = kv.get("layout").filter(|s| !s.is_empty()).cloned();
        let layout_gds = kv.get("layout_gds").filter(|s| !s.is_empty()).cloned();
        if layout.is_none() && layout_gds.is_none() {
            return Err(JobError("need `layout` (SPICE) or `layout_gds` + `rules`".into()));
        }
        Ok(LvsJob {
            layout,
            layout_gds,
            rules: kv.get("rules").filter(|s| !s.is_empty()).cloned(),
            schematic: get("schematic")?,
            top: kv.get("top").filter(|s| !s.is_empty()).cloned(),
            base_dir: base_dir.to_string(),
        })
    }

    pub fn load(path: &str) -> Result<LvsJob, JobError> {
        let text = std::fs::read_to_string(path).map_err(|e| JobError(format!("{path}: {e}")))?;
        let base = Path::new(path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        LvsJob::parse(&text, &base)
    }
}

#[derive(Debug)]
pub struct JobError(pub String);
impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "job error: {}", self.0)
    }
}
impl std::error::Error for JobError {}
