//! The spending guard for cloud runs: when a run asks before it is sent, and the daily limit.
//!
//! - **Confirmation** ([`confirm_reasons`]): every cloud video (unless the user turned that
//!   off), any cloud run estimated at [`CONFIRM_AT_USD`] or more, and any cloud run whose
//!   price is unknown. The backend refuses with `studio_needs_confirmation` when the request
//!   is not confirmed and a reason applies.
//! - **The daily limit** ([`check_limit`]): a run that would take today's spending past the
//!   limit ([`DEFAULT_DAILY_LIMIT_USD`] unless the user changed it) is not sent; reaching the
//!   limit exactly is allowed. An unknown price needs [`UNKNOWN_PRICE_RESERVE_USD`] of the
//!   limit left.
//! - **What a run counts** ([`counted_usd`], [`spent_since`]): a run counts on the local
//!   calendar day it was created ([`day_start_in`]). A run still going counts its estimate;
//!   an ended run counts what the company said it charged ([`charge_of`]): its cost when it
//!   was charged, its estimate when that is unknown, nothing when it was not charged. A run
//!   with no price at all reserves [`UNKNOWN_PRICE_RESERVE_USD`] until its cost arrives.
//!
//! A run on this computer ([`Price::Free`], [`Billing::Free`]) never asks and is never
//! refused for the limit. It is recorded with an estimate of $0 and, when done, a cost of $0,
//! so it adds nothing to any sum.

use agent::media::{MediaKind, Price};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::Serialize;

/// A cloud run estimated at this many US dollars or more asks before it is sent.
pub const CONFIRM_AT_USD: f64 = 1.0;
/// The daily spending limit, in US dollars, until the user changes it.
pub const DEFAULT_DAILY_LIMIT_USD: f64 = 10.0;
/// What a run with an unknown price counts until its cost arrives, in US dollars, and how
/// much of the limit must be left for it to be sent.
pub const UNKNOWN_PRICE_RESERVE_USD: f64 = 5.0;

/// Leeway for sums of dollar amounts: a run that reaches a threshold to within this much
/// counts as reaching it (`0.1 + 0.2` is not exactly `0.3` in floating point).
const LEEWAY_USD: f64 = 1e-9;

/// When a cloud run asks before it is sent (Settings → Studio).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConfirmRules {
    /// Every cloud video asks, whatever its price.
    pub every_video: bool,
    /// A cloud run estimated at this many US dollars or more asks.
    pub at_usd: f64,
}

impl Default for ConfirmRules {
    fn default() -> Self {
        Self {
            every_video: true,
            at_usd: CONFIRM_AT_USD,
        }
    }
}

/// Why a run asks before it is sent. The dollar threshold travels beside it, as the
/// estimate's `confirm_at_usd`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfirmReason {
    /// It is a cloud video.
    Video,
    /// Its estimate is at or above the threshold.
    AtLeast,
    /// Its price is unknown.
    UnknownPrice,
}

/// Whether a run is paid for, and what it is estimated to cost.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Billing {
    /// A run on this computer: nothing is charged.
    Free,
    /// A cloud run, with its estimate in US dollars; `None` when the price is unknown.
    Paid(Option<f64>),
}

/// The billing of a run priced `price` with the estimate `estimate` (from
/// `agent::media::estimate`). A run on this computer is free whatever the estimate says.
pub fn billing_of(price: &Price, estimate: Option<f64>) -> Billing {
    match price {
        Price::Free => Billing::Free,
        _ => Billing::Paid(estimate),
    }
}

/// Why a run of `kind` billed `billing` asks before it is sent under `rules`; empty when it
/// does not ask. A run on this computer never asks.
pub fn confirm_reasons(
    kind: MediaKind,
    billing: Billing,
    rules: &ConfirmRules,
) -> Vec<ConfirmReason> {
    let Billing::Paid(estimate) = billing else {
        return Vec::new();
    };
    let mut reasons = Vec::new();
    if rules.every_video && kind == MediaKind::Video {
        reasons.push(ConfirmReason::Video);
    }
    match estimate {
        Some(usd) if usd + LEEWAY_USD >= rules.at_usd => reasons.push(ConfirmReason::AtLeast),
        Some(_) => {}
        None => reasons.push(ConfirmReason::UnknownPrice),
    }
    reasons
}

/// How much of a run counts toward today's spending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Charge {
    /// Still going: it counts its estimate (or its cost, once reported) until it ends.
    Pending,
    /// Charged: its cost, or its estimate while no cost is recorded.
    Final,
    /// Not charged: it counts nothing.
    None,
    /// Ended, and the company did not say whether it charged: counted as charged.
    Maybe,
}

