//! Service work uses the original normalized request ledger, cap policy and exact decimal pricer.
//! These are factual producer ports. They never manufacture provider Use, current source authority or IO.
use crate::rates::{RateBook, Source};
use crate::{Engine, EngineError};
use orch_api_contract::usage::{RequestTiming, ServiceUnitPrice, ServiceUsage, UsageMeasurement};
use orch_db::Conn;
use orch_db::usage::ledger::{self, Attribution, NewEvent, NewSummary, SummaryAttribution, Tokens};
use orch_provider_contract::capabilities::Price;
use orch_types::Id;

/// Existing native attribution facts retained by the service's admitted producer.
#[derive(Debug, Clone, Copy)]
pub enum ServiceSource<'a> {
    /// A real turn; its original segment/environment/thread associations remain intact.
    Turn(&'a Attribution),
    /// Real standalone work, with no invented turn or session.
    Standalone(&'a SummaryAttribution),
}
impl ServiceSource<'_> {
    fn place(self) -> Id {
        match self {
            Self::Turn(source) => source.thread_id,
            Self::Standalone(source) => source.thread_id.unwrap_or(source.topic_id),
        }
    }
}

/// One actual service request, identified by its existing stable native call id.
#[derive(Debug)]
pub struct ServiceUse<'a> {
    /// Stable native request id; replay never creates a second row.
    pub id: Id,
    /// Actual completion time.
    pub at: chrono::DateTime<chrono::Utc>,
    /// Original native source facts, independently of the selected service provider.
    pub source: ServiceSource<'a>,
    /// The actual selected paying account.
    pub provider: Id,
    /// Its actual current Use entry, not the generation provider's entry.
    pub entry: Option<Id>,
    /// Original admitted issuing person; absent for automatic work.
    pub person: Option<Id>,
    /// Actual common capability purpose.
    pub purpose: &'a str,
    /// Exact service offering, never encoded as a fake model name.
    pub offering: &'a str,
    /// Actual selected offering groups retained with this immutable observation.
    pub groups: &'a [String],
    /// The capability declaration's charged unit.
    pub unit: &'a str,
    /// Actual native quantity, including an explicit unknown state.
    pub quantity: UsageMeasurement<i64>,
    /// Trusted selected quote; never supplied as tool arguments or inferred from result counts.
    pub quote: Option<&'a Price>,
    /// The quote's actual authority: provider, rate card or catalogue estimate.
    pub rate_source: Source,
    /// Actual request boundaries/outcome and original vendor cost where reported.
    pub timing: &'a RequestTiming,
    /// Native upstream request identity if supplied.
    pub request_ref: Option<&'a str>,
}
impl Engine {
    /// Count service work AFTER its actual guarded execution, inside that producer's transaction.
    /// Model-backed services use count_own_use with their actual model instead of another unit row.
    pub async fn count_service_use(
        &self,
        conn: &mut Conn,
        used: &ServiceUse<'_>,
    ) -> Result<bool, EngineError> {
        if matches!(used.timing.outcome, UsageMeasurement::Unread) {
            return Err(EngineError::unreadable(
                "a running service request is not a completed usage observation",
            ));
        }
        if used.offering.trim().is_empty() || used.unit.trim().is_empty() {
            return Err(EngineError::unreadable(
                "a service observation lacks its actual offering or unit",
            ));
        }
        let quantity = match &used.quantity {
            UsageMeasurement::Told { value } if *value < 0 => UsageMeasurement::Empty,
            value => value.clone(),
        };
        let book = RateBook::load(conn, used.source.place()).await?;
        let mut priced = book
            .price_service(conn, used.quote, used.rate_source, used.unit, &quantity)
            .await?;
        let mut source = used.quote.map(|_| used.rate_source);
        if let Some(quote) = used.quote.filter(|quote| quote.unit == used.unit)
            && let Some(total) = book
                .reported_total(conn, used.timing, source, &quote.currency)
                .await?
        {
            priced.credits = Some(total);
            priced.credits_per_unit = None;
            source = Some(Source::Reported);
        }
        let mut groups = used.groups.to_vec();
        groups.sort();
        groups.dedup();
        let mut timing = used.timing.clone();
        timing.service_usage = Some(ServiceUsage {
            offering: used.offering.to_owned(),
            unit: used.unit.to_owned(),
            quantity,
            price: used.quote.map(|quote| ServiceUnitPrice {
                amount: quote.amount.clone(),
                currency: quote.currency.clone(),
                unit: quote.unit.clone(),
            }),
            credits_per_unit: priced.credits_per_unit,
        });
        let rate = timing
            .service_usage
            .as_ref()
            .and_then(|service| serde_json::to_value(service.price.as_ref()?).ok());
        let source = source.map(Source::word);
        let wrote = match used.source {
            ServiceSource::Turn(original) => {
                let mut attribution = original.clone();
                attribution.provider_id = used.provider;
                attribution.entry_id = used.entry;
                attribution.user_id = used.person;
                ledger::record_service_measured(
                    conn,
                    used.purpose,
                    &NewEvent {
                        id: used.id,
                        at: used.at,
                        origin: ledger::origin::ORCH,
                        scope: ledger::scope::REQUEST,
                        counted: true,
                        attribution: &attribution,
                        model: "",
                        tier: None,
                        tokens: Tokens::default(),
                        context_tokens: None,
                        request_ref: used.request_ref,
                        rate: rate.as_ref(),
                        rate_source: source,
                        credits: priced.credits.as_deref(),
                    },
                    &timing,
                    &groups,
                )
                .await?
            }
            ServiceSource::Standalone(original) => {
                let mut attribution = original.clone();
                attribution.provider_id = used.provider;
                attribution.entry_id = used.entry;
                attribution.user_id = used.person;
                ledger::record_service_own_measured(
                    conn,
                    used.purpose,
                    &NewSummary {
                        id: used.id,
                        at: used.at,
                        attribution: &attribution,
                        model: "",
                        tier: None,
                        tokens: Tokens::default(),
                        context_tokens: None,
                        request_ref: used.request_ref,
                        rate: rate.as_ref(),
                        rate_source: source,
                        credits: priced.credits.as_deref(),
                    },
                    &timing,
                    &groups,
                )
                .await?
            }
        };
        if wrote {
            Box::pin(self.note_service_caps(
                conn,
                &crate::caps::ServiceCapAsk {
                    place: used.source.place(),
                    provider: used.provider,
                    offering: used.offering,
                    groups: used.groups,
                    purpose: used.purpose,
                    person: used.person,
                    priced: priced.credits.is_some(),
                    harness_kind: match used.source {
                        ServiceSource::Turn(source) => source.harness_kind.as_deref(),
                        ServiceSource::Standalone(_) => None,
                    },
                },
            ))
            .await?;
        }
        Ok(wrote)
    }
}
