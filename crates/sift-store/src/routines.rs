//! Saved routines: habits the user accepted as standing tasks.
//!
//! A routine suggestion mined from the decision log is ephemeral until the
//! product has somewhere durable to put the accepted task. This module is that
//! place: it owns CRUD over the routine set, the per-routine run mode, and the
//! set of suggestions the user dismissed. The actual scheduling lives in the
//! monitor/background layer; this is only the persisted definition.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Default ceiling on saved routines.
pub const DEFAULT_ROUTINE_CAP: usize = 100;

/// How a routine runs when its schedule or trigger fires.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RoutineMode {
    /// Run unattended; only `safe` conclusions are ever removed.
    Auto,
    /// Notify and wait for explicit confirmation before removing anything.
    #[default]
    Approve,
}

/// One accepted routine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineEntry {
    pub id: String,
    pub title: String,
    /// Candidate family token the routine acts on.
    pub kind: String,
    /// Human-readable schedule label (a translation key).
    pub cadence: String,
    pub average_bytes: u64,
    pub mode: RoutineMode,
    /// Explicit user-chosen targets for a path-based routine (e.g. a folder
    /// picked from the explorer). Empty for a kind-based routine, whose targets
    /// resolve from the cleanable list when the routine runs.
    #[serde(default)]
    pub paths: Vec<String>,
}

/// On-disk shape: definitions plus dismissed suggestion keys.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineDoc {
    pub entries: Vec<RoutineEntry>,
    pub ignored_suggestions: Vec<String>,
}

/// Bounded set of saved routines.
#[derive(Debug, Clone, Default)]
pub struct RoutineList {
    entries: Vec<RoutineEntry>,
    ignored: HashSet<String>,
    cap: usize,
}

/// Stable key identifying one mined suggestion.
pub fn suggestion_key(name: &str, kind: &str) -> String {
    format!("{kind}:{name}")
}

impl RoutineList {
    pub fn with_cap(cap: usize) -> Self {
        Self {
            entries: Vec::new(),
            ignored: HashSet::new(),
            cap,
        }
    }

    pub fn from_doc(mut doc: RoutineDoc, cap: usize) -> Self {
        doc.entries.truncate(cap);
        Self {
            entries: doc.entries,
            ignored: doc.ignored_suggestions.into_iter().collect(),
            cap,
        }
    }

    pub fn to_doc(&self) -> RoutineDoc {
        RoutineDoc {
            entries: self.entries.clone(),
            ignored_suggestions: self.ignored.iter().cloned().collect(),
        }
    }

    pub fn entries(&self) -> &[RoutineEntry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn get(&self, id: &str) -> Option<&RoutineEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// Whether a mined suggestion was previously dismissed.
    pub fn is_ignored(&self, key: &str) -> bool {
        self.ignored.contains(key)
    }

    /// Add or replace one routine.
    pub fn upsert(&mut self, entry: RoutineEntry) {
        if let Some(slot) = self
            .entries
            .iter_mut()
            .find(|existing| existing.id == entry.id)
        {
            *slot = entry;
            return;
        }
        if self.entries.len() < self.cap {
            self.entries.push(entry);
        }
    }

    /// Remove one routine; returns whether it existed.
    pub fn remove(&mut self, id: &str) -> bool {
        if let Some(pos) = self.entries.iter().position(|entry| entry.id == id) {
            self.entries.remove(pos);
            true
        } else {
            false
        }
    }

    /// Forget one mined suggestion for good.
    pub fn ignore_suggestion(&mut self, key: String) {
        self.ignored.insert(key);
    }

    /// Flip one routine between auto-run and approve-before-run.
    pub fn toggle_mode(&mut self, id: &str) -> bool {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
            entry.mode = match entry.mode {
                RoutineMode::Auto => RoutineMode::Approve,
                RoutineMode::Approve => RoutineMode::Auto,
            };
            true
        } else {
            false
        }
    }
}