/// Where a run stands, for what it is charged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    /// Queued, sending or running.
    Live,
    /// Finished with its results.
    Done,
    /// Failed, blocked, cancelled or interrupted, with whether the company charged for it
    /// (`None` when that is not known).
    Ended {
        /// Whether the company charged for the run, when that is known.
        charged: Option<bool>,
    },
}

/// What a run in `state` is charged.
pub fn charge_of(state: RunState) -> Charge {
    match state {
        RunState::Live => Charge::Pending,
        RunState::Done
        | RunState::Ended {
            charged: Some(true),
        } => Charge::Final,
        RunState::Ended {
            charged: Some(false),
        } => Charge::None,
        RunState::Ended { charged: None } => Charge::Maybe,
    }
}

/// One run, as the spending sums see it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpendRow {
    /// When the run was created, in Unix milliseconds.
    pub created_at: i64,
    /// What the run was estimated to cost before it was sent, in US dollars.
    pub estimate_usd: Option<f64>,
    /// What the company reported the run cost, in US dollars.
    pub cost_usd: Option<f64>,
    /// How much of the run counts.
    pub charge: Charge,
}

/// What `row` adds to today's spending, in US dollars: nothing when it was not charged;
/// otherwise its reported cost, else its estimate, else [`UNKNOWN_PRICE_RESERVE_USD`].
pub fn counted_usd(row: &SpendRow) -> f64 {
    match row.charge {
        Charge::None => 0.0,
        Charge::Pending | Charge::Final | Charge::Maybe => row
            .cost_usd
            .or(row.estimate_usd)
            .unwrap_or(UNKNOWN_PRICE_RESERVE_USD),
    }
}

/// Today's spending so far, in US dollars.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Spend {
    /// What ended and finished runs count.
    pub settled_usd: f64,
    /// What runs still going count (their estimates, or the reserve for an unknown price).
    pub pending_usd: f64,
}

/// The spending of the runs in `rows` created at or after `day_start_ms` (from
/// [`day_start_in`]).
pub fn spent_since(rows: &[SpendRow], day_start_ms: i64) -> Spend {
    let mut spend = Spend::default();
    for row in rows.iter().filter(|row| row.created_at >= day_start_ms) {
        let usd = counted_usd(row);
        match row.charge {
            Charge::Pending => spend.pending_usd += usd,
            Charge::Final | Charge::None | Charge::Maybe => spend.settled_usd += usd,
        }
    }
    spend
}

/// Why a run is not sent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LimitRefusal {
    /// The run would take today's spending past the daily limit.
    Over {
        /// Today's spending so far, settled and pending, in US dollars.
        spent: f64,
        /// The daily limit, in US dollars.
        limit: f64,
    },
    /// The run's price is unknown, and less than [`UNKNOWN_PRICE_RESERVE_USD`] of the limit
    /// is left (with a limit below that, always).
    UnknownNeedsReserve {
        /// Today's spending so far, settled and pending, in US dollars.
        spent: f64,
        /// The daily limit, in US dollars.
        limit: f64,
    },
}

/// The daily limit as it stands for a run billed `billing` (the webview's `LimitDto`).
pub fn limit_dto(limit: Option<f64>, spend: &Spend, billing: Billing) -> crate::dto::LimitDto {
    use crate::dto::LimitRefusalDto;
    let refusal = check_limit(limit, spend, billing).err();
    crate::dto::LimitDto {
        limit_usd: limit,
        spent_today_usd: spend.settled_usd,
        pending_usd: spend.pending_usd,
        would_pass: refusal.is_none(),
        refusal: refusal.map(|r| match r {
            LimitRefusal::Over { .. } => LimitRefusalDto::Over,
            LimitRefusal::UnknownNeedsReserve { .. } => LimitRefusalDto::UnknownNeedsReserve,
        }),
        reserve_usd: UNKNOWN_PRICE_RESERVE_USD,
    }
}

impl LimitRefusal {
    /// The refusal in Studio's words (`studio_daily_limit`).
    pub fn error(self) -> crate::error::AppError {
        match self {
            Self::Over { spent, limit } => super::errors::daily_limit(spent, limit),
            Self::UnknownNeedsReserve { spent, limit } => {
                super::errors::unknown_price_needs_reserve(spent, limit)
            }
        }
    }
}

/// Whether a run billed `billing` may be sent with today's spending at `spend` under the
/// daily `limit` (`None`: no limit). A run whose estimate takes the spending past the limit
/// is refused; one that reaches it exactly is allowed. A run with an unknown price counts
/// as [`UNKNOWN_PRICE_RESERVE_USD`], so it needs that much of the limit left. A run on this
/// computer always passes.
pub fn check_limit(
    limit: Option<f64>,
    spend: &Spend,
    billing: Billing,
) -> Result<(), LimitRefusal> {
    let (Some(limit), Billing::Paid(estimate)) = (limit, billing) else {
        return Ok(());
    };
    let spent = spend.settled_usd + spend.pending_usd;
    let usd = estimate.unwrap_or(UNKNOWN_PRICE_RESERVE_USD);
    if spent + usd > limit + LEEWAY_USD {
        return Err(match estimate {
            Some(_) => LimitRefusal::Over { spent, limit },
            None => LimitRefusal::UnknownNeedsReserve { spent, limit },
        });
    }
    Ok(())
}

