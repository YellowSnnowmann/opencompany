use super::*;
use crate::analytics::RecordingTracker;
use crate::ports::usage::SampleKind;
use std::sync::Mutex;

#[derive(Default)]
struct InMemory {
    rows: Mutex<Vec<UsageSample>>,
}

#[async_trait]
impl UsageMeter for InMemory {
    async fn record(&self, _company: &CompanyId, sample: &UsageSample) -> Result<()> {
        self.rows.lock().unwrap().push(sample.clone());
        Ok(())
    }
    async fn query(&self, _company: &CompanyId, _since: u64) -> Result<Vec<UsageSample>> {
        Ok(self.rows.lock().unwrap().clone())
    }
}

struct Failing;

#[async_trait]
impl UsageMeter for Failing {
    async fn record(&self, _company: &CompanyId, _sample: &UsageSample) -> Result<()> {
        Err(crate::OpenCompanyError::Store("nope".into()))
    }
    async fn query(&self, _company: &CompanyId, _since: u64) -> Result<Vec<UsageSample>> {
        Ok(Vec::new())
    }
}

fn sample() -> UsageSample {
    UsageSample {
        at_millis: 1,
        agent: "maya".into(),
        provider: "openrouter".into(),
        input_tokens: 10,
        output_tokens: 4,
        cached_input_tokens: 2,
        cost_usd: 0.5,
        kind: SampleKind::Inference,
        run_id: Some("run-1".into()),
        model: None,
    }
}

#[tokio::test]
async fn a_recorded_sample_is_reported_and_still_stored() {
    let inner = Arc::new(InMemory::default());
    let tracker = Arc::new(RecordingTracker::new());
    let meter = TrackingUsageMeter::new(inner.clone(), tracker.clone());

    meter
        .record(&CompanyId::new("acme"), &sample())
        .await
        .unwrap();

    assert_eq!(inner.rows.lock().unwrap().len(), 1, "the write still lands");
    assert_eq!(
        tracker.events(),
        vec![Event::TurnMetered {
            kind: "inference",
            provider: "openrouter",
            model: None,
            input_tokens: 10,
            output_tokens: 4,
            cached_input_tokens: 2,
            cost_usd: 0.5,
            attributed_to_run: true,
        }]
    );
}

/// The model the sample was classified against reaches the event. This is
/// the seam #1749 stops at: it puts a [`ModelSlug`] on every sample, and
/// without this forwarding the fleet-wide "what is the spend going to?"
/// question is answerable from a company's own meter and from nowhere else.
///
/// [`ModelSlug`]: crate::metering::ModelSlug
#[tokio::test]
async fn a_sample_model_reaches_the_event() {
    let inner = Arc::new(InMemory::default());
    let tracker = Arc::new(RecordingTracker::new());
    let meter = TrackingUsageMeter::new(inner, tracker.clone());

    let mut with_model = sample();
    with_model.model = Some(crate::metering::ModelSlug::classify(
        "anthropic/claude-sonnet-4-6",
    ));
    meter
        .record(&CompanyId::new("acme"), &with_model)
        .await
        .unwrap();

    assert_eq!(
        tracker.events(),
        vec![Event::TurnMetered {
            kind: "inference",
            provider: "openrouter",
            model: Some("anthropic-sonnet"),
            input_tokens: 10,
            output_tokens: 4,
            cached_input_tokens: 2,
            cost_usd: 0.5,
            attributed_to_run: true,
        }]
    );
}

/// A sample that did not persist is not usage that happened, so it is not
/// reported. Without this the metered counts would drift above the meter's
/// own rows on any store fault.
#[tokio::test]
async fn a_failed_write_reports_nothing() {
    let tracker = Arc::new(RecordingTracker::new());
    let meter = TrackingUsageMeter::new(Arc::new(Failing), tracker.clone());

    assert!(
        meter
            .record(&CompanyId::new("acme"), &sample())
            .await
            .is_err()
    );
    assert!(tracker.events().is_empty());
}

/// The point of the wrapper, stated as a test: the agent name and the raw
/// provider are in the sample and reach nothing.
#[tokio::test]
async fn the_agent_name_never_reaches_the_event() {
    let inner = Arc::new(InMemory::default());
    let tracker = Arc::new(RecordingTracker::new());
    let meter = TrackingUsageMeter::new(inner, tracker.clone());

    let mut hostile = sample();
    hostile.agent = "project-titan-ceo".into();
    hostile.provider = "mcp:acme-internal-crm".into();
    meter
        .record(&CompanyId::new("acme"), &hostile)
        .await
        .unwrap();

    let rendered = format!("{:?}", tracker.events());
    assert!(!rendered.contains("project-titan-ceo"), "{rendered}");
    assert!(!rendered.contains("acme-internal-crm"), "{rendered}");
    assert!(
        rendered.contains("\"mcp\""),
        "the shape survives: {rendered}"
    );
}
