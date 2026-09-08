//! Reading rules from a TOML file, and checking them against the loaded DBC.
//!
//! A rule naming a signal the DBC does not define would never fire and never
//! explain itself, so every name is checked at load time and anything unknown
//! is reported.

use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use serde::Deserialize;

use super::{Act, Condition, Op, Rule, RuleSet, Source};
use crate::dbc::Database;

/// Looked for next to the DBC and in the working directory.
pub const DEFAULT_NAME: &str = "tuican.rules.toml";

#[derive(Deserialize)]
struct File {
    #[serde(default)]
    rule: Vec<RuleSpec>,
}

#[derive(Deserialize)]
struct RuleSpec {
    name: String,
    #[serde(default = "enabled_by_default")]
    enabled: bool,
    #[serde(default)]
    all: Vec<CondSpec>,
    #[serde(default)]
    any: Vec<CondSpec>,
    then: Vec<ActSpec>,
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Deserialize)]
struct CondSpec {
    message: String,
    signal: String,
    #[serde(default)]
    source: Source,
    #[serde(default)]
    op: Op,
    #[serde(default)]
    value: f64,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum ActSpec {
    Set {
        message: String,
        signal: String,
        value: f64,
    },
    Send {
        message: String,
    },
    Cyclic {
        message: String,
        period_ms: f64,
    },
    Stop {
        message: String,
    },
}

pub fn load(path: &Path, db: &Database) -> anyhow::Result<RuleSet> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let file: File = toml::from_str(&text).map_err(|e| anyhow!("{}: {e}", path.display()))?;

    let mut warnings = Vec::new();
    let mut rules = Vec::new();
    for spec in file.rule {
        // Collected per rule so the panel can point at the one that is broken,
        // not just report a count.
        let mut mine: Vec<String> = Vec::new();
        if spec.then.is_empty() {
            mine.push("has no actions, so it does nothing".into());
        }
        if spec.all.is_empty() && spec.any.is_empty() {
            mine.push("has no conditions, so it never fires".into());
        }
        let convert = |c: CondSpec, warnings: &mut Vec<String>| {
            check(db, &c.message, Some(&c.signal), warnings);
            Condition {
                source: c.source,
                message: c.message,
                signal: c.signal,
                op: c.op,
                value: c.value,
                seen: None,
            }
        };
        let all = spec
            .all
            .into_iter()
            .map(|c| convert(c, &mut mine))
            .collect();
        let any = spec
            .any
            .into_iter()
            .map(|c| convert(c, &mut mine))
            .collect();

        let then = spec
            .then
            .into_iter()
            .map(|a| match a {
                ActSpec::Set {
                    message,
                    signal,
                    value,
                } => {
                    check(db, &message, Some(&signal), &mut mine);
                    Act::Set {
                        message,
                        signal,
                        value,
                    }
                }
                ActSpec::Send { message } => {
                    check(db, &message, None, &mut mine);
                    Act::Send { message }
                }
                ActSpec::Cyclic { message, period_ms } => {
                    check(db, &message, None, &mut mine);
                    if period_ms <= 0.0 {
                        mine.push("period must be above zero".into());
                    }
                    Act::Cyclic { message, period_ms }
                }
                ActSpec::Stop { message } => Act::Stop { message },
            })
            .collect();

        warnings.extend(mine.iter().map(|w| format!("{}: {w}", spec.name)));
        rules.push(Rule {
            name: spec.name,
            enabled: spec.enabled,
            all,
            any,
            then,
            fired: 0,
            last_fired: None,
            warning: mine.into_iter().next(),
            holding: false,
        });
    }

    Ok(RuleSet {
        path: Some(path.to_path_buf()),
        rules,
        warnings,
        looped: false,
    })
}

/// Names checked against the DBC, but only when one is loaded: rules may be
/// loaded first, and complaining then would be noise.
fn check(db: &Database, message: &str, signal: Option<&str>, out: &mut Vec<String>) {
    if db.is_empty() {
        return;
    }
    let Some(def) = db.messages.iter().find(|m| m.name == message) else {
        out.push(format!("no message named {message} in this DBC"));
        return;
    };
    if let Some(signal) = signal
        && !def.signals.iter().any(|s| s.name == signal)
    {
        out.push(format!("{message} has no signal named {signal}"));
    }
}

/// Where to look when the user did not pass `--rules`: beside the DBC first,
/// since rules belong to a board, then the working directory.
pub fn discover(dbc: Option<&Path>) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = dbc.and_then(|p| p.parent()) {
        candidates.push(dir.join(DEFAULT_NAME));
    }
    candidates.push(PathBuf::from(DEFAULT_NAME));
    candidates.into_iter().find(|p| p.is_file())
}