/// The first instant, in Unix milliseconds, of the calendar day in `tz` that holds `at_ms`:
/// local midnight, or the first instant after it when the clocks skip midnight that day.
/// The app passes `TimeZone::system()`.
///
/// An instant outside the dates jiff can represent (years -9999 to 9999) gives `i64::MIN`,
/// so every run counts: the guard would rather count too much than too little.
pub fn day_start_in(tz: &TimeZone, at_ms: i64) -> i64 {
    Timestamp::from_millisecond(at_ms)
        .and_then(|at| at.to_zoned(tz.clone()).start_of_day())
        .map_or(i64::MIN, |start| start.timestamp().as_millisecond())
}

#[cfg(test)]
mod tests {
    use super::*;
    use Billing::Paid;
    use jiff::civil::date;
    use jiff::tz::Offset;
    use serde_json::json;

    fn row(estimate_usd: Option<f64>, cost_usd: Option<f64>, charge: Charge) -> SpendRow {
        SpendRow {
            created_at: 0,
            estimate_usd,
            cost_usd,
            charge,
        }
    }

    fn row_at(created_at: i64, estimate_usd: Option<f64>, charge: Charge) -> SpendRow {
        SpendRow {
            created_at,
            ..row(estimate_usd, None, charge)
        }
    }

    /// The instant of a wall-clock time in `tz`, in Unix milliseconds.
    fn ms(tz: &TimeZone, y: i16, mo: i8, d: i8, h: i8, mi: i8, s: i8) -> i64 {
        date(y, mo, d)
            .at(h, mi, s, 0)
            .to_zoned(tz.clone())
            .unwrap()
            .timestamp()
            .as_millisecond()
    }

    fn fixed(seconds: i32) -> TimeZone {
        TimeZone::fixed(Offset::from_seconds(seconds).unwrap())
    }

    #[test]
    fn every_video_asks() {
        assert_eq!(
            confirm_reasons(MediaKind::Video, Paid(Some(0.09)), &ConfirmRules::default()),
            [ConfirmReason::Video]
        );
    }

    #[test]
    fn a_dollar_or_more_asks() {
        assert!(
            confirm_reasons(MediaKind::Image, Paid(Some(0.99)), &ConfirmRules::default())
                .is_empty()
        );
        assert_eq!(
            confirm_reasons(MediaKind::Image, Paid(Some(1.00)), &ConfirmRules::default()),
            [ConfirmReason::AtLeast]
        );
    }

    #[test]
    fn a_local_run_never_asks_even_for_a_video() {
        assert_eq!(billing_of(&Price::Free, Some(0.0)), Billing::Free);
        assert_eq!(billing_of(&Price::Unknown, None), Paid(None));
        assert!(
            confirm_reasons(MediaKind::Video, Billing::Free, &ConfirmRules::default()).is_empty()
        );
    }

    #[test]
    fn a_local_run_passes_any_limit() {
        assert!(
            check_limit(
                Some(10.0),
                &Spend {
                    settled_usd: 10.0,
                    pending_usd: 3.0
                },
                Billing::Free
            )
            .is_ok()
        );
        // How a finished local run is recorded.
        assert_eq!(counted_usd(&row(Some(0.0), Some(0.0), Charge::Final)), 0.0);
    }

    #[test]
    fn reasons_serialise_as_plain_names() {
        assert_eq!(
            serde_json::to_value([
                ConfirmReason::Video,
                ConfirmReason::AtLeast,
                ConfirmReason::UnknownPrice
            ])
            .unwrap(),
            json!(["video", "atLeast", "unknownPrice"])
        );
    }

    #[test]
    fn what_a_run_is_charged_follows_what_the_company_said() {
        assert_eq!(charge_of(RunState::Live), Charge::Pending);
        assert_eq!(charge_of(RunState::Done), Charge::Final);
        assert_eq!(
            charge_of(RunState::Ended {
                charged: Some(true)
            }),
            Charge::Final
        );
        assert_eq!(
            charge_of(RunState::Ended {
                charged: Some(false)
            }),
            Charge::None
        );
        assert_eq!(charge_of(RunState::Ended { charged: None }), Charge::Maybe);
    }

    #[test]
    fn an_unknown_price_always_asks() {
        let lax = ConfirmRules {
            every_video: false,
            at_usd: 100.0,
        };
        assert_eq!(
            confirm_reasons(MediaKind::Image, Paid(None), &lax),
            [ConfirmReason::UnknownPrice]
        );
    }

