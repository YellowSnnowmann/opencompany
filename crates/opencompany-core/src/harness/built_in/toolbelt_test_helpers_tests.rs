use super::*;

pub(crate) fn names(tools: &[Box<dyn Tool>]) -> Vec<&str> {
    tools.iter().map(|t| t.name()).collect()
}

pub(crate) fn test_security(workspace: &Path, mode: PolicyMode) -> Arc<SecurityPolicy> {
    Arc::new(exec_security(workspace, mode))
}
