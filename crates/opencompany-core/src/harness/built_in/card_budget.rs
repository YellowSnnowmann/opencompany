//! How many cards one conversation may still open, decided in the turn that
//! asks.
//!
//! A HiveMind room runs several seats against one operator message, and each
//! of them carries `spawn_task`. Without a shared budget one message becomes as
//! many cards as there are seats, and two seats tracking the same work open it
//! twice. The budget is installed for a seat's turn the way the publish and
//! approval claims are, so `spawn_task` can refuse in that turn rather than
//! the card silently not appearing later.

use std::sync::Arc;

/// Why a card could not be reserved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardRefusal {
    /// A card with the same title is already open or queued here.
    Duplicate,
    /// This conversation has already opened `cap` cards.
    Full {
        /// The most cards one conversation may open.
        cap: usize,
    },
}

/// A conversation's remaining room for cards.
pub trait CardBudget: Send + Sync {
    /// Holds one card under `title`, or says why it cannot be opened.
    ///
    /// # Errors
    ///
    /// [`CardRefusal`] when the title is taken or the budget is spent.
    fn reserve(&self, title: &str) -> Result<(), CardRefusal>;
    /// Gives back a hold that will not become a card.
    fn release(&self, title: &str);
}

tokio::task_local! {
    static BUDGET: Arc<dyn CardBudget>;
}

/// Runs `fut` with `budget` deciding every `spawn_task` inside it.
pub async fn scoped<F, T>(budget: Arc<dyn CardBudget>, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    BUDGET.scope(budget, fut).await
}

/// Reserves `title` against the budget in scope, or `None` when this turn
/// runs under none.
pub fn reserve(title: &str) -> Option<Result<(), CardRefusal>> {
    BUDGET.try_with(|budget| budget.reserve(title)).ok()
}

/// Releases `title` back to the budget in scope, if there is one.
pub fn release(title: &str) {
    let _ = BUDGET.try_with(|budget| budget.release(title));
}

/// A title reduced to what makes two titles the same card: lower case,
/// letters and digits, single spaces.
#[must_use]
pub fn normalize_title(title: &str) -> String {
    title
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
#[path = "card_budget_tests.rs"]
mod tests;