    #[test]
    fn dont_ask_again_for_videos_keeps_the_dollar_rule() {
        let r = ConfirmRules {
            every_video: false,
            at_usd: 1.0,
        };
        assert!(confirm_reasons(MediaKind::Video, Paid(Some(0.96)), &r).is_empty());
        assert_eq!(
            confirm_reasons(MediaKind::Video, Paid(Some(1.20)), &r),
            [ConfirmReason::AtLeast]
        );
    }

    #[test]
    fn reaching_the_limit_exactly_is_allowed() {
        let s = Spend {
            settled_usd: 9.04,
            pending_usd: 0.0,
        };
        assert!(check_limit(Some(10.0), &s, Paid(Some(0.96))).is_ok());
        assert_eq!(
            check_limit(Some(10.0), &s, Paid(Some(0.97))),
            Err(LimitRefusal::Over {
                spent: 9.04,
                limit: 10.0
            })
        );
    }

    #[test]
    fn an_unknown_price_needs_five_dollars_left() {
        assert!(
            check_limit(
                Some(10.0),
                &Spend {
                    settled_usd: 4.0,
                    pending_usd: 1.0
                },
                Paid(None)
            )
            .is_ok()
        );
        assert_eq!(
            check_limit(
                Some(10.0),
                &Spend {
                    settled_usd: 5.01,
                    pending_usd: 0.0
                },
                Paid(None)
            ),
            Err(LimitRefusal::UnknownNeedsReserve {
                spent: 5.01,
                limit: 10.0
            })
        );
        // A limit below the reserve refuses every run whose price is unknown, for that
        // reason, not because the limit was reached.
        assert_eq!(
            check_limit(Some(3.0), &Spend::default(), Paid(None)),
            Err(LimitRefusal::UnknownNeedsReserve {
                spent: 0.0,
                limit: 3.0
            })
        );
    }

    #[test]
    fn no_limit_refuses_nothing() {
        assert!(
            check_limit(
                None,
                &Spend {
                    settled_usd: 1e6,
                    pending_usd: 0.0
                },
                Paid(None)
            )
            .is_ok()
        );
    }

    #[test]
    fn what_each_run_counts() {
        assert_eq!(
            counted_usd(&row(Some(0.5), Some(0.42), Charge::Final)),
            0.42
        );
        assert_eq!(counted_usd(&row(Some(3.0), None, Charge::None)), 0.0);
        assert_eq!(counted_usd(&row(Some(0.96), None, Charge::Pending)), 0.96);
        assert_eq!(counted_usd(&row(Some(0.96), None, Charge::Maybe)), 0.96);
        assert_eq!(
            counted_usd(&row(None, None, Charge::Pending)),
            UNKNOWN_PRICE_RESERVE_USD
        );
    }

    #[test]
    fn a_run_counts_on_the_day_it_was_created() {
        let tz = fixed(3600);
        let late = ms(&tz, 2026, 11, 17, 23, 59, 30);
        let next_day = day_start_in(&tz, ms(&tz, 2026, 11, 18, 0, 0, 10));
        assert_eq!(
            spent_since(
                &[row_at(late, Some(0.96), Charge::Pending)],
                day_start_in(&tz, late)
            )
            .pending_usd,
            0.96
        );
        assert_eq!(
            spent_since(&[row_at(late, Some(0.96), Charge::Pending)], next_day).pending_usd,
            0.0
        );
    }

    #[test]
    fn the_day_starts_at_local_midnight() {
        for secs in [14 * 3600, -12 * 3600] {
            let tz = fixed(secs);
            assert_eq!(
                day_start_in(&tz, ms(&tz, 2026, 11, 17, 13, 5, 0)),
                ms(&tz, 2026, 11, 17, 0, 0, 0)
            );
        }
    }

    #[test]
    fn the_day_starts_when_the_clocks_skip_midnight() {
        // Brazil's old summer time began at midnight: on 2015-10-18 the clocks went from
        // 23:59:59 straight to 01:00, so that day began at 01:00 (-02:00).
        let tz = TimeZone::posix("BRT3BRST,M10.3.0/0,M2.3.0/0").unwrap();
        let noon = ms(&tz, 2015, 10, 18, 12, 0, 0);
        let start = day_start_in(&tz, noon);
        assert_eq!(start, ms(&fixed(-2 * 3600), 2015, 10, 18, 1, 0, 0));
        assert_eq!(
            noon - start,
            11 * 3_600_000,
            "the day is 11 hours old at noon"
        );
    }

    #[test]
    fn an_instant_jiff_cannot_read_counts_every_run() {
        assert_eq!(day_start_in(&TimeZone::UTC, i64::MAX), i64::MIN);
    }
}
