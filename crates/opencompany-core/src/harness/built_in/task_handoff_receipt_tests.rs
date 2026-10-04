use super::*;
fn handoff(name: &str) -> Delegation {
    Delegation::DelegateToTeammate {
        teammate: name.into(),
        instruction: "Synthetic review".into(),
    }
}

