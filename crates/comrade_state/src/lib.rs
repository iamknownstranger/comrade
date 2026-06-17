/*!
 * comrade_state — Progressive-Disclosure Application State Machine
 *
 * Manages workspace context transitions between Base (Sabha + Vault),
 * OffGridTravel (Saathi mesh), and CoupleSandbox (Sakha/Sakhi realm).
 */

use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

// ── Role within the Coupled Sandbox ─────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PairRole {
    /// Optimised environment for the Boyfriend/Male partner
    Sakha,
    /// Optimised environment for the Girlfriend/Female partner
    Sakhi,
}

impl fmt::Display for PairRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PairRole::Sakha => write!(f, "Sakha (Boyfriend)"),
            PairRole::Sakhi => write!(f, "Sakhi (Girlfriend)"),
        }
    }
}

// ── Primary workspace discriminant ──────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppWorkspace {
    /// Default: public microblogging (Sabha) + E2E messaging (Vault)
    Base,
    /// Off-grid modality: remote relays disabled, libp2p mesh active
    OffGridTravel,
    /// Isolated couple sandbox with CRDT ledger and paired key context
    CoupleSandbox(PairRole),
}

impl AppWorkspace {
    /// Attempt a state transition, enforcing the allowed transition graph.
    ///
    /// Allowed edges:
    ///   Base ↔ OffGridTravel
    ///   Base ↔ CoupleSandbox(_)
    ///
    /// Direct cross-transitions between OffGridTravel and CoupleSandbox are
    /// intentionally blocked to avoid ambiguous UX state.
    pub fn transition_to(&self, target: AppWorkspace) -> Result<AppWorkspace, TransitionError> {
        match (self, &target) {
            // Base → OffGridTravel
            (AppWorkspace::Base, AppWorkspace::OffGridTravel) => Ok(target),
            // OffGridTravel → Base
            (AppWorkspace::OffGridTravel, AppWorkspace::Base) => Ok(target),
            // Base → CoupleSandbox(_)
            (AppWorkspace::Base, AppWorkspace::CoupleSandbox(_)) => Ok(target),
            // CoupleSandbox(_) → Base
            (AppWorkspace::CoupleSandbox(_), AppWorkspace::Base) => Ok(target),
            // No self-transitions
            (from, to) if from == to => Err(TransitionError::AlreadyInState {
                state: format!("{from}"),
            }),
            // Any other cross-transition is invalid
            (from, to) => Err(TransitionError::InvalidTransition {
                from: format!("{from}"),
                to: format!("{to}"),
            }),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            AppWorkspace::Base => "Base — Sabha (Public Feed) + Vault (E2E DMs)",
            AppWorkspace::OffGridTravel => "Off-Grid Travel — Saathi Mesh Active",
            AppWorkspace::CoupleSandbox(PairRole::Sakha) => "Couple Sandbox — Sakha View",
            AppWorkspace::CoupleSandbox(PairRole::Sakhi) => "Couple Sandbox — Sakhi View",
        }
    }

    pub fn is_relay_connected(&self) -> bool {
        !matches!(self, AppWorkspace::OffGridTravel)
    }

    pub fn is_mesh_active(&self) -> bool {
        matches!(self, AppWorkspace::OffGridTravel)
    }

    pub fn is_couple_sandbox(&self) -> bool {
        matches!(self, AppWorkspace::CoupleSandbox(_))
    }

    /// Stable string key for serialisation across FFI / IPC boundaries
    /// (JNI bridge, Tauri commands, etc.). The inverse of [`AppWorkspace::from_key`].
    pub fn key(&self) -> &'static str {
        match self {
            AppWorkspace::Base => "Base",
            AppWorkspace::OffGridTravel => "OffGridTravel",
            AppWorkspace::CoupleSandbox(PairRole::Sakha) => "CoupleSandboxSakha",
            AppWorkspace::CoupleSandbox(PairRole::Sakhi) => "CoupleSandboxSakhi",
        }
    }

    /// Parse a workspace from its stable string [`AppWorkspace::key`].
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "Base" => Some(AppWorkspace::Base),
            "OffGridTravel" => Some(AppWorkspace::OffGridTravel),
            "CoupleSandboxSakha" => Some(AppWorkspace::CoupleSandbox(PairRole::Sakha)),
            "CoupleSandboxSakhi" => Some(AppWorkspace::CoupleSandbox(PairRole::Sakhi)),
            _ => None,
        }
    }

    /// All workspace variants, in canonical display order.
    pub fn all() -> [AppWorkspace; 4] {
        [
            AppWorkspace::Base,
            AppWorkspace::OffGridTravel,
            AppWorkspace::CoupleSandbox(PairRole::Sakha),
            AppWorkspace::CoupleSandbox(PairRole::Sakhi),
        ]
    }
}

