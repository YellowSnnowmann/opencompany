//! Lifetime task-cost reconciliation.
//!
//! Attempt usage is durable on `RunRecord`; planning usage is durable on the
//! task because planning deliberately has no run. This module is the only place
//! those sources are added and lineage is rolled up.

use std::collections::{HashMap, HashSet};

use crate::ports::runs::RunRecord;
use crate::ports::tasks::TaskRecord;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct CostEntry {
    pub key: String,
    pub at_millis: u64,
    pub label: String,
    pub amount_usd: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct CostTotal {
    pub own_usd: f64,
    pub total_usd: f64,
}

#[derive(Clone, Debug, Default)]
pub(super) struct TaskCosts {
    pub entries: HashMap<String, Vec<CostEntry>>,
    pub totals: HashMap<String, CostTotal>,
    pub run_ids: HashMap<String, HashSet<String>>,
}

/// Reconciles task-owned planning calls and every attempt, then rolls each
/// child's total into its parent. `live_run_usd` comes from the durable usage
/// meter and only advances an unsettled run beyond its last stored snapshot.
pub(super) fn reconcile(
    tasks: &[TaskRecord],
    runs: &[RunRecord],
    live_run_usd: &HashMap<String, f64>,
) -> TaskCosts {
    let task_ids: HashSet<&str> = tasks.iter().map(|task| task.id.as_str()).collect();
    let mut out = TaskCosts::default();

    for task in tasks {
        let entries = out.entries.entry(task.id.clone()).or_default();
        for (index, attempt) in task.planning_attempts.iter().enumerate() {
            if attempt.usage.cost_usd > 0.0 {
                entries.push(CostEntry {
                    key: format!("planning:{}:{index}", attempt.at_millis),
                    at_millis: attempt.at_millis,
                    label: "Planning pass".to_string(),
                    amount_usd: attempt.usage.cost_usd,
                });
            }
        }
    }

    for run in runs {
        let Some(task_id) = run.task_id.as_ref() else {
            continue;
        };
        if !task_ids.contains(task_id.as_str()) {
            continue;
        }
        out.run_ids
            .entry(task_id.clone())
            .or_default()
            .insert(run.id.clone());
        let amount_usd = live_run_usd
            .get(&run.id)
            .copied()
            .unwrap_or_default()
            .max(run.usage.cost_usd);
        if amount_usd <= 0.0 {
            continue;
        }
        out.entries
            .entry(task_id.clone())
            .or_default()
            .push(CostEntry {
                key: format!("run:{}", run.id),
                at_millis: run
                    .finished_at_millis
                    .or(run.started_at_millis)
                    .unwrap_or(run.created_at_millis),
                label: format!("Attempt {} · {}", run.attempt, run.status),
                amount_usd,
            });
    }

    for entries in out.entries.values_mut() {
        entries.sort_by(|a, b| {
            a.at_millis
                .cmp(&b.at_millis)
                .then_with(|| a.key.cmp(&b.key))
        });
    }

    let own: HashMap<String, f64> = tasks
        .iter()
        .map(|task| {
            let amount = out
                .entries
                .get(&task.id)
                .into_iter()
                .flatten()
                .map(|entry| entry.amount_usd)
                .sum();
            (task.id.clone(), amount)
        })
        .collect();
    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    for task in tasks {
        if let Some(parent) = &task.parent_task_id
            && task_ids.contains(parent.as_str())
        {
            children
                .entry(parent.clone())
                .or_default()
                .push(task.id.clone());
        }
    }

    fn total_for(
        id: &str,
        own: &HashMap<String, f64>,
        children: &HashMap<String, Vec<String>>,
        visiting: &mut HashSet<String>,
        memo: &mut HashMap<String, f64>,
    ) -> f64 {
        if let Some(total) = memo.get(id) {
            return *total;
        }
        if !visiting.insert(id.to_string()) {
            return own.get(id).copied().unwrap_or_default();
        }
        let total = own.get(id).copied().unwrap_or_default()
            + children
                .get(id)
                .into_iter()
                .flatten()
                .map(|child| total_for(child, own, children, visiting, memo))
                .sum::<f64>();
        visiting.remove(id);
        memo.insert(id.to_string(), total);
        total
    }

    let mut memo = HashMap::new();
    for task in tasks {
        let total_usd = total_for(&task.id, &own, &children, &mut HashSet::new(), &mut memo);
        out.totals.insert(
            task.id.clone(),
            CostTotal {
                own_usd: own.get(&task.id).copied().unwrap_or_default(),
                total_usd,
            },
        );
    }
    out
}

#[cfg(test)]
#[path = "task_cost_tests.rs"]
mod tests;
