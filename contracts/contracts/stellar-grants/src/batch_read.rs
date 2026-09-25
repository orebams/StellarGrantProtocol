use soroban_sdk::{Address, Env, Vec};

use crate::errors::ContractError;
use crate::grant_index;
use crate::storage::Storage;
use crate::types::{
    DashboardView, GrantCard, GrantDetailView, Milestone, MilestoneState, ProtocolMetrics,
    ReviewerProfile, ReviewerView,
};

const MAX_MULTI_GRANT_BATCH: u32 = 10;

fn build_export_grant(grant: &crate::types::Grant) -> GrantCard {
    GrantCard {
        id: grant.id,
        owner: grant.owner.clone(),
        title: grant.title.clone(),
        description: grant.description.clone(),
        token: grant.token.clone(),
        status: grant.status,
        total_amount: grant.total_amount,
        milestone_amount: grant.milestone_amount,
        total_milestones: grant.total_milestones,
        milestones_paid_out: grant.milestones_paid_out,
        escrow_balance: grant.escrow_balance,
        timestamp: grant.timestamp,
    }
}

fn compute_completion_pct(approved: u32, total: u32) -> u32 {
    if total == 0 {
        return 0;
    }
    (approved * 100) / total
}

/// Return all data needed for the grant detail page. Single RPC call.
pub fn grant_detail(env: &Env, grant_id: u64) -> Result<GrantDetailView, ContractError> {
    let grant = Storage::get_grant(env, grant_id).ok_or(ContractError::GrantNotFound)?;

    let mut milestones = Vec::new(env);
    let mut approved_count: u32 = 0;
    let mut current_milestone_idx: u32 = 0;

    for idx in 0..grant.total_milestones {
        if let Some(ms) = Storage::get_milestone(env, grant_id, idx) {
            if ms.state == MilestoneState::Approved {
                approved_count += 1;
            }
            if ms.state == MilestoneState::Submitted {
                current_milestone_idx = idx;
            }
            milestones.push_back(ms);
        } else {
            milestones.push_back(Milestone {
                idx,
                description: soroban_sdk::String::from_str(env, ""),
                amount: grant.milestone_amount,
                state: MilestoneState::Pending,
                votes: soroban_sdk::Map::new(env),
                approvals: 0,
                rejections: 0,
                reasons: soroban_sdk::Map::new(env),
                status_updated_at: 0,
                proof_url: None,
                submission_timestamp: 0,
                deadline: None,
                reviewer_count_snapshot: 0,
            });
        }
    }

    let completion_pct = compute_completion_pct(approved_count, grant.total_milestones);

    let funder_count = grant.funders.len();
    let reviewer_count = grant.reviewers.len();
    let escrow_balance = grant.escrow_balance;

    let mut reputation_scores = Vec::new(env);
    for reviewer in grant.reviewers.iter() {
        let rep = Storage::get_reviewer_reputation(env, reviewer.clone());
        reputation_scores.push_back((reviewer, rep));
    }

    Ok(GrantDetailView {
        grant,
        milestones,
        escrow_balance,
        funder_count,
        reviewer_count,
        current_milestone_idx,
        completion_pct,
        reputation_scores,
    })
}

/// Return all data needed for the protocol dashboard. Single RPC call.
pub fn dashboard(env: &Env) -> DashboardView {
    let active_grant_ids = grant_index::by_status(env, crate::types::GrantStatus::Active, 0, 1001);
    let truncated = active_grant_ids.len() > 1000;
    let active_grants = if truncated {
        1000
    } else {
        active_grant_ids.len() as u32
    };

    let protocol_metrics = get_or_default_metrics(env);

    let recent_grant_ids = grant_index::recent(env, 0, 10);

    let mut total_funded_usd: i128 = 0;
    let mut total_paid_out_usd: i128 = 0;

    for (i, grant_id) in active_grant_ids.iter().enumerate() {
        if i >= 1000 {
            break;
        }
        if let Some(grant) = Storage::get_grant(env, grant_id) {
            total_funded_usd += grant.escrow_balance;
            total_paid_out_usd += grant.milestone_amount * grant.milestones_paid_out as i128;
        }
    }

    let contributor_count = protocol_metrics.total_contributors_registered;
    let reviewer_count = Storage::get_reviewer_allowlist(env).len();

    DashboardView {
        active_grants,
        total_funded_usd,
        total_paid_out_usd,
        total_contributors: contributor_count,
        total_reviewers: reviewer_count,
        recent_grant_ids,
        protocol_metrics,
        truncated,
    }
}

/// Return all data needed for a reviewer's personal dashboard.
pub fn reviewer_dashboard(env: &Env, reviewer: &Address) -> ReviewerView {
    let profile = Storage::get_reviewer_profile(env, reviewer).unwrap_or(ReviewerProfile {
        reviewer: reviewer.clone(),
        display_name: soroban_sdk::String::from_str(env, ""),
        expertise_tags: soroban_sdk::Vec::new(env),
        hourly_rate: None,
        reviews_completed: 0,
        average_turnaround_ledgers: 0,
        availability: crate::types::ReviewerAvailability::Available,
        registered_at: 0,
        reputation_score: 0,
    });

    let reputation = Storage::get_reviewer_reputation(env, reviewer.clone());

    let mut pending_votes = soroban_sdk::Vec::new(env);
    let mut sla_breach_count: u32 = 0;

    let active_grants = grant_index::by_status(env, crate::types::GrantStatus::Active, 0, 101);
    let mut truncated = false;
    for (i, grant_id) in active_grants.iter().enumerate() {
        if i >= 100 {
            truncated = true;
            break;
        }
        if let Some(grant) = Storage::get_grant(env, grant_id) {
            if !grant.reviewers.contains(reviewer.clone()) {
                continue;
            }
            for idx in 0..grant.total_milestones {
                // Issue #611: surface SLA breaches this reviewer has accrued.
                let sla_id = crate::reviewer_sla::milestone_sla_id(grant_id, idx);
                if let Some(sla) = crate::reviewer_sla::get_sla(env, reviewer, sla_id) {
                    if sla.breached {
                        sla_breach_count = sla_breach_count.saturating_add(1);
                    }
                }
                if let Some(ms) = Storage::get_milestone(env, grant_id, idx) {
                    if ms.state == MilestoneState::Submitted
                        && !ms.votes.contains_key(reviewer.clone())
                    {
                        pending_votes.push_back((grant_id, idx));
                    }
                }
            }
        }
    }

    ReviewerView {
        reviewer: reviewer.clone(),
        profile,
        reputation,
        pending_votes,
        sla_breach_count,
        pending_rewards: 0,
        truncated,
    }
}