impl fmt::Display for AppWorkspace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}

// ── Transition errors ────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum TransitionError {
    #[error("Invalid workspace transition: {from} → {to}")]
    InvalidTransition { from: String, to: String },

    #[error("Already in state: {state}")]
    AlreadyInState { state: String },
}

// ── Runtime context carrying the live workspace ──────────────────────────────

#[derive(Debug)]
pub struct RuntimeContext {
    workspace: AppWorkspace,
    history: Vec<AppWorkspace>,
}

impl RuntimeContext {
    pub fn new() -> Self {
        Self {
            workspace: AppWorkspace::Base,
            history: Vec::new(),
        }
    }

    pub fn current(&self) -> &AppWorkspace {
        &self.workspace
    }

    pub fn transition(&mut self, target: AppWorkspace) -> Result<(), TransitionError> {
        let next = self.workspace.transition_to(target)?;
        let prev = std::mem::replace(&mut self.workspace, next);
        self.history.push(prev);
        tracing::info!(
            workspace = %self.workspace,
            "workspace transition"
        );
        Ok(())
    }

    /// Step back to the previous workspace (undo last transition).
    pub fn step_back(&mut self) -> Option<AppWorkspace> {
        if let Some(prev) = self.history.pop() {
            let current = std::mem::replace(&mut self.workspace, prev);
            tracing::info!(
                workspace = %self.workspace,
                "workspace step-back"
            );
            Some(current)
        } else {
            None
        }
    }
}

impl Default for RuntimeContext {
    fn default() -> Self {
        Self::new()
    }
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_to_offgrid_allowed() {
        let ctx = AppWorkspace::Base;
        let result = ctx.transition_to(AppWorkspace::OffGridTravel);
        assert!(result.is_ok());
    }

    #[test]
    fn offgrid_to_couple_sandbox_blocked() {
        let ctx = AppWorkspace::OffGridTravel;
        let result = ctx.transition_to(AppWorkspace::CoupleSandbox(PairRole::Sakha));
        assert!(result.is_err());
    }

    #[test]
    fn self_transition_is_error() {
        let ctx = AppWorkspace::Base;
        let result = ctx.transition_to(AppWorkspace::Base);
        assert!(result.is_err());
    }

    #[test]
    fn runtime_context_history() {
        let mut ctx = RuntimeContext::new();
        ctx.transition(AppWorkspace::OffGridTravel).unwrap();
        ctx.transition(AppWorkspace::Base).unwrap();
        assert_eq!(ctx.history.len(), 2);
        ctx.step_back();
        // step_back pops OffGridTravel from history, restoring it as current
        assert_eq!(*ctx.current(), AppWorkspace::OffGridTravel);
        assert_eq!(ctx.history.len(), 1);
    }

    #[test]
    fn couple_sandbox_back_to_base() {
        let ctx = AppWorkspace::CoupleSandbox(PairRole::Sakhi);
        let result = ctx.transition_to(AppWorkspace::Base);
        assert!(result.is_ok());
    }

    #[test]
    fn key_from_key_roundtrip() {
        for ws in AppWorkspace::all() {
            let key = ws.key();
            assert_eq!(AppWorkspace::from_key(key), Some(ws.clone()));
        }
        assert_eq!(AppWorkspace::from_key("nonsense"), None);
    }

    #[test]
    fn label_is_non_empty() {
        for ws in [
            AppWorkspace::Base,
            AppWorkspace::OffGridTravel,
            AppWorkspace::CoupleSandbox(PairRole::Sakha),
            AppWorkspace::CoupleSandbox(PairRole::Sakhi),
        ] {
            assert!(!ws.label().is_empty());
        }
    }
}
