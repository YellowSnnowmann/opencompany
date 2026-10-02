use super::*;
use std::sync::Mutex;

struct Counting {
    cap: usize,
    held: Mutex<Vec<String>>,
}

impl CardBudget for Counting {
    fn reserve(&self, title: &str) -> Result<(), CardRefusal> {
        let mut held = self.held.lock().unwrap();
        let key = normalize_title(title);
        if held.contains(&key) {
            return Err(CardRefusal::Duplicate);
        }
        if held.len() >= self.cap {
            return Err(CardRefusal::Full { cap: self.cap });
        }
        held.push(key);
        Ok(())
    }

    fn release(&self, title: &str) {
        let key = normalize_title(title);
        self.held.lock().unwrap().retain(|held| *held != key);
    }
}

#[test]
fn titles_that_differ_only_in_case_and_punctuation_are_one_card() {
    assert_eq!(
        normalize_title("Draft the  Launch-Post!"),
        "draft the launch post"
    );
    assert_eq!(
        normalize_title("draft the launch post"),
        normalize_title("DRAFT: the launch post.")
    );
    assert_ne!(normalize_title("Draft A"), normalize_title("Draft B"));
}

#[tokio::test]
async fn outside_a_scope_nothing_is_reserved() {
    assert_eq!(reserve("Anything"), None);
    release("Anything");
}

#[tokio::test]
async fn inside_a_scope_the_budget_decides_and_a_release_frees_the_title() {
    let budget: Arc<dyn CardBudget> = Arc::new(Counting {
        cap: 2,
        held: Mutex::new(Vec::new()),
    });
    scoped(budget, async {
        assert_eq!(reserve("One"), Some(Ok(())));
        assert_eq!(reserve("one."), Some(Err(CardRefusal::Duplicate)));
        assert_eq!(reserve("Two"), Some(Ok(())));
        assert_eq!(reserve("Three"), Some(Err(CardRefusal::Full { cap: 2 })));
        release("Two");
        assert_eq!(reserve("Three"), Some(Ok(())));
    })
    .await;
}