/// Return detail views for multiple grants at once (max 10).
pub fn multi_grant_detail(
    env: &Env,
    grant_ids: Vec<u64>,
) -> Result<Vec<GrantDetailView>, ContractError> {
    if grant_ids.len() > MAX_MULTI_GRANT_BATCH {
        return Err(ContractError::BatchSizeExceeded);
    }

    let mut views = Vec::new(env);
    for grant_id in grant_ids.iter() {
        views.push_back(grant_detail(env, grant_id)?);
    }
    Ok(views)
}

/// Return minimal grant cards for a list of grant IDs (cheaper than full detail).
pub fn grant_cards(env: &Env, grant_ids: Vec<u64>) -> Vec<GrantCard> {
    let mut cards = Vec::new(env);
    for grant_id in grant_ids.iter() {
        if let Some(grant) = Storage::get_grant(env, grant_id) {
            cards.push_back(build_export_grant(&grant));
        }
    }
    cards
}

fn get_or_default_metrics(env: &Env) -> ProtocolMetrics {
    Storage::get_protocol_metrics(env).unwrap_or(ProtocolMetrics {
        total_grants_created: 0,
        total_grants_active: 0,
        total_grants_completed: 0,
        total_grants_cancelled: 0,
        total_milestones_approved: 0,
        total_milestones_rejected: 0,
        total_milestones_paid: 0,
        total_contributors_registered: 0,
        total_disputes_raised: 0,
        total_disputes_resolved: 0,
        total_bounties_created: 0,
        total_bounties_awarded: 0,
        last_updated: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Grant, GrantStatus};
    use soroban_sdk::{testutils::Address as _, Env, String, Vec};

    fn create_test_grant(env: &Env, grant_id: u64, owner: &Address) -> Grant {
        let token = Address::generate(env);
        Grant {
            id: grant_id,
            owner: owner.clone(),
            title: String::from_str(env, "Title"),
            description: String::from_str(env, "Desc"),
            token,
            status: GrantStatus::Active,
            total_amount: 1000,
            milestone_amount: 500,
            reviewers: Vec::new(env),
            total_milestones: 2,
            milestones_paid_out: 0,
            escrow_balance: 1000,
            funders: Vec::new(env),
            reason: None,
            timestamp: env.ledger().timestamp(),
            require_compliance: None,
        }
    }

    #[test]
    fn test_multi_grant_detail_batch_size_exceeded() {
        let env = Env::default();
        let mut ids = Vec::new(&env);
        for i in 0..11u64 {
            ids.push_back(i);
        }

        let res = multi_grant_detail(&env, ids);
        assert_eq!(res, Err(ContractError::BatchSizeExceeded));
    }

    #[test]
    fn test_multi_grant_detail_success_within_limit() {
        let env = Env::default();
        env.mock_all_auths();
        let owner = Address::generate(&env);

        let mut ids = Vec::new(&env);
        for i in 1..=5u64 {
            let grant = create_test_grant(&env, i, &owner);
            Storage::set_grant(&env, i, &grant);
            ids.push_back(i);
        }

        let views = multi_grant_detail(&env, ids).unwrap();
        assert_eq!(views.len(), 5);
        assert_eq!(views.get(0).unwrap().grant.id, 1);
        assert_eq!(views.get(4).unwrap().grant.id, 5);
    }

    #[test]
    fn test_dashboard_truncation_flag() {
        let env = Env::default();
        env.mock_all_auths();

        // 1. Untruncated case (0 grants)
        let dash_empty = dashboard(&env);
        assert!(!dash_empty.truncated);
        assert_eq!(dash_empty.active_grants, 0);

        // 2. Truncated case (>1000 active grants in index)
        let owner = Address::generate(&env);
        let token = Address::generate(&env);
        for i in 1..=1001u64 {
            grant_index::on_grant_created(&env, i, &owner, &token, GrantStatus::Active);
        }

        let dash_full = dashboard(&env);
        assert!(dash_full.truncated);
        assert_eq!(dash_full.active_grants, 1000);
    }

    #[test]
    fn test_reviewer_dashboard_truncation_flag() {
        let env = Env::default();
        env.mock_all_auths();
        let reviewer = Address::generate(&env);

        // 1. Untruncated case
        let rev_empty = reviewer_dashboard(&env, &reviewer);
        assert!(!rev_empty.truncated);

        // 2. Truncated case (>100 active grants in index)
        let owner = Address::generate(&env);
        let token = Address::generate(&env);
        for i in 1..=101u64 {
            grant_index::on_grant_created(&env, i, &owner, &token, GrantStatus::Active);
        }

        let rev_full = reviewer_dashboard(&env, &reviewer);
        assert!(rev_full.truncated);
    }
}
