// SPDX-License-Identifier: MIT

//! Read-only query helpers for the Credit contract.
//!
//! Every function in this module is side-effect free (modulo TTL bumps in
//! [`crate::storage::get_credit_line`], which write only when the remaining
//! TTL is below `LEDGER_BUMP_THRESHOLD`).
//!
//! These helpers are the primary surface for off-chain indexers: returned
//! structs are designed for stable serialization order (see
//! [`crate::types::CreditLineData`] field ordering note).

use crate::storage::{grace_period_key, MAX_ENUMERATION_LIMIT};
use crate::types::{
    CreditLineData, CreditStatus, GracePeriodConfig, ProtocolSummary, RepaymentSchedule,
};
use soroban_sdk::{Address, Env, Vec};

/// Return the credit line for `borrower`, or `None` if no line exists.
///
/// # Authentication
/// No authentication required. This is a pure read — it does not mutate
/// any storage and carries no trust boundary. Any caller (indexer, client,
/// or another contract) may invoke it freely.
///
/// # Stability
/// The returned [`CreditLineData`] struct is stable for integrators.
/// All fields — including `last_rate_update_ts`, `accrued_interest`, and
/// `last_accrual_ts` — are serialized in the order declared in `types.rs`.
/// New fields will only be appended; existing field positions will not change.
///
/// # Note on accrual
/// Interest accrual is lazy: `accrued_interest` and `utilized_amount` reflect
/// the last mutating call (draw, repay, suspend, etc.). Pending interest since
/// the last checkpoint is **not** applied by this query.
#[allow(dead_code)]
pub fn get_credit_line(env: Env, borrower: Address) -> Option<CreditLineData> {
    crate::storage::get_credit_line(&env, &borrower)
}

/// Return protocol-level dashboard aggregates in one read-only call.
///
/// This reads only aggregate storage slots and does not touch per-borrower
/// records, so it does not bump persistent-entry TTL.
pub fn get_protocol_summary(env: Env) -> ProtocolSummary {
    ProtocolSummary {
        count: crate::storage::get_credit_line_count(&env),
        total_utilized: crate::storage::get_total_utilized(&env),
        total_collateral: crate::storage::get_total_collateral(&env),
        treasury_balance: crate::storage::get_treasury_balance(&env),
    }
}

/// Return the configured installment repayment schedule for `borrower`, if any.
pub fn get_repayment_schedule(env: Env, borrower: Address) -> Option<RepaymentSchedule> {
    env.storage()
        .persistent()
        .get(&crate::storage::DataKey::RepaymentSchedule(borrower))
}

/// Return `true` when the borrower has missed an installment past the grace window.
///
/// Returns `false` for the following short-circuit cases:
/// - The borrower has no credit line.
/// - The line is `Closed` or has zero outstanding principal.
/// - The line has no configured [`RepaymentSchedule`].
///
/// The grace window is determined by the global [`GracePeriodConfig`]. When no
/// config is set, `grace_seconds` defaults to `0`, so any timestamp strictly
/// greater than `next_due_ts` is treated as delinquent. The comparison uses
/// `saturating_add` to ensure timestamps near `u64::MAX` do not wrap.
pub fn is_delinquent(env: Env, borrower: Address) -> bool {
    let Some(line) = get_credit_line(env.clone(), borrower.clone()) else {
        return false;
    };

    if line.status == CreditStatus::Closed || line.utilized_amount <= 0 {
        return false;
    }

    let Some(schedule) = get_repayment_schedule(env.clone(), borrower) else {
        return false;
    };

    let grace_cfg: Option<GracePeriodConfig> =
        env.storage().instance().get(&grace_period_key(&env));
    let grace_seconds = grace_cfg.map(|cfg| cfg.grace_period_seconds).unwrap_or(0);
    let delinquent_after = schedule.next_due_ts.saturating_add(grace_seconds);

    env.ledger().timestamp() > delinquent_after
}

/// Return the draw amount recorded for `borrower` at `timestamp`, or `None`
/// if no draw occurred at that timestamp.
///
/// # Authentication
/// No authentication required. Read-only — does not mutate storage.
pub fn get_draw_audit(env: Env, borrower: Address, timestamp: u64) -> Option<i128> {
    env.storage()
        .persistent()
        .get(&crate::storage::DataKey::DrawAudit(borrower, timestamp))
}

/// Enumerate draw audit entries for `borrower` in reverse chronological order
/// (most recent first).
///
/// `cursor` is an exclusive upper bound: only entries with timestamp strictly
/// less than `cursor` are returned. Pass `None` to start from the most recent
/// draw. Results are capped by `MAX_ENUMERATION_LIMIT`.
///
/// # CPU complexity
///
/// O(log N + limit) — a binary search locates the first qualifying entry in the
/// timestamp index, then walks backward collecting at most `limit` results.
///
/// # Authentication
/// No authentication required. Read-only — does not mutate storage.
pub fn enumerate_draw_audit(
    env: Env,
    borrower: Address,
    cursor: Option<u64>,
    limit: u32,
) -> Vec<(u64, i128)> {
    let capped = limit.min(MAX_ENUMERATION_LIMIT);
    let mut out: Vec<(u64, i128)> = Vec::new(&env);
    if capped == 0 {
        return out;
    }

    let timestamps = crate::storage::get_draw_audit_timestamps(&env, &borrower);
    let len = timestamps.len();
    if len == 0 {
        return out;
    }

    // Binary search: find the rightmost index with timestamp < cursor.
    // When cursor is None, start from the newest entry (last index).
    let start_idx: i64 = match cursor {
        Some(cursor_ts) => {
            let mut lo: i64 = 0;
            let mut hi: i64 = len as i64 - 1;
            let mut result: i64 = -1;
            while lo <= hi {
                let mid = lo + (hi - lo) / 2;
                let ts = timestamps.get(mid as u32).unwrap();
                if ts < cursor_ts {
                    result = mid;
                    lo = mid + 1;
                } else {
                    hi = mid - 1;
                }
            }
            if result < 0 {
                return out;
            }
            result
        }
        None => len as i64 - 1,
    };

    // Walk backward from start_idx, collecting at most `capped` entries.
    let mut idx = start_idx;
    let mut collected = 0u32;
    while idx >= 0 {
        let ts = timestamps.get(idx as u32).unwrap();
        let amount: i128 = env
            .storage()
            .persistent()
            .get(&crate::storage::DataKey::DrawAudit(borrower.clone(), ts))
            .unwrap_or(0);
        out.push_back((ts, amount));
        collected += 1;
        if collected >= capped {
            break;
        }
        idx -= 1;
    }

    out
}
