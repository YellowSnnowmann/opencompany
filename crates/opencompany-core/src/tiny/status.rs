use serde::Serialize;

/// Runtime integration status for an inherited module.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeModuleStatus {
    /// Module name.
    pub name: &'static str,
    /// Whether the module is compiled into this build.
    pub enabled: bool,
    /// Intended role in OpenCompany.
    pub role: &'static str,
    /// Local source location.
    pub path: &'static str,
}

impl RuntimeModuleStatus {
    /// Returns the status of all inherited runtime modules.
    pub fn all() -> Vec<Self> {
        vec![
            Self {
                name: "tinyagents",
                // The checkout is still `tinyagents`, but it is a workspace now
                // and this crate links two of its members: the harness, and
                // `tinyinference` (vendored under it) for the model layer. The
                // graph/registry/RLM members are openhuman's, not ours.
                enabled: cfg!(feature = "tinyagents-harness"),
                role: "agent harness and inference model layer",
                path: "vendor/openhuman/vendor/tinyagents",
            },
            Self {
                name: "openhuman",
                enabled: true,
                role: "OpenHuman checkout launched through Cargo",
                path: "vendor/openhuman",
            },
        ]
    }
}

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
